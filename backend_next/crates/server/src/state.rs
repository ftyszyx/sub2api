use crate::admin_ops::AdminOpsService;
use crate::admin_portal::AdminPortalService;
use crate::auth::{AuthService, UserRecord};
use crate::config::AppConfig;
use crate::email_delivery::EmailDeliveryService;
use crate::email_queue::{EmailDispatchService, EmailQueueConfig};
use crate::gateway_runtime::GatewayRuntimeService;
use crate::pages::PageService;
use crate::password_reset_store::{
    store_from_config as password_reset_store_from_config, DynPasswordResetStore,
    MemoryPasswordResetStore,
};
use crate::payment_portal::PaymentPortalService;
use crate::setup::SetupService;
use crate::totp_secret::TotpSecretEncryptor;
use crate::user_portal::UserPortalService;
use crate::verification_code_store::{
    store_from_config as verification_store_from_config, DynVerificationCodeStore,
    MemoryVerificationCodeStore,
};
use domain::{ApiKey, ApiKeyId, ApiKeyStatus, Group, GroupId, GroupStatus};
use repository::AuthCredentialRecord;
use repository::{AppRepository, InMemoryRepository, PostgresRepository};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub admin_ops: Arc<AdminOpsService>,
    pub admin_portal: Arc<AdminPortalService>,
    pub auth: Arc<AuthService>,
    pub email_dispatch: Arc<EmailDispatchService>,
    pub email_delivery: Arc<EmailDeliveryService>,
    pub gateway_runtime: Arc<GatewayRuntimeService>,
    pub pages: Arc<PageService>,
    pub payment_portal: Arc<PaymentPortalService>,
    pub repository: Arc<dyn AppRepository>,
    pub setup: Arc<SetupService>,
    pub totp_secret: Arc<TotpSecretEncryptor>,
    pub user_portal: Arc<UserPortalService>,
}

const DEMO_TOTP_ENCRYPTION_KEY: &str =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

impl AppState {
    pub fn demo() -> Self {
        Self::with_setup(SetupService::installed_memory())
    }

    pub fn setup_demo() -> Self {
        Self::with_setup(SetupService::setup_memory())
    }

    pub fn with_disabled_totp_encryption_for_tests() -> Self {
        let repository = Arc::new(InMemoryRepository::new());
        seed_in_memory_demo_gateway_key(repository.as_ref());
        Self::with_setup_repository_and_gateway(
            SetupService::installed_memory(),
            repository,
            Arc::new(GatewayRuntimeService::new()),
            Arc::new(TotpSecretEncryptor::disabled()),
        )
    }

    pub async fn production() -> anyhow::Result<Self> {
        let config = AppConfig::load()?;
        let repository = production_repository(&config).await?;
        let gateway_runtime = GatewayRuntimeService::from_config(&config).await?;
        let verification_codes = verification_store_from_config(&config).await?;
        let password_resets = password_reset_store_from_config(&config).await?;
        let totp_secret =
            TotpSecretEncryptor::from_hex_key(config.totp_encryption_key().as_deref())?;
        let email_queue_config = config.email_queue_config();
        Ok(
            Self::with_setup_repository_gateway_auth_stores_and_email_queue(
                SetupService::from_environment(),
                repository,
                Arc::new(gateway_runtime),
                Arc::new(totp_secret),
                verification_codes,
                password_resets,
                email_queue_config,
            )
            .await,
        )
    }

    fn with_setup(setup: SetupService) -> Self {
        let repository = Arc::new(InMemoryRepository::new());
        seed_in_memory_demo_gateway_key(repository.as_ref());
        let totp_secret = TotpSecretEncryptor::from_hex_key(Some(DEMO_TOTP_ENCRYPTION_KEY))
            .expect("demo totp encryption key must be valid");
        Self::with_setup_repository_gateway_auth_stores_and_email_queue_sync(
            setup,
            repository,
            Arc::new(GatewayRuntimeService::new()),
            Arc::new(totp_secret),
            MemoryVerificationCodeStore::shared(),
            MemoryPasswordResetStore::shared(),
            None,
        )
    }

