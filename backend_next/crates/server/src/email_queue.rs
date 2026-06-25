use crate::email_delivery::{EmailDeliveryService, VerificationEmailKind};
use crate::response::ApiError;
use repository::{AppRepository, EmailQueueTaskRecord};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailTaskKind {
    VerifyCode {
        code: String,
        kind: VerificationEmailKind,
    },
    PasswordReset {
        token: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailTask {
    pub email: String,
    pub site_name: String,
    pub locale: Option<String>,
    pub kind: EmailTaskKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedEmailTask {
    id: Option<i64>,
    task: EmailTask,
}

#[derive(Clone)]
pub struct EmailDispatchService {
    delivery: Arc<EmailDeliveryService>,
    queue: Option<Arc<EmailQueueService>>,
    repository: Option<Arc<dyn AppRepository>>,
    config: Option<EmailQueueConfig>,
}

impl EmailDispatchService {
    pub fn synchronous(delivery: Arc<EmailDeliveryService>) -> Self {
        Self {
            delivery,
            queue: None,
            repository: None,
            config: None,
        }
    }

    pub fn queued(delivery: Arc<EmailDeliveryService>, config: EmailQueueConfig) -> Self {
        let sender_delivery = delivery.clone();
        let queue = EmailQueueService::start(email_task_sender(sender_delivery), config);
        Self {
            delivery,
            queue: Some(Arc::new(queue)),
            repository: None,
            config: Some(config),
        }
    }

    pub async fn queued_with_repository(
        delivery: Arc<EmailDeliveryService>,
        repository: Arc<dyn AppRepository>,
        config: EmailQueueConfig,
    ) -> Self {
        let sender = durable_email_task_sender(delivery.clone(), repository.clone());
        let queue = EmailQueueService::start(sender, config);
        let _ = recover_pending_email_tasks(repository.clone(), &queue, config).await;
        Self {
            delivery,
            queue: Some(Arc::new(queue)),
            repository: Some(repository),
            config: Some(config),
        }
    }

    #[cfg(test)]
    pub fn queued_with_sender(
        delivery: Arc<EmailDeliveryService>,
        sender: Arc<dyn EmailTaskSender>,
        config: EmailQueueConfig,
    ) -> Self {
        Self {
            delivery,
            queue: Some(Arc::new(EmailQueueService::start(sender, config))),
            repository: None,
            config: Some(config),
        }
    }

    #[cfg(test)]
    pub async fn queued_with_repository_sender(
        delivery: Arc<EmailDeliveryService>,
        repository: Arc<dyn AppRepository>,
        sender: Arc<dyn EmailTaskSender>,
        config: EmailQueueConfig,
    ) -> Self {
        let sender = Arc::new(DurableEmailTaskSender {
            repository: repository.clone(),
            inner: sender,
        });
        let queue = EmailQueueService::start(sender, config);
        let _ = recover_pending_email_tasks(repository.clone(), &queue, config).await;
        Self {
            delivery,
            queue: Some(Arc::new(queue)),
            repository: Some(repository),
            config: Some(config),
        }
    }

    pub fn is_queue_enabled(&self) -> bool {
        self.queue.is_some()
    }

    pub fn queue_stats(&self) -> Option<EmailQueueStats> {
        self.queue.as_ref().map(|queue| queue.stats())
    }

    pub fn queue_config(&self) -> Option<EmailQueueConfig> {
        self.config
    }

    pub async fn pending_recoverable_tasks(&self) -> Result<usize, ApiError> {
        let (Some(repository), Some(config)) = (&self.repository, self.config) else {
            return Ok(0);
        };
        if !config.durable {
            return Ok(0);
        }
        let tasks = repository
            .list_pending_email_tasks(1000)
            .await
            .map_err(|error| {
                ApiError::internal_server_error(format!(
                    "failed to inspect email queue tasks: {error}"
                ))
            })?;
        Ok(tasks.len())
    }

    pub async fn recover_pending_tasks(&self) -> Result<usize, ApiError> {
        let (Some(repository), Some(queue), Some(config)) =
            (&self.repository, &self.queue, self.config)
        else {
            return Ok(0);
        };
        recover_pending_email_tasks(repository.clone(), queue.as_ref(), config).await
    }

    pub async fn send_verification_code(
        &self,
        to: &str,
        code: &str,
        kind: VerificationEmailKind,
    ) -> Result<(), ApiError> {
        if let Some(queue) = &self.queue {
            let task = EmailTask {
                email: to.trim().to_owned(),
                site_name: "Sub2API".to_owned(),
                locale: None,
                kind: EmailTaskKind::VerifyCode {
                    code: code.to_owned(),
                    kind,
                },
            };
            return enqueue_email_task(queue, self.repository.as_deref(), task, self.config)
                .await
                .map_err(email_queue_api_error);
        }
        self.delivery.send_verification_code(to, code, kind).await
    }

    pub async fn ensure_password_reset_config(&self) -> Result<(), ApiError> {
        self.delivery.ensure_password_reset_config().await
    }

    pub async fn send_password_reset_link(&self, to: &str, token: &str) -> Result<(), ApiError> {
        if let Some(queue) = &self.queue {
            let task = EmailTask {
                email: to.trim().to_owned(),
                site_name: "Sub2API".to_owned(),
                locale: None,
                kind: EmailTaskKind::PasswordReset {
                    token: token.to_owned(),
                },
            };
            return enqueue_email_task(queue, self.repository.as_deref(), task, self.config)
                .await
                .map_err(email_queue_api_error);
        }
        self.delivery.send_password_reset_link(to, token).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmailQueueConfig {
    pub durable: bool,
    pub recover_limit: usize,
    pub workers: usize,
    pub capacity: usize,
    pub max_attempts: usize,
    pub retry_delay: Duration,
}

impl Default for EmailQueueConfig {
    fn default() -> Self {
        Self {
            durable: false,
            recover_limit: 100,
            workers: 3,
            capacity: 100,
            max_attempts: 1,
            retry_delay: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmailQueueStats {
    pub enqueued: u64,
    pub sent: u64,
    pub failed: u64,
    pub rejected_full: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailQueueError {
    Full,
    Closed,
}

fn email_queue_api_error(error: EmailQueueError) -> ApiError {
    match error {
        EmailQueueError::Full => ApiError::too_many_requests("email queue is full"),
        EmailQueueError::Closed => ApiError::internal_server_error("email queue is closed"),
    }
}

fn email_task_sender(delivery: Arc<EmailDeliveryService>) -> Arc<dyn EmailTaskSender> {
    Arc::new(move |task: EmailTask| {
        let delivery = delivery.clone();
        async move { send_email_task(delivery, task).await }
    })
}

fn durable_email_task_sender(
    delivery: Arc<EmailDeliveryService>,
    repository: Arc<dyn AppRepository>,
) -> Arc<dyn EmailTaskSender> {
    Arc::new(DurableEmailTaskSender {
        repository,
        inner: email_task_sender(delivery),
    })
}

async fn enqueue_email_task(
    queue: &EmailQueueService,
    repository: Option<&dyn AppRepository>,
    task: EmailTask,
    config: Option<EmailQueueConfig>,
) -> Result<(), EmailQueueError> {
    let Some(repository) = repository else {
        return queue.enqueue(task);
    };
    let max_attempts = config.map(|config| config.max_attempts).unwrap_or(1).max(1);
    let record = repository
        .enqueue_email_task(email_task_record_from_task(&task, max_attempts))
        .await
        .map_err(|_| EmailQueueError::Closed)?;
    queue.enqueue_queued(QueuedEmailTask {
        id: Some(record.id),
        task,
    })
}

async fn recover_pending_email_tasks(
    repository: Arc<dyn AppRepository>,
    queue: &EmailQueueService,
    config: EmailQueueConfig,
) -> Result<usize, ApiError> {
    if !config.durable {
        return Ok(0);
    }
    let tasks = repository
        .list_pending_email_tasks(config.recover_limit as i64)
        .await
        .map_err(|error| {
            ApiError::internal_server_error(format!("failed to recover email queue tasks: {error}"))
        })?;
    let mut recovered = 0;
    for record in tasks {
        let task = email_task_from_record(&record).map_err(ApiError::internal_server_error)?;
        queue
            .enqueue_queued(QueuedEmailTask {
                id: Some(record.id),
                task,
            })
            .map_err(email_queue_api_error)?;
        recovered += 1;
    }
    Ok(recovered)
}

type SendResult = Result<(), String>;
type BoxSendFuture = Pin<Box<dyn Future<Output = SendResult> + Send + 'static>>;

pub trait EmailTaskSender: Send + Sync + 'static {
    fn send(&self, task: EmailTask) -> BoxSendFuture;

    fn send_queued(&self, task: QueuedEmailTask) -> BoxSendFuture {
        self.send(task.task)
    }
}

impl<F, Fut> EmailTaskSender for F
where
    F: Fn(EmailTask) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = SendResult> + Send + 'static,
{
    fn send(&self, task: EmailTask) -> BoxSendFuture {
        Box::pin(self(task))
    }
}

struct DurableEmailTaskSender {
    repository: Arc<dyn AppRepository>,
    inner: Arc<dyn EmailTaskSender>,
}

impl EmailTaskSender for DurableEmailTaskSender {
    fn send(&self, task: EmailTask) -> BoxSendFuture {
        self.inner.send(task)
    }

    fn send_queued(&self, task: QueuedEmailTask) -> BoxSendFuture {
        let repository = self.repository.clone();
        let inner = self.inner.clone();
        Box::pin(async move {
            let Some(id) = task.id else {
                return inner.send(task.task).await;
            };
            repository
                .mark_email_task_processing(id)
                .await
                .map_err(|error| error.to_string())?;
            match inner.send(task.task).await {
                Ok(()) => {
                    repository
                        .mark_email_task_sent(id)
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(())
                }
                Err(error) => {
                    let _ = repository.mark_email_task_failed(id, error.clone()).await;
                    Err(error)
                }
            }
        })
    }
}

pub struct EmailQueueService {
    tx: Option<mpsc::Sender<QueuedEmailTask>>,
    stats: Arc<EmailQueueStatsAtomic>,
    workers: Vec<JoinHandle<()>>,
}

impl EmailQueueService {
    pub fn start(sender: Arc<dyn EmailTaskSender>, config: EmailQueueConfig) -> Self {
        let workers = config.workers.max(1);
        let capacity = config.capacity.max(1);
        let (tx, rx) = mpsc::channel(capacity);
        let rx = Arc::new(Mutex::new(rx));
        let stats = Arc::new(EmailQueueStatsAtomic::default());
        let mut handles = Vec::with_capacity(workers);

        for _ in 0..workers {
            let rx = rx.clone();
            let sender = sender.clone();
            let stats = stats.clone();
            let config = config;
            handles.push(tokio::spawn(async move {
                loop {
                    let task = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };
                    let Some(task) = task else {
                        break;
                    };
                    process_task(sender.as_ref(), task, config, stats.as_ref()).await;
                }
            }));
        }

        Self {
            tx: Some(tx),
            stats,
            workers: handles,
        }
    }

    pub fn enqueue(&self, task: EmailTask) -> Result<(), EmailQueueError> {
        self.enqueue_queued(QueuedEmailTask { id: None, task })
    }

    fn enqueue_queued(&self, task: QueuedEmailTask) -> Result<(), EmailQueueError> {
        let Some(tx) = &self.tx else {
            return Err(EmailQueueError::Closed);
        };
        match tx.try_send(task) {
            Ok(()) => {
                self.stats.enqueued.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.stats.rejected_full.fetch_add(1, Ordering::Relaxed);
                Err(EmailQueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(EmailQueueError::Closed),
        }
    }

    pub fn stats(&self) -> EmailQueueStats {
        self.stats.snapshot()
    }

    pub async fn shutdown(mut self) {
        self.tx.take();
        for handle in self.workers {
            let _ = handle.await;
        }
    }
}

async fn process_task(
    sender: &dyn EmailTaskSender,
    task: QueuedEmailTask,
    config: EmailQueueConfig,
    stats: &EmailQueueStatsAtomic,
) {
    let attempts = config.max_attempts.max(1);
    for attempt in 1..=attempts {
        if sender.send_queued(task.clone()).await.is_ok() {
            stats.sent.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if attempt < attempts && !config.retry_delay.is_zero() {
            tokio::time::sleep(config.retry_delay).await;
        }
    }
    stats.failed.fetch_add(1, Ordering::Relaxed);
}

async fn send_email_task(delivery: Arc<EmailDeliveryService>, task: EmailTask) -> SendResult {
    match task.kind {
        EmailTaskKind::VerifyCode { code, kind } => delivery
            .send_verification_code(&task.email, &code, kind)
            .await
            .map_err(|error| error.message().to_owned()),
        EmailTaskKind::PasswordReset { token } => delivery
            .send_password_reset_link(&task.email, &token)
            .await
            .map_err(|error| error.message().to_owned()),
    }
}

fn email_task_record_from_task(task: &EmailTask, max_attempts: usize) -> EmailQueueTaskRecord {
    let (task_type, payload) = match &task.kind {
        EmailTaskKind::VerifyCode { code, kind } => (
            "verify_code",
            json!({
                "email": task.email,
                "site_name": task.site_name,
                "locale": task.locale,
                "code": code,
                "kind": verification_kind_to_str(*kind),
            }),
        ),
        EmailTaskKind::PasswordReset { token } => (
            "password_reset",
            json!({
                "email": task.email,
                "site_name": task.site_name,
                "locale": task.locale,
                "token": token,
            }),
        ),
    };
    EmailQueueTaskRecord {
        id: 0,
        task_type: task_type.to_owned(),
        status: "pending".to_owned(),
        payload,
        attempts: 0,
        max_attempts: max_attempts.min(i32::MAX as usize) as i32,
        last_error: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn email_task_from_record(record: &EmailQueueTaskRecord) -> Result<EmailTask, String> {
    match record.task_type.as_str() {
        "verify_code" => Ok(EmailTask {
            email: required_payload_string(&record.payload, "email")?,
            site_name: optional_payload_string(&record.payload, "site_name")
                .unwrap_or_else(|| "Sub2API".to_owned()),
            locale: optional_payload_string(&record.payload, "locale"),
            kind: EmailTaskKind::VerifyCode {
                code: required_payload_string(&record.payload, "code")?,
                kind: verification_kind_from_str(&required_payload_string(
                    &record.payload,
                    "kind",
                )?)?,
            },
        }),
        "password_reset" => Ok(EmailTask {
            email: required_payload_string(&record.payload, "email")?,
            site_name: optional_payload_string(&record.payload, "site_name")
                .unwrap_or_else(|| "Sub2API".to_owned()),
            locale: optional_payload_string(&record.payload, "locale"),
            kind: EmailTaskKind::PasswordReset {
                token: required_payload_string(&record.payload, "token")?,
            },
        }),
        other => Err(format!("unsupported email task type: {other}")),
    }
}

fn required_payload_string(payload: &Value, key: &str) -> Result<String, String> {
    optional_payload_string(payload, key).ok_or_else(|| format!("email task payload missing {key}"))
}

fn optional_payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn verification_kind_to_str(kind: VerificationEmailKind) -> &'static str {
    match kind {
        VerificationEmailKind::Auth => "auth",
        VerificationEmailKind::NotificationEmail => "notification_email",
    }
}

fn verification_kind_from_str(value: &str) -> Result<VerificationEmailKind, String> {
    match value.trim() {
        "auth" => Ok(VerificationEmailKind::Auth),
        "notification_email" => Ok(VerificationEmailKind::NotificationEmail),
        other => Err(format!("unsupported verification email kind: {other}")),
    }
}

#[derive(Default)]
struct EmailQueueStatsAtomic {
    enqueued: AtomicU64,
    sent: AtomicU64,
    failed: AtomicU64,
    rejected_full: AtomicU64,
}

impl EmailQueueStatsAtomic {
    fn snapshot(&self) -> EmailQueueStats {
        EmailQueueStats {
            enqueued: self.enqueued.load(Ordering::Relaxed),
            sent: self.sent.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            rejected_full: self.rejected_full.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repository::{AppRepository, EmailQueueTaskRepository, InMemoryRepository};
    use std::sync::Mutex as StdMutex;
    use tokio::time::{sleep, timeout};

    #[tokio::test]
    async fn queue_processes_tasks_and_shuts_down() {
        let delivered = Arc::new(StdMutex::new(Vec::new()));
        let delivered_for_sender = delivered.clone();
        let queue = EmailQueueService::start(
            Arc::new(move |task: EmailTask| {
                let delivered = delivered_for_sender.clone();
                async move {
                    delivered.lock().unwrap().push(task.email);
                    Ok(())
                }
            }),
            EmailQueueConfig {
                workers: 2,
                capacity: 4,
                max_attempts: 1,
                retry_delay: Duration::from_millis(0),
                ..EmailQueueConfig::default()
            },
        );

        queue
            .enqueue(EmailTask {
                email: "one@example.com".to_owned(),
                site_name: "Sub2API".to_owned(),
                locale: Some("zh-CN".to_owned()),
                kind: EmailTaskKind::VerifyCode {
                    code: "123456".to_owned(),
                    kind: VerificationEmailKind::Auth,
                },
            })
            .unwrap();

        timeout(Duration::from_secs(2), async {
            loop {
                if queue.stats().sent == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        queue.shutdown().await;

        assert_eq!(delivered.lock().unwrap().as_slice(), ["one@example.com"]);
    }

    #[tokio::test]
    async fn queue_retries_then_records_failure() {
        let attempts = Arc::new(AtomicU64::new(0));
        let attempts_for_sender = attempts.clone();
        let queue = EmailQueueService::start(
            Arc::new(move |_task: EmailTask| {
                let attempts = attempts_for_sender.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err("smtp down".to_owned())
                }
            }),
            EmailQueueConfig {
                workers: 1,
                capacity: 2,
                max_attempts: 3,
                retry_delay: Duration::from_millis(1),
                ..EmailQueueConfig::default()
            },
        );

        queue
            .enqueue(EmailTask {
                email: "fail@example.com".to_owned(),
                site_name: "Sub2API".to_owned(),
                locale: None,
                kind: EmailTaskKind::PasswordReset {
                    token: "reset-token".to_owned(),
                },
            })
            .unwrap();

        timeout(Duration::from_secs(2), async {
            loop {
                if queue.stats().failed == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        queue.shutdown().await;

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn queue_reports_full_capacity() {
        let queue = EmailQueueService::start(
            Arc::new(|_task: EmailTask| async move {
                sleep(Duration::from_millis(200)).await;
                Ok(())
            }),
            EmailQueueConfig {
                workers: 1,
                capacity: 1,
                max_attempts: 1,
                retry_delay: Duration::from_millis(0),
                ..EmailQueueConfig::default()
            },
        );

        let task = EmailTask {
            email: "queued@example.com".to_owned(),
            site_name: "Sub2API".to_owned(),
            locale: None,
            kind: EmailTaskKind::VerifyCode {
                code: "654321".to_owned(),
                kind: VerificationEmailKind::Auth,
            },
        };
        queue.enqueue(task.clone()).unwrap();
        let result = queue.enqueue(task);

        assert_eq!(result, Err(EmailQueueError::Full));
        assert_eq!(queue.stats().rejected_full, 1);
        queue.shutdown().await;
    }

    #[tokio::test]
    async fn durable_queue_persists_and_marks_sent_tasks() {
        let repository = Arc::new(InMemoryRepository::new());
        let delivered = Arc::new(StdMutex::new(Vec::new()));
        let delivered_for_sender = delivered.clone();
        let config = EmailQueueConfig {
            durable: true,
            workers: 1,
            capacity: 4,
            max_attempts: 1,
            retry_delay: Duration::from_millis(0),
            ..EmailQueueConfig::default()
        };
        let queue = EmailQueueService::start(
            Arc::new(DurableEmailTaskSender {
                repository: repository.clone(),
                inner: Arc::new(move |task: EmailTask| {
                    let delivered = delivered_for_sender.clone();
                    async move {
                        delivered.lock().unwrap().push(task.email);
                        Ok(())
                    }
                }),
            }),
            config,
        );

        enqueue_email_task(
            &queue,
            Some(repository.as_ref() as &dyn AppRepository),
            EmailTask {
                email: "durable@example.com".to_owned(),
                site_name: "Sub2API".to_owned(),
                locale: None,
                kind: EmailTaskKind::PasswordReset {
                    token: "reset-token".to_owned(),
                },
            },
            Some(config),
        )
        .await
        .unwrap();

        timeout(Duration::from_secs(2), async {
            loop {
                if queue.stats().sent == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        queue.shutdown().await;

        assert_eq!(
            delivered.lock().unwrap().as_slice(),
            ["durable@example.com"]
        );
        assert!(repository
            .list_pending_email_tasks(10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn durable_queue_recovers_pending_repository_tasks() {
        let repository = Arc::new(InMemoryRepository::new());
        let record = repository
            .enqueue_email_task(EmailQueueTaskRecord {
                id: 0,
                task_type: "verify_code".to_owned(),
                status: "processing".to_owned(),
                payload: json!({
                    "email": "recover@example.com",
                    "site_name": "Sub2API",
                    "code": "123456",
                    "kind": "auth"
                }),
                attempts: 0,
                max_attempts: 2,
                last_error: None,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap();
        let delivered = Arc::new(StdMutex::new(Vec::new()));
        let delivered_for_sender = delivered.clone();
        let config = EmailQueueConfig {
            durable: true,
            recover_limit: 10,
            workers: 1,
            capacity: 4,
            max_attempts: 2,
            retry_delay: Duration::from_millis(0),
        };
        let queue = EmailQueueService::start(
            Arc::new(DurableEmailTaskSender {
                repository: repository.clone(),
                inner: Arc::new(move |task: EmailTask| {
                    let delivered = delivered_for_sender.clone();
                    async move {
                        delivered.lock().unwrap().push(task.email);
                        Ok(())
                    }
                }),
            }),
            config,
        );

        let recovered = recover_pending_email_tasks(repository.clone(), &queue, config)
            .await
            .unwrap();
        assert_eq!(recovered, 1);
        timeout(Duration::from_secs(2), async {
            loop {
                if queue.stats().sent == 1 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        queue.shutdown().await;

        assert_eq!(
            delivered.lock().unwrap().as_slice(),
            ["recover@example.com"]
        );
        assert!(repository
            .list_pending_email_tasks(10)
            .await
            .unwrap()
            .is_empty());
        let sent = repository.mark_email_task_sent(record.id).await.unwrap();
        assert_eq!(sent.status, "sent");
        assert!(sent.attempts >= 1);
    }
}
