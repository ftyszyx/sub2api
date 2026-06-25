use crate::auth::OAuthResolvedIdentity;
use crate::response::ApiError;
use repository::{AppRepository, RepositoryError};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackInput {
    pub provider: String,
    pub code: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OAuthProviderConfig {
    provider: String,
    client_id: String,
    client_secret: String,
    token_url: String,
    userinfo_url: String,
}

pub async fn exchange_oauth_callback(
    repository: Arc<dyn AppRepository>,
    input: OAuthCallbackInput,
) -> Result<Option<OAuthResolvedIdentity>, ApiError> {
    let Some(config) = load_oauth_provider_config(repository.as_ref(), &input.provider).await?
    else {
        return Ok(None);
    };
    if input.code.trim().is_empty() {
        return Err(ApiError::bad_request("oauth code is required"));
    }
    let client = reqwest::Client::new();
    let token = request_oauth_token(&client, &config, &input).await?;
    let profile = request_oauth_profile(&client, &config, &token).await?;
    Ok(Some(identity_from_profile(&config.provider, &profile)?))
}

async fn load_oauth_provider_config(
    repository: &dyn AppRepository,
    provider: &str,
) -> Result<Option<OAuthProviderConfig>, ApiError> {
    let provider = normalize_provider(provider)?;
    let settings = match repository.get_system_setting("admin", "settings").await {
        Ok(record) => record.value,
        Err(RepositoryError::NotFound { .. }) => Value::Null,
        Err(error) => return Err(repository_error(error)),
    };
    let config = settings
        .get(format!("{provider}_oauth_config"))
        .or_else(|| settings.get("oauth").and_then(|oauth| oauth.get(&provider)))
        .cloned()
        .unwrap_or(Value::Null);
    let enabled = bool_from_config(&settings, &format!("{provider}_oauth_enabled"))
        .or_else(|| bool_from_config(&config, "enabled"))
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }
    let token_url = string_from_config(&config, "token_url")
        .or_else(|| string_from_config(&settings, &format!("{provider}_oauth_token_url")));
    let userinfo_url = string_from_config(&config, "userinfo_url")
        .or_else(|| string_from_config(&config, "user_info_url"))
        .or_else(|| string_from_config(&settings, &format!("{provider}_oauth_userinfo_url")));
    let Some(token_url) = token_url else {
        return Ok(None);
    };
    let Some(userinfo_url) = userinfo_url else {
        return Ok(None);
    };
    let client_id = string_from_config(&config, "client_id")
        .or_else(|| string_from_config(&settings, &format!("{provider}_oauth_client_id")))
        .ok_or_else(|| ApiError::bad_request("oauth client_id is required"))?;
    let client_secret = string_from_config(&config, "client_secret")
        .or_else(|| string_from_config(&settings, &format!("{provider}_oauth_client_secret")))
        .ok_or_else(|| ApiError::bad_request("oauth client_secret is required"))?;
    Ok(Some(OAuthProviderConfig {
        provider,
        client_id,
        client_secret,
        token_url,
        userinfo_url,
    }))
}

async fn request_oauth_token(
    client: &reqwest::Client,
    config: &OAuthProviderConfig,
    input: &OAuthCallbackInput,
) -> Result<String, ApiError> {
    let response = client
        .post(&config.token_url)
        .header(ACCEPT, "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", input.code.trim()),
            ("redirect_uri", input.redirect_uri.trim()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|error| provider_error(format!("oauth token request failed: {error}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| provider_error(format!("oauth token body failed: {error}")))?;
    if !status.is_success() {
        return Err(provider_error(format!(
            "oauth token endpoint returned {status}: {body}"
        )));
    }
    let payload: Value = serde_json::from_str(&body)
        .map_err(|error| provider_error(format!("oauth token JSON failed: {error}")))?;
    payload
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| provider_error("oauth token response missing access_token"))
}

async fn request_oauth_profile(
    client: &reqwest::Client,
    config: &OAuthProviderConfig,
    access_token: &str,
) -> Result<Value, ApiError> {
    let response = client
        .get(&config.userinfo_url)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|error| provider_error(format!("oauth profile request failed: {error}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| provider_error(format!("oauth profile body failed: {error}")))?;
    if !status.is_success() {
        return Err(provider_error(format!(
            "oauth profile endpoint returned {status}: {body}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|error| provider_error(format!("oauth profile JSON failed: {error}")))
}

fn identity_from_profile(
    provider: &str,
    profile: &Value,
) -> Result<OAuthResolvedIdentity, ApiError> {
    let subject = first_profile_string(profile, &["sub", "id", "openid", "unionid", "user_id"])
        .ok_or_else(|| provider_error("oauth profile missing subject"))?;
    let email = first_profile_string(profile, &["email", "email_address"]);
    Ok(OAuthResolvedIdentity {
        provider_key: provider.to_owned(),
        provider_subject: subject,
        email,
    })
}

fn first_profile_string(profile: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        profile
            .get(*key)
            .and_then(|value| match value {
                Value::String(value) => Some(value.trim().to_owned()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
    })
}

fn normalize_provider(provider: &str) -> Result<String, ApiError> {
    let provider = provider.trim().to_ascii_lowercase();
    match provider.as_str() {
        "linuxdo" | "github" | "google" | "wechat" | "oidc" | "dingtalk" => Ok(provider),
        _ => Err(ApiError::bad_request("unsupported oauth provider")),
    }
}

fn string_from_config(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn bool_from_config(value: &Value, key: &str) -> Option<bool> {
    match value.get(key) {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn repository_error(error: RepositoryError) -> ApiError {
    ApiError::internal_server_error(format!("repository error: {error}"))
}

fn provider_error(message: impl Into<String>) -> ApiError {
    ApiError::internal_server_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use repository::{InMemoryRepository, SystemSettingRecord, SystemSettingRepository};

    #[test]
    fn identity_extracts_standard_subject_and_email() {
        let identity = identity_from_profile(
            "github",
            &serde_json::json!({
                "id": 12345,
                "email": "octo@example.com"
            }),
        )
        .unwrap();

        assert_eq!(identity.provider_subject, "12345");
        assert_eq!(identity.email.as_deref(), Some("octo@example.com"));
    }

    #[tokio::test]
    async fn missing_provider_config_returns_none() {
        let repository = Arc::new(InMemoryRepository::new());
        let result = exchange_oauth_callback(
            repository,
            OAuthCallbackInput {
                provider: "github".to_owned(),
                code: "code".to_owned(),
                redirect_uri: "http://localhost/callback".to_owned(),
            },
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn loads_nested_oauth_config() {
        let repository = InMemoryRepository::new();
        repository
            .upsert_system_setting(SystemSettingRecord {
                namespace: "admin".to_owned(),
                key: "settings".to_owned(),
                value: serde_json::json!({
                    "oauth": {
                        "github": {
                            "enabled": true,
                            "client_id": "id",
                            "client_secret": "secret",
                            "token_url": "http://127.0.0.1/token",
                            "userinfo_url": "http://127.0.0.1/user"
                        }
                    }
                }),
                updated_at: String::new(),
            })
            .await
            .unwrap();

        let config = load_oauth_provider_config(&repository, "github")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(config.client_id, "id");
        assert_eq!(config.token_url, "http://127.0.0.1/token");
    }
}
