use crate::response::ApiError;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REDIS_TIMEOUT: Duration = Duration::from_secs(3);
const DATABASE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
    pub sslmode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub db: Option<u8>,
    pub enable_tls: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminConfig {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ServerConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub admin: AdminConfig,
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Clone)]
pub struct SetupStatus {
    pub needs_setup: bool,
    pub step: &'static str,
}

#[derive(Debug, Clone)]
enum SetupStorage {
    Memory,
    FileSystem { data_dir: PathBuf },
}

#[derive(Debug)]
struct SetupInner {
    installed: bool,
    config_yaml: Option<String>,
    install_lock: Option<String>,
}

#[derive(Debug)]
pub struct SetupService {
    storage: SetupStorage,
    inner: Mutex<SetupInner>,
}

impl SetupService {
    pub fn installed_memory() -> Self {
        Self::new(SetupStorage::Memory, true)
    }

    pub fn setup_memory() -> Self {
        Self::new(SetupStorage::Memory, false)
    }

    pub fn from_environment() -> Self {
        let data_dir = data_dir_from_environment();
        let installed = config_path(&data_dir).exists() || install_lock_path(&data_dir).exists();
        Self::new(SetupStorage::FileSystem { data_dir }, installed)
    }

    fn new(storage: SetupStorage, installed: bool) -> Self {
        Self {
            storage,
            inner: Mutex::new(SetupInner {
                installed,
                config_yaml: None,
                install_lock: None,
            }),
        }
    }

    pub fn status(&self) -> SetupStatus {
        let installed = self.is_installed();
        SetupStatus {
            needs_setup: !installed,
            step: if installed { "completed" } else { "welcome" },
        }
    }

    pub fn config_yaml(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("setup mutex poisoned")
            .config_yaml
            .clone()
    }

    pub fn install_lock(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("setup mutex poisoned")
            .install_lock
            .clone()
    }

    pub async fn test_database(&self, config: &DatabaseConfig) -> Result<Value, ApiError> {
        self.guard_setup_allowed()?;
        test_database_connection(config).await
    }

    pub fn test_redis(&self, config: &RedisConfig) -> Result<Value, ApiError> {
        self.guard_setup_allowed()?;
        test_redis_connection(config)
    }

    pub fn install(&self, request: &InstallRequest) -> Result<Value, ApiError> {
        let mut inner = self.inner.lock().expect("setup mutex poisoned");
        if inner.installed {
            return Err(ApiError::forbidden(
                "Setup is not allowed: system is already installed",
            ));
        }

        validate_install_request(request)?;
        let normalized = NormalizedInstallConfig::from_request(request);
        let config_yaml = render_config_yaml(&normalized);
        let install_lock = format!(
            "installed_at={}\n",
            unix_timestamp_rfc3339_like(SystemTime::now())
        );

        match &self.storage {
            SetupStorage::Memory => {}
            SetupStorage::FileSystem { data_dir } => {
                fs::create_dir_all(data_dir).map_err(|error| {
                    ApiError::internal_server_error(format!(
                        "Installation failed: unable to create data directory: {error}"
                    ))
                })?;
                fs::write(config_path(data_dir), config_yaml.as_bytes()).map_err(|error| {
                    ApiError::internal_server_error(format!(
                        "Installation failed: config file creation failed: {error}"
                    ))
                })?;
                fs::write(install_lock_path(data_dir), install_lock.as_bytes()).map_err(
                    |error| {
                        ApiError::internal_server_error(format!(
                            "Installation failed: install lock creation failed: {error}"
                        ))
                    },
                )?;
            }
        }

        inner.installed = true;
        inner.config_yaml = Some(config_yaml);
        inner.install_lock = Some(install_lock);

        Ok(json!({
            "message": "Installation completed successfully. Service will restart automatically.",
            "restart": true
        }))
    }

    fn is_installed(&self) -> bool {
        let mut inner = self.inner.lock().expect("setup mutex poisoned");
        if inner.installed {
            return true;
        }
        if let SetupStorage::FileSystem { data_dir } = &self.storage {
            if config_path(data_dir).exists() || install_lock_path(data_dir).exists() {
                inner.installed = true;
            }
        }
        inner.installed
    }

