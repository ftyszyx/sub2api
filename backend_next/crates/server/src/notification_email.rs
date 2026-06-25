use axum::response::{Html, IntoResponse, Response};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use repository::{AppRepository, RepositoryError, SystemSettingRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::response::ApiError;

const PREFERENCES_NAMESPACE: &str = "notification_email";
const SECRET_KEY: &str = "notification_email_unsubscribe_secret";
const PREFERENCE_KEY_PREFIX: &str = "notification_email_preference:";

const EVENT_SUBSCRIPTION_EXPIRY_REMINDER: &str = "subscription.expiry_reminder";
const EVENT_BALANCE_LOW: &str = "balance.low";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationEmailUnsubscribeResult {
    pub email: String,
    pub event: String,
    pub done: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct UnsubscribeClaims {
    email: String,
    event: String,
    exp: i64,
}

pub async fn unsubscribe(
    repository: &Arc<dyn AppRepository>,
    token: &str,
) -> Result<NotificationEmailUnsubscribeResult, ApiError> {
    let claims = parse_unsubscribe_token(repository, token).await?;
    let event = normalize_event(&claims.event)?;
    if !is_optional_event(event) {
        return Err(ApiError::bad_request(format!(
            "{event} is transactional and cannot be unsubscribed"
        )));
    }

    let key = preference_key(event, &claims.email)?;
    repository
        .upsert_system_setting(SystemSettingRecord {
            namespace: PREFERENCES_NAMESPACE.to_owned(),
            key,
            value: json!("unsubscribed"),
            updated_at: Utc::now().to_rfc3339(),
        })
        .await
        .map_err(repository_api_error)?;

    Ok(NotificationEmailUnsubscribeResult {
        email: claims.email.trim().to_owned(),
        event: event.to_owned(),
        done: true,
    })
}

pub async fn is_unsubscribed(
    repository: &Arc<dyn AppRepository>,
    email: &str,
    event: &str,
) -> Result<bool, ApiError> {
    let event = normalize_supported_event(event)?;
    if !is_optional_event(event) {
        return Ok(false);
    }
    for key in [
        preference_key(event, email)?,
        legacy_preference_key(event, email)?,
    ] {
        match repository
            .get_system_setting(PREFERENCES_NAMESPACE, &key)
            .await
        {
            Ok(record) => {
                return Ok(record
                    .value
                    .as_str()
                    .map(|value| value.eq_ignore_ascii_case("unsubscribed"))
                    .unwrap_or(false));
            }
            Err(RepositoryError::NotFound { .. }) => {}
            Err(error) => return Err(repository_api_error(error)),
        }
    }
    Ok(false)
}

pub async fn create_unsubscribe_token_for_tests(
    repository: &Arc<dyn AppRepository>,
    email: &str,
    event: &str,
    exp: i64,
) -> Result<String, ApiError> {
    create_unsubscribe_token(repository, email, event, exp).await
}

pub fn unsubscribe_html(result: &NotificationEmailUnsubscribeResult) -> Response {
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Unsubscribed</title></head><body style=\"font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;padding:32px;\"><h1>Unsubscribed</h1><p>You have unsubscribed <strong>{}</strong> from <strong>{}</strong> emails.</p></body></html>",
        escape_html(&result.email),
        escape_html(&result.event)
    ))
    .into_response()
}

async fn create_unsubscribe_token(
    repository: &Arc<dyn AppRepository>,
    email: &str,
    event: &str,
    exp: i64,
) -> Result<String, ApiError> {
    let claims = UnsubscribeClaims {
        email: email.trim().to_owned(),
        event: normalize_supported_event(event)?.to_owned(),
        exp,
    };
    if claims.email.is_empty() {
        return Err(ApiError::bad_request("email is required"));
    }
    let payload = serde_json::to_vec(&claims)
        .map_err(|error| ApiError::internal_server_error(error.to_string()))?;
    let encoded_payload = URL_SAFE_NO_PAD.encode(payload);
    let secret = unsubscribe_secret(repository).await?;
    Ok(format!(
        "{}.{}",
        encoded_payload,
        sign_unsubscribe_token(&secret, &encoded_payload)?
    ))
}

async fn parse_unsubscribe_token(
    repository: &Arc<dyn AppRepository>,
    token: &str,
) -> Result<UnsubscribeClaims, ApiError> {
    let mut parts = token.trim().split('.');
    let payload = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid unsubscribe token"))?;
    let signature = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid unsubscribe token"))?;
    if parts.next().is_some() {
        return Err(ApiError::bad_request("invalid unsubscribe token"));
    }

    let secret = unsubscribe_secret(repository).await?;
    let expected = sign_unsubscribe_token(&secret, payload)?;
    if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        return Err(ApiError::bad_request("invalid unsubscribe token signature"));
    }

    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ApiError::bad_request("invalid unsubscribe token payload"))?;
    let claims: UnsubscribeClaims = serde_json::from_slice(&decoded)
        .map_err(|_| ApiError::bad_request("invalid unsubscribe token payload"))?;
    if claims.email.trim().is_empty() || claims.event.trim().is_empty() {
        return Err(ApiError::bad_request("invalid unsubscribe token claims"));
    }
    if claims.exp <= Utc::now().timestamp() {
        return Err(ApiError::bad_request("unsubscribe token expired"));
    }
    Ok(claims)
}

