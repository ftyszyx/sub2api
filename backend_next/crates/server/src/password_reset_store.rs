use crate::config::AppConfig;
use crate::redis_client::{RedisClient, RespValue};
use crate::response::ApiError;
use crate::setup::RedisConfig;
use anyhow::{anyhow, Context};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

pub(crate) const PASSWORD_RESET_TOKEN_TTL_SECONDS: i64 = 30 * 60;
const PASSWORD_RESET_EMAIL_COOLDOWN_SECONDS: i64 = 30;
const DEFAULT_REDIS_KEY_PREFIX: &str = "";

pub(crate) trait PasswordResetStore: Send + Sync {
    fn reusable_token(&self, email: &str, now: i64) -> Result<Option<String>, ApiError>;
    fn save_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError>;
    fn consume_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError>;
    fn is_email_in_cooldown(&self, email: &str, now: i64) -> Result<bool, ApiError>;
    fn mark_email_sent(&self, email: &str, now: i64) -> Result<(), ApiError>;

    #[cfg(test)]
    fn token_for_tests(&self, email: &str) -> Option<String>;
}

pub(crate) type DynPasswordResetStore = Arc<dyn PasswordResetStore>;

#[derive(Default)]
pub(crate) struct MemoryPasswordResetStore {
    tokens: RwLock<HashMap<String, PasswordResetTokenEntry>>,
    sent_cooldowns: RwLock<HashMap<String, i64>>,
}

impl MemoryPasswordResetStore {
    pub(crate) fn shared() -> DynPasswordResetStore {
        Arc::new(Self::default())
    }
}

impl PasswordResetStore for MemoryPasswordResetStore {
    fn reusable_token(&self, email: &str, now: i64) -> Result<Option<String>, ApiError> {
        let key = normalize_email_key(email);
        let mut tokens = self.tokens.write().expect("password reset token lock");
        if let Some(existing) = tokens.get(&key) {
            if existing.created_at + PASSWORD_RESET_TOKEN_TTL_SECONDS > now {
                return Ok(Some(existing.token.clone()));
            }
            tokens.remove(&key);
        }
        Ok(None)
    }

    fn save_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError> {
        self.tokens
            .write()
            .expect("password reset token lock")
            .insert(
                normalize_email_key(email),
                PasswordResetTokenEntry {
                    token: token.to_owned(),
                    created_at: now,
                },
            );
        Ok(())
    }

    fn consume_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError> {
        let key = normalize_email_key(email);
        let mut tokens = self.tokens.write().expect("password reset token lock");
        let Some(entry) = tokens.get(&key) else {
            return Err(invalid_reset_token());
        };
        if entry.created_at + PASSWORD_RESET_TOKEN_TTL_SECONDS <= now {
            tokens.remove(&key);
            return Err(invalid_reset_token());
        }
        if !constant_time_eq(entry.token.as_bytes(), token.trim().as_bytes()) {
            return Err(invalid_reset_token());
        }
        tokens.remove(&key);
        Ok(())
    }

    fn is_email_in_cooldown(&self, email: &str, now: i64) -> Result<bool, ApiError> {
        let key = normalize_email_key(email);
        let mut cooldowns = self
            .sent_cooldowns
            .write()
            .expect("password reset cooldown lock");
        if let Some(sent_at) = cooldowns.get(&key).copied() {
            if sent_at + PASSWORD_RESET_EMAIL_COOLDOWN_SECONDS > now {
                return Ok(true);
            }
            cooldowns.remove(&key);
        }
        Ok(false)
    }

    fn mark_email_sent(&self, email: &str, now: i64) -> Result<(), ApiError> {
        self.sent_cooldowns
            .write()
            .expect("password reset cooldown lock")
            .insert(normalize_email_key(email), now);
        Ok(())
    }

