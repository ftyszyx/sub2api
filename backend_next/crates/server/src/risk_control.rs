use axum::http::{header::CONTENT_TYPE, HeaderValue};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::response::ApiError;

pub const CATEGORIES: &[&str] = &[
    "harassment",
    "harassment/threatening",
    "hate",
    "hate/threatening",
    "illicit",
    "illicit/violent",
    "self-harm",
    "self-harm/intent",
    "self-harm/instructions",
    "sexual",
    "sexual/minors",
    "violence",
    "violence/graphic",
];

#[derive(Debug, Clone)]
pub struct ModerationTestConfig {
    pub base_url: String,
    pub model: String,
    pub api_keys: Vec<String>,
    pub timeout_ms: u64,
    pub thresholds: HashMap<String, f64>,
}

#[derive(Debug, Clone)]
pub struct ModerationKeyTestResult {
    pub index: usize,
    pub key_hash: String,
    pub masked: String,
    pub status: String,
    pub last_error: String,
    pub last_latency_ms: i64,
    pub last_http_status: i64,
    pub configured: bool,
    pub result: Option<ModerationApiResult>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModerationApiResult {
    #[serde(default)]
    pub flagged: bool,
    #[serde(default)]
    pub category_scores: HashMap<String, f64>,
}

#[derive(Debug, serde::Deserialize)]
struct ModerationApiResponse {
    results: Vec<ModerationApiResult>,
}

impl ModerationTestConfig {
    pub fn from_values(config: &Value, payload: &Value) -> Self {
        let base_url = string_value(payload, "base_url")
            .or_else(|| string_value(config, "base_url"))
            .unwrap_or_else(|| "https://api.openai.com".to_owned());
        let model = string_value(payload, "model")
            .or_else(|| string_value(config, "model"))
            .unwrap_or_else(|| "omni-moderation-latest".to_owned());
        let timeout_ms = payload
            .get("timeout_ms")
            .and_then(Value::as_i64)
            .or_else(|| config.get("timeout_ms").and_then(Value::as_i64))
            .and_then(|value| u64::try_from(value).ok())
            .filter(|value| (1..=30_000).contains(value))
            .unwrap_or(5_000);
        let keys = moderation_api_keys(payload);
        let api_keys = if keys.is_empty() {
            moderation_api_keys(config)
        } else {
            keys
        };
        Self {
            base_url,
            model,
            api_keys,
            timeout_ms,
            thresholds: moderation_thresholds(config.get("thresholds")),
        }
    }
}

pub async fn test_moderation_api_keys(
    client: &reqwest::Client,
    config: &ModerationTestConfig,
    payload: &Value,
) -> Result<Value, ApiError> {
    let input = moderation_test_input(payload)?;
    let image_count = payload
        .get("images")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .count()
        })
        .unwrap_or(0);
    if config.api_keys.is_empty() {
        return Ok(json!({
            "items": [],
            "image_count": image_count
        }));
    }

    let mut items = Vec::with_capacity(config.api_keys.len());
    let mut audit_result = None;
    for (index, api_key) in config.api_keys.iter().enumerate() {
        let result = test_single_key(client, config, index, api_key, &input).await;
        if audit_result.is_none() {
            if let Some(moderation) = result.result.as_ref() {
                audit_result = Some(moderation_audit_result(moderation, &config.thresholds));
            }
        }
        items.push(moderation_key_status_json(result));
    }

    let mut response = json!({
        "items": items,
        "image_count": image_count
    });
    if let Some(audit_result) = audit_result {
        response["audit_result"] = audit_result;
    }
    Ok(response)
}

async fn test_single_key(
    client: &reqwest::Client,
    config: &ModerationTestConfig,
    index: usize,
    api_key: &str,
    input: &Value,
) -> ModerationKeyTestResult {
    let started = Instant::now();
    let mut status_code = 0_i64;
    let result = call_moderation_once(client, config, api_key, input, &mut status_code).await;
    let latency = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
    match result {
        Ok(result) => ModerationKeyTestResult {
            index,
            key_hash: moderation_api_key_hash(api_key),
            masked: mask_secret_tail(api_key),
            status: "ok".to_owned(),
            last_error: String::new(),
            last_latency_ms: latency,
            last_http_status: status_code,
            configured: true,
            result: Some(result),
        },
        Err(error) => ModerationKeyTestResult {
            index,
            key_hash: moderation_api_key_hash(api_key),
            masked: mask_secret_tail(api_key),
            status: "error".to_owned(),
            last_error: error,
            last_latency_ms: latency,
            last_http_status: status_code,
            configured: true,
            result: None,
        },
    }
}

