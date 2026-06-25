use crate::email_queue::EmailQueueConfig;
use crate::setup::{DatabaseConfig, RedisConfig, ServerConfig};
use serde::Deserialize;
use std::time::Duration;
use std::{env, fs, path::PathBuf};

#[cfg(test)]
pub(crate) fn runtime_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    pub database: Option<DatabaseConfig>,
    pub redis: Option<RedisConfig>,
    pub totp: Option<TotpConfig>,
    pub email_queue: Option<EmailQueueRuntimeConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TotpConfig {
    pub encryption_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmailQueueRuntimeConfig {
    pub enabled: Option<bool>,
    pub durable: Option<bool>,
    pub recover_limit: Option<usize>,
    pub workers: Option<usize>,
    pub capacity: Option<usize>,
    pub max_attempts: Option<usize>,
    pub retry_delay_ms: Option<u64>,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        if env_flag("BACKEND_NEXT_CONFIG_DISABLED") {
            return Ok(Self::default());
        }
        let Some(path) = first_config_path() else {
            return Ok(Self::default());
        };
        let raw = fs::read_to_string(&path)?;
        let mut config: Self = serde_yaml::from_str(&raw)?;
        normalize_config(&mut config);
        Ok(config)
    }

    pub fn database_url(&self) -> Option<String> {
        env::var("DATABASE_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .or_else(|| self.database.as_ref().map(database_url_from_config))
    }

    pub fn redis_config(&self) -> RedisConfig {
        let base = self.redis.clone().unwrap_or_else(default_redis_config);
        let password = if env_flag("REDIS_PASSWORD_DISABLED") {
            None
        } else {
            match env_optional_string("REDIS_PASSWORD") {
                Some(password) => password,
                None => base.password,
            }
        };
        RedisConfig {
            host: env_string("REDIS_HOST").unwrap_or(base.host),
            port: env_u16("REDIS_PORT").unwrap_or(base.port),
            password,
            db: env_u8("REDIS_DB").or(base.db).or(Some(0)),
            enable_tls: env_bool("REDIS_ENABLE_TLS")
                .or(base.enable_tls)
                .or(Some(false)),
        }
    }

    pub fn server_bind_addr(&self) -> String {
        let host = env_string("SERVER_HOST")
            .or_else(|| self.server.host.clone())
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let port = env_u16("SERVER_PORT").or(self.server.port).unwrap_or(8081);
        format!("{host}:{port}")
    }

    pub fn totp_encryption_key(&self) -> Option<String> {
        env_string("TOTP_ENCRYPTION_KEY").or_else(|| {
            self.totp
                .as_ref()
                .and_then(|totp| totp.encryption_key.clone())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
    }

    pub fn email_queue_config(&self) -> Option<EmailQueueConfig> {
        let yaml = self.email_queue.as_ref();
        let enabled = env_bool("BACKEND_NEXT_EMAIL_QUEUE_ENABLED")
            .or_else(|| yaml.and_then(|config| config.enabled))
            .unwrap_or(false);
        if !enabled {
            return None;
        }
        Some(EmailQueueConfig {
            durable: env_bool("BACKEND_NEXT_EMAIL_QUEUE_DURABLE")
                .or_else(|| yaml.and_then(|config| config.durable))
                .unwrap_or(false),
            recover_limit: env_usize("BACKEND_NEXT_EMAIL_QUEUE_RECOVER_LIMIT")
                .or_else(|| yaml.and_then(|config| config.recover_limit))
                .unwrap_or(100),
            workers: env_usize("BACKEND_NEXT_EMAIL_QUEUE_WORKERS")
                .or_else(|| yaml.and_then(|config| config.workers))
                .unwrap_or(3),
            capacity: env_usize("BACKEND_NEXT_EMAIL_QUEUE_CAPACITY")
                .or_else(|| yaml.and_then(|config| config.capacity))
                .unwrap_or(100),
            max_attempts: env_usize("BACKEND_NEXT_EMAIL_QUEUE_MAX_ATTEMPTS")
                .or_else(|| yaml.and_then(|config| config.max_attempts))
                .unwrap_or(1),
            retry_delay: Duration::from_millis(
                env_u64("BACKEND_NEXT_EMAIL_QUEUE_RETRY_DELAY_MS")
                    .or_else(|| yaml.and_then(|config| config.retry_delay_ms))
                    .unwrap_or(100),
            ),
        })
    }
}

fn normalize_config(config: &mut AppConfig) {
    if let Some(database) = &mut config.database {
        if database.sslmode.as_deref().unwrap_or("").is_empty() {
            database.sslmode = Some("disable".to_owned());
        }
    }
    if let Some(redis) = &mut config.redis {
        if redis.db.is_none() {
            redis.db = Some(0);
        }
        if redis.enable_tls.is_none() {
            redis.enable_tls = Some(false);
        }
    }
}

fn first_config_path() -> Option<PathBuf> {
    candidate_config_paths()
        .into_iter()
        .find(|path| path.is_file())
}

fn candidate_config_paths() -> Vec<PathBuf> {
    if let Some(path) = env_string("CONFIG_PATH") {
        return vec![PathBuf::from(path)];
    }

    let mut paths = Vec::new();
    if let Some(data_dir) = env_string("DATA_DIR") {
        paths.push(PathBuf::from(data_dir).join("config.yaml"));
    }
    paths.push(PathBuf::from("/app/data/config.yaml"));
    paths.push(PathBuf::from("backend/config.yaml"));
    paths.push(PathBuf::from("../backend/config.yaml"));
    paths.push(PathBuf::from("config.yaml"));
    paths
}

fn database_url_from_config(config: &DatabaseConfig) -> String {
    let sslmode = config.sslmode.as_deref().unwrap_or("disable");
    let password = config.password.as_deref().unwrap_or("");
    format!(
        "postgres://{}:{}@{}:{}/{}?sslmode={}",
        percent_encode(&config.user),
        percent_encode(password),
        config.host,
        config.port,
        percent_encode(&config.dbname),
        percent_encode(sslmode)
    )
}

fn default_redis_config() -> RedisConfig {
    RedisConfig {
        host: "127.0.0.1".to_owned(),
        port: 6379,
        password: None,
        db: Some(0),
        enable_tls: Some(false),
    }
}

fn env_string(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_optional_string(key: &str) -> Option<Option<String>> {
    env::var(key).ok().map(|value| {
        let value = value.trim().to_owned();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    })
}

fn env_u16(key: &str) -> Option<u16> {
    env_string(key).and_then(|value| value.parse().ok())
}

fn env_u8(key: &str) -> Option<u8> {
    env_string(key).and_then(|value| value.parse().ok())
}

fn env_usize(key: &str) -> Option<usize> {
    env_string(key).and_then(|value| value.parse().ok())
}

fn env_u64(key: &str) -> Option<u64> {
    env_string(key).and_then(|value| value.parse().ok())
}

fn env_bool(key: &str) -> Option<bool> {
    env_string(key).and_then(|value| match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}

fn env_flag(key: &str) -> bool {
    env_bool(key).unwrap_or(false)
}

fn percent_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const RUNTIME_ENV_KEYS: &[&str] = &[
        "BACKEND_NEXT_CONFIG_DISABLED",
        "CONFIG_PATH",
        "DATA_DIR",
        "DATABASE_URL",
        "REDIS_HOST",
        "REDIS_PORT",
        "REDIS_PASSWORD",
        "REDIS_PASSWORD_DISABLED",
        "REDIS_DB",
        "REDIS_ENABLE_TLS",
        "SERVER_HOST",
        "SERVER_PORT",
        "TOTP_ENCRYPTION_KEY",
        "BACKEND_NEXT_EMAIL_QUEUE_ENABLED",
        "BACKEND_NEXT_EMAIL_QUEUE_DURABLE",
        "BACKEND_NEXT_EMAIL_QUEUE_RECOVER_LIMIT",
        "BACKEND_NEXT_EMAIL_QUEUE_WORKERS",
        "BACKEND_NEXT_EMAIL_QUEUE_CAPACITY",
        "BACKEND_NEXT_EMAIL_QUEUE_MAX_ATTEMPTS",
        "BACKEND_NEXT_EMAIL_QUEUE_RETRY_DELAY_MS",
    ];

    struct EnvSnapshot {
        values: Vec<(&'static str, Option<String>)>,
    }

    impl EnvSnapshot {
        fn capture() -> Self {
            Self {
                values: RUNTIME_ENV_KEYS
                    .iter()
                    .map(|key| (*key, env::var(key).ok()))
                    .collect(),
            }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    fn temp_config_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir()
            .join("backend_next_config_tests")
            .join(format!("{name}-{unique}.yaml"))
    }

    fn clear_runtime_env() {
        for key in RUNTIME_ENV_KEYS {
            env::remove_var(key);
        }
    }

    #[test]
    fn loads_yaml_database_redis_and_server_config() {
        let _guard = runtime_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvSnapshot::capture();
        clear_runtime_env();
        let path = temp_config_path("full");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"
server:
  host: "0.0.0.0"
  port: 9090
database:
  host: "127.0.0.1"
  port: 5432
  user: "test"
  password: "123456"
  dbname: "sub2api_new"
redis:
  host: "127.0.0.1"
  port: 6379
  password: "123456"
  db: 0
totp:
  encryption_key: "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
email_queue:
  enabled: true
  durable: true
  recover_limit: 32
  workers: 2
  capacity: 16
  max_attempts: 3
  retry_delay_ms: 250
"#,
        )
        .unwrap();
        env::set_var("CONFIG_PATH", &path);

        let config = AppConfig::load().unwrap();

        assert_eq!(
            config.database_url().unwrap(),
            "postgres://test:123456@127.0.0.1:5432/sub2api_new?sslmode=disable"
        );
        assert_eq!(config.redis_config().password.as_deref(), Some("123456"));
        assert_eq!(config.server_bind_addr(), "0.0.0.0:9090");
        assert_eq!(
            config.totp_encryption_key().as_deref(),
            Some("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
        );
        let email_queue = config.email_queue_config().unwrap();
        assert_eq!(email_queue.durable, true);
        assert_eq!(email_queue.recover_limit, 32);
        assert_eq!(email_queue.workers, 2);
        assert_eq!(email_queue.capacity, 16);
        assert_eq!(email_queue.max_attempts, 3);
        assert_eq!(email_queue.retry_delay, Duration::from_millis(250));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn environment_values_override_yaml_runtime_config() {
        let _guard = runtime_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvSnapshot::capture();
        clear_runtime_env();
        let path = temp_config_path("override");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"
server:
  host: "0.0.0.0"
  port: 8080
database:
  host: "db"
  port: 5432
  user: "yaml"
  password: "yaml"
  dbname: "yaml_db"
redis:
  host: "redis"
  port: 6379
  password: "yaml"
  db: 1
"#,
        )
        .unwrap();
        env::set_var("CONFIG_PATH", &path);
        env::set_var("DATABASE_URL", "postgres://env/db");
        env::set_var("REDIS_PASSWORD", "");
        env::set_var("REDIS_PASSWORD_DISABLED", "1");
        env::set_var("REDIS_DB", "2");
        env::set_var("SERVER_PORT", "18080");
        env::set_var(
            "TOTP_ENCRYPTION_KEY",
            "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
        );
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_ENABLED", "true");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_DURABLE", "true");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_RECOVER_LIMIT", "12");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_WORKERS", "4");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_CAPACITY", "64");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_MAX_ATTEMPTS", "5");
        env::set_var("BACKEND_NEXT_EMAIL_QUEUE_RETRY_DELAY_MS", "25");

        let config = AppConfig::load().unwrap();

        assert_eq!(config.database_url().as_deref(), Some("postgres://env/db"));
        let redis = config.redis_config();
        assert_eq!(redis.password, None);
        assert_eq!(redis.db, Some(2));
        assert_eq!(config.server_bind_addr(), "0.0.0.0:18080");
        assert_eq!(
            config.totp_encryption_key().as_deref(),
            Some("202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f")
        );
        let email_queue = config.email_queue_config().unwrap();
        assert_eq!(email_queue.durable, true);
        assert_eq!(email_queue.recover_limit, 12);
        assert_eq!(email_queue.workers, 4);
        assert_eq!(email_queue.capacity, 64);
        assert_eq!(email_queue.max_attempts, 5);
        assert_eq!(email_queue.retry_delay, Duration::from_millis(25));

        let _ = fs::remove_file(path);
    }
}