    fn with_setup_repository_and_gateway(
        setup: SetupService,
        repository: Arc<dyn AppRepository>,
        gateway_runtime: Arc<GatewayRuntimeService>,
        totp_secret: Arc<TotpSecretEncryptor>,
    ) -> Self {
        Self::with_setup_repository_gateway_auth_stores_and_email_queue_sync(
            setup,
            repository,
            gateway_runtime,
            totp_secret,
            MemoryVerificationCodeStore::shared(),
            MemoryPasswordResetStore::shared(),
            None,
        )
    }

    fn with_setup_repository_gateway_auth_stores_and_email_queue_sync(
        setup: SetupService,
        repository: Arc<dyn AppRepository>,
        gateway_runtime: Arc<GatewayRuntimeService>,
        totp_secret: Arc<TotpSecretEncryptor>,
        verification_codes: DynVerificationCodeStore,
        password_resets: DynPasswordResetStore,
        email_queue_config: Option<EmailQueueConfig>,
    ) -> Self {
        let email_dispatch = match email_queue_config {
            Some(config) => Arc::new(EmailDispatchService::queued(
                Arc::new(EmailDeliveryService::new(repository.clone())),
                config,
            )),
            None => Arc::new(EmailDispatchService::synchronous(Arc::new(
                EmailDeliveryService::new(repository.clone()),
            ))),
        };
        Self::build(
            setup,
            repository,
            gateway_runtime,
            totp_secret,
            verification_codes,
            password_resets,
            email_dispatch,
            true,
        )
    }

    async fn with_setup_repository_gateway_auth_stores_and_email_queue(
        setup: SetupService,
        repository: Arc<dyn AppRepository>,
        gateway_runtime: Arc<GatewayRuntimeService>,
        totp_secret: Arc<TotpSecretEncryptor>,
        verification_codes: DynVerificationCodeStore,
        password_resets: DynPasswordResetStore,
        email_queue_config: Option<EmailQueueConfig>,
    ) -> Self {
        let email_delivery = Arc::new(EmailDeliveryService::new(repository.clone()));
        let email_dispatch = match email_queue_config {
            Some(config) if config.durable => Arc::new(
                EmailDispatchService::queued_with_repository(
                    email_delivery.clone(),
                    repository.clone(),
                    config,
                )
                .await,
            ),
            Some(config) => Arc::new(EmailDispatchService::queued(email_delivery.clone(), config)),
            None => Arc::new(EmailDispatchService::synchronous(email_delivery.clone())),
        };
        Self::build(
            setup,
            repository,
            gateway_runtime,
            totp_secret,
            verification_codes,
            password_resets,
            email_dispatch,
            false,
        )
    }

    fn build(
        setup: SetupService,
        repository: Arc<dyn AppRepository>,
        gateway_runtime: Arc<GatewayRuntimeService>,
        totp_secret: Arc<TotpSecretEncryptor>,
        verification_codes: DynVerificationCodeStore,
        password_resets: DynPasswordResetStore,
        email_dispatch: Arc<EmailDispatchService>,
        seed_ops_demo_data: bool,
    ) -> Self {
        let auth = AuthService::with_auth_stores(verification_codes, password_resets);
        auth.insert_seed_user(UserRecord::new(
            1,
            "admin@example.com",
            "admin",
            "admin",
            "admin123",
        ));
        let email_delivery = Arc::new(EmailDeliveryService::new(repository.clone()));
        let admin_ops = if seed_ops_demo_data {
            AdminOpsService::demo()
        } else {
            AdminOpsService::new()
        };
        Self {
            admin_ops: Arc::new(admin_ops),
            admin_portal: Arc::new(AdminPortalService::new()),
            auth: Arc::new(auth),
            email_dispatch,
            email_delivery,
            gateway_runtime,
            pages: Arc::new(PageService::new()),
            payment_portal: Arc::new(PaymentPortalService::new()),
            repository,
            setup: Arc::new(setup),
            totp_secret,
            user_portal: Arc::new(UserPortalService::new()),
        }
    }

