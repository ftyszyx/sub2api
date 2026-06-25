use crate::response::ApiError;
use crate::smtp_probe::{self, SmtpConfig};
use chrono::Utc;
use repository::{AppRepository, RepositoryError};
use serde_json::{json, Map, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationEmailKind {
    Auth,
    NotificationEmail,
}

impl VerificationEmailKind {
    fn subject(self, site_name: &str) -> String {
        match self {
            Self::Auth => format!("[{site_name}] Email Verification Code"),
            Self::NotificationEmail => {
                format!("[{site_name}] Notification Email Verification")
            }
        }
    }

    fn purpose(self) -> &'static str {
        match self {
            Self::Auth => "You are verifying your email address.",
            Self::NotificationEmail => "You are adding an extra notification email address.",
        }
    }
}

#[derive(Clone)]
pub struct EmailDeliveryService {
    repository: Arc<dyn AppRepository>,
}

impl EmailDeliveryService {
    pub fn new(repository: Arc<dyn AppRepository>) -> Self {
        Self { repository }
    }

    pub async fn send_verification_code(
        &self,
        to: &str,
        code: &str,
        kind: VerificationEmailKind,
    ) -> Result<(), ApiError> {
        let settings = admin_settings(&self.repository).await?;
        let config = smtp_config_from_settings(&settings)?;
        let site_name = site_name_from_settings(&settings);
        let variables = template_variables([
            ("site_name", site_name.as_str()),
            ("code", code),
            ("verification_code", code),
            ("expires_in_minutes", "15"),
            ("recipient_email", to.trim()),
            ("username", to.trim()),
        ]);
        let (subject, body) = self
            .render_email_template(
                kind.template_event(),
                locale_from_settings(&settings),
                &variables,
            )
            .await?
            .unwrap_or_else(|| {
                (
                    kind.subject(&site_name),
                    verification_body(kind, &site_name, code),
                )
            });
        send_email(config, to, subject, body).await
    }

    pub async fn ensure_password_reset_config(&self) -> Result<(), ApiError> {
        let settings = admin_settings(&self.repository).await?;
        ensure_password_reset_enabled(&settings)?;
        let _ = frontend_url_from_settings(&settings)?;
        Ok(())
    }

    pub async fn send_password_reset_link(&self, to: &str, token: &str) -> Result<(), ApiError> {
        let settings = admin_settings(&self.repository).await?;
        ensure_password_reset_enabled(&settings)?;
        let config = smtp_config_from_settings(&settings)?;
        let frontend_url = frontend_url_from_settings(&settings)?;
        let site_name = site_name_from_settings(&settings);
        let reset_url = format!(
            "{}/reset-password?email={}&token={}",
            frontend_url.trim_end_matches('/'),
            percent_encode_query(to.trim()),
            percent_encode_query(token.trim())
        );
        let variables = template_variables([
            ("site_name", site_name.as_str()),
            ("reset_url", reset_url.as_str()),
            ("recipient_email", to.trim()),
            ("username", to.trim()),
            ("expires_in_minutes", "30"),
        ]);
        let (subject, body) = self
            .render_email_template(
                "reset_password",
                locale_from_settings(&settings),
                &variables,
            )
            .await?
            .unwrap_or_else(|| {
                (
                    format!("[{site_name}] Reset your password"),
                    password_reset_body(&site_name, &reset_url),
                )
            });
        send_email(config, to, subject, body).await
    }

    async fn render_email_template(
        &self,
        event: &str,
        locale: &str,
        variables: &Map<String, Value>,
    ) -> Result<Option<(String, String)>, ApiError> {
        let Some(template) = load_email_template(&self.repository, event, locale).await? else {
            return Ok(None);
        };
        let subject = template
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let html = template
            .get("html")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if subject.trim().is_empty() || html.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some((
            render_template_text(subject, variables),
            render_template_text(html, variables),
        )))
    }
}

async fn admin_settings(repository: &Arc<dyn AppRepository>) -> Result<Value, ApiError> {
    match repository.get_system_setting("admin", "settings").await {
        Ok(record) => Ok(record.value),
        Err(RepositoryError::NotFound { .. }) => Ok(serde_json::json!({})),
        Err(error) => Err(ApiError::internal_server_error(format!(
            "failed to load email settings: {error}"
        ))),
    }
}

