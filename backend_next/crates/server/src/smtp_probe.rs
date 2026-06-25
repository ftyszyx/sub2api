use crate::response::ApiError;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::ServerName;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

const SMTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub from_name: String,
    pub use_tls: bool,
}

impl SmtpConfig {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub fn test_connection(config: &SmtpConfig) -> Result<(), ApiError> {
    let mut session = SmtpSession::connect(config)?;
    session.hello("sub2api.local")?;
    session.authenticate(config)?;
    session.quit()
}

pub fn send_test_email(
    config: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), ApiError> {
    let mut session = SmtpSession::connect(config)?;
    session.hello("sub2api.local")?;
    session.authenticate(config)?;
    let from = if config.from.trim().is_empty() {
        config.username.trim()
    } else {
        config.from.trim()
    };
    if from.is_empty() {
        return Err(ApiError::bad_request("SMTP from email is required"));
    }
    session.command(&format!("MAIL FROM:<{from}>"), &[250])?;
    session.command(&format!("RCPT TO:<{}>", to.trim()), &[250, 251])?;
    session.command("DATA", &[354])?;
    let message = build_message(from, &config.from_name, to, subject, body);
    session.write_raw(&message)?;
    session.expect_codes(&[250])?;
    session.quit()
}

fn build_message(from: &str, from_name: &str, to: &str, subject: &str, body: &str) -> String {
    let from_header = if from_name.trim().is_empty() {
        from.to_owned()
    } else {
        format!("{} <{}>", from_name.trim(), from)
    };
    format!(
        "From: {from_header}\r\nTo: {to}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{body}\r\n.\r\n"
    )
}

struct SmtpSession {
    reader: BufReader<Box<dyn SmtpStream>>,
}

impl SmtpSession {
    fn connect(config: &SmtpConfig) -> Result<Self, ApiError> {
        let stream = connect_tcp(config)?;
        let stream: Box<dyn SmtpStream> = if config.use_tls {
            Box::new(connect_tls(config, stream)?)
        } else {
            Box::new(stream)
        };
        Self::from_stream(stream)
    }

    fn from_stream(stream: Box<dyn SmtpStream>) -> Result<Self, ApiError> {
        let mut session = Self {
            reader: BufReader::new(stream),
        };
        session.expect_codes(&[220])?;
        Ok(session)
    }

    #[cfg(test)]
    fn connect_with_tls_config(
        config: &SmtpConfig,
        tls_config: ClientConfig,
    ) -> Result<Self, ApiError> {
        let stream = connect_tcp(config)?;
        Self::from_stream(Box::new(connect_tls_with_config(
            config, stream, tls_config,
        )?))
    }

    fn hello(&mut self, client_name: &str) -> Result<(), ApiError> {
        self.command(&format!("EHLO {client_name}"), &[250])
            .or_else(|_| self.command(&format!("HELO {client_name}"), &[250]))
    }

    fn authenticate(&mut self, config: &SmtpConfig) -> Result<(), ApiError> {
        if config.username.trim().is_empty() {
            return Ok(());
        }
        self.command("AUTH LOGIN", &[334])?;
        self.command(&base64_encode(config.username.as_bytes()), &[334])?;
        self.command(&base64_encode(config.password.as_bytes()), &[235])
            .map_err(|error| {
                ApiError::bad_request(format!("SMTP authentication failed: {}", error.message()))
            })
    }

    fn quit(&mut self) -> Result<(), ApiError> {
        self.command("QUIT", &[221]).or(Ok(()))
    }

    fn command(&mut self, command: &str, expected: &[u16]) -> Result<(), ApiError> {
        self.write_raw(&format!("{command}\r\n"))?;
        self.expect_codes(expected)
    }

    fn write_raw(&mut self, raw: &str) -> Result<(), ApiError> {
        let stream = self.reader.get_mut();
        stream
            .write_all(raw.as_bytes())
            .and_then(|_| stream.flush())
            .map_err(|error| ApiError::bad_request(format!("SMTP write failed: {error}")))
    }

    fn expect_codes(&mut self, expected: &[u16]) -> Result<(), ApiError> {
        let response = self.read_response()?;
        if expected.contains(&response.code) {
            Ok(())
        } else {
            Err(ApiError::bad_request(format!(
                "SMTP command failed: expected {:?}, got {} {}",
                expected, response.code, response.message
            )))
        }
    }