    #[cfg(test)]
    pub fn with_test_email_sender(
        sender: Arc<dyn crate::email_queue::EmailTaskSender>,
        config: EmailQueueConfig,
    ) -> Self {
        let mut state = Self::demo();
        state.email_dispatch = Arc::new(EmailDispatchService::queued_with_sender(
            state.email_delivery.clone(),
            sender,
            config,
        ));
        state
    }

    #[cfg(test)]
    pub async fn with_test_durable_email_sender(
        sender: Arc<dyn crate::email_queue::EmailTaskSender>,
        config: EmailQueueConfig,
    ) -> Self {
        let mut state = Self::demo();
        let email_delivery = state.email_delivery.clone();
        let repository = state.repository.clone();
        state.email_dispatch = Arc::new(
            crate::email_queue::EmailDispatchService::queued_with_repository_sender(
                email_delivery,
                repository,
                sender,
                config,
            )
            .await,
        );
        state
    }
}

fn seed_in_memory_demo_gateway_key(repository: &InMemoryRepository) {
    repository.seed_gateway_principals(
        repository::UserRecord {
            id: 1,
            email: "admin@example.com".to_owned(),
            username: "admin".to_owned(),
            role: "admin".to_owned(),
            status: "active".to_owned(),
        },
        Group {
            id: GroupId(1),
            name: "Demo Gateway Group".to_owned(),
            status: GroupStatus::Active,
        },
        ApiKey {
            id: ApiKeyId(0),
            user_id: 1,
            key: "sk-dev-admin".to_owned(),
            name: "backend_next development key".to_owned(),
            group_id: Some(GroupId(1)),
            status: ApiKeyStatus::Active,
            ..ApiKey::default()
        },
        Some(seed_admin_auth_credential()),
    );
}

async fn production_repository(config: &AppConfig) -> anyhow::Result<Arc<dyn AppRepository>> {
    match config.database_url() {
        Some(database_url) if !database_url.trim().is_empty() => {
            let repository = PostgresRepository::connect(&database_url).await?;
            let repository: Arc<dyn AppRepository> = Arc::new(repository);
            seed_repository_admin_principal(repository.as_ref()).await?;
            Ok(repository)
        }
        _ => {
            let repository = Arc::new(InMemoryRepository::new());
            seed_in_memory_demo_gateway_key(repository.as_ref());
            Ok(repository)
        }
    }
}

fn seed_admin_user_record() -> repository::UserRecord {
    repository::UserRecord {
        id: 1,
        email: "admin@example.com".to_owned(),
        username: "admin".to_owned(),
        role: "admin".to_owned(),
        status: "active".to_owned(),
    }
}

fn seed_admin_auth_credential() -> AuthCredentialRecord {
    AuthCredentialRecord {
        user_id: 1,
        email: "admin@example.com".to_owned(),
        password_hash: crate::auth::hash_password_for_repository("admin123")
            .expect("seed admin password must be valid"),
        status: "active".to_owned(),
        updated_at_unix: 1_780_704_000,
    }
}

