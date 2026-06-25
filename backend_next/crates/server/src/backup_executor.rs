use crate::config::AppConfig;
use crate::response::ApiError;
use crate::s3_probe::{self, S3ProbeConfig, S3PutObjectInput};
use crate::setup::DatabaseConfig;
use async_trait::async_trait;
use chrono::Utc;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{thread, time::Instant};

const POSTGRES_BACKUP_CONTENT_TYPE: &str = "application/gzip";
const DEFAULT_BACKUP_PREFIX: &str = "backups";
const DEFAULT_PG_DUMP_COMMAND: &str = "pg_dump";
const DEFAULT_PSQL_COMMAND: &str = "psql";
const PG_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct BackupExecutionInput {
    pub base_record: Value,
    pub manifest: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresDumpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
    pub sslmode: String,
    pub pg_dump_path: String,
    pub psql_path: String,
}

#[derive(Debug, Clone)]
pub struct PostgresS3BackupConfig {
    pub database: PostgresDumpConfig,
    pub s3: S3ProbeConfig,
    pub prefix: String,
}

#[async_trait]
pub trait BackupExecutor: Send + Sync {
    async fn execute(&self, input: BackupExecutionInput) -> Result<Value, ApiError>;
}

#[derive(Debug, Default)]
pub struct RepositoryManifestBackupExecutor;

#[async_trait]
impl BackupExecutor for RepositoryManifestBackupExecutor {
    async fn execute(&self, input: BackupExecutionInput) -> Result<Value, ApiError> {
        let manifest_size = serde_json::to_vec(&input.manifest)
            .map_err(|error| ApiError::internal_server_error(error.to_string()))?
            .len();
        let now = Utc::now().to_rfc3339();
        let mut record = input.base_record;
        record["status"] = json!("completed");
        record["executor"] = json!("repository_manifest");
        record["artifact_kind"] = json!("repository_manifest");
        record["manifest_version"] = json!(1);
        record["content_type"] = json!("application/vnd.sub2api.repository-backup+json");
        record["size_bytes"] = json!(manifest_size);
        record["manifest"] = input.manifest;
        record["finished_at"] = json!(now);
        record["progress"] = json!("completed");
        record["error_message"] = Value::Null;
        Ok(record)
    }
}

pub async fn execute_repository_manifest_backup(
    base_record: Value,
    manifest: Value,
) -> Result<Value, ApiError> {
    RepositoryManifestBackupExecutor
        .execute(BackupExecutionInput {
            base_record,
            manifest,
        })
        .await
}

#[derive(Debug, Clone)]
pub struct PostgresS3BackupExecutor {
    pub config: PostgresS3BackupConfig,
}

#[async_trait]
impl BackupExecutor for PostgresS3BackupExecutor {
    async fn execute(&self, input: BackupExecutionInput) -> Result<Value, ApiError> {
        let started_at = Utc::now().to_rfc3339();
        let file_name = postgres_backup_file_name();
        let s3_key = backup_s3_key(&self.config.prefix, &file_name);
        let database = self.config.database.clone();
        let sql_gzip = tokio::task::spawn_blocking(move || dump_postgres_gzip(&database))
            .await
            .map_err(|error| {
                ApiError::internal_server_error(format!("postgres backup task failed: {error}"))
            })??;
        let size_bytes = s3_probe::put_object(
            &self.config.s3,
            S3PutObjectInput {
                key: s3_key.clone(),
                body: sql_gzip,
                content_type: POSTGRES_BACKUP_CONTENT_TYPE.to_owned(),
            },
        )
        .await?;
        let now = Utc::now().to_rfc3339();
        let mut record = input.base_record;
        record["status"] = json!("completed");
        record["backup_type"] = json!("postgres");
        record["executor"] = json!("postgres_s3");
        record["artifact_kind"] = json!("postgres_sql_gzip");
        record["file_name"] = json!(file_name);
        record["s3_key"] = json!(s3_key);
        record["content_type"] = json!(POSTGRES_BACKUP_CONTENT_TYPE);
        record["size_bytes"] = json!(size_bytes);
        record["s3"] = redacted_s3_config(&self.config.s3, &self.config.prefix);
        record["manifest_version"] = json!(1);
        record["manifest"] = input.manifest;
        record["started_at"] = json!(started_at);
        record["finished_at"] = json!(now);
        record["progress"] = json!("completed");
        record["error_message"] = Value::Null;
        Ok(record)
    }
}

