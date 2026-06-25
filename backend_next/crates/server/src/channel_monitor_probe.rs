use crate::response::ApiError;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const DEGRADED_THRESHOLD: Duration = Duration::from_secs(6);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const ERROR_BODY_SNIPPET_BYTES: usize = 300;
const MESSAGE_MAX_BYTES: usize = 500;
const CHALLENGE_PROMPT: &str = "Calculate and respond with ONLY the number, nothing else.\n\nQ: 3 + 5 = ?\nA: 8\n\nQ: 12 - 7 = ?\nA: 5\n\nQ: 19 + 23 = ?\nA:";
const CHALLENGE_EXPECTED: &str = "42";

#[derive(Debug, Clone)]
pub struct ChannelMonitorConfig {
    pub id: i64,
    pub provider: String,
    pub api_mode: String,
    pub endpoint: String,
    pub api_key: String,
    pub primary_model: String,
    pub extra_models: Vec<String>,
    pub extra_headers: Value,
    pub body_override_mode: String,
    pub body_override: Value,
}

#[derive(Debug, Clone)]
pub struct ChannelMonitorResult {
    pub model: String,
    pub status: &'static str,
    pub latency_ms: Option<i64>,
    pub ping_latency_ms: Option<i64>,
    pub message: String,
}

pub async fn run_monitor(
    config: &ChannelMonitorConfig,
) -> Result<Vec<ChannelMonitorResult>, ApiError> {
    validate_config(config)?;
    let mut models = Vec::with_capacity(config.extra_models.len() + 1);
    models.push(config.primary_model.trim().to_owned());
    models.extend(
        config
            .extra_models
            .iter()
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty()),
    );

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            ApiError::internal_server_error(format!("channel monitor client setup failed: {error}"))
        })?;
    let mut results = Vec::with_capacity(models.len());
    for model in models {
        results.push(run_model_check(&client, config, &model).await);
    }
    Ok(results)
}

fn validate_config(config: &ChannelMonitorConfig) -> Result<(), ApiError> {
    if config.endpoint.trim().is_empty() {
        return Err(ApiError::bad_request(
            "channel monitor endpoint is required",
        ));
    }
    if config.api_key.trim().is_empty() {
        return Err(ApiError::bad_request("channel monitor api_key is required"));
    }
    if config.primary_model.trim().is_empty() {
        return Err(ApiError::bad_request(
            "channel monitor primary_model is required",
        ));
    }
    adapter_for(&config.provider, &config.api_mode)?;
    Ok(())
}

async fn run_model_check(
    client: &reqwest::Client,
    config: &ChannelMonitorConfig,
    model: &str,
) -> ChannelMonitorResult {
    let started = Instant::now();
    let outcome = call_provider(client, config, model).await;
    let elapsed = started.elapsed();
    let latency_ms = elapsed.as_millis().min(i64::MAX as u128) as i64;

    let (text, raw_body, status_code) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            return ChannelMonitorResult {
                model: model.to_owned(),
                status: "error",
                latency_ms: Some(latency_ms),
                ping_latency_ms: None,
                message: truncate_message(&sanitize_error_message(&error)),
            };
        }
    };

    if !(200..300).contains(&status_code) {
        return ChannelMonitorResult {
            model: model.to_owned(),
            status: "error",
            latency_ms: Some(latency_ms),
            ping_latency_ms: None,
            message: truncate_message(&format!(
                "upstream HTTP {status_code}: {}",
                truncate_error_body(&raw_body)
            )),
        };
    }

    if body_override_mode(config) != "replace" && !validate_challenge(&text, CHALLENGE_EXPECTED) {
        return ChannelMonitorResult {
            model: model.to_owned(),
            status: "failed",
            latency_ms: Some(latency_ms),
            ping_latency_ms: None,
            message: truncate_message(&format!(
                "challenge mismatch (expected {CHALLENGE_EXPECTED}, got {:?})",
                text
            )),
        };
    }
    if body_override_mode(config) == "replace" && text.trim().is_empty() {
        return ChannelMonitorResult {
            model: model.to_owned(),
            status: "failed",
            latency_ms: Some(latency_ms),
            ping_latency_ms: None,
            message: "replace-mode: upstream returned 2xx with empty text".to_owned(),
        };
    }

    if elapsed >= DEGRADED_THRESHOLD {
        ChannelMonitorResult {
            model: model.to_owned(),
            status: "degraded",
            latency_ms: Some(latency_ms),
            ping_latency_ms: None,
            message: truncate_message(&format!("slow response: {latency_ms}ms")),
        }
    } else {
        ChannelMonitorResult {
            model: model.to_owned(),
            status: "operational",
            latency_ms: Some(latency_ms),
            ping_latency_ms: None,
            message: String::new(),
        }
    }
}