fn smtp_config_from_settings(settings: &Value) -> Result<SmtpConfig, ApiError> {
    let host = string_setting(settings, "smtp_host");
    if host.is_empty() {
        return Err(ApiError::bad_request(
            "EMAIL_NOT_CONFIGURED: SMTP host is required",
        ));
    }
    let port = settings
        .get("smtp_port")
        .and_then(Value::as_i64)
        .unwrap_or(587);
    if !(1..=65535).contains(&port) {
        return Err(ApiError::bad_request(
            "SMTP port must be between 1 and 65535",
        ));
    }
    let mut from = string_setting(settings, "smtp_from");
    if from.is_empty() {
        from = string_setting(settings, "smtp_from_email");
    }
    if from.is_empty() {
        from = string_setting(settings, "smtp_username");
    }
    Ok(SmtpConfig {
        host,
        port: port as u16,
        username: string_setting(settings, "smtp_username"),
        password: string_setting(settings, "smtp_password"),
        from,
        from_name: string_setting(settings, "smtp_from_name"),
        use_tls: settings
            .get("smtp_use_tls")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn frontend_url_from_settings(settings: &Value) -> Result<String, ApiError> {
    let frontend_url = string_setting(settings, "frontend_url");
    if frontend_url.is_empty() {
        return Err(ApiError::internal_server_error(
            "Password reset is not configured",
        ));
    }
    if !is_absolute_http_url(&frontend_url) {
        return Err(ApiError::internal_server_error(
            "Password reset frontend URL is invalid",
        ));
    }
    Ok(frontend_url.trim_end_matches('/').to_owned())
}

fn ensure_password_reset_enabled(settings: &Value) -> Result<(), ApiError> {
    if !bool_setting(settings, "password_reset_enabled", true)
        || !email_feature_enabled_for_password_reset(settings)
    {
        return Err(ApiError::forbidden(
            "PASSWORD_RESET_DISABLED: password reset is not enabled",
        ));
    }
    Ok(())
}

fn email_feature_enabled_for_password_reset(settings: &Value) -> bool {
    if settings.get("email_verify_enabled").is_some() {
        bool_setting(settings, "email_verify_enabled", false)
    } else {
        bool_setting(settings, "email_enabled", false)
    }
}

fn site_name_from_settings(settings: &Value) -> String {
    settings
        .get("site_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Sub2API")
        .to_owned()
}

fn locale_from_settings(settings: &Value) -> &str {
    settings
        .get("email_template_locale")
        .or_else(|| settings.get("locale"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("zh-CN")
}

fn bool_setting(settings: &Value, key: &str, default: bool) -> bool {
    match settings.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        _ => default,
    }
}

impl VerificationEmailKind {
    fn template_event(self) -> &'static str {
        match self {
            Self::Auth => "verify_email",
            Self::NotificationEmail => "notification_email.verify_code",
        }
    }
}

async fn load_email_template(
    repository: &Arc<dyn AppRepository>,
    event: &str,
    locale: &str,
) -> Result<Option<Value>, ApiError> {
    for key in email_template_candidate_keys(event, locale) {
        match repository.get_system_setting("email_templates", &key).await {
            Ok(record) => return Ok(Some(record.value)),
            Err(RepositoryError::NotFound { .. }) => {}
            Err(error) => {
                return Err(ApiError::internal_server_error(format!(
                    "failed to load email template: {error}"
                )))
            }
        }
    }
    Ok(default_email_template(event, locale))
}

fn email_template_candidate_keys(event: &str, locale: &str) -> Vec<String> {
    let normalized_event = canonical_template_event(event);
    let normalized_locale = if locale.trim().is_empty() {
        "zh-CN"
    } else {
        locale.trim()
    };
    let mut keys = vec![format!("{normalized_event}.{normalized_locale}")];
    if normalized_event != event.trim() {
        keys.push(format!("{}.{normalized_locale}", event.trim()));
    }
    if normalized_locale != "zh-CN" {
        keys.push(format!("{normalized_event}.zh-CN"));
    }
    keys.sort();
    keys.dedup();
    keys
}

fn canonical_template_event(event: &str) -> &str {
    match event.trim() {
        "auth.verify_code" | "verify_email" => "verify_email",
        "auth.password_reset" | "reset_password" => "reset_password",
        "notification_email.verify_code" | "notify_email_verify" => {
            "notification_email.verify_code"
        }
        other => other,
    }
}

fn default_email_template(event: &str, locale: &str) -> Option<Value> {
    let canonical = canonical_template_event(event);
    let (subject, html) = match canonical {
        "reset_password" => (
            "[{{site_name}}] Reset your password",
            "<p>Hello {{username}}, reset your password: {{reset_url}}</p>",
        ),
        "notification_email.verify_code" => (
            "[{{site_name}}] Notification Email Verification",
            "<p>Your notification email verification code is {{code}}</p>",
        ),
        "verify_email" => (
            "[{{site_name}}] Verification code",
            "<p>Your verification code is {{code}}</p>",
        ),
        _ => return None,
    };
    Some(json!({
        "event": canonical,
        "locale": if locale.trim().is_empty() { "zh-CN" } else { locale.trim() },
        "subject": subject,
        "html": html,
        "is_custom": false,
        "updated_at": Utc::now().to_rfc3339()
    }))
}

fn template_variables<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
        .collect()
}

fn render_template_text(input: &str, variables: &Map<String, Value>) -> String {
    let mut output = input.to_owned();
    for (key, value) in variables {
        let replacement = value
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value.to_string());
        output = output.replace(&format!("{{{{{key}}}}}"), &replacement);
    }
    output
}

fn string_setting(settings: &Value, key: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_owned()
}

async fn send_email(
    config: SmtpConfig,
    to: &str,
    subject: String,
    body: String,
) -> Result<(), ApiError> {
    let config_for_send = config.clone();
    let to = to.trim().to_owned();
    tokio::task::spawn_blocking(move || {
        smtp_probe::send_test_email(&config_for_send, &to, &subject, &body)
    })
    .await
    .map_err(|error| ApiError::internal_server_error(format!("email send task failed: {error}")))?
}

fn verification_body(kind: VerificationEmailKind, site_name: &str, code: &str) -> String {
    format!(
        "<!DOCTYPE html><html><body><h1>{}</h1><p>{}</p><p style=\"font-size:24px;font-weight:bold;letter-spacing:4px;\">{}</p><p>This code expires in 15 minutes.</p></body></html>",
        escape_html(site_name),
        kind.purpose(),
        escape_html(code)
    )
}

fn password_reset_body(site_name: &str, reset_url: &str) -> String {
    format!(
        "<!DOCTYPE html><html><body><h1>{}</h1><p>A password reset was requested for your account.</p><p><a href=\"{}\">Reset your password</a></p><p>This link expires in 30 minutes.</p></body></html>",
        escape_html(site_name),
        escape_html(reset_url)
    )
}

fn is_absolute_http_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.trim().is_empty()
}