pub async fn execute_postgres_s3_backup(
    base_record: Value,
    manifest: Value,
    config: PostgresS3BackupConfig,
) -> Result<Value, ApiError> {
    PostgresS3BackupExecutor { config }
        .execute(BackupExecutionInput {
            base_record,
            manifest,
        })
        .await
}

impl PostgresDumpConfig {
    pub fn from_payload_or_runtime(payload: &Value) -> Result<Self, ApiError> {
        if let Some(database) = payload.get("database").or_else(|| payload.get("postgres")) {
            return Self::from_value(database);
        }
        if let Some(database_url) = std::env::var("DATABASE_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            return Self::from_database_url(&database_url);
        }
        let app_config = AppConfig::load()
            .map_err(|error| ApiError::internal_server_error(format!("load config: {error}")))?;
        if let Some(database) = app_config.database {
            return Ok(Self::from_database_config(database));
        }
        Err(ApiError::bad_request(
            "postgres backup database config is required",
        ))
    }

    pub fn from_value(value: &Value) -> Result<Self, ApiError> {
        let host = required_string(value, "host")?;
        let port = value
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| ApiError::bad_request("database.port is required"))?;
        let user = required_string(value, "user")?;
        let dbname = string_field(value, "dbname")
            .or_else(|| string_field(value, "database"))
            .ok_or_else(|| ApiError::bad_request("database.dbname is required"))?;
        let password = string_field(value, "password");
        let sslmode = string_field(value, "sslmode")
            .or_else(|| string_field(value, "ssl_mode"))
            .unwrap_or_else(|| "disable".to_owned());
        let pg_dump_path = string_field(value, "pg_dump_path")
            .or_else(|| string_field(value, "command"))
            .unwrap_or_else(|| DEFAULT_PG_DUMP_COMMAND.to_owned());
        let psql_path =
            string_field(value, "psql_path").unwrap_or_else(|| DEFAULT_PSQL_COMMAND.to_owned());
        Ok(Self {
            host,
            port,
            user,
            password,
            dbname,
            sslmode,
            pg_dump_path,
            psql_path,
        })
    }

    fn from_database_config(config: DatabaseConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            user: config.user,
            password: config.password,
            dbname: config.dbname,
            sslmode: config.sslmode.unwrap_or_else(|| "disable".to_owned()),
            pg_dump_path: DEFAULT_PG_DUMP_COMMAND.to_owned(),
            psql_path: DEFAULT_PSQL_COMMAND.to_owned(),
        }
    }

    fn from_database_url(database_url: &str) -> Result<Self, ApiError> {
        let url = reqwest::Url::parse(database_url)
            .map_err(|error| ApiError::bad_request(format!("invalid DATABASE_URL: {error}")))?;
        let host = url
            .host_str()
            .map(percent_decode)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("DATABASE_URL host is required"))?;
        let port = url.port().unwrap_or(5432);
        let user = percent_decode(url.username());
        if user.is_empty() {
            return Err(ApiError::bad_request("DATABASE_URL user is required"));
        }
        let password = url.password().map(percent_decode);
        let dbname = percent_decode(url.path().trim_start_matches('/'));
        if dbname.is_empty() {
            return Err(ApiError::bad_request(
                "DATABASE_URL database name is required",
            ));
        }
        let sslmode = query_param(&url, "sslmode").unwrap_or_else(|| "disable".to_owned());
        Ok(Self {
            host,
            port,
            user,
            password,
            dbname,
            sslmode,
            pg_dump_path: DEFAULT_PG_DUMP_COMMAND.to_owned(),
            psql_path: DEFAULT_PSQL_COMMAND.to_owned(),
        })
    }
}

