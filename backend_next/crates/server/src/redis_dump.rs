use crate::redis_client::{RedisClient, RespValue};
use crate::setup::RedisConfig;
use anyhow::{anyhow, Context};
use chrono::Utc;
use serde_json::{json, Value};

const DEFAULT_SCAN_COUNT: &str = "100";

#[derive(Debug, Clone)]
pub struct RedisDumpConfig {
    pub connection: RedisConfig,
    pub username: Option<String>,
    pub scan_count: usize,
}

#[derive(Debug, Clone)]
pub struct RedisDump {
    pub manifest: Value,
    pub jsonl: Vec<u8>,
}

pub fn dump_redis(config: &RedisDumpConfig) -> anyhow::Result<RedisDump> {
    let mut connection = config.connection.clone();
    let password = connection.password.take();
    let db = connection.db.take();
    let mut client = RedisClient::connect(&connection)?;
    if let Some(password) = password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(username) = config
            .username
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            client.command(&["AUTH", username, password])?;
        } else {
            client.command(&["AUTH", password])?;
        }
    }
    if let Some(db) = db.filter(|value| *value > 0) {
        client.command(&["SELECT", &db.to_string()])?;
    }
    let mut cursor = "0".to_owned();
    let scan_count = if config.scan_count == 0 {
        DEFAULT_SCAN_COUNT.to_owned()
    } else {
        config.scan_count.to_string()
    };
    let mut records = Vec::new();
    let mut key_count = 0_u64;

    loop {
        let scan = client
            .command(&["SCAN", &cursor, "COUNT", &scan_count])
            .context("redis SCAN failed")?;
        let (next_cursor, keys) = parse_scan_response(scan)?;
        for key in keys {
            let record = dump_key(&mut client, &key)
                .with_context(|| format!("redis key dump failed: {}", display_key(&key)))?;
            records.extend_from_slice(record.to_string().as_bytes());
            records.push(b'\n');
            key_count += 1;
        }
        cursor = next_cursor;
        if cursor == "0" {
            break;
        }
    }

    Ok(RedisDump {
        manifest: json!({
            "kind": "sub2api.redis-dump",
            "version": 1,
            "created_at": Utc::now().to_rfc3339(),
            "db": config.connection.db.unwrap_or(0),
            "key_count": key_count,
            "format": "jsonl"
        }),
        jsonl: records,
    })
}

fn dump_key(client: &mut RedisClient, key: &[u8]) -> anyhow::Result<Value> {
    let key_arg = key.to_vec();
    let key_display = display_key(key);
    let key_base64 = base64_encode(key);
    let redis_type = resp_string(client.command_bytes(&[b"TYPE".to_vec(), key_arg.clone()])?)?;
    let ttl_ms = match client.command_bytes(&[b"PTTL".to_vec(), key_arg.clone()])? {
        RespValue::Integer(value) => value,
        other => return Err(anyhow!("unexpected PTTL response: {other:?}")),
    };
    let value = match redis_type.as_str() {
        "string" => {
            let bulk = resp_optional_bytes(client.command_bytes(&[b"GET".to_vec(), key_arg])?)?;
            json!({
                "encoding": "base64",
                "data": bulk.map(|bytes| base64_encode(&bytes))
            })
        }
        "hash" => {
            let entries = resp_flat_pairs(client.command_bytes(&[b"HGETALL".to_vec(), key_arg])?)?;
            json!({
                "encoding": "base64",
                "entries": entries
                    .into_iter()
                    .map(|(field, value)| json!({
                        "field": base64_encode(&field),
                        "value": base64_encode(&value)
                    }))
                    .collect::<Vec<_>>()
            })
        }
        "list" => {
            let items = resp_bytes_array(client.command_bytes(&[
                b"LRANGE".to_vec(),
                key_arg,
                b"0".to_vec(),
                b"-1".to_vec(),
            ])?)?;
            json!({
                "encoding": "base64",
                "items": items.into_iter().map(|item| base64_encode(&item)).collect::<Vec<_>>()
            })
        }
        "set" => {
            let items = resp_bytes_array(client.command_bytes(&[b"SMEMBERS".to_vec(), key_arg])?)?;
            json!({
                "encoding": "base64",
                "items": items.into_iter().map(|item| base64_encode(&item)).collect::<Vec<_>>()
            })
        }
        "zset" => {
            let items = resp_flat_pairs(client.command_bytes(&[
                b"ZRANGE".to_vec(),
                key_arg,
                b"0".to_vec(),
                b"-1".to_vec(),
                b"WITHSCORES".to_vec(),
            ])?)?;
            json!({
                "encoding": "base64",
                "items": items
                    .into_iter()
                    .map(|(member, score)| json!({
                        "member": base64_encode(&member),
                        "score": String::from_utf8_lossy(&score).to_string()
                    }))
                    .collect::<Vec<_>>()
            })
        }
        "none" => Value::Null,
        other => {
            return Err(anyhow!(
                "unsupported Redis key type {other} for key {key_display}"
            ));
        }
    };

    Ok(json!({
        "key": key_display,
        "key_base64": key_base64,
        "type": redis_type,
        "ttl_ms": ttl_ms,
        "value": value
    }))
}