async fn call_moderation_once(
    client: &reqwest::Client,
    config: &ModerationTestConfig,
    api_key: &str,
    input: &Value,
    status_code: &mut i64,
) -> Result<ModerationApiResult, String> {
    let endpoint = format!("{}/v1/moderations", config.base_url.trim_end_matches('/'));
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .timeout(Duration::from_millis(config.timeout_ms))
        .json(&json!({
            "model": config.model,
            "input": input
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    *status_code = response.status().as_u16() as i64;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| String::new())
            .chars()
            .take(512)
            .collect::<String>();
        return Err(format!("moderation api status {status}: {}", body.trim()));
    }
    let parsed = response
        .json::<ModerationApiResponse>()
        .await
        .map_err(|error| error.to_string())?;
    parsed
        .results
        .into_iter()
        .next()
        .ok_or_else(|| "moderation api returned empty results".to_owned())
}

fn moderation_test_input(payload: &Value) -> Result<Value, ApiError> {
    let prompt = payload
        .get("prompt")
        .and_then(Value::as_str)
        .map(normalize_moderation_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "hello".to_owned());
    let images = payload
        .get("images")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if images.len() > 1 {
        return Err(ApiError::bad_request("too many moderation test images"));
    }
    if images.is_empty() {
        return Ok(json!(prompt));
    }
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(json!({ "type": "text", "text": prompt }));
    }
    for image in images {
        if !image.starts_with("data:image/") {
            return Err(ApiError::bad_request(
                "moderation test images must be data:image/* URLs",
            ));
        }
        parts.push(json!({
            "type": "image_url",
            "image_url": { "url": image }
        }));
    }
    Ok(Value::Array(parts))
}

pub fn moderation_audit_result(
    result: &ModerationApiResult,
    thresholds: &HashMap<String, f64>,
) -> Value {
    let (flagged_by_threshold, highest_category, highest_score, category_scores) =
        evaluate_moderation_scores(&result.category_scores, thresholds);
    let flagged = result.flagged || flagged_by_threshold;
    json!({
        "flagged": flagged,
        "highest_category": highest_category,
        "highest_score": highest_score,
        "composite_score": highest_score,
        "category_scores": category_scores,
        "thresholds": thresholds
    })
}

pub fn evaluate_moderation_scores(
    scores: &HashMap<String, f64>,
    thresholds: &HashMap<String, f64>,
) -> (bool, String, f64, Value) {
    let mut flagged = false;
    let mut highest_category = String::new();
    let mut highest_score = 0.0;
    for category in CATEGORIES {
        let score = scores.get(*category).copied().unwrap_or(0.0);
        if score > highest_score || highest_category.is_empty() {
            highest_score = score;
            highest_category = (*category).to_owned();
        }
        if let Some(threshold) = thresholds.get(*category) {
            if score >= *threshold {
                flagged = true;
            }
        }
    }
    for (category, score) in scores {
        if *score > highest_score || highest_category.is_empty() {
            highest_score = *score;
            highest_category = category.clone();
        }
    }
    (
        flagged,
        highest_category,
        highest_score,
        serde_json::to_value(scores).unwrap_or_else(|_| json!({})),
    )
}

pub fn moderation_thresholds(overrides: Option<&Value>) -> HashMap<String, f64> {
    let mut thresholds = [
        ("harassment", 0.98),
        ("harassment/threatening", 0.90),
        ("hate", 0.65),
        ("hate/threatening", 0.65),
        ("illicit", 0.95),
        ("illicit/violent", 0.95),
        ("self-harm", 0.65),
        ("self-harm/intent", 0.85),
        ("self-harm/instructions", 0.65),
        ("sexual", 0.65),
        ("sexual/minors", 0.65),
        ("violence", 0.95),
        ("violence/graphic", 0.95),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect::<HashMap<_, _>>();
    if let Some(object) = overrides.and_then(Value::as_object) {
        for category in CATEGORIES {
            if let Some(value) = object.get(*category).and_then(Value::as_f64) {
                thresholds.insert((*category).to_owned(), value.clamp(0.0, 1.0));
            }
        }
    }
    thresholds
}

fn moderation_key_status_json(result: ModerationKeyTestResult) -> Value {
    json!({
        "index": result.index,
        "key_hash": result.key_hash,
        "masked": result.masked,
        "status": result.status,
        "failure_count": if result.status == "ok" { 0 } else { 1 },
        "success_count": if result.status == "ok" { 1 } else { 0 },
        "last_error": result.last_error,
        "last_checked_at": chrono::Utc::now().to_rfc3339(),
        "frozen_until": Value::Null,
        "last_latency_ms": result.last_latency_ms,
        "last_http_status": result.last_http_status,
        "last_tested": true,
        "configured": result.configured
    })
}

fn moderation_api_keys(value: &Value) -> Vec<String> {
    let mut keys = string_array(value, "api_keys");
    if let Some(api_key) = string_value(value, "api_key") {
        keys.push(api_key);
    }
    let mut out = Vec::new();
    for key in keys {
        if !out.iter().any(|existing: &String| existing == &key) {
            out.push(key);
        }
    }
    out
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn moderation_api_key_hash(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn mask_secret_tail(secret: &str) -> String {
    let secret = secret.trim();
    if secret.is_empty() {
        return String::new();
    }
    let tail = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("***{tail}")
}

fn normalize_moderation_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn evaluates_thresholds_and_highest_score() {
        let scores = HashMap::from([("violence".to_owned(), 0.8), ("hate".to_owned(), 0.2)]);
        let thresholds = HashMap::from([("violence".to_owned(), 0.7)]);
        let (flagged, category, score, values) = evaluate_moderation_scores(&scores, &thresholds);
        assert!(flagged);
        assert_eq!(category, "violence");
        assert_eq!(score, 0.8);
        assert_eq!(values["violence"], 0.8);
    }

    #[test]
    fn builds_text_and_image_test_input() {
        let input = moderation_test_input(&json!({
            "prompt": " hello   world ",
            "images": ["data:image/png;base64,aGVsbG8="]
        }))
        .unwrap();
        assert_eq!(input[0]["text"], "hello world");
        assert_eq!(
            input[1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }
}