    fn guard_setup_allowed(&self) -> Result<(), ApiError> {
        if self.is_installed() {
            Err(ApiError::forbidden(
                "Setup is not allowed: system is already installed",
            ))
        } else {
            Ok(())
        }
    }
}

pub fn validate_database(config: &DatabaseConfig) -> Result<Value, ApiError> {
    validate_hostname(&config.host, "Invalid hostname format")?;
    validate_port(config.port, "Invalid port number")?;
    validate_username(&config.user, "Invalid username format")?;
    validate_db_name(&config.dbname, "Invalid database name format")?;
    validate_ssl_mode(config.sslmode.as_deref().unwrap_or("disable"))?;
    let _ = &config.password;

    Ok(json!({ "message": "Connection configuration is valid" }))
}

pub async fn test_database_connection(config: &DatabaseConfig) -> Result<Value, ApiError> {
    validate_database(config)?;
    let database_url = database_url_from_config(config);
    let pool = tokio::time::timeout(
        DATABASE_TIMEOUT,
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(DATABASE_TIMEOUT)
            .connect(&database_url),
    )
    .await
    .map_err(|_| ApiError::bad_request("Database connection timed out"))?
    .map_err(|error| ApiError::bad_request(format!("Database connection failed: {error}")))?;
    sqlx::query("SELECT 1")
        .execute(&pool)
        .await
        .map_err(|error| ApiError::bad_request(format!("Database validation failed: {error}")))?;
    pool.close().await;
    Ok(json!({ "message": "Database connection successful" }))
}

pub fn validate_redis(config: &RedisConfig) -> Result<Value, ApiError> {
    validate_redis_config(config)?;
    Ok(json!({ "message": "Connection configuration is valid" }))
}

fn validate_redis_config(config: &RedisConfig) -> Result<(), ApiError> {
    validate_hostname(&config.host, "Invalid hostname format")?;
    validate_port(config.port, "Invalid port number")?;
    if config.db.unwrap_or(0) > 15 {
        return Err(ApiError::bad_request(
            "Invalid Redis database number (0-15)",
        ));
    }
    let _ = (config.port, &config.password, config.enable_tls);
    Ok(())
}

pub fn test_redis_connection(config: &RedisConfig) -> Result<Value, ApiError> {
    validate_redis_config(config)?;
    let mut stream = open_redis_stream(config)?;
    if let Some(password) = config
        .password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        redis_command(&mut stream, &["AUTH", password])?;
    }
    if let Some(db) = config.db {
        if db > 0 {
            redis_command(&mut stream, &["SELECT", &db.to_string()])?;
        }
    }
    redis_command(&mut stream, &["PING"])?;
    Ok(json!({ "message": "Redis connection successful" }))
}

pub fn validate_install(request: &InstallRequest) -> Result<Value, ApiError> {
    validate_install_request(request)?;
    Ok(json!({
        "message": "Installation configuration is valid",
        "restart": false
    }))
}

fn validate_install_request(request: &InstallRequest) -> Result<(), ApiError> {
    validate_database(&request.database)?;
    validate_redis_config(&request.redis)?;
    validate_email(&request.admin.email)?;
    validate_password(&request.admin.password)?;
    if let Some(host) = request
        .server
        .host
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        validate_hostname(host, "Invalid server hostname")?;
    }
    if let Some(mode) = request.server.mode.as_deref() {
        if mode != "release" && mode != "debug" {
            return Err(ApiError::bad_request(
                "Invalid server mode (must be 'release' or 'debug')",
            ));
        }
    }
    let _ = request.server.port;

    Ok(())
}

trait RedisStream: Read + Write + Send {}

impl<T> RedisStream for T where T: Read + Write + Send {}

fn open_redis_stream(config: &RedisConfig) -> Result<Box<dyn RedisStream>, ApiError> {
    let stream = open_redis_tcp_stream(config)?;
    if config.enable_tls.unwrap_or(false) {
        Ok(Box::new(connect_redis_tls(config, stream)?))
    } else {
        Ok(Box::new(stream))
    }
}

