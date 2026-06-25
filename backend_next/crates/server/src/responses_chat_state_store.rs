use crate::config::AppConfig;
use crate::redis_client::{RedisClient, RespValue};
use crate::setup::RedisConfig;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

const DEFAULT_STATE_TTL_SECONDS: u64 = 24 * 60 * 60;
const DEFAULT_REDIS_KEY_PREFIX: &str = "backend_next:responses_chat:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ResponsesChatState {
    pub group_id: i64,
    pub account_id: i64,
    pub messages: Vec<Value>,
}

#[async_trait]
pub(crate) trait ResponsesChatStateStore: Send + Sync {
    async fn get(&self, response_id: &str) -> anyhow::Result<Option<ResponsesChatState>>;
    async fn set(&self, response_id: &str, state: ResponsesChatState) -> anyhow::Result<()>;
}

pub(crate) type DynResponsesChatStateStore = Arc<dyn ResponsesChatStateStore>;

#[derive(Debug, Default)]
pub(crate) struct MemoryResponsesChatStateStore {
    states: RwLock<HashMap<String, ResponsesChatState>>,
}

impl MemoryResponsesChatStateStore {
    pub fn shared() -> DynResponsesChatStateStore {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl ResponsesChatStateStore for MemoryResponsesChatStateStore {
    async fn get(&self, response_id: &str) -> anyhow::Result<Option<ResponsesChatState>> {
        Ok(self
            .states
            .read()
            .expect("responses chat state lock")
            .get(response_id)
            .cloned())
    }

    async fn set(&self, response_id: &str, state: ResponsesChatState) -> anyhow::Result<()> {
        self.states
            .write()
            .expect("responses chat state lock")
            .insert(response_id.to_owned(), state);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RedisResponsesChatStateStore {
    config: RedisConfig,
    key_prefix: String,
    ttl_seconds: u64,
}

impl RedisResponsesChatStateStore {
    pub fn from_app_config(config: &AppConfig) -> Option<Self> {
        if !responses_state_redis_enabled(config) {
            return None;
        }
        Some(Self {
            config: config.redis_config(),
            key_prefix: std::env::var("BACKEND_NEXT_RESPONSES_STATE_REDIS_PREFIX")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_REDIS_KEY_PREFIX.to_owned()),
            ttl_seconds: std::env::var("BACKEND_NEXT_RESPONSES_STATE_TTL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_STATE_TTL_SECONDS),
        })
    }

    pub async fn ping(&self) -> anyhow::Result<()> {
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = RedisClient::connect(&config)?;
            match connection.command(&["PING"])? {
                RespValue::Simple(value) if value.eq_ignore_ascii_case("PONG") => Ok(()),
                other => Err(anyhow!("unexpected Redis PING response: {other:?}")),
            }
        })
        .await
        .context("Redis response state ping task failed")?
    }

    fn redis_key(&self, response_id: &str) -> String {
        format!("{}{}", self.key_prefix, response_id)
    }
}

#[async_trait]
impl ResponsesChatStateStore for RedisResponsesChatStateStore {
    async fn get(&self, response_id: &str) -> anyhow::Result<Option<ResponsesChatState>> {
        let config = self.config.clone();
        let key = self.redis_key(response_id);
        tokio::task::spawn_blocking(move || {
            let mut connection = RedisClient::connect(&config)?;
            match connection.command(&["GET", &key])? {
                RespValue::Bulk(None) => Ok(None),
                RespValue::Bulk(Some(bytes)) => {
                    let state = serde_json::from_slice(&bytes)
                        .context("failed to decode responses chat state from Redis")?;
                    Ok(Some(state))
                }
                other => Err(anyhow!("unexpected Redis GET response: {other:?}")),
            }
        })
        .await
        .context("Redis response state get task failed")?
    }

    async fn set(&self, response_id: &str, state: ResponsesChatState) -> anyhow::Result<()> {
        let config = self.config.clone();
        let key = self.redis_key(response_id);
        let ttl = self.ttl_seconds.to_string();
        let value = serde_json::to_string(&state)?;
        tokio::task::spawn_blocking(move || {
            let mut connection = RedisClient::connect(&config)?;
            match connection.command(&["SETEX", &key, &ttl, &value])? {
                RespValue::Simple(value) if value.eq_ignore_ascii_case("OK") => Ok(()),
                other => Err(anyhow!("unexpected Redis SETEX response: {other:?}")),
            }
        })
        .await
        .context("Redis response state set task failed")?
    }
}

fn responses_state_redis_enabled(config: &AppConfig) -> bool {
    match std::env::var("BACKEND_NEXT_RESPONSES_STATE_STORE")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("memory") | Some("in_memory") | Some("off") | Some("disabled") => false,
        Some("redis") => true,
        Some(_) => true,
        None => {
            config.redis.is_some()
                || [
                    "REDIS_HOST",
                    "REDIS_PORT",
                    "REDIS_PASSWORD",
                    "REDIS_DB",
                    "REDIS_ENABLE_TLS",
                ]
                .iter()
                .any(|key| std::env::var(key).is_ok())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::redis_client::read_resp_value;
    use std::{
        env,
        io::{BufReader, Write},
        net::TcpListener,
        net::TcpStream,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
        thread,
        time::Duration,
    };

    const STORE_ENV_KEYS: &[&str] = &[
        "BACKEND_NEXT_RESPONSES_STATE_STORE",
        "BACKEND_NEXT_RESPONSES_STATE_REDIS_PREFIX",
        "BACKEND_NEXT_RESPONSES_STATE_TTL_SECONDS",
        "REDIS_HOST",
        "REDIS_PORT",
        "REDIS_PASSWORD",
        "REDIS_DB",
        "REDIS_ENABLE_TLS",
    ];

    struct EnvSnapshot {
        values: Vec<(&'static str, Option<String>)>,
    }

    impl EnvSnapshot {
        fn capture() -> Self {
            Self {
                values: STORE_ENV_KEYS
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

    #[tokio::test]
    async fn memory_store_round_trips_response_state() {
        let store = MemoryResponsesChatStateStore::shared();
        let state = state_fixture();

        store.set("resp_memory", state.clone()).await.unwrap();

        assert_eq!(store.get("resp_memory").await.unwrap(), Some(state));
        assert_eq!(store.get("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn redis_store_round_trips_response_state_against_resp_server() {
        let fake = FakeRedis::spawn();
        let store = RedisResponsesChatStateStore {
            config: RedisConfig {
                host: "127.0.0.1".to_owned(),
                port: fake.port,
                password: None,
                db: Some(0),
                enable_tls: Some(false),
            },
            key_prefix: "test:".to_owned(),
            ttl_seconds: 60,
        };
        let state = state_fixture();

        store.ping().await.unwrap();
        store.set("resp_redis", state.clone()).await.unwrap();

        assert_eq!(store.get("resp_redis").await.unwrap(), Some(state));
        assert_eq!(store.get("missing").await.unwrap(), None);
        fake.stop();
    }

    #[tokio::test]
    async fn gateway_runtime_uses_configured_redis_state_store() {
        let _guard = crate::config::runtime_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvSnapshot::capture();
        clear_store_env();
        let fake = FakeRedis::spawn();
        env::set_var("BACKEND_NEXT_RESPONSES_STATE_STORE", "redis");
        env::set_var("REDIS_HOST", "127.0.0.1");
        env::set_var("REDIS_PORT", fake.port.to_string());
        env::set_var("REDIS_DB", "0");

        let config = AppConfig::default();
        let service = crate::gateway_runtime::GatewayRuntimeService::from_config(&config)
            .await
            .unwrap();
        service
            .responses_chat_store_for_tests()
            .set("resp_runtime", state_fixture())
            .await
            .unwrap();
        assert!(service
            .responses_chat_store_for_tests()
            .get("resp_runtime")
            .await
            .unwrap()
            .is_some());

        fake.stop();
    }

    fn state_fixture() -> ResponsesChatState {
        ResponsesChatState {
            group_id: 2,
            account_id: 7,
            messages: vec![
                serde_json::json!({ "role": "user", "content": "hello" }),
                serde_json::json!({ "role": "assistant", "content": "world" }),
            ],
        }
    }

    fn clear_store_env() {
        for key in STORE_ENV_KEYS {
            env::remove_var(key);
        }
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
                        Ok((stream, _)) => handle_fake_redis_connection(stream, values.clone()),
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

    fn handle_fake_redis_connection(
        mut stream: TcpStream,
        values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    ) {
        let parsed = {
            let mut reader = BufReader::new(&mut stream);
            read_resp_value(&mut reader)
        };
        let response = match parsed {
            Ok(RespValue::Array(parts)) => fake_redis_response(parts, values),
            _ => b"-ERR invalid request\r\n".to_vec(),
        };
        let _ = stream.write_all(&response);
        let _ = stream.flush();
    }

    fn fake_redis_response(
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
            _ => b"-ERR unsupported command\r\n".to_vec(),
        }
    }
}