fn percent_encode_query(value: &str) -> String {
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

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use repository::SystemSettingRepository;

    #[test]
    fn builds_smtp_config_from_admin_settings() {
        let settings = serde_json::json!({
            "smtp_host": "smtp.example.com",
            "smtp_port": 2525,
            "smtp_username": "user",
            "smtp_password": "secret",
            "smtp_from": "noreply@example.com",
            "smtp_from_name": "Sub2API",
            "smtp_use_tls": true
        });
        let config = smtp_config_from_settings(&settings).unwrap();
        assert_eq!(config.host, "smtp.example.com");
        assert_eq!(config.port, 2525);
        assert_eq!(config.from, "noreply@example.com");
        assert!(config.use_tls);
    }

    #[test]
    fn rejects_missing_smtp_host() {
        assert!(smtp_config_from_settings(&serde_json::json!({})).is_err());
    }

    #[test]
    fn password_reset_requires_feature_and_email_verification_enabled() {
        let enabled = serde_json::json!({
            "password_reset_enabled": true,
            "email_verify_enabled": true,
            "frontend_url": "https://sub2api.example"
        });
        assert!(ensure_password_reset_enabled(&enabled).is_ok());

        let legacy_email_enabled = serde_json::json!({
            "password_reset_enabled": true,
            "email_enabled": true,
            "frontend_url": "https://sub2api.example"
        });
        assert!(ensure_password_reset_enabled(&legacy_email_enabled).is_ok());

        let disabled = serde_json::json!({
            "password_reset_enabled": true,
            "email_verify_enabled": false,
            "frontend_url": "https://sub2api.example"
        });
        assert_eq!(
            ensure_password_reset_enabled(&disabled)
                .unwrap_err()
                .status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn builds_password_reset_url_from_frontend_setting() {
        let settings = serde_json::json!({
            "frontend_url": "https://sub2api.example/app/",
        });
        assert_eq!(
            frontend_url_from_settings(&settings).unwrap(),
            "https://sub2api.example/app"
        );
        let body = password_reset_body(
            "Sub2API",
            "https://sub2api.example/reset-password?email=user%2Btag%40example.com&token=abc",
        );
        assert!(body.contains("user%2Btag%40example.com"));
    }

    #[tokio::test]
    async fn renders_persisted_email_templates_with_fallback_keys() {
        let repository = Arc::new(repository::InMemoryRepository::new());
        repository
            .upsert_system_setting(repository::SystemSettingRecord {
                namespace: "email_templates".to_owned(),
                key: "reset_password.zh-CN".to_owned(),
                value: serde_json::json!({
                    "event": "reset_password",
                    "locale": "zh-CN",
                    "subject": "[{{site_name}}] Reset {{username}}",
                    "html": "<a href=\"{{reset_url}}\">reset</a>",
                    "is_custom": true,
                    "updated_at": "2026-06-06T00:00:00Z"
                }),
                updated_at: "2026-06-06T00:00:00Z".to_owned(),
            })
            .await
            .unwrap();
        let service = EmailDeliveryService::new(repository);
        let variables = template_variables([
            ("site_name", "Sub2API"),
            ("username", "user@example.com"),
            ("reset_url", "https://sub2api.example/reset"),
        ]);

        let rendered = service
            .render_email_template("auth.password_reset", "en-US", &variables)
            .await
            .unwrap()
            .expect("fallback zh-CN reset template should render");

        assert_eq!(rendered.0, "[Sub2API] Reset user@example.com");
        assert_eq!(
            rendered.1,
            "<a href=\"https://sub2api.example/reset\">reset</a>"
        );
    }
}