fn parse_scan_response(value: RespValue) -> anyhow::Result<(String, Vec<Vec<u8>>)> {
    let RespValue::Array(mut items) = value else {
        return Err(anyhow!("unexpected SCAN response"));
    };
    if items.len() != 2 {
        return Err(anyhow!("unexpected SCAN response length"));
    }
    let keys = resp_bytes_array(items.pop().expect("scan keys"))?;
    let cursor = resp_string(items.pop().expect("scan cursor"))?;
    Ok((cursor, keys))
}

fn resp_string(value: RespValue) -> anyhow::Result<String> {
    match value {
        RespValue::Simple(value) => Ok(value),
        RespValue::Bulk(Some(bytes)) => Ok(String::from_utf8_lossy(&bytes).to_string()),
        other => Err(anyhow!("expected Redis string response, got {other:?}")),
    }
}

fn resp_optional_bytes(value: RespValue) -> anyhow::Result<Option<Vec<u8>>> {
    match value {
        RespValue::Bulk(value) => Ok(value),
        other => Err(anyhow!("expected Redis bulk response, got {other:?}")),
    }
}

fn resp_bytes_array(value: RespValue) -> anyhow::Result<Vec<Vec<u8>>> {
    let RespValue::Array(items) = value else {
        return Err(anyhow!("expected Redis array response"));
    };
    items
        .into_iter()
        .map(|item| match item {
            RespValue::Bulk(Some(bytes)) => Ok(bytes),
            RespValue::Simple(value) => Ok(value.into_bytes()),
            other => Err(anyhow!("expected Redis array bulk item, got {other:?}")),
        })
        .collect()
}

fn resp_flat_pairs(value: RespValue) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let items = resp_bytes_array(value)?;
    if items.len() % 2 != 0 {
        return Err(anyhow!("expected even Redis pair array length"));
    }
    Ok(items
        .chunks_exact(2)
        .map(|chunk| (chunk[0].clone(), chunk[1].clone()))
        .collect())
}

