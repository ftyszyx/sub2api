use crate::config::AppConfig;
use crate::redis_client::{RedisClient, RespValue};
use crate::response::ApiError;
use crate::setup::RedisConfig;
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use uuid::Uuid;

pub(crate) const CODE_TTL_SECONDS: i64 = 15 * 60;
pub(crate) const CODE_COOLDOWN_SECONDS: i64 = 60;
const CODE_MAX_ATTEMPTS: u8 = 5;
const USER_RATE_WINDOW_SECONDS: i64 = 10 * 60;
const USER_RATE_LIMIT: u32 = 5;
const DEFAULT_REDIS_KEY_PREFIX: &str = "";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VerificationCodePurpose {
    NotifyEmail,
    EmailBinding,
    Totp,
}

impl VerificationCodePurpose {
    fn as_key(self) -> &'static str {
        match self {
            Self::NotifyEmail => "notify_verify",
            Self::EmailBinding | Self::Totp => "verify_code",
        }
    }
}

pub(crate) trait VerificationCodeStore: Send + Sync {
    fn can_create(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        now: i64,
    ) -> Result<(), ApiError>;

    fn save_code(
        &self,
        purpose: VerificationCodePurpose,
        user_id: i64,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError>;

    fn verify(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError>;

    fn remove(&self, purpose: VerificationCodePurpose, target: &str) -> Result<(), ApiError>;

    #[cfg(test)]
    fn code_for_tests(&self, purpose: VerificationCodePurpose, target: &str) -> Option<String>;
}

pub(crate) type DynVerificationCodeStore = Arc<dyn VerificationCodeStore>;

#[derive(Default)]
pub(crate) struct MemoryVerificationCodeStore {
    codes: RwLock<HashMap<String, VerificationCodeEntry>>,
    rates: RwLock<HashMap<String, RateWindow>>,
}

impl MemoryVerificationCodeStore {
    pub(crate) fn shared() -> DynVerificationCodeStore {
        Arc::new(Self::default())
    }
}

impl VerificationCodeStore for MemoryVerificationCodeStore {
    fn can_create(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        ensure_memory_cooldown(&self.codes, &code_key("", purpose, target), now)
    }

    fn save_code(
        &self,
        purpose: VerificationCodePurpose,
        user_id: i64,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let code_key = code_key("", purpose, target);
        create_in_maps(
            &self.codes,
            &self.rates,
            &code_key,
            &rate_key("", user_id),
            purpose,
            user_id,
            target,
            now,
            Some(code.to_owned()),
        )
    }

    fn verify(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let code_key = code_key("", purpose, target);
        verify_in_map(&self.codes, &code_key, code, now)
    }

    fn remove(&self, purpose: VerificationCodePurpose, target: &str) -> Result<(), ApiError> {
        self.codes
            .write()
            .expect("verification code lock")
            .remove(&code_key("", purpose, target));
        Ok(())
    }

    #[cfg(test)]
    fn code_for_tests(&self, purpose: VerificationCodePurpose, target: &str) -> Option<String> {
        self.codes
            .read()
            .expect("verification code lock")
            .get(&code_key("", purpose, target))
            .map(|entry| entry.code.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RedisVerificationCodeStore {
    config: RedisConfig,
    key_prefix: String,
}

impl RedisVerificationCodeStore {
    pub(crate) fn from_app_config(config: &AppConfig) -> Option<Self> {
        if !verification_code_redis_enabled(config) {
            return None;
        }
        Some(Self {
            config: config.redis_config(),
            key_prefix: std::env::var("BACKEND_NEXT_VERIFICATION_CODE_REDIS_PREFIX")
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
        .context("Redis verification-code ping task failed")?
    }

    fn code_key(&self, purpose: VerificationCodePurpose, target: &str) -> String {
        code_key(&self.key_prefix, purpose, target)
    }

    fn rate_key(&self, user_id: i64) -> String {
        rate_key(&self.key_prefix, user_id)
    }
}

impl VerificationCodeStore for RedisVerificationCodeStore {
    fn can_create(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        ensure_redis_cooldown(&self.config, &self.code_key(purpose, target), now)
    }

    fn save_code(
        &self,
        purpose: VerificationCodePurpose,
        user_id: i64,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let config = self.config.clone();
        let code_key = self.code_key(purpose, target);
        ensure_redis_cooldown(&config, &code_key, now)?;
        if purpose == VerificationCodePurpose::NotifyEmail {
            increment_notify_rate(&config, &self.rate_key(user_id))?;
        }
        save_redis_entry(&config, &code_key, user_id, target, now, code)
    }

    fn verify(
        &self,
        purpose: VerificationCodePurpose,
        target: &str,
        code: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let config = self.config.clone();
        let code_key = self.code_key(purpose, target);
        let code = code.trim().to_owned();
        if code.is_empty() {
            return Err(invalid_code());
        }
        let Some(mut stored) = get_redis_code_once(&config, &code_key).map_err(redis_error)? else {
            return Err(invalid_code());
        };
        if stored.expires_at <= now {
            let _ = redis_command(&config, &["DEL", &code_key]).map_err(redis_error)?;
            return Err(invalid_code());
        }
        if stored.attempts >= CODE_MAX_ATTEMPTS {
            return Err(max_attempts());
        }
        if !constant_time_eq(stored.code.as_bytes(), code.as_bytes()) {
            stored.attempts = stored.attempts.saturating_add(1);
            if stored.attempts >= CODE_MAX_ATTEMPTS {
                save_redis_code_once(&config, &code_key, &stored, now).map_err(redis_error)?;
                return Err(max_attempts());
            }
            save_redis_code_once(&config, &code_key, &stored, now).map_err(redis_error)?;
            return Err(invalid_code());
        }
        Ok(())
    }

    fn remove(&self, purpose: VerificationCodePurpose, target: &str) -> Result<(), ApiError> {
        let config = self.config.clone();
        let key = self.code_key(purpose, target);
        let _ = redis_command(&config, &["DEL", &key]).map_err(redis_error)?;
        Ok(())
    }

    #[cfg(test)]
    fn code_for_tests(&self, purpose: VerificationCodePurpose, target: &str) -> Option<String> {
        get_redis_code_once(&self.config, &self.code_key(purpose, target))
            .ok()
            .flatten()
            .map(|entry| entry.code)
    }
}

pub(crate) async fn store_from_config(
    config: &AppConfig,
) -> anyhow::Result<DynVerificationCodeStore> {
    if let Some(store) = RedisVerificationCodeStore::from_app_config(config) {
        store.ping().await?;
        return Ok(Arc::new(store));
    }
    Ok(MemoryVerificationCodeStore::shared())
}

fn verification_code_redis_enabled(_config: &AppConfig) -> bool {
    match std::env::var("BACKEND_NEXT_VERIFICATION_CODE_STORE")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("redis") => true,
        Some("memory") | Some("in_memory") | Some("off") | Some("disabled") => false,
        Some(_) => true,
        None => false,
    }
}

fn create_in_maps(
    codes: &RwLock<HashMap<String, VerificationCodeEntry>>,
    rates: &RwLock<HashMap<String, RateWindow>>,
    code_key: &str,
    rate_key: &str,
    purpose: VerificationCodePurpose,
    user_id: i64,
    target: &str,
    now: i64,
    prepared_code: Option<String>,
) -> Result<(), ApiError> {
    ensure_memory_cooldown(codes, code_key, now)?;

    if purpose == VerificationCodePurpose::NotifyEmail {
        let mut rates = rates.write().expect("verification code rate lock");
        let window = rates.entry(rate_key.to_owned()).or_insert(RateWindow {
            count: 0,
            window_start: now,
        });
        if window.window_start + USER_RATE_WINDOW_SECONDS <= now {
            window.count = 0;
            window.window_start = now;
        }
        if window.count >= USER_RATE_LIMIT {
            return Err(rate_limited(
                "NOTIFY_CODE_USER_RATE_LIMIT: too many verification code requests",
            ));
        }
        window.count += 1;
    }

    codes.write().expect("verification code lock").insert(
        code_key.to_owned(),
        VerificationCodeEntry {
            code: prepared_code.unwrap_or_else(|| generate_code(user_id, target, now)),
            attempts: 0,
            created_at: now,
            expires_at: now + CODE_TTL_SECONDS,
        },
    );
    Ok(())
}

fn ensure_memory_cooldown(
    codes: &RwLock<HashMap<String, VerificationCodeEntry>>,
    code_key: &str,
    now: i64,
) -> Result<(), ApiError> {
    let mut codes = codes.write().expect("verification code lock");
    if let Some(existing) = codes.get(code_key) {
        if existing.expires_at <= now {
            codes.remove(code_key);
        } else if existing.created_at + CODE_COOLDOWN_SECONDS > now {
            return Err(rate_limited(
                "VERIFY_CODE_TOO_FREQUENT: please wait before requesting a new code",
            ));
        }
    }
    Ok(())
}

fn verify_in_map(
    codes: &RwLock<HashMap<String, VerificationCodeEntry>>,
    code_key: &str,
    code: &str,
    now: i64,
) -> Result<(), ApiError> {
    let code = code.trim();
    if code.is_empty() {
        return Err(invalid_code());
    }

    let mut codes = codes.write().expect("verification code lock");
    let Some(stored) = codes.get_mut(code_key) else {
        return Err(invalid_code());
    };
    if stored.expires_at <= now {
        codes.remove(code_key);
        return Err(invalid_code());
    }
    if stored.attempts >= CODE_MAX_ATTEMPTS {
        return Err(max_attempts());
    }
    if !constant_time_eq(stored.code.as_bytes(), code.as_bytes()) {
        stored.attempts = stored.attempts.saturating_add(1);
        if stored.attempts >= CODE_MAX_ATTEMPTS {
            return Err(max_attempts());
        }
        return Err(invalid_code());
    }
    Ok(())
}

fn redis_command(config: &RedisConfig, parts: &[&str]) -> anyhow::Result<RespValue> {
    let mut client = RedisClient::connect(config)?;
    client.command(parts)
}

fn get_redis_code_once(
    config: &RedisConfig,
    key: &str,
) -> anyhow::Result<Option<VerificationCodeEntry>> {
    match redis_command(config, &["GET", key])? {
        RespValue::Bulk(None) => Ok(None),
        RespValue::Bulk(Some(bytes)) => Ok(Some(serde_json::from_slice(&bytes)?)),
        other => Err(anyhow!("unexpected Redis GET response: {other:?}")),
    }
}

fn ensure_redis_cooldown(config: &RedisConfig, code_key: &str, now: i64) -> Result<(), ApiError> {
    if let Some(existing) = get_redis_code_once(config, code_key).map_err(redis_error)? {
        if existing.expires_at <= now {
            let _ = redis_command(config, &["DEL", code_key]).map_err(redis_error)?;
        } else if existing.created_at + CODE_COOLDOWN_SECONDS > now {
            return Err(rate_limited(
                "VERIFY_CODE_TOO_FREQUENT: please wait before requesting a new code",
            ));
        }
    }
    Ok(())
}

fn increment_notify_rate(config: &RedisConfig, rate_key: &str) -> Result<(), ApiError> {
    match redis_command(config, &["INCR", rate_key]).map_err(redis_error)? {
        RespValue::Integer(count) => {
            let _ = redis_command(
                config,
                &["EXPIRE", rate_key, &USER_RATE_WINDOW_SECONDS.to_string()],
            )
            .map_err(redis_error)?;
            if count > i64::from(USER_RATE_LIMIT) {
                return Err(rate_limited(
                    "NOTIFY_CODE_USER_RATE_LIMIT: too many verification code requests",
                ));
            }
            Ok(())
        }
        other => Err(redis_error(anyhow!(
            "unexpected Redis INCR response: {other:?}"
        ))),
    }
}

fn save_redis_entry(
    config: &RedisConfig,
    key: &str,
    user_id: i64,
    target: &str,
    now: i64,
    code: &str,
) -> Result<(), ApiError> {
    let entry = VerificationCodeEntry {
        code: code.to_owned(),
        attempts: 0,
        created_at: now,
        expires_at: now + CODE_TTL_SECONDS,
    };
    let value = serde_json::to_string(&entry).map_err(redis_error)?;
    match redis_command(
        config,
        &["SETEX", key, &CODE_TTL_SECONDS.to_string(), &value],
    )
    .map_err(redis_error)?
    {
        RespValue::Simple(value) if value.eq_ignore_ascii_case("OK") => Ok(()),
        other => Err(redis_error(anyhow!(
            "unexpected Redis SETEX response: {other:?}; user_id={user_id}; target={target}"
        ))),
    }
}

fn save_redis_code_once(
    config: &RedisConfig,
    key: &str,
    entry: &VerificationCodeEntry,
    now: i64,
) -> anyhow::Result<()> {
    let ttl = (entry.expires_at - now).max(1).to_string();
    let value = serde_json::to_string(entry)?;
    match redis_command(config, &["SETEX", key, &ttl, &value])? {
        RespValue::Simple(value) if value.eq_ignore_ascii_case("OK") => Ok(()),
        other => Err(anyhow!("unexpected Redis SETEX response: {other:?}")),
    }
}

fn code_key(prefix: &str, purpose: VerificationCodePurpose, target: &str) -> String {
    format!(
        "{}{}:{}",
        prefix,
        purpose.as_key(),
        target.trim().to_ascii_lowercase()
    )
}

fn rate_key(prefix: &str, user_id: i64) -> String {
    format!("{prefix}notify_code_user_rate:{user_id}")
}

pub(crate) fn generate_prepared_code(user_id: i64, target: &str, now: i64) -> String {
    let seed = format!("{user_id}:{target}:{now}:{}", Uuid::new_v4());
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    format!("{:06}", u64::from_be_bytes(bytes) % 1_000_000)
}

fn generate_code(user_id: i64, target: &str, now: i64) -> String {
    generate_prepared_code(user_id, target, now)
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

fn invalid_code() -> ApiError {
    ApiError::bad_request("INVALID_VERIFY_CODE: invalid or expired verification code")
}

fn max_attempts() -> ApiError {
    ApiError::too_many_requests(
        "VERIFY_CODE_MAX_ATTEMPTS: too many failed attempts, please request a new code",
    )
}

fn rate_limited(message: impl Into<String>) -> ApiError {
    ApiError::too_many_requests(message)
}

fn redis_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal_server_error(format!("verification code store error: {error}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationCodeEntry {
    code: String,
    attempts: u8,
    created_at: i64,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct RateWindow {
    count: u32,
    window_start: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_client::read_resp_value;
    use std::{
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
    fn memory_store_enforces_code_lifecycle() {
        let store = MemoryVerificationCodeStore::default();
        store
            .save_code(
                VerificationCodePurpose::NotifyEmail,
                1,
                "a@example.com",
                "123456",
                100,
            )
            .unwrap();
        let code = store
            .code_for_tests(VerificationCodePurpose::NotifyEmail, "a@example.com")
            .unwrap();

        assert!(store
            .save_code(
                VerificationCodePurpose::NotifyEmail,
                1,
                "a@example.com",
                "654321",
                120
            )
            .is_err());
        assert!(store
            .verify(
                VerificationCodePurpose::NotifyEmail,
                "a@example.com",
                "000000",
                130
            )
            .is_err());
        store
            .verify(
                VerificationCodePurpose::NotifyEmail,
                "a@example.com",
                &code,
                130,
            )
            .unwrap();
        store
            .remove(VerificationCodePurpose::NotifyEmail, "a@example.com")
            .unwrap();
        assert!(store
            .verify(
                VerificationCodePurpose::NotifyEmail,
                "a@example.com",
                &code,
                131
            )
            .is_err());
    }

    #[test]
    fn redis_store_round_trips_code_lifecycle() {
        let fake = FakeRedis::spawn();
        let store = RedisVerificationCodeStore {
            config: RedisConfig {
                host: "127.0.0.1".to_owned(),
                port: fake.port,
                password: None,
                db: Some(0),
                enable_tls: Some(false),
            },
            key_prefix: "test:verify:".to_owned(),
        };

        store
            .save_code(VerificationCodePurpose::Totp, 7, "7", "123456", 1_000)
            .unwrap();
        let code = store
            .code_for_tests(VerificationCodePurpose::Totp, "7")
            .unwrap();
        assert!(store
            .save_code(VerificationCodePurpose::Totp, 7, "7", "654321", 1_010)
            .is_err());
        store
            .verify(VerificationCodePurpose::Totp, "7", &code, 1_020)
            .unwrap();
        store.remove(VerificationCodePurpose::Totp, "7").unwrap();
        assert!(store
            .verify(VerificationCodePurpose::Totp, "7", &code, 1_021)
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
                while !worker_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => handle_connection(stream, values.clone()),
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

    fn handle_connection(stream: TcpStream, values: Arc<Mutex<HashMap<String, Vec<u8>>>>) {
        let mut reader = BufReader::new(stream);
        loop {
            let response = match read_resp_value(&mut reader) {
                Ok(RespValue::Array(parts)) => fake_response(parts, values.clone()),
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
                format!(":{}\r\n", if existed { 1 } else { 0 }).into_bytes()
            }
            Some("INCR") if args.len() == 2 => {
                let mut values = values.lock().unwrap();
                let next = values
                    .get(&args[1])
                    .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0)
                    + 1;
                values.insert(args[1].clone(), next.to_string().into_bytes());
                format!(":{next}\r\n").into_bytes()
            }
            Some("EXPIRE") if args.len() == 3 => b":1\r\n".to_vec(),
            _ => b"-ERR unsupported command\r\n".to_vec(),
        }
    }
}
