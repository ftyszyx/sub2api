use repository::AppRepository;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccountScheduleOptions {
    pub max_concurrent: Option<usize>,
    pub failure_cooldown_seconds: u64,
    pub max_failure_cooldown_seconds: u64,
}

impl Default for AccountScheduleOptions {
    fn default() -> Self {
        Self {
            max_concurrent: None,
            failure_cooldown_seconds: 30,
            max_failure_cooldown_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountScheduleRejection {
    CoolingDown { retry_after_seconds: u64 },
    ConcurrentLimit { max_concurrent: usize },
}

impl AccountScheduleRejection {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::CoolingDown {
                retry_after_seconds,
            } => {
                format!(
                    "upstream account is cooling down; retry after {retry_after_seconds} seconds"
                )
            }
            Self::ConcurrentLimit { max_concurrent } => {
                format!("upstream account concurrency limit reached: {max_concurrent}")
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct AccountRuntimeScheduler {
    inner: Arc<Mutex<HashMap<i64, AccountRuntimeState>>>,
}

#[derive(Debug, Clone, Default)]
struct AccountRuntimeState {
    in_flight: usize,
    consecutive_failures: u32,
    cooling_until_unix: Option<u64>,
    last_error: Option<String>,
    total_successes: u64,
    total_failures: u64,
    metadata: AccountRuntimeMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccountRuntimeMetadata {
    pub account_name: Option<String>,
    pub platform: Option<String>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    pub max_concurrent: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountRuntimeSnapshot {
    pub account_id: i64,
    pub account_name: Option<String>,
    pub platform: Option<String>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    pub max_concurrent: Option<usize>,
    pub in_flight: usize,
    pub consecutive_failures: u32,
    pub cooling_until_unix: Option<u64>,
    pub retry_after_seconds: Option<u64>,
    pub last_error: Option<String>,
    pub total_successes: u64,
    pub total_failures: u64,
}

pub(crate) struct AccountAttemptGuard {
    scheduler: AccountRuntimeScheduler,
    account_id: i64,
    request_id: Option<String>,
    repository: Option<Arc<dyn AppRepository>>,
    released: bool,
}

impl AccountRuntimeScheduler {
    #[cfg(test)]
    pub(crate) fn try_acquire(
        &self,
        account_id: i64,
        options: AccountScheduleOptions,
    ) -> Result<AccountAttemptGuard, AccountScheduleRejection> {
        self.try_acquire_with_metadata(account_id, options, AccountRuntimeMetadata::default())
    }

    pub(crate) fn try_acquire_with_metadata(
        &self,
        account_id: i64,
        options: AccountScheduleOptions,
        metadata: AccountRuntimeMetadata,
    ) -> Result<AccountAttemptGuard, AccountScheduleRejection> {
        let now = now_unix();
        let mut states = self.inner.lock().expect("account runtime scheduler lock");
        let state = states.entry(account_id).or_default();
        state.metadata.merge(metadata);
        if state.metadata.max_concurrent.is_none() {
            state.metadata.max_concurrent = options.max_concurrent;
        }
        if let Some(cooling_until) = state.cooling_until_unix {
            if cooling_until > now {
                return Err(AccountScheduleRejection::CoolingDown {
                    retry_after_seconds: cooling_until - now,
                });
            }
            state.cooling_until_unix = None;
            state.last_error = None;
        }
        if let Some(max_concurrent) = options.max_concurrent {
            if max_concurrent > 0 && state.in_flight >= max_concurrent {
                return Err(AccountScheduleRejection::ConcurrentLimit { max_concurrent });
            }
        }
        state.in_flight += 1;
        Ok(AccountAttemptGuard {
            scheduler: self.clone(),
            account_id,
            request_id: None,
            repository: None,
            released: false,
        })
    }

    pub(crate) fn mark_success(&self, account_id: i64) {
        let mut states = self.inner.lock().expect("account runtime scheduler lock");
        let state = states.entry(account_id).or_default();
        state.consecutive_failures = 0;
        state.cooling_until_unix = None;
        state.last_error = None;
        state.total_successes += 1;
    }

    pub(crate) fn mark_failure(
        &self,
        account_id: i64,
        error: impl Into<String>,
        options: AccountScheduleOptions,
    ) {
        let now = now_unix();
        let mut states = self.inner.lock().expect("account runtime scheduler lock");
        let state = states.entry(account_id).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.total_failures += 1;
        state.last_error = Some(error.into());
        let multiplier = 2_u64
            .saturating_pow(state.consecutive_failures.saturating_sub(1).min(8))
            .max(1);
        let cooldown = options
            .failure_cooldown_seconds
            .saturating_mul(multiplier)
            .min(options.max_failure_cooldown_seconds.max(1));
        state.cooling_until_unix = Some(now.saturating_add(cooldown));
    }

    fn release(&self, account_id: i64) {
        let mut states = self.inner.lock().expect("account runtime scheduler lock");
        let state = states.entry(account_id).or_default();
        state.in_flight = state.in_flight.saturating_sub(1);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self, account_id: i64) -> Option<AccountRuntimeSnapshot> {
        let now = now_unix();
        self.inner
            .lock()
            .expect("account runtime scheduler lock")
            .get(&account_id)
            .map(|state| runtime_snapshot(account_id, state, now))
    }

    pub(crate) fn snapshots(&self) -> HashMap<i64, AccountRuntimeSnapshot> {
        let now = now_unix();
        self.inner
            .lock()
            .expect("account runtime scheduler lock")
            .iter()
            .map(|(account_id, state)| (*account_id, runtime_snapshot(*account_id, state, now)))
            .collect()
    }
}

impl AccountAttemptGuard {
    pub(crate) fn with_repository_slot(
        mut self,
        repository: Arc<dyn AppRepository>,
        request_id: String,
    ) -> Self {
        self.repository = Some(repository);
        self.request_id = Some(request_id);
        self
    }
}

impl AccountRuntimeMetadata {
    fn merge(&mut self, update: Self) {
        if update.account_name.is_some() {
            self.account_name = update.account_name;
        }
        if update.platform.is_some() {
            self.platform = update.platform;
        }
        if update.group_id.is_some() {
            self.group_id = update.group_id;
        }
        if update.group_name.is_some() {
            self.group_name = update.group_name;
        }
        if update.max_concurrent.is_some() {
            self.max_concurrent = update.max_concurrent;
        }
    }
}

impl Drop for AccountAttemptGuard {
    fn drop(&mut self) {
        if !self.released {
            self.scheduler.release(self.account_id);
            if let (Some(repository), Some(request_id)) =
                (self.repository.clone(), self.request_id.clone())
            {
                let account_id = self.account_id;
                tokio::spawn(async move {
                    if let Err(error) = repository
                        .release_account_concurrency_slot(account_id, &request_id)
                        .await
                    {
                        tracing::warn!(
                            account_id,
                            request_id = %request_id,
                            error = %error,
                            "failed to release postgres account concurrency slot"
                        );
                    }
                });
            }
            self.released = true;
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn runtime_snapshot(
    account_id: i64,
    state: &AccountRuntimeState,
    now: u64,
) -> AccountRuntimeSnapshot {
    AccountRuntimeSnapshot {
        account_id,
        account_name: state.metadata.account_name.clone(),
        platform: state.metadata.platform.clone(),
        group_id: state.metadata.group_id,
        group_name: state.metadata.group_name.clone(),
        max_concurrent: state.metadata.max_concurrent,
        in_flight: state.in_flight,
        consecutive_failures: state.consecutive_failures,
        cooling_until_unix: state.cooling_until_unix,
        retry_after_seconds: state
            .cooling_until_unix
            .and_then(|cooling_until| cooling_until.checked_sub(now)),
        last_error: state.last_error.clone(),
        total_successes: state.total_successes,
        total_failures: state.total_failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_releases_in_flight_on_drop() {
        let scheduler = AccountRuntimeScheduler::default();
        let guard = scheduler
            .try_acquire(1, AccountScheduleOptions::default())
            .unwrap();
        assert_eq!(scheduler.snapshot(1).unwrap().in_flight, 1);

        drop(guard);

        assert_eq!(scheduler.snapshot(1).unwrap().in_flight, 0);
    }

    #[test]
    fn concurrent_limit_rejects_until_guard_is_released() {
        let scheduler = AccountRuntimeScheduler::default();
        let options = AccountScheduleOptions {
            max_concurrent: Some(1),
            ..AccountScheduleOptions::default()
        };
        let guard = scheduler.try_acquire(1, options).unwrap();

        assert!(matches!(
            scheduler.try_acquire(1, options),
            Err(AccountScheduleRejection::ConcurrentLimit { max_concurrent: 1 })
        ));

        drop(guard);
        assert!(scheduler.try_acquire(1, options).is_ok());
    }

    #[test]
    fn failure_cools_account_and_success_clears_failure_state() {
        let scheduler = AccountRuntimeScheduler::default();
        let options = AccountScheduleOptions {
            failure_cooldown_seconds: 10,
            max_failure_cooldown_seconds: 60,
            ..AccountScheduleOptions::default()
        };
        scheduler.mark_failure(1, "upstream 502", options);
        let snapshot = scheduler.snapshot(1).unwrap();
        assert_eq!(snapshot.consecutive_failures, 1);
        assert_eq!(snapshot.total_failures, 1);
        assert!(snapshot.cooling_until_unix.is_some());
        assert!(matches!(
            scheduler.try_acquire(1, options),
            Err(AccountScheduleRejection::CoolingDown { .. })
        ));

        scheduler.mark_success(1);

        let snapshot = scheduler.snapshot(1).unwrap();
        assert_eq!(snapshot.consecutive_failures, 0);
        assert_eq!(snapshot.total_successes, 1);
        assert!(snapshot.cooling_until_unix.is_none());
    }
}