fn open_redis_tcp_stream(config: &RedisConfig) -> Result<TcpStream, ApiError> {
    let address = format!("{}:{}", config.host, config.port);
    let addresses = address
        .to_socket_addrs()
        .map_err(|error| {
            ApiError::bad_request(format!("Redis address resolution failed: {error}"))
        })?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(ApiError::bad_request("Redis address resolution failed"));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, REDIS_TIMEOUT) {
            Ok(stream) => return configure_redis_tcp_timeouts(stream),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(ApiError::bad_request(format!(
        "Redis connection failed: {}",
        last_error.unwrap_or_else(|| "no resolved address connected".to_owned())
    )))
}

fn configure_redis_tcp_timeouts(stream: TcpStream) -> Result<TcpStream, ApiError> {
    stream
        .set_read_timeout(Some(REDIS_TIMEOUT))
        .map_err(|error| {
            ApiError::internal_server_error(format!("Redis read timeout setup failed: {error}"))
        })?;
    stream
        .set_write_timeout(Some(REDIS_TIMEOUT))
        .map_err(|error| {
            ApiError::internal_server_error(format!("Redis write timeout setup failed: {error}"))
        })?;
    Ok(stream)
}

fn connect_redis_tls(
    config: &RedisConfig,
    stream: TcpStream,
) -> Result<StreamOwned<ClientConnection, TcpStream>, ApiError> {
    connect_redis_tls_with_config(config, stream, default_redis_tls_config())
}

fn connect_redis_tls_with_config(
    config: &RedisConfig,
    stream: TcpStream,
    tls_config: ClientConfig,
) -> Result<StreamOwned<ClientConnection, TcpStream>, ApiError> {
    let server_name = ServerName::try_from(config.host.trim().to_owned())
        .map_err(|_| ApiError::bad_request("Redis TLS server name is invalid"))?;
    let connection = ClientConnection::new(Arc::new(tls_config), server_name)
        .map_err(|error| ApiError::bad_request(format!("Redis TLS setup failed: {error}")))?;
    let mut stream = StreamOwned::new(connection, stream);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|error| ApiError::bad_request(format!("Redis TLS handshake failed: {error}")))?;
    Ok(stream)
}