    fn read_response(&mut self) -> Result<SmtpResponse, ApiError> {
        let mut message = String::new();
        loop {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .map_err(|error| ApiError::bad_request(format!("SMTP read failed: {error}")))?;
            if read == 0 {
                return Err(ApiError::bad_request("SMTP connection closed"));
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.len() < 3 {
                return Err(ApiError::bad_request(format!(
                    "SMTP invalid response: {trimmed}"
                )));
            }
            let parsed_code = trimmed[..3].parse::<u16>().map_err(|_| {
                ApiError::bad_request(format!("SMTP invalid response code: {trimmed}"))
            })?;
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(trimmed);
            if trimmed.as_bytes().get(3).copied() != Some(b'-') {
                return Ok(SmtpResponse {
                    code: parsed_code,
                    message,
                });
            }
        }
    }
}

trait SmtpStream: Read + Write + Send {}

impl<T> SmtpStream for T where T: Read + Write + Send {}

fn connect_tcp(config: &SmtpConfig) -> Result<TcpStream, ApiError> {
    let addresses = config
        .address()
        .to_socket_addrs()
        .map_err(|error| ApiError::bad_request(format!("SMTP address resolve failed: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(ApiError::bad_request("SMTP address resolve failed"));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, SMTP_TIMEOUT) {
            Ok(stream) => return configure_tcp_timeouts(stream),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(ApiError::bad_request(format!(
        "SMTP connection failed: {}",
        last_error.unwrap_or_else(|| "no resolved address connected".to_owned())
    )))
}

fn configure_tcp_timeouts(stream: TcpStream) -> Result<TcpStream, ApiError> {
    stream
        .set_read_timeout(Some(SMTP_TIMEOUT))
        .map_err(|error| ApiError::bad_request(format!("SMTP set read timeout failed: {error}")))?;
    stream
        .set_write_timeout(Some(SMTP_TIMEOUT))
        .map_err(|error| {
            ApiError::bad_request(format!("SMTP set write timeout failed: {error}"))
        })?;
    Ok(stream)
}

fn connect_tls(
    config: &SmtpConfig,
    stream: TcpStream,
) -> Result<StreamOwned<ClientConnection, TcpStream>, ApiError> {
    let tls_config = default_tls_config();
    connect_tls_with_config(config, stream, tls_config)
}

fn connect_tls_with_config(
    config: &SmtpConfig,
    stream: TcpStream,
    tls_config: ClientConfig,
) -> Result<StreamOwned<ClientConnection, TcpStream>, ApiError> {
    let server_name = ServerName::try_from(config.host.trim().to_owned())
        .map_err(|_| ApiError::bad_request("SMTP TLS server name is invalid"))?;
    let connection = ClientConnection::new(Arc::new(tls_config), server_name)
        .map_err(|error| ApiError::bad_request(format!("SMTP TLS setup failed: {error}")))?;
    let mut stream = StreamOwned::new(connection, stream);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|error| ApiError::bad_request(format!("SMTP TLS handshake failed: {error}")))?;
    Ok(stream)
}

fn default_tls_config() -> ClientConfig {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring provider supports default TLS protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

struct SmtpResponse {
    code: u16,
    message: String,
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, SmtpConfig, SmtpSession, SMTP_TIMEOUT};
    use rcgen::{generate_simple_self_signed, CertifiedKey};
    use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};
    use std::thread;

    #[test]
    fn base64_encoder_matches_auth_login_examples() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"u"), "dQ==");
        assert_eq!(base64_encode(b"user"), "dXNlcg==");
        assert_eq!(base64_encode(b"password"), "cGFzc3dvcmQ=");
    }

    #[test]
    fn tls_smtp_connection_authenticates_and_quits() {
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
        let (lines_tx, lines_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(false).unwrap();
            port_tx.send(listener.local_addr().unwrap().port()).unwrap();
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(SMTP_TIMEOUT)).unwrap();
            tcp.set_write_timeout(Some(SMTP_TIMEOUT)).unwrap();
            let tls = ServerConnection::new(Arc::new(server_config)).unwrap();
            let mut stream = StreamOwned::new(tls, tcp);
            stream.write_all(b"220 tls-smtp ESMTP\r\n").unwrap();
            stream.flush().unwrap();
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line).unwrap();
                if read == 0 {
                    break;
                }
                let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                lines_tx.send(trimmed.clone()).unwrap();
                let upper = trimmed.to_ascii_uppercase();
                let response: &[u8] = if upper.starts_with("EHLO ") || upper.starts_with("HELO ") {
                    b"250-tls-smtp\r\n250 AUTH LOGIN\r\n"
                } else if upper == "AUTH LOGIN" {
                    b"334 VXNlcm5hbWU6\r\n"
                } else if trimmed == "dXNlcg==" {
                    b"334 UGFzc3dvcmQ6\r\n"
                } else if trimmed == "cGFzc3dvcmQ=" {
                    b"235 authenticated\r\n"
                } else if upper == "QUIT" {
                    reader.get_mut().write_all(b"221 bye\r\n").unwrap();
                    reader.get_mut().flush().unwrap();
                    break;
                } else {
                    b"250 ok\r\n"
                };
                reader.get_mut().write_all(response).unwrap();
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
        let config = SmtpConfig {
            host: "localhost".to_owned(),
            port: port_rx.recv().unwrap(),
            username: "user".to_owned(),
            password: "password".to_owned(),
            from: String::new(),
            from_name: String::new(),
            use_tls: true,
        };

        let mut session = SmtpSession::connect_with_tls_config(&config, tls_config).unwrap();
        session.hello("sub2api.local").unwrap();
        session.authenticate(&config).unwrap();
        session.quit().unwrap();

        server.join().unwrap();
        let lines = lines_rx.try_iter().collect::<Vec<_>>();
        assert!(lines.iter().any(|line| line == "AUTH LOGIN"));
        assert!(lines.iter().any(|line| line == "dXNlcg=="));
        assert!(lines.iter().any(|line| line == "cGFzc3dvcmQ="));
        assert!(lines.iter().any(|line| line == "QUIT"));
    }
}