async fn call_provider(
    client: &reqwest::Client,
    config: &ChannelMonitorConfig,
    model: &str,
) -> Result<(String, String, u16), String> {
    let adapter = adapter_for(&config.provider, &config.api_mode)
        .map_err(|error| error.message().to_owned())?;
    let request_body = build_request_body(config, adapter, model)?;
    let url = join_url(&config.endpoint, adapter.path(model));
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json");
    for (name, value) in adapter.headers(&config.api_key) {
        request = request.header(name, value);
    }
    request = apply_extra_headers(request, &config.extra_headers);

    let response = request
        .json(&request_body)
        .send()
        .await
        .map_err(|error| format!("do request: {error}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read body: {error}"))?;
    let limited = if bytes.len() > MAX_RESPONSE_BYTES {
        &bytes[..MAX_RESPONSE_BYTES]
    } else {
        bytes.as_ref()
    };
    let raw_body = String::from_utf8_lossy(limited).to_string();
    let text = extract_text(adapter, &raw_body);
    Ok((text, raw_body, status))
}

#[derive(Debug, Clone, Copy)]
struct ProviderAdapter {
    kind: ProviderKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    OpenAIChat,
    OpenAIResponses,
    Anthropic,
    Gemini,
}

impl ProviderAdapter {
    fn path(self, model: &str) -> String {
        match self.kind {
            ProviderKind::OpenAIChat => "/v1/chat/completions".to_owned(),
            ProviderKind::OpenAIResponses => "/v1/responses".to_owned(),
            ProviderKind::Anthropic => "/v1/messages".to_owned(),
            ProviderKind::Gemini => format!("/v1beta/models/{model}:generateContent"),
        }
    }

    fn headers(self, api_key: &str) -> Vec<(&'static str, String)> {
        match self.kind {
            ProviderKind::OpenAIChat | ProviderKind::OpenAIResponses => {
                vec![("Authorization", format!("Bearer {}", api_key.trim()))]
            }
            ProviderKind::Anthropic => vec![
                ("x-api-key", api_key.trim().to_owned()),
                ("anthropic-version", "2023-06-01".to_owned()),
            ],
            ProviderKind::Gemini => vec![("x-goog-api-key", api_key.trim().to_owned())],
        }
    }
}

fn adapter_for(provider: &str, api_mode: &str) -> Result<ProviderAdapter, ApiError> {
    let provider = provider.trim().to_ascii_lowercase();
    let api_mode = if api_mode.trim().is_empty() {
        "chat_completions"
    } else {
        api_mode.trim()
    };
    let kind = match (provider.as_str(), api_mode) {
        ("openai", "responses") => ProviderKind::OpenAIResponses,
        ("openai", "chat_completions") => ProviderKind::OpenAIChat,
        ("anthropic", _) => ProviderKind::Anthropic,
        ("gemini", _) | ("google", _) => ProviderKind::Gemini,
        _ => {
            return Err(ApiError::bad_request(
                "channel monitor provider must be openai, anthropic, or gemini",
            ))
        }
    };
    Ok(ProviderAdapter { kind })
}

fn build_request_body(
    config: &ChannelMonitorConfig,
    adapter: ProviderAdapter,
    model: &str,
) -> Result<Value, String> {
    if body_override_mode(config) == "replace" {
        if !config.body_override.is_object()
            || config
                .body_override
                .as_object()
                .is_some_and(|body| body.is_empty())
        {
            return Err("replace mode: body_override is empty".to_owned());
        }
        return Ok(config.body_override.clone());
    }

    let mut body = match adapter.kind {
        ProviderKind::OpenAIChat => json!({
            "model": model,
            "messages": [{ "role": "user", "content": CHALLENGE_PROMPT }],
            "max_tokens": 50,
            "stream": false
        }),
        ProviderKind::OpenAIResponses => json!({
            "model": model,
            "instructions": "You are a channel health-check endpoint. Answer the arithmetic challenge exactly and briefly.",
            "input": CHALLENGE_PROMPT,
            "max_output_tokens": 50,
            "stream": false
        }),
        ProviderKind::Anthropic => json!({
            "model": model,
            "messages": [{ "role": "user", "content": CHALLENGE_PROMPT }],
            "max_tokens": 50
        }),
        ProviderKind::Gemini => json!({
            "contents": [{
                "parts": [{ "text": CHALLENGE_PROMPT }]
            }],
            "generationConfig": { "maxOutputTokens": 50 }
        }),
    };

    if body_override_mode(config) == "merge" {
        merge_body_override(&mut body, &config.body_override, adapter);
    }
    Ok(body)
}