fn default_redis_tls_config() -> ClientConfig {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring provider supports default TLS protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

fn redis_command(stream: &mut dyn RedisStream, parts: &[&str]) -> Result<String, ApiError> {
    let mut command = format!("*{}\r\n", parts.len());
    for part in parts {
        command.push_str(&format!("${}\r\n{}\r\n", part.as_bytes().len(), part));
    }
    stream
        .write_all(command.as_bytes())
        .map_err(|error| ApiError::bad_request(format!("Redis command failed: {error}")))?;
    stream
        .flush()
        .map_err(|error| ApiError::bad_request(format!("Redis command flush failed: {error}")))?;
    let mut buffer = [0_u8; 512];
    let read = stream
        .read(&mut buffer)
        .map_err(|error| ApiError::bad_request(format!("Redis response read failed: {error}")))?;
    if read == 0 {
        return Err(ApiError::bad_request("Redis closed the connection"));
    }
    let response = String::from_utf8_lossy(&buffer[..read]).to_string();
    if response.starts_with("-") {
        return Err(ApiError::bad_request(format!(
            "Redis command rejected: {}",
            response.trim()
        )));
    }
    Ok(response)
}

#[derive(Debug, Clone)]
struct NormalizedInstallConfig {
    database: DatabaseConfig,
    redis: RedisConfig,
    server: ServerConfig,
    jwt_secret: String,
    jwt_expire_hour: u16,
    timezone: String,
}

impl NormalizedInstallConfig {
    fn from_request(request: &InstallRequest) -> Self {
        let mut database = request.database.clone();
        if database.sslmode.as_deref().unwrap_or("").is_empty() {
            database.sslmode = Some("disable".to_owned());
        }

        let mut redis = request.redis.clone();
        if redis.db.is_none() {
            redis.db = Some(0);
        }
        if redis.enable_tls.is_none() {
            redis.enable_tls = Some(false);
        }

        let mut server = request.server.clone();
        if server.host.as_deref().unwrap_or("").is_empty() {
            server.host = Some("0.0.0.0".to_owned());
        }
        if server.port.is_none() {
            server.port = Some(8080);
        }
        if server.mode.as_deref().unwrap_or("").is_empty() {
            server.mode = Some("release".to_owned());
        }

        Self {
            database,
            redis,
            server,
            jwt_secret: generate_secret_hex(32),
            jwt_expire_hour: 24,
            timezone: "Asia/Shanghai".to_owned(),
        }
    }
}

fn render_config_yaml(config: &NormalizedInstallConfig) -> String {
    format!(
        concat!(
            "server:\n",
            "  host: \"{}\"\n",
            "  port: {}\n",
            "  mode: \"{}\"\n",
            "\n",
            "database:\n",
            "  host: \"{}\"\n",
            "  port: {}\n",
            "  user: \"{}\"\n",
            "  password: \"{}\"\n",
            "  dbname: \"{}\"\n",
            "  sslmode: \"{}\"\n",
            "\n",
            "redis:\n",
            "  host: \"{}\"\n",
            "  port: {}\n",
            "  password: \"{}\"\n",
            "  db: {}\n",
            "  enable_tls: {}\n",
            "\n",
            "jwt:\n",
            "  secret: \"{}\"\n",
            "  expire_hour: {}\n",
            "\n",
            "default:\n",
            "  user_concurrency: 5\n",
            "  user_balance: 0\n",
            "  api_key_prefix: \"sk-\"\n",
            "  rate_multiplier: 1.0\n",
            "\n",
            "rate_limit:\n",
            "  requests_per_minute: 60\n",
            "  burst_size: 10\n",
            "\n",
            "timezone: \"{}\"\n"
        ),
        yaml_escape(config.server.host.as_deref().unwrap_or("0.0.0.0")),
        config.server.port.unwrap_or(8080),
        yaml_escape(config.server.mode.as_deref().unwrap_or("release")),
        yaml_escape(&config.database.host),
        config.database.port,
        yaml_escape(&config.database.user),
        yaml_escape(config.database.password.as_deref().unwrap_or("")),
        yaml_escape(&config.database.dbname),
        yaml_escape(config.database.sslmode.as_deref().unwrap_or("disable")),
        yaml_escape(&config.redis.host),
        config.redis.port,
        yaml_escape(config.redis.password.as_deref().unwrap_or("")),
        config.redis.db.unwrap_or(0),
        config.redis.enable_tls.unwrap_or(false),
        config.jwt_secret,
        config.jwt_expire_hour,
        yaml_escape(&config.timezone),
    )
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn generate_secret_hex(bytes: usize) -> String {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15_u128;
    let mut output = String::with_capacity(bytes * 2);
    for index in 0..bytes {
        state ^= state << 7;
        state ^= state >> 9;
        state = state.wrapping_mul(0x1000_0000_01b3);
        let byte = ((state >> ((index % 8) * 8)) & 0xff) as u8;
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn unix_timestamp_rfc3339_like(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("{seconds}")
}

fn data_dir_from_environment() -> PathBuf {
    if let Ok(dir) = std::env::var("DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    let docker_data_dir = PathBuf::from("/app/data");
    if docker_data_dir.is_dir() && is_writable_dir(&docker_data_dir) {
        return docker_data_dir;
    }

    PathBuf::from(".")
}

fn is_writable_dir(path: &Path) -> bool {
    let test_file = path.join(".write_test_backend_next");
    match fs::write(&test_file, b"test") {
        Ok(()) => {
            let _ = fs::remove_file(test_file);
            true
        }
        Err(_) => false,
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.yaml")
}

fn install_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".installed")
}

fn validate_hostname(value: &str, message: &'static str) -> Result<(), ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'));
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(message))
    }
}

fn validate_port(value: u16, message: &'static str) -> Result<(), ApiError> {
    if value > 0 {
        Ok(())
    } else {
        Err(ApiError::bad_request(message))
    }
}

