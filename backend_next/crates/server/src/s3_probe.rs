use crate::response::ApiError;
use chrono::Utc;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST},
    Client, Method, Url,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::time::Duration;

const EMPTY_SHA256_HEX: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const S3_SERVICE: &str = "s3";
const S3_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct S3ProbeConfig {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
    pub use_ssl: bool,
}

#[derive(Debug, Clone)]
pub struct S3PutObjectInput {
    pub key: String,
    pub body: Vec<u8>,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct S3GetObjectOutput {
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl S3ProbeConfig {
    pub fn from_payload(payload: &Value) -> Self {
        Self {
            endpoint: string_field(payload, "endpoint"),
            region: string_field(payload, "region"),
            bucket: string_field(payload, "bucket"),
            access_key_id: string_field(payload, "access_key_id"),
            secret_access_key: string_field(payload, "secret_access_key"),
            force_path_style: payload
                .get("force_path_style")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            use_ssl: payload
                .get("use_ssl")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }

    pub fn merge_missing_secret(mut self, fallback: &Value) -> Self {
        if self.secret_access_key.trim().is_empty() {
            self.secret_access_key = string_field(fallback, "secret_access_key");
        }
        self
    }

    fn normalized_region(&self) -> &str {
        let region = self.region.trim();
        if region.is_empty() {
            "auto"
        } else {
            region
        }
    }
}

pub async fn probe_head_bucket(config: &S3ProbeConfig) -> Value {
    match head_bucket(config).await {
        Ok(()) => json!({
            "ok": true,
            "message": "connection successful"
        }),
        Err(error) => json!({
            "ok": false,
            "message": error.message()
        }),
    }
}

pub async fn put_object(config: &S3ProbeConfig, input: S3PutObjectInput) -> Result<u64, ApiError> {
    validate_config(config)?;
    let key = input.key.trim();
    if key.is_empty() {
        return Err(ApiError::bad_request("S3 object key is required"));
    }
    let url = object_url(config, key)?;
    let size = input.body.len() as u64;
    let headers = signed_put_headers(config, &url, &input.body, &input.content_type)?;
    let response = Client::builder()
        .timeout(S3_PROBE_TIMEOUT)
        .build()
        .map_err(|error| ApiError::bad_request(format!("S3 client creation failed: {error}")))?
        .request(Method::PUT, url)
        .headers(headers)
        .body(input.body)
        .send()
        .await
        .map_err(|error| ApiError::bad_request(format!("S3 PutObject request failed: {error}")))?;
    if response.status().is_success() {
        Ok(size)
    } else {
        Err(ApiError::bad_request(format!(
            "S3 PutObject failed: HTTP {}",
            response.status()
        )))
    }
}

pub async fn get_object(config: &S3ProbeConfig, key: &str) -> Result<S3GetObjectOutput, ApiError> {
    validate_config(config)?;
    let key = key.trim();
    if key.is_empty() {
        return Err(ApiError::bad_request("S3 object key is required"));
    }
    let url = object_url(config, key)?;
    let headers = signed_get_headers(config, &url)?;
    let response = Client::builder()
        .timeout(S3_PROBE_TIMEOUT)
        .build()
        .map_err(|error| ApiError::bad_request(format!("S3 client creation failed: {error}")))?
        .request(Method::GET, url)
        .headers(headers)
        .send()
        .await
        .map_err(|error| ApiError::bad_request(format!("S3 GetObject request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "S3 GetObject failed: HTTP {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = response
        .bytes()
        .await
        .map_err(|error| ApiError::bad_request(format!("S3 GetObject body read failed: {error}")))?
        .to_vec();
    Ok(S3GetObjectOutput { body, content_type })
}

pub async fn delete_object(config: &S3ProbeConfig, key: &str) -> Result<(), ApiError> {
    validate_config(config)?;
    let key = key.trim();
    if key.is_empty() {
        return Err(ApiError::bad_request("S3 object key is required"));
    }
    let url = object_url(config, key)?;
    let headers = signed_delete_headers(config, &url)?;
    let response = Client::builder()
        .timeout(S3_PROBE_TIMEOUT)
        .build()
        .map_err(|error| ApiError::bad_request(format!("S3 client creation failed: {error}")))?
        .request(Method::DELETE, url)
        .headers(headers)
        .send()
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("S3 DeleteObject request failed: {error}"))
        })?;
    if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "S3 DeleteObject failed: HTTP {}",
            response.status()
        )))
    }
}