pub async fn restore_postgres_s3_backup(
    mut record: Value,
    database: PostgresDumpConfig,
    s3: S3ProbeConfig,
) -> Result<Value, ApiError> {
    if record.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(ApiError::bad_request(
            "can only restore from a completed backup",
        ));
    }
    if record.get("artifact_kind").and_then(Value::as_str) != Some("postgres_sql_gzip") {
        return Err(ApiError::bad_request(
            "backup artifact is not a postgres SQL gzip backup",
        ));
    }
    let s3_key = string_field(&record, "s3_key")
        .ok_or_else(|| ApiError::bad_request("backup s3_key is required"))?;
    record["restore_status"] = json!("running");
    let object = s3_probe::get_object(&s3, &s3_key).await?;
    tokio::task::spawn_blocking(move || restore_postgres_gzip(&database, &object.body))
        .await
        .map_err(|error| {
            ApiError::internal_server_error(format!("postgres restore task failed: {error}"))
        })??;
    let restored_at = Utc::now().to_rfc3339();
    record["restore_status"] = json!("completed");
    record["restore_error"] = Value::Null;
    record["restored_at"] = json!(restored_at);
    Ok(record)
}

pub fn dump_postgres_gzip(config: &PostgresDumpConfig) -> Result<Vec<u8>, ApiError> {
    let mut child = Command::new(&config.pg_dump_path)
        .args([
            "-h",
            config.host.as_str(),
            "-p",
            &config.port.to_string(),
            "-U",
            config.user.as_str(),
            "-d",
            config.dbname.as_str(),
            "--no-owner",
            "--no-acl",
            "--clean",
            "--if-exists",
        ])
        .envs(pg_dump_env(config))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ApiError::bad_request(format!("start pg_dump failed: {error}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal_server_error("pg_dump stdout pipe is unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal_server_error("pg_dump stderr pipe is unavailable"))?;
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).map(|_| buffer)
    });
    let start = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| ApiError::bad_request(format!("wait pg_dump failed: {error}")))?
        {
            Some(status) => break status,
            None if start.elapsed() > PG_COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ApiError::bad_request("pg_dump timed out"));
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| ApiError::internal_server_error("pg_dump stdout reader panicked"))?
        .map_err(|error| ApiError::bad_request(format!("read pg_dump stdout failed: {error}")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ApiError::internal_server_error("pg_dump stderr reader panicked"))?
        .map_err(|error| ApiError::bad_request(format!("read pg_dump stderr failed: {error}")))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(ApiError::bad_request(format!(
            "pg_dump failed: {}",
            if stderr.is_empty() {
                status.to_string()
            } else {
                stderr
            }
        )));
    }
    gzip_bytes(&stdout)
}

fn restore_postgres_gzip(config: &PostgresDumpConfig, gzip_data: &[u8]) -> Result<(), ApiError> {
    let sql = gunzip_bytes(gzip_data)?;
    let mut child = Command::new(&config.psql_path)
        .args([
            "-h",
            config.host.as_str(),
            "-p",
            &config.port.to_string(),
            "-U",
            config.user.as_str(),
            "-d",
            config.dbname.as_str(),
            "--single-transaction",
        ])
        .envs(pg_dump_env(config))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ApiError::bad_request(format!("start psql failed: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::internal_server_error("psql stdin pipe is unavailable"))?;
    stdin
        .write_all(&sql)
        .map_err(|error| ApiError::bad_request(format!("write psql stdin failed: {error}")))?;
    drop(stdin);

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal_server_error("psql stdout pipe is unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal_server_error("psql stderr pipe is unavailable"))?;
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).map(|_| buffer)
    });
    let start = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| ApiError::bad_request(format!("wait psql failed: {error}")))?
        {
            Some(status) => break status,
            None if start.elapsed() > PG_COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ApiError::bad_request("psql restore timed out"));
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    };
    let _stdout = stdout_reader
        .join()
        .map_err(|_| ApiError::internal_server_error("psql stdout reader panicked"))?
        .map_err(|error| ApiError::bad_request(format!("read psql stdout failed: {error}")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ApiError::internal_server_error("psql stderr reader panicked"))?
        .map_err(|error| ApiError::bad_request(format!("read psql stderr failed: {error}")))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(ApiError::bad_request(format!(
            "psql restore failed: {}",
            if stderr.is_empty() {
                status.to_string()
            } else {
                stderr
            }
        )));
    }
    Ok(())
}