    #[cfg(test)]
    fn token_for_tests(&self, email: &str) -> Option<String> {
        self.tokens
            .read()
            .expect("password reset token lock")
            .get(&normalize_email_key(email))
            .map(|entry| entry.token.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RedisPasswordResetStore {
    config: RedisConfig,
    key_prefix: String,
}

impl RedisPasswordResetStore {
    pub(crate) fn from_app_config(config: &AppConfig) -> Option<Self> {
        if !password_reset_redis_enabled() {
            return None;
        }
        Some(Self {
            config: config.redis_config(),
            key_prefix: std::env::var("BACKEND_NEXT_PASSWORD_RESET_REDIS_PREFIX")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_REDIS_KEY_PREFIX.to_owned()),
        })
    }

    pub(crate) async fn ping(&self) -> anyhow::Result<()> {
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let mut client = RedisClient::connect(&config)?;
            match client.command(&["PING"])? {
                RespValue::Simple(value) if value.eq_ignore_ascii_case("PONG") => Ok(()),
                other => Err(anyhow!("unexpected Redis PING response: {other:?}")),
            }
        })
        .await
        .context("Redis password-reset ping task failed")?
    }

    fn token_key(&self, email: &str) -> String {
        password_reset_key(&self.key_prefix, email)
    }

    fn sent_key(&self, email: &str) -> String {
        password_reset_sent_key(&self.key_prefix, email)
    }
}

impl PasswordResetStore for RedisPasswordResetStore {
    fn reusable_token(&self, email: &str, now: i64) -> Result<Option<String>, ApiError> {
        let key = self.token_key(email);
        let Some(entry) = get_redis_token_once(&self.config, &key).map_err(redis_error)? else {
            return Ok(None);
        };
        if entry.created_at + PASSWORD_RESET_TOKEN_TTL_SECONDS <= now {
            let _ = redis_command(&self.config, &["DEL", &key]).map_err(redis_error)?;
            return Ok(None);
        }
        Ok(Some(entry.token))
    }

    fn save_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError> {
        save_redis_token_once(&self.config, &self.token_key(email), token, now).map_err(redis_error)
    }

    fn consume_token(&self, email: &str, token: &str, now: i64) -> Result<(), ApiError> {
        let key = self.token_key(email);
        let Some(entry) = get_redis_token_once(&self.config, &key).map_err(redis_error)? else {
            return Err(invalid_reset_token());
        };
        if entry.created_at + PASSWORD_RESET_TOKEN_TTL_SECONDS <= now {
            let _ = redis_command(&self.config, &["DEL", &key]).map_err(redis_error)?;
            return Err(invalid_reset_token());
        }
        if !constant_time_eq(entry.token.as_bytes(), token.trim().as_bytes()) {
            return Err(invalid_reset_token());
        }
        let _ = redis_command(&self.config, &["DEL", &key]).map_err(redis_error)?;
        Ok(())
    }

    fn is_email_in_cooldown(&self, email: &str, _now: i64) -> Result<bool, ApiError> {
        match redis_command(&self.config, &["EXISTS", &self.sent_key(email)])
            .map_err(redis_error)?
        {
            RespValue::Integer(count) => Ok(count > 0),
            other => Err(redis_error(anyhow!(
                "unexpected Redis EXISTS response: {other:?}"
            ))),
        }
    }

    fn mark_email_sent(&self, email: &str, _now: i64) -> Result<(), ApiError> {
        match redis_command(
            &self.config,
            &[
                "SETEX",
                &self.sent_key(email),
                &PASSWORD_RESET_EMAIL_COOLDOWN_SECONDS.to_string(),
                "1",
            ],
        )
        .map_err(redis_error)?
        {
            RespValue::Simple(value) if value.eq_ignore_ascii_case("OK") => Ok(()),
            other => Err(redis_error(anyhow!(
                "unexpected Redis SETEX response: {other:?}"
            ))),
        }
    }

    #[cfg(test)]
    fn token_for_tests(&self, email: &str) -> Option<String> {
        get_redis_token_once(&self.config, &self.token_key(email))
            .ok()
            .flatten()
            .map(|entry| entry.token)
    }
}