async fn seed_repository_admin_principal(repository: &dyn AppRepository) -> anyhow::Result<()> {
    repository.upsert_user(seed_admin_user_record()).await?;
    repository
        .upsert_auth_credential(seed_admin_auth_credential())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{test_redis_connection, RedisConfig};

    #[tokio::test]
    async fn app_state_exposes_repository_for_persistent_domain_migration() {
        let state = AppState::demo();
        assert_eq!(
            state
                .repository
                .get_api_key_by_key("sk-dev-admin")
                .await
                .unwrap()
                .name,
            "backend_next development key"
        );
        let seed_credential = state
            .repository
            .get_auth_credential_by_email("ADMIN@example.com")
            .await
            .unwrap();
        assert_eq!(seed_credential.user_id, 1);
        assert!(crate::auth::verify_repository_password(
            "admin123",
            &seed_credential.password_hash
        ));
        state
            .repository
            .upsert_group(Group {
                id: GroupId(42),
                name: "repository-backed-group".to_owned(),
                status: GroupStatus::Active,
            })
            .await
            .unwrap();

        let groups = state.repository.list_active_groups().await.unwrap();
        assert!(groups
            .iter()
            .any(|group| group.id == GroupId(42) && group.name == "repository-backed-group"));
    }

    #[tokio::test]
    async fn production_state_uses_in_memory_repository_when_database_url_is_absent() {
        let _guard = crate::config::runtime_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original_database_url = std::env::var("DATABASE_URL").ok();
        let original_config_disabled = std::env::var("BACKEND_NEXT_CONFIG_DISABLED").ok();
        let original_state_store = std::env::var("BACKEND_NEXT_RESPONSES_STATE_STORE").ok();
        let original_code_store = std::env::var("BACKEND_NEXT_VERIFICATION_CODE_STORE").ok();
        let original_reset_store = std::env::var("BACKEND_NEXT_PASSWORD_RESET_STORE").ok();
        let original_redis_password_disabled = std::env::var("REDIS_PASSWORD_DISABLED").ok();
        std::env::remove_var("DATABASE_URL");
        std::env::set_var("BACKEND_NEXT_CONFIG_DISABLED", "1");
        std::env::set_var("BACKEND_NEXT_RESPONSES_STATE_STORE", "memory");
        std::env::set_var("BACKEND_NEXT_VERIFICATION_CODE_STORE", "memory");
        std::env::set_var("BACKEND_NEXT_PASSWORD_RESET_STORE", "memory");
        std::env::remove_var("REDIS_PASSWORD_DISABLED");

        let state = AppState::production().await.unwrap();
        assert_eq!(
            state
                .repository
                .get_api_key_by_key("sk-dev-admin")
                .await
                .unwrap()
                .user_id,
            1
        );
        assert_eq!(
            state
                .repository
                .get_auth_credential_by_email("admin@example.com")
                .await
                .unwrap()
                .user_id,
            1
        );

        if let Some(database_url) = original_database_url {
            std::env::set_var("DATABASE_URL", database_url);
        }
        if let Some(value) = original_config_disabled {
            std::env::set_var("BACKEND_NEXT_CONFIG_DISABLED", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_CONFIG_DISABLED");
        }
        if let Some(value) = original_state_store {
            std::env::set_var("BACKEND_NEXT_RESPONSES_STATE_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_RESPONSES_STATE_STORE");
        }
        if let Some(value) = original_code_store {
            std::env::set_var("BACKEND_NEXT_VERIFICATION_CODE_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_VERIFICATION_CODE_STORE");
        }
        if let Some(value) = original_reset_store {
            std::env::set_var("BACKEND_NEXT_PASSWORD_RESET_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_PASSWORD_RESET_STORE");
        }
        if let Some(value) = original_redis_password_disabled {
            std::env::set_var("REDIS_PASSWORD_DISABLED", value);
        } else {
            std::env::remove_var("REDIS_PASSWORD_DISABLED");
        }
        state
            .repository
            .upsert_group(Group {
                id: GroupId(7),
                name: "fallback-memory-group".to_owned(),
                status: GroupStatus::Active,
            })
            .await
            .unwrap();
        assert!(state
            .repository
            .list_active_groups()
            .await
            .unwrap()
            .iter()
            .any(|group| group.id == GroupId(7) && group.name == "fallback-memory-group"));
    }

    #[tokio::test]
    async fn external_postgres_and_redis_dependencies_are_reachable_when_enabled() {
        if std::env::var("BACKEND_NEXT_EXTERNAL_DEPS").ok().as_deref() != Some("1") {
            return;
        }
        let _guard = crate::config::runtime_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required when BACKEND_NEXT_EXTERNAL_DEPS=1");
        let original_database_url = std::env::var("DATABASE_URL").ok();
        let original_config_disabled = std::env::var("BACKEND_NEXT_CONFIG_DISABLED").ok();
        let original_state_store = std::env::var("BACKEND_NEXT_RESPONSES_STATE_STORE").ok();
        let original_code_store = std::env::var("BACKEND_NEXT_VERIFICATION_CODE_STORE").ok();
        let original_reset_store = std::env::var("BACKEND_NEXT_PASSWORD_RESET_STORE").ok();
        let original_redis_password_disabled = std::env::var("REDIS_PASSWORD_DISABLED").ok();
        std::env::set_var("DATABASE_URL", database_url);
        std::env::remove_var("BACKEND_NEXT_CONFIG_DISABLED");
        std::env::set_var("BACKEND_NEXT_VERIFICATION_CODE_STORE", "redis");
        std::env::set_var("BACKEND_NEXT_PASSWORD_RESET_STORE", "redis");
        if std::env::var("REDIS_PASSWORD")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            std::env::set_var("REDIS_PASSWORD_DISABLED", "1");
        }

        let state = AppState::production().await.unwrap();
        state
            .repository
            .upsert_group(Group {
                id: GroupId(9_999),
                name: "external-postgres-smoke".to_owned(),
                status: GroupStatus::Active,
            })
            .await
            .unwrap();
        assert!(state
            .repository
            .list_active_groups()
            .await
            .unwrap()
            .iter()
            .any(|group| group.id == GroupId(9_999)));

        test_redis_connection(&RedisConfig {
            host: std::env::var("REDIS_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            port: std::env::var("REDIS_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(6379),
            password: std::env::var("REDIS_PASSWORD")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            db: std::env::var("REDIS_DB")
                .ok()
                .and_then(|value| value.parse().ok())
                .or(Some(0)),
            enable_tls: Some(false),
        })
        .unwrap();

        state
            .auth
            .send_notify_email_code(1, "external-redis-code@example.com".to_owned())
            .unwrap();
        assert!(state
            .auth
            .notify_code_for_tests("external-redis-code@example.com")
            .is_some());
        let prepared_reset = state
            .auth
            .prepare_password_reset(crate::auth::EmailCodeRequest {
                email: "admin@example.com".to_owned(),
                turnstile_token: None,
            })
            .unwrap()
            .expect("seed admin can request password reset");
        state
            .auth
            .commit_prepared_password_reset(&prepared_reset)
            .unwrap();
        assert!(state
            .auth
            .password_reset_token_for_tests("admin@example.com")
            .is_some());

        if let Some(database_url) = original_database_url {
            std::env::set_var("DATABASE_URL", database_url);
        } else {
            std::env::remove_var("DATABASE_URL");
        }
        if let Some(value) = original_config_disabled {
            std::env::set_var("BACKEND_NEXT_CONFIG_DISABLED", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_CONFIG_DISABLED");
        }
        if let Some(value) = original_state_store {
            std::env::set_var("BACKEND_NEXT_RESPONSES_STATE_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_RESPONSES_STATE_STORE");
        }
        if let Some(value) = original_code_store {
            std::env::set_var("BACKEND_NEXT_VERIFICATION_CODE_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_VERIFICATION_CODE_STORE");
        }
        if let Some(value) = original_reset_store {
            std::env::set_var("BACKEND_NEXT_PASSWORD_RESET_STORE", value);
        } else {
            std::env::remove_var("BACKEND_NEXT_PASSWORD_RESET_STORE");
        }
        if let Some(value) = original_redis_password_disabled {
            std::env::set_var("REDIS_PASSWORD_DISABLED", value);
        } else {
            std::env::remove_var("REDIS_PASSWORD_DISABLED");
        }
    }
}