fn pg_dump_env(config: &PostgresDumpConfig) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if let Some(password) = &config.password {
        env.push(("PGPASSWORD", password.clone()));
    }
    if !config.sslmode.trim().is_empty() {
        env.push(("PGSSLMODE", config.sslmode.clone()));
    }
    env
}

fn gzip_bytes(input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| ApiError::internal_server_error(format!("gzip backup failed: {error}")))?;
    encoder
        .finish()
        .map_err(|error| ApiError::internal_server_error(format!("gzip backup failed: {error}")))
}

fn gunzip_bytes(input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut decoder = GzDecoder::new(input);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).map_err(|error| {
        ApiError::bad_request(format!("decompress postgres backup failed: {error}"))
    })?;
    Ok(output)
}

fn postgres_backup_file_name() -> String {
    format!(
        "sub2api-postgres-{}.sql.gz",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    )
}

fn backup_s3_key(prefix: &str, file_name: &str) -> String {
    let prefix = prefix.trim().trim_matches('/');
    let prefix = if prefix.is_empty() {
        DEFAULT_BACKUP_PREFIX
    } else {
        prefix
    };
    format!(
        "{}/{}/{}",
        prefix,
        Utc::now().format("%Y/%m/%d"),
        file_name.trim_matches('/')
    )
}

fn redacted_s3_config(config: &S3ProbeConfig, prefix: &str) -> Value {
    json!({
        "endpoint": config.endpoint,
        "region": config.region,
        "bucket": config.bucket,
        "access_key_id": config.access_key_id,
        "secret_access_key": "",
        "secret_access_key_configured": !config.secret_access_key.trim().is_empty(),
        "prefix": prefix,
        "force_path_style": config.force_path_style,
        "use_ssl": config.use_ssl
    })
}

fn required_string(value: &Value, key: &str) -> Result<String, ApiError> {
    string_field(value, key)
        .ok_or_else(|| ApiError::bad_request(format!("database.{key} is required")))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn query_param(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.to_string()))
        .filter(|value| !value.is_empty())
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    output.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[tokio::test]
    async fn repository_manifest_executor_marks_record_and_size() {
        let record = execute_repository_manifest_backup(
            json!({
                "id": "backup-test",
                "status": "running",
                "size_bytes": 0,
                "manifest": Value::Null
            }),
            json!({
                "version": 1,
                "system_settings": [],
                "admin_collections": []
            }),
        )
        .await
        .unwrap();

        assert_eq!(record["status"], "completed");
        assert_eq!(record["executor"], "repository_manifest");
        assert_eq!(record["artifact_kind"], "repository_manifest");
        assert_eq!(record["manifest_version"], 1);
        assert!(record["size_bytes"].as_u64().unwrap() > 0);
        assert_eq!(record["manifest"]["version"], 1);
    }

    #[test]
    fn postgres_dump_config_accepts_payload_and_database_url() {
        let config = PostgresDumpConfig::from_value(&json!({
            "host": "127.0.0.1",
            "port": 5432,
            "user": "test",
            "password": "123456",
            "dbname": "sub2api_new",
            "sslmode": "disable",
            "pg_dump_path": "fake-pg-dump"
        }))
        .unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 5432);
        assert_eq!(config.user, "test");
        assert_eq!(config.password.as_deref(), Some("123456"));
        assert_eq!(config.dbname, "sub2api_new");
        assert_eq!(config.pg_dump_path, "fake-pg-dump");

        let from_url = PostgresDumpConfig::from_database_url(
            "postgres://test:p%40ss@localhost:5433/sub2api_new?sslmode=require",
        )
        .unwrap();
        assert_eq!(from_url.host, "localhost");
        assert_eq!(from_url.port, 5433);
        assert_eq!(from_url.password.as_deref(), Some("p@ss"));
        assert_eq!(from_url.sslmode, "require");
    }

    #[test]
    fn gzip_bytes_round_trips_backup_sql() {
        let compressed = gzip_bytes(b"select 1;\n").unwrap();
        let mut decoder = GzDecoder::new(compressed.as_slice());
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, "select 1;\n");
    }
}