pub(crate) async fn store_from_config(config: &AppConfig) -> anyhow::Result<DynPasswordResetStore> {
    if let Some(store) = RedisPasswordResetStore::from_app_config(config) {
        store.ping().await?;
        return Ok(Arc::new(store));
    }
    Ok(MemoryPasswordResetStore::shared())
}

pub(crate) fn generate_reset_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn password_reset_redis_enabled() -> bool {
    let setting = std::env::var("BACKEND_NEXT_PASSWORD_RESET_STORE")
        .ok()
        .or_else(|| std::env::var("BACKEND_NEXT_VERIFICATION_CODE_STORE").ok());
    match setting
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("redis") => true,
        Some("memory") | Some("in_memory") | Some("off") | Some("disabled") => false,
        Some(_) => true,
        None => false,
    }
}

fn redis_command(config: &RedisConfig, parts: &[&str]) -> anyhow::Result<RespValue> {
    let mut client = RedisClient::connect(config)?;
    client.command(parts)
}

fn get_redis_token_once(
    config: &RedisConfig,
    key: &str,
) -> anyhow::Result<Option<PasswordResetTokenEntry>> {
    match redis_command(config, &["GET", key])? {
        RespValue::Bulk(None) => Ok(None),
        RespValue::Bulk(Some(bytes)) => Ok(Some(serde_json::from_slice(&bytes)?)),
        other => Err(anyhow!("unexpected Redis GET response: {other:?}")),
    }
}

fn save_redis_token_once(
    config: &RedisConfig,
    key: &str,
    token: &str,
    now: i64,
) -> anyhow::Result<()> {
    let entry = PasswordResetTokenEntry {
        token: token.to_owned(),
        created_at: now,
    };
    let value = serde_json::to_string(&entry)?;
    match redis_command(
        config,
        &[
            "SETEX",
            key,
            &PASSWORD_RESET_TOKEN_TTL_SECONDS.to_string(),
            &value,
        ],
    )? {
        RespValue::Simple(value) if value.eq_ignore_ascii_case("OK") => Ok(()),
        other => Err(anyhow!("unexpected Redis SETEX response: {other:?}")),
    }
}