async fn head_bucket(config: &S3ProbeConfig) -> Result<(), ApiError> {
    validate_config(config)?;
    let url = bucket_url(config)?;
    let signed_headers = signed_head_headers(config, &url)?;
    let response = Client::builder()
        .timeout(S3_PROBE_TIMEOUT)
        .build()
        .map_err(|error| ApiError::bad_request(format!("S3 client creation failed: {error}")))?
        .request(Method::HEAD, url)
        .headers(signed_headers)
        .send()
        .await
        .map_err(|error| ApiError::bad_request(format!("S3 HeadBucket request failed: {error}")))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "S3 HeadBucket failed: HTTP {}",
            response.status()
        )))
    }
}

fn validate_config(config: &S3ProbeConfig) -> Result<(), ApiError> {
    if config.bucket.trim().is_empty()
        || config.access_key_id.trim().is_empty()
        || config.secret_access_key.trim().is_empty()
    {
        return Err(ApiError::bad_request(
            "incomplete S3 config: bucket, access_key_id, secret_access_key are required",
        ));
    }
    Ok(())
}

fn bucket_url(config: &S3ProbeConfig) -> Result<Url, ApiError> {
    let endpoint = config.endpoint.trim();
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("S3 endpoint is required"));
    }
    let endpoint = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        let scheme = if config.use_ssl { "https" } else { "http" };
        format!("{scheme}://{endpoint}")
    };
    let mut url = Url::parse(&endpoint)
        .map_err(|error| ApiError::bad_request(format!("invalid S3 endpoint: {error}")))?;
    let bucket = percent_encode_path(config.bucket.trim());
    if config.force_path_style {
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}/{bucket}"));
    } else {
        let host = url
            .host_str()
            .ok_or_else(|| ApiError::bad_request("S3 endpoint host is required"))?;
        url.set_host(Some(&format!("{}.{}", config.bucket.trim(), host)))
            .map_err(|_| ApiError::bad_request("invalid virtual-hosted S3 bucket host"))?;
    }
    url.set_query(None);
    Ok(url)
}

fn object_url(config: &S3ProbeConfig, key: &str) -> Result<Url, ApiError> {
    let mut url = bucket_url(config)?;
    let object_path = key
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(percent_encode_path)
        .collect::<Vec<_>>()
        .join("/");
    let base_path = url.path().trim_end_matches('/');
    url.set_path(&format!("{base_path}/{object_path}"));
    Ok(url)
}

fn signed_head_headers(config: &S3ProbeConfig, url: &Url) -> Result<HeaderMap, ApiError> {
    signed_empty_payload_headers(config, url, Method::HEAD)
}

fn signed_get_headers(config: &S3ProbeConfig, url: &Url) -> Result<HeaderMap, ApiError> {
    signed_empty_payload_headers(config, url, Method::GET)
}

fn signed_delete_headers(config: &S3ProbeConfig, url: &Url) -> Result<HeaderMap, ApiError> {
    signed_empty_payload_headers(config, url, Method::DELETE)
}

fn signed_empty_payload_headers(
    config: &S3ProbeConfig,
    url: &Url,
    method: Method,
) -> Result<HeaderMap, ApiError> {
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_scope = now.format("%Y%m%d").to_string();
    let region = config.normalized_region();
    let credential_scope = format!("{date_scope}/{region}/{S3_SERVICE}/aws4_request");
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::bad_request("S3 endpoint host is required"))?;
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let canonical_uri = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let canonical_headers = format!(
        "host:{host_header}\nx-amz-content-sha256:{EMPTY_SHA256_HEX}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{EMPTY_SHA256_HEX}",
        method.as_str()
    );
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");
    let signing_key = sigv4_signing_key(config.secret_access_key.trim(), &date_scope, region);
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        config.access_key_id.trim()
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_str(&host_header)
            .map_err(|_| ApiError::bad_request("invalid S3 host header"))?,
    );
    headers.insert(
        "x-amz-content-sha256",
        HeaderValue::from_static(EMPTY_SHA256_HEX),
    );
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&amz_date)
            .map_err(|_| ApiError::bad_request("invalid S3 x-amz-date header"))?,
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&authorization)
            .map_err(|_| ApiError::bad_request("invalid S3 authorization header"))?,
    );
    Ok(headers)
}