fn merge_body_override(body: &mut Value, override_body: &Value, adapter: ProviderAdapter) {
    let Some(target) = body.as_object_mut() else {
        return;
    };
    let Some(source) = override_body.as_object() else {
        return;
    };
    for (key, value) in source {
        if body_override_deny_key(adapter, key) {
            continue;
        }
        target.insert(key.clone(), value.clone());
    }
}

fn body_override_deny_key(adapter: ProviderAdapter, key: &str) -> bool {
    match adapter.kind {
        ProviderKind::OpenAIChat => matches!(key, "model" | "messages" | "stream"),
        ProviderKind::OpenAIResponses => {
            matches!(key, "model" | "instructions" | "input" | "stream")
        }
        ProviderKind::Anthropic => matches!(key, "model" | "messages"),
        ProviderKind::Gemini => key == "contents",
    }
}

fn apply_extra_headers(
    mut request: reqwest::RequestBuilder,
    headers: &Value,
) -> reqwest::RequestBuilder {
    let Some(headers) = headers.as_object() else {
        return request;
    };
    for (name, value) in headers {
        if is_forbidden_header(name) {
            continue;
        }
        if let Some(value) = value.as_str() {
            request = request.header(name, value);
        }
    }
    request
}

fn is_forbidden_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "connection"
            | "transfer-encoding"
            | "upgrade"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
    )
}

fn extract_text(adapter: ProviderAdapter, body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    match adapter.kind {
        ProviderKind::OpenAIChat => value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        ProviderKind::OpenAIResponses => extract_openai_responses_text(&value),
        ProviderKind::Anthropic => value
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        ProviderKind::Gemini => value
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    }
}

fn extract_openai_responses_text(value: &Value) -> String {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.to_owned();
        }
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .map(|kind| kind == "message")
                .unwrap_or(true)
        })
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|block| {
            let block_type = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("output_text");
            if block_type == "output_text" {
                block.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn validate_challenge(text: &str, expected: &str) -> bool {
    text.split(|character: char| !character.is_ascii_digit() && character != '-')
        .any(|part| part == expected)
}

fn body_override_mode(config: &ChannelMonitorConfig) -> &str {
    let mode = config.body_override_mode.trim();
    if mode.is_empty() {
        "off"
    } else {
        mode
    }
}

fn join_url(base: &str, path: String) -> String {
    let base = base.trim().trim_end_matches('/');
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn sanitize_error_message(message: &str) -> String {
    message
        .replace("Authorization", "authorization")
        .replace("Bearer ", "Bearer ***")
}

fn truncate_error_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_bytes(&compact, ERROR_BODY_SNIPPET_BYTES, "...(body truncated)")
}

fn truncate_message(message: &str) -> String {
    truncate_bytes(message, MESSAGE_MAX_BYTES, "...(truncated)")
}

fn truncate_bytes(message: &str, max_bytes: usize, suffix: &str) -> String {
    if message.len() <= max_bytes {
        return message.to_owned();
    }
    let cutoff = max_bytes.saturating_sub(suffix.len());
    format!("{}{}", &message[..cutoff], suffix)
}

#[cfg(test)]
mod tests {
    use super::{
        extract_openai_responses_text, extract_text, validate_challenge, ProviderAdapter,
        ProviderKind,
    };
    use serde_json::json;

    #[test]
    fn challenge_validation_matches_integer_answer_only() {
        assert!(validate_challenge("The answer is 42.", "42"));
        assert!(validate_challenge("42", "42"));
        assert!(!validate_challenge("forty two", "42"));
        assert!(!validate_challenge("142", "42"));
    }

    #[test]
    fn openai_responses_text_extraction_supports_output_text_and_content_blocks() {
        assert_eq!(
            extract_openai_responses_text(&json!({ "output_text": "42" })),
            "42"
        );
        assert_eq!(
            extract_openai_responses_text(&json!({
                "output": [
                    { "type": "reasoning", "content": [] },
                    {
                        "type": "message",
                        "content": [
                            { "type": "output_text", "text": "4" },
                            { "type": "output_text", "text": "2" }
                        ]
                    }
                ]
            })),
            "42"
        );
    }

    #[test]
    fn provider_text_extractors_follow_expected_response_shapes() {
        assert_eq!(
            extract_text(
                ProviderAdapter {
                    kind: ProviderKind::OpenAIChat
                },
                r#"{"choices":[{"message":{"content":"42"}}]}"#
            ),
            "42"
        );
        assert_eq!(
            extract_text(
                ProviderAdapter {
                    kind: ProviderKind::Anthropic
                },
                r#"{"content":[{"type":"text","text":"42"}]}"#
            ),
            "42"
        );
        assert_eq!(
            extract_text(
                ProviderAdapter {
                    kind: ProviderKind::Gemini
                },
                r#"{"candidates":[{"content":{"parts":[{"text":"42"}]}}]}"#
            ),
            "42"
        );
    }
}