fn normalize_email_key(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn password_reset_key(prefix: &str, email: &str) -> String {
    format!("{prefix}password_reset:{}", normalize_email_key(email))
}

fn password_reset_sent_key(prefix: &str, email: &str) -> String {
    format!("{prefix}password_reset_sent:{}", normalize_email_key(email))
}

fn invalid_reset_token() -> ApiError {
    ApiError::bad_request("INVALID_RESET_TOKEN: invalid or expired password reset token")
}

fn redis_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal_server_error(format!("password reset store error: {error}"))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= (left_byte ^ right_byte) as usize;
    }
    diff == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PasswordResetTokenEntry {
    token: String,
    created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_client::read_resp_value;
    use std::{
        collections::HashSet,
        io::{BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
        thread,
        time::Duration,
    };

    #[test]
    fn memory_store_consumes_token_once_and_enforces_cooldown() {
        let store = MemoryPasswordResetStore::default();
        store
            .save_token("User@Example.com", "reset-token", 1_000)
            .unwrap();
        assert_eq!(
            store.reusable_token("user@example.com", 1_010).unwrap(),
            Some("reset-token".to_owned())
        );
        assert!(!store
            .is_email_in_cooldown("user@example.com", 1_010)
            .unwrap());
        store.mark_email_sent("user@example.com", 1_010).unwrap();
        assert!(store
            .is_email_in_cooldown("user@example.com", 1_020)
            .unwrap());
        store
            .consume_token("user@example.com", "reset-token", 1_020)
            .unwrap();
        assert!(store
            .consume_token("user@example.com", "reset-token", 1_021)
            .is_err());
    }

    #[test]
    fn memory_store_rejects_expired_token() {
        let store = MemoryPasswordResetStore::default();
        store
            .save_token("user@example.com", "reset-token", 1_000)
            .unwrap();
        assert!(store
            .consume_token(
                "user@example.com",
                "reset-token",
                1_000 + PASSWORD_RESET_TOKEN_TTL_SECONDS,
            )
            .is_err());
    }

    #[test]
    fn redis_store_round_trips_password_reset_lifecycle() {
        let fake = FakeRedis::spawn();
        let store = RedisPasswordResetStore {
            config: RedisConfig {
                host: "127.0.0.1".to_owned(),
                port: fake.port,
                password: None,
                db: Some(0),
                enable_tls: Some(false),
            },
            key_prefix: "test:reset:".to_owned(),
        };

        store
            .save_token("user@example.com", "reset-token", 1_000)
            .unwrap();
        assert_eq!(
            store.reusable_token("user@example.com", 1_001).unwrap(),
            Some("reset-token".to_owned())
        );
        assert!(!store
            .is_email_in_cooldown("user@example.com", 1_001)
            .unwrap());
        store.mark_email_sent("user@example.com", 1_001).unwrap();
        assert!(store
            .is_email_in_cooldown("user@example.com", 1_002)
            .unwrap());
        store
            .consume_token("user@example.com", "reset-token", 1_003)
            .unwrap();
        assert!(store
            .consume_token("user@example.com", "reset-token", 1_004)
            .is_err());

        fake.stop();
    }

    struct FakeRedis {
        port: u16,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl FakeRedis {
        fn spawn() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let handle = thread::spawn(move || {
                let values = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
                let expirations = Arc::new(Mutex::new(HashSet::<String>::new()));
                while !worker_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            handle_connection(stream, values.clone(), expirations.clone())
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                port,
                stop,
                handle: Some(handle),
            }
        }

        fn stop(mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(("127.0.0.1", self.port));
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_connection(
        stream: TcpStream,
        values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        expirations: Arc<Mutex<HashSet<String>>>,
    ) {
        let mut reader = BufReader::new(stream);
        loop {
            let response = match read_resp_value(&mut reader) {
                Ok(RespValue::Array(parts)) => {
                    fake_response(parts, values.clone(), expirations.clone())
                }
                Ok(_) => b"-ERR invalid request\r\n".to_vec(),
                Err(_) => break,
            };
            let stream = reader.get_mut();
            if stream.write_all(&response).is_err() || stream.flush().is_err() {
                break;
            }
        }
    }

    fn fake_response(
        parts: Vec<RespValue>,
        values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        expirations: Arc<Mutex<HashSet<String>>>,
    ) -> Vec<u8> {
        let args = parts
            .into_iter()
            .filter_map(|part| match part {
                RespValue::Bulk(Some(bytes)) => String::from_utf8(bytes).ok(),
                _ => None,
            })
            .collect::<Vec<_>>();
        match args
            .first()
            .map(|value| value.to_ascii_uppercase())
            .as_deref()
        {
            Some("PING") => b"+PONG\r\n".to_vec(),
            Some("SETEX") if args.len() == 4 => {
                values
                    .lock()
                    .unwrap()
                    .insert(args[1].clone(), args[3].as_bytes().to_vec());
                expirations.lock().unwrap().insert(args[1].clone());
                b"+OK\r\n".to_vec()
            }
            Some("GET") if args.len() == 2 => match values.lock().unwrap().get(&args[1]).cloned() {
                Some(value) => {
                    let mut response = format!("${}\r\n", value.len()).into_bytes();
                    response.extend(value);
                    response.extend(b"\r\n");
                    response
                }
                None => b"$-1\r\n".to_vec(),
            },
            Some("DEL") if args.len() == 2 => {
                let existed = values.lock().unwrap().remove(&args[1]).is_some();
                expirations.lock().unwrap().remove(&args[1]);
                format!(":{}\r\n", if existed { 1 } else { 0 }).into_bytes()
            }
            Some("EXISTS") if args.len() == 2 => {
                let exists = values.lock().unwrap().contains_key(&args[1]);
                format!(":{}\r\n", if exists { 1 } else { 0 }).into_bytes()
            }
            _ => b"-ERR unsupported command\r\n".to_vec(),
        }
    }
}