fn signed_put_headers(
    config: &S3ProbeConfig,
    url: &Url,
    body: &[u8],
    content_type: &str,
) -> Result<HeaderMap, ApiError> {
    let content_type = if content_type.trim().is_empty() {
        "application/octet-stream"
    } else {
        content_type.trim()
    };
    let payload_hash = sha256_hex(body);
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_scope = now.format("%Y%m%d").to_string();
    let region = config.normalized_region();
    let credential_scope = format!("{date_scope}/{region}/{S3_SERVICE}/aws4_request");
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::bad_request("S3 endpoint host is required"))?;
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let canonical_uri = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let content_length = body.len().to_string();
    let canonical_headers = format!(
        "content-length:{content_length}\ncontent-type:{content_type}\nhost:{host_header}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "content-length;content-type;host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("PUT\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");
    let signing_key = sigv4_signing_key(config.secret_access_key.trim(), &date_scope, region);
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        config.access_key_id.trim()
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_str(&host_header)
            .map_err(|_| ApiError::bad_request("invalid S3 host header"))?,
    );
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&content_length)
            .map_err(|_| ApiError::bad_request("invalid S3 content-length header"))?,
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .map_err(|_| ApiError::bad_request("invalid S3 content-type header"))?,
    );
    headers.insert(
        "x-amz-content-sha256",
        HeaderValue::from_str(&payload_hash)
            .map_err(|_| ApiError::bad_request("invalid S3 payload hash header"))?,
    );
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&amz_date)
            .map_err(|_| ApiError::bad_request("invalid S3 x-amz-date header"))?,
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&authorization)
            .map_err(|_| ApiError::bad_request("invalid S3 authorization header"))?,
    );
    Ok(headers)
}

fn sigv4_signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> {
    let date_key = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, S3_SERVICE.as_bytes());
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    hex_lower(&hmac_sha256(key, message))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut key_block = [0_u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().to_vec()
}

fn sha256_hex(input: &[u8]) -> String {
    hex_lower(&Sha256::digest(input))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn percent_encode_path(input: &str) -> String {
    let mut output = String::new();
    for byte in input.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if keep {
            output.push(byte as char);
        } else {
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn string_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{bucket_url, hmac_sha256_hex, object_url, S3ProbeConfig};
    use serde_json::json;

    #[test]
    fn hmac_sha256_hex_matches_known_vector() {
        assert_eq!(
            hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn bucket_url_supports_path_style_and_virtual_hosted_style() {
        let mut config = S3ProbeConfig::from_payload(&json!({
            "endpoint": "http://127.0.0.1:9000/base/",
            "region": "auto",
            "bucket": "my-bucket",
            "access_key_id": "ak",
            "secret_access_key": "sk",
            "force_path_style": true
        }));
        assert_eq!(
            bucket_url(&config).unwrap().as_str(),
            "http://127.0.0.1:9000/base/my-bucket"
        );

        config.force_path_style = false;
        config.endpoint = "https://s3.example.com".to_owned();
        assert_eq!(
            bucket_url(&config).unwrap().as_str(),
            "https://my-bucket.s3.example.com/"
        );
    }

    #[test]
    fn object_url_percent_encodes_key_segments() {
        let config = S3ProbeConfig::from_payload(&json!({
            "endpoint": "http://127.0.0.1:9000/base/",
            "region": "auto",
            "bucket": "my-bucket",
            "access_key_id": "ak",
            "secret_access_key": "sk",
            "force_path_style": true
        }));

        assert_eq!(
            object_url(&config, "daily backup/sub2api.sql.gz")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:9000/base/my-bucket/daily%20backup/sub2api.sql.gz"
        );
    }
}