async fn unsubscribe_secret(repository: &Arc<dyn AppRepository>) -> Result<String, ApiError> {
    match repository
        .get_system_setting(PREFERENCES_NAMESPACE, SECRET_KEY)
        .await
    {
        Ok(record) => record
            .value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::internal_server_error("unsubscribe secret is invalid")),
        Err(RepositoryError::NotFound { .. }) => {
            let secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
            repository
                .upsert_system_setting(SystemSettingRecord {
                    namespace: PREFERENCES_NAMESPACE.to_owned(),
                    key: SECRET_KEY.to_owned(),
                    value: json!(secret),
                    updated_at: Utc::now().to_rfc3339(),
                })
                .await
                .map_err(repository_api_error)?;
            Ok(secret)
        }
        Err(error) => Err(repository_api_error(error)),
    }
}

fn sign_unsubscribe_token(secret: &str, payload: &str) -> Result<String, ApiError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|error| ApiError::internal_server_error(error.to_string()))?;
    mac.update(payload.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn normalize_event(event: &str) -> Result<&str, ApiError> {
    let event = normalize_supported_event(event)?;
    if is_optional_event(event) {
        Ok(event)
    } else {
        Err(ApiError::bad_request(format!(
            "{event} is transactional and cannot be unsubscribed"
        )))
    }
}

fn normalize_supported_event(event: &str) -> Result<&str, ApiError> {
    match event.trim().to_ascii_lowercase().as_str() {
        EVENT_SUBSCRIPTION_EXPIRY_REMINDER => Ok(EVENT_SUBSCRIPTION_EXPIRY_REMINDER),
        EVENT_BALANCE_LOW => Ok(EVENT_BALANCE_LOW),
        "auth.verify_code" => Ok("auth.verify_code"),
        "auth.password_reset" => Ok("auth.password_reset"),
        "notification_email.verify_code" => Ok("notification_email.verify_code"),
        "subscription.purchase_success" => Ok("subscription.purchase_success"),
        "balance.recharge_success" => Ok("balance.recharge_success"),
        "account.quota_alert" => Ok("account.quota_alert"),
        "content_moderation.violation_notice" => Ok("content_moderation.violation_notice"),
        "content_moderation.account_disabled" => Ok("content_moderation.account_disabled"),
        "ops.alert" => Ok("ops.alert"),
        "ops.scheduled_report" => Ok("ops.scheduled_report"),
        _ => Err(ApiError::bad_request(format!(
            "unsupported email template event: {event}"
        ))),
    }
}

fn is_optional_event(event: &str) -> bool {
    matches!(
        event,
        EVENT_SUBSCRIPTION_EXPIRY_REMINDER | EVENT_BALANCE_LOW
    )
}

fn preference_key(event: &str, email: &str) -> Result<String, ApiError> {
    if event.trim().is_empty() || email.trim().is_empty() {
        return Err(ApiError::bad_request("email and event are required"));
    }
    Ok(format!(
        "{PREFERENCE_KEY_PREFIX}v2:{}",
        notification_hash(&format!(
            "{}\0{}",
            event.trim(),
            email.trim().to_ascii_lowercase()
        ))
    ))
}

fn legacy_preference_key(event: &str, email: &str) -> Result<String, ApiError> {
    if event.trim().is_empty() || email.trim().is_empty() {
        return Err(ApiError::bad_request("email and event are required"));
    }
    Ok(format!(
        "{PREFERENCE_KEY_PREFIX}{}:{}",
        event.trim(),
        notification_hash(email)
    ))
}

fn notification_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.trim().to_ascii_lowercase().as_bytes());
    hex::encode(hasher.finalize())
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

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn repository_api_error(error: RepositoryError) -> ApiError {
    match error {
        RepositoryError::NotFound { entity, id } => {
            ApiError::not_found(format!("{entity} {id} not found"))
        }
        RepositoryError::InvalidInput(message) => ApiError::bad_request(message),
        RepositoryError::Conflict(message) => ApiError::conflict(message),
        RepositoryError::Duplicate { entity, key } => {
            ApiError::conflict(format!("duplicate {entity}: {key}"))
        }
        RepositoryError::Database(message) => {
            ApiError::internal_server_error(format!("repository error: {message}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[tokio::test]
    async fn unsubscribe_token_roundtrip_stores_preference() {
        let state = AppState::demo();
        let token = create_unsubscribe_token_for_tests(
            &state.repository,
            "User@Example.com",
            EVENT_BALANCE_LOW,
            Utc::now().timestamp() + 3600,
        )
        .await
        .unwrap();

        let result = unsubscribe(&state.repository, &token).await.unwrap();

        assert_eq!(result.email, "User@Example.com");
        assert_eq!(result.event, EVENT_BALANCE_LOW);
        assert!(
            is_unsubscribed(&state.repository, "user@example.com", EVENT_BALANCE_LOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn unsubscribe_rejects_transactional_and_expired_tokens() {
        let state = AppState::demo();
        let transactional = create_unsubscribe_token_for_tests(
            &state.repository,
            "user@example.com",
            "balance.recharge_success",
            Utc::now().timestamp() + 3600,
        )
        .await
        .unwrap();
        let transactional_error = unsubscribe(&state.repository, &transactional)
            .await
            .unwrap_err();
        assert!(transactional_error.message().contains("transactional"));

        let expired = create_unsubscribe_token_for_tests(
            &state.repository,
            "user@example.com",
            EVENT_BALANCE_LOW,
            Utc::now().timestamp() - 1,
        )
        .await
        .unwrap();
        let error = unsubscribe(&state.repository, &expired).await.unwrap_err();
        assert!(error.message().contains("expired"));
    }
}