fn display_key(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| base64_encode(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_client::read_resp_value;
    use std::collections::HashMap;
    use std::io::{BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[derive(Clone)]
    struct FakeRedisValue {
        redis_type: &'static str,
        ttl_ms: i64,
        value: ValueData,
    }

    #[derive(Clone)]
    enum ValueData {
        String(Vec<u8>),
        Hash(Vec<(Vec<u8>, Vec<u8>)>),
        List(Vec<Vec<u8>>),
        Set(Vec<Vec<u8>>),
        ZSet(Vec<(Vec<u8>, String)>),
    }

    #[test]
    fn dumps_string_and_collections_to_jsonl() {
        let mut data = HashMap::new();
        data.insert(
            b"plain".to_vec(),
            FakeRedisValue {
                redis_type: "string",
                ttl_ms: -1,
                value: ValueData::String(b"hello".to_vec()),
            },
        );
        data.insert(
            b"hash".to_vec(),
            FakeRedisValue {
                redis_type: "hash",
                ttl_ms: 5000,
                value: ValueData::Hash(vec![(b"a".to_vec(), b"b".to_vec())]),
            },
        );
        data.insert(
            b"list".to_vec(),
            FakeRedisValue {
                redis_type: "list",
                ttl_ms: -1,
                value: ValueData::List(vec![b"one".to_vec(), b"two".to_vec()]),
            },
        );
        data.insert(
            b"set".to_vec(),
            FakeRedisValue {
                redis_type: "set",
                ttl_ms: -1,
                value: ValueData::Set(vec![b"member".to_vec()]),
            },
        );
        data.insert(
            b"zset".to_vec(),
            FakeRedisValue {
                redis_type: "zset",
                ttl_ms: -1,
                value: ValueData::ZSet(vec![(b"ranked".to_vec(), "1.25".to_owned())]),
            },
        );
        let (port, handle) = spawn_fake_dump_redis(data);

        let dump = dump_redis(&RedisDumpConfig {
            connection: RedisConfig {
                host: "127.0.0.1".to_owned(),
                port,
                password: Some("secret".to_owned()),
                db: Some(2),
                enable_tls: Some(false),
            },
            username: None,
            scan_count: 10,
        })
        .unwrap();

        assert_eq!(dump.manifest["key_count"], 5);
        let jsonl = String::from_utf8(dump.jsonl).unwrap();
        assert!(jsonl.contains("\"key\":\"plain\""));
        assert!(jsonl.contains("\"type\":\"hash\""));
        assert!(jsonl.contains("\"score\":\"1.25\""));
        let commands = handle.join().unwrap();
        assert!(commands.contains(&vec!["AUTH".to_owned(), "secret".to_owned()]));
        assert!(commands.contains(&vec!["SELECT".to_owned(), "2".to_owned()]));
    }

    fn spawn_fake_dump_redis(
        data: HashMap<Vec<u8>, FakeRedisValue>,
    ) -> (u16, thread::JoinHandle<Vec<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let data = Arc::new(data);
        let commands = Arc::new(Mutex::new(Vec::new()));
        let commands_out = commands.clone();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            while let Ok(command) = read_command(&mut reader) {
                commands.lock().unwrap().push(display_command(&command));
                let name =
                    String::from_utf8_lossy(command.first().map(Vec::as_slice).unwrap_or(b""))
                        .to_ascii_uppercase();
                let response = match name.as_str() {
                    "AUTH" | "SELECT" => simple("OK"),
                    "SCAN" => {
                        let mut items = Vec::new();
                        items.push(RespValue::Bulk(Some(b"0".to_vec())));
                        items.push(RespValue::Array(
                            data.keys()
                                .cloned()
                                .map(|key| RespValue::Bulk(Some(key)))
                                .collect(),
                        ));
                        RespValue::Array(items)
                    }
                    "TYPE" => {
                        let entry = data.get(&command[1]).unwrap();
                        simple(entry.redis_type)
                    }
                    "PTTL" => RespValue::Integer(data.get(&command[1]).unwrap().ttl_ms),
                    "GET" => match &data.get(&command[1]).unwrap().value {
                        ValueData::String(value) => RespValue::Bulk(Some(value.clone())),
                        _ => RespValue::Bulk(None),
                    },
                    "HGETALL" => match &data.get(&command[1]).unwrap().value {
                        ValueData::Hash(entries) => pair_array(entries.clone()),
                        _ => RespValue::Array(Vec::new()),
                    },
                    "LRANGE" => match &data.get(&command[1]).unwrap().value {
                        ValueData::List(items) => bulk_array(items.clone()),
                        _ => RespValue::Array(Vec::new()),
                    },
                    "SMEMBERS" => match &data.get(&command[1]).unwrap().value {
                        ValueData::Set(items) => bulk_array(items.clone()),
                        _ => RespValue::Array(Vec::new()),
                    },
                    "ZRANGE" => match &data.get(&command[1]).unwrap().value {
                        ValueData::ZSet(items) => RespValue::Array(
                            items
                                .iter()
                                .flat_map(|(member, score)| {
                                    [
                                        RespValue::Bulk(Some(member.clone())),
                                        RespValue::Bulk(Some(score.as_bytes().to_vec())),
                                    ]
                                })
                                .collect(),
                        ),
                        _ => RespValue::Array(Vec::new()),
                    },
                    _ => simple("OK"),
                };
                write_value(&mut stream, &response).unwrap();
            }
            commands_out.lock().unwrap().clone()
        });
        (port, handle)
    }

    fn read_command<R: std::io::BufRead>(reader: &mut R) -> anyhow::Result<Vec<Vec<u8>>> {
        match read_resp_value(reader)? {
            RespValue::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    RespValue::Bulk(Some(bytes)) => Ok(bytes),
                    other => Err(anyhow!("unexpected command item: {other:?}")),
                })
                .collect(),
            other => Err(anyhow!("unexpected command: {other:?}")),
        }
    }

    fn write_value<W: Write>(writer: &mut W, value: &RespValue) -> anyhow::Result<()> {
        match value {
            RespValue::Simple(value) => writer.write_all(format!("+{value}\r\n").as_bytes())?,
            RespValue::Integer(value) => writer.write_all(format!(":{value}\r\n").as_bytes())?,
            RespValue::Bulk(Some(bytes)) => {
                writer.write_all(format!("${}\r\n", bytes.len()).as_bytes())?;
                writer.write_all(bytes)?;
                writer.write_all(b"\r\n")?;
            }
            RespValue::Bulk(None) => writer.write_all(b"$-1\r\n")?,
            RespValue::Array(items) => {
                writer.write_all(format!("*{}\r\n", items.len()).as_bytes())?;
                for item in items {
                    write_value(writer, item)?;
                }
            }
        }
        writer.flush()?;
        Ok(())
    }

    fn simple(value: &str) -> RespValue {
        RespValue::Simple(value.to_owned())
    }

    fn bulk_array(items: Vec<Vec<u8>>) -> RespValue {
        RespValue::Array(
            items
                .into_iter()
                .map(|item| RespValue::Bulk(Some(item)))
                .collect(),
        )
    }

    fn pair_array(entries: Vec<(Vec<u8>, Vec<u8>)>) -> RespValue {
        RespValue::Array(
            entries
                .into_iter()
                .flat_map(|(key, value)| [RespValue::Bulk(Some(key)), RespValue::Bulk(Some(value))])
                .collect(),
        )
    }

    fn display_command(command: &[Vec<u8>]) -> Vec<String> {
        command
            .iter()
            .map(|item| String::from_utf8_lossy(item).to_string())
            .collect()
    }
}