fn validate_db_name(value: &str, message: &'static str) -> Result<(), ApiError> {
    let mut bytes = value.bytes();
    let starts_with_letter = bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic());
    let rest_valid = bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if starts_with_letter && rest_valid && value.len() <= 63 {
        Ok(())
    } else {
        Err(ApiError::bad_request(message))
    }
}

fn validate_username(value: &str, message: &'static str) -> Result<(), ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(message))
    }
}

fn validate_ssl_mode(value: &str) -> Result<(), ApiError> {
    match value {
        "disable" | "require" | "verify-ca" | "verify-full" => Ok(()),
        _ => Err(ApiError::bad_request("Invalid SSL mode")),
    }
}

fn database_url_from_config(config: &DatabaseConfig) -> String {
    let sslmode = config.sslmode.as_deref().unwrap_or("disable");
    format!(
        "postgres://{}:{}@{}:{}/{}?sslmode={}",
        percent_encode(&config.user),
        percent_encode(config.password.as_deref().unwrap_or("")),
        config.host,
        config.port,
        percent_encode(&config.dbname),
        percent_encode(sslmode)
    )
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

fn validate_email(value: &str) -> Result<(), ApiError> {
    if value.contains('@') && value.len() <= 254 {
        Ok(())
    } else {
        Err(ApiError::bad_request("Invalid admin email format"))
    }
}

fn validate_password(value: &str) -> Result<(), ApiError> {
    if (8..=128).contains(&value.len()) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        connect_redis_tls_with_config, open_redis_tcp_stream, redis_command, RedisConfig,
        REDIS_TIMEOUT,
    };
    use rcgen::{generate_simple_self_signed, CertifiedKey};
    use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};
    use std::thread;

    #[test]
    fn tls_redis_connection_sends_ping_over_secure_stream() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(["localhost".to_owned()]).unwrap();
        let cert_der = cert.der().clone();
        let server_config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert_der.clone()],
                    PrivateKeyDer::try_from(signing_key.serialize_der()).unwrap(),
                )
                .unwrap();
        let (port_tx, port_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            port_tx.send(listener.local_addr().unwrap().port()).unwrap();
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(REDIS_TIMEOUT)).unwrap();
            tcp.set_write_timeout(Some(REDIS_TIMEOUT)).unwrap();
            let tls = ServerConnection::new(Arc::new(server_config)).unwrap();
            let stream = StreamOwned::new(tls, tcp);
            let mut reader = BufReader::new(stream);

            let command = read_resp_command(&mut reader).unwrap();
            commands_tx.send(command.clone()).unwrap();
            if command == vec!["PING".to_owned()] {
                reader.get_mut().write_all(b"+PONG\r\n").unwrap();
                reader.get_mut().flush().unwrap();
            }
        });

        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(cert_der.to_vec())).unwrap();
        let tls_config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();

        let config = RedisConfig {
            host: "localhost".to_owned(),
            port: port_rx.recv().unwrap(),
            password: None,
            db: Some(0),
            enable_tls: Some(true),
        };
        let tcp = open_redis_tcp_stream(&config).unwrap();
        let mut tls = connect_redis_tls_with_config(&config, tcp, tls_config).unwrap();

        let response = redis_command(&mut tls, &["PING"]).unwrap();

        assert_eq!(response, "+PONG\r\n");
        assert_eq!(commands_rx.recv().unwrap(), vec!["PING".to_owned()]);
        server.join().unwrap();
    }

    fn read_resp_command<R: BufRead>(reader: &mut R) -> std::io::Result<Vec<String>> {
        let line = read_line(reader)?;
        let Some(count) = line
            .strip_prefix('*')
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return Ok(Vec::new());
        };
        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            let bulk_header = read_line(reader)?;
            let Some(length) = bulk_header
                .strip_prefix('$')
                .and_then(|value| value.parse::<usize>().ok())
            else {
                return Ok(Vec::new());
            };
            let mut data = vec![0_u8; length + 2];
            reader.read_exact(&mut data)?;
            parts.push(String::from_utf8_lossy(&data[..length]).to_string());
        }
        Ok(parts)
    }

    fn read_line<R: BufRead>(reader: &mut R) -> std::io::Result<String> {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}
