use crate::setup::RedisConfig;
use anyhow::{anyhow, Context};
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

#[derive(Debug)]
pub(crate) struct RedisClient {
    stream: TcpStream,
}

impl RedisClient {
    pub(crate) fn connect(config: &RedisConfig) -> anyhow::Result<Self> {
        if config.enable_tls.unwrap_or(false) {
            return Err(anyhow!(
                "Redis TLS is not supported by this lightweight client yet"
            ));
        }
        let address = format!("{}:{}", config.host, config.port)
            .to_socket_addrs()
            .with_context(|| format!("failed to resolve Redis address {}", config.host))?
            .next()
            .ok_or_else(|| anyhow!("failed to resolve Redis address {}", config.host))?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .with_context(|| format!("failed to connect Redis at {address}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut client = Self { stream };
        if let Some(password) = config
            .password
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            client.command(&["AUTH", password])?;
        }
        if let Some(db) = config.db {
            if db > 0 {
                client.command(&["SELECT", &db.to_string()])?;
            }
        }
        Ok(client)
    }

    pub(crate) fn command(&mut self, parts: &[&str]) -> anyhow::Result<RespValue> {
        let parts = parts
            .iter()
            .map(|part| part.as_bytes().to_vec())
            .collect::<Vec<_>>();
        self.command_bytes(&parts)
    }

    pub(crate) fn command_bytes(&mut self, parts: &[Vec<u8>]) -> anyhow::Result<RespValue> {
        write_resp_command(&mut self.stream, parts)?;
        let mut reader = BufReader::new(&mut self.stream);
        read_resp_value(&mut reader)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum RespValue {
    Simple(String),
    Bulk(Option<Vec<u8>>),
    Array(Vec<RespValue>),
    Integer(i64),
}

pub(crate) fn write_resp_command<W: Write>(
    writer: &mut W,
    parts: &[Vec<u8>],
) -> anyhow::Result<()> {
    writer.write_all(format!("*{}\r\n", parts.len()).as_bytes())?;
    for part in parts {
        writer.write_all(format!("${}\r\n", part.len()).as_bytes())?;
        writer.write_all(part)?;
        writer.write_all(b"\r\n")?;
    }
    writer.flush()?;
    Ok(())
}

pub(crate) fn read_resp_value<R: BufRead>(reader: &mut R) -> anyhow::Result<RespValue> {
    let mut marker = [0_u8; 1];
    reader.read_exact(&mut marker)?;
    match marker[0] {
        b'+' => Ok(RespValue::Simple(read_resp_line(reader)?)),
        b'-' => Err(anyhow!(
            "Redis command rejected: -{}",
            read_resp_line(reader)?
        )),
        b':' => Ok(RespValue::Integer(read_resp_line(reader)?.parse()?)),
        b'$' => read_resp_bulk(reader),
        b'*' => read_resp_array(reader),
        other => Err(anyhow!("invalid Redis response marker: {}", other as char)),
    }
}

fn read_resp_bulk<R: BufRead>(reader: &mut R) -> anyhow::Result<RespValue> {
    let len: isize = read_resp_line(reader)?.parse()?;
    if len < 0 {
        return Ok(RespValue::Bulk(None));
    }
    let mut bytes = vec![0_u8; len as usize];
    reader.read_exact(&mut bytes)?;
    let mut crlf = [0_u8; 2];
    reader.read_exact(&mut crlf)?;
    if crlf != [b'\r', b'\n'] {
        return Err(anyhow!("invalid Redis bulk string terminator"));
    }
    Ok(RespValue::Bulk(Some(bytes)))
}

fn read_resp_array<R: BufRead>(reader: &mut R) -> anyhow::Result<RespValue> {
    let len: isize = read_resp_line(reader)?.parse()?;
    if len < 0 {
        return Ok(RespValue::Array(Vec::new()));
    }
    let mut values = Vec::with_capacity(len as usize);
    for _ in 0..len {
        values.push(read_resp_value(reader)?);
    }
    Ok(RespValue::Array(values))
}

fn read_resp_line<R: BufRead>(reader: &mut R) -> anyhow::Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

#[cfg(test)]
mod tests {
    use super::{read_resp_value, RespValue};
    use std::io::Cursor;

    #[test]
    fn resp_parser_reads_bulk_nil_and_arrays() {
        let mut bulk = Cursor::new(b"$5\r\nhello\r\n".as_slice());
        assert_eq!(
            read_resp_value(&mut bulk).unwrap(),
            RespValue::Bulk(Some(b"hello".to_vec()))
        );

        let mut nil = Cursor::new(b"$-1\r\n".as_slice());
        assert_eq!(read_resp_value(&mut nil).unwrap(), RespValue::Bulk(None));

        let mut array = Cursor::new(b"*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n".as_slice());
        assert_eq!(
            read_resp_value(&mut array).unwrap(),
            RespValue::Array(vec![
                RespValue::Bulk(Some(b"GET".to_vec())),
                RespValue::Bulk(Some(b"key".to_vec()))
            ])
        );
    }
}
