use crate::password_reset_store::{
    generate_reset_token, DynPasswordResetStore, MemoryPasswordResetStore,
};
use crate::response::ApiError;
use crate::verification_code_store::{
    generate_prepared_code, DynVerificationCodeStore, MemoryVerificationCodeStore,
    VerificationCodePurpose, CODE_COOLDOWN_SECONDS, CODE_TTL_SECONDS,
};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const ACCESS_TOKEN_PREFIX: &str = "dev-access-";
const REFRESH_TOKEN_PREFIX: &str = "dev-refresh-";
const ACCESS_TOKEN_EXPIRES_IN_SECONDS: u64 = 3600;
const RATE_LIMIT_WINDOW_5H_SECONDS: i64 = 5 * 60 * 60;
const RATE_LIMIT_WINDOW_1D_SECONDS: i64 = 24 * 60 * 60;
const RATE_LIMIT_WINDOW_7D_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_NOTIFY_EMAILS: usize = 3;

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub username: Option<String>,
    pub aff_code: Option<String>,
    pub affiliate_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct Login2FARequest {
    pub temp_token: Option<String>,
    pub totp_code: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmailCodeRequest {
    pub email: String,
    pub turnstile_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub email: String,
    pub token: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct OAuthCompletionRequest {
    pub email: Option<String>,
    pub password: Option<String>,
    pub verify_code: Option<String>,
    pub pending_token: Option<String>,
    pub pending_auth_token: Option<String>,
    pub pending_oauth_token: Option<String>,
    pub invitation_code: Option<String>,
    pub aff_code: Option<String>,
    pub adopt_display_name: Option<bool>,
    pub adopt_avatar: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct PendingOAuthExchangeRequest {
    pub pending_token: Option<String>,
    pub pending_auth_token: Option<String>,
    pub pending_oauth_token: Option<String>,
    pub adopt_display_name: Option<bool>,
    pub adopt_avatar: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub balance_notify_enabled: Option<bool>,
    pub balance_notify_threshold: Option<f64>,
    pub balance_notify_extra_emails: Option<Vec<NotifyEmailEntry>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotifyEmailEntry {
    pub email: String,
    pub disabled: bool,
    pub verified: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub group_id: Option<i64>,
    pub custom_key: Option<String>,
    pub ip_whitelist: Option<Vec<String>>,
    pub ip_blacklist: Option<Vec<String>>,
    pub quota: Option<f64>,
    pub rate_limit_5h: Option<f64>,
    pub rate_limit_1d: Option<f64>,
    pub rate_limit_7d: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub group_id: Option<i64>,
    pub status: Option<String>,
    pub ip_whitelist: Option<Vec<String>>,
    pub ip_blacklist: Option<Vec<String>>,
    pub quota: Option<f64>,
    pub reset_quota: Option<bool>,
    pub rate_limit_5h: Option<f64>,
    pub rate_limit_1d: Option<f64>,
    pub rate_limit_7d: Option<f64>,
    pub reset_rate_limit_usage: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdateApiKeyRequest {
    pub group_id: Option<i64>,
    pub reset_rate_limit_usage: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: &'static str,
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: i64,
    pub email: String,
    pub username: String,
    pub role: String,
    avatar_url: Option<String>,
    balance_notify_enabled: bool,
    balance_notify_threshold: Option<f64>,
    balance_notify_extra_emails: Vec<NotifyEmailEntry>,
    password_hash: String,
}

impl UserRecord {
    pub fn new(
        id: i64,
        email: impl Into<String>,
        username: impl Into<String>,
        role: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let password = password.into();
        Self {
            id,
            email: email.into(),
            username: username.into(),
            role: role.into(),
            avatar_url: None,
            balance_notify_enabled: false,
            balance_notify_threshold: None,
            balance_notify_extra_emails: Vec::new(),
            password_hash: Sha256PasswordHasher::hash_password(&password),
        }
    }

    fn public_json(&self) -> Value {
        let now = "2026-06-06T00:00:00Z";
        json!({
            "id": self.id,
            "username": self.username,
            "email": self.email,
            "email_bound": true,
            "role": self.role,
            "avatar_url": self.avatar_url,
            "balance": 0.0,
            "concurrency": 5,
            "rpm_limit": 0,
            "status": "active",
            "allowed_groups": null,
            "balance_notify_enabled": self.balance_notify_enabled,
            "balance_notify_threshold": self.balance_notify_threshold,
            "balance_notify_extra_emails": self.balance_notify_extra_emails,
            "linuxdo_bound": false,
            "oidc_bound": false,
            "wechat_bound": false,
            "dingtalk_bound": false,
            "github_bound": false,
            "google_bound": false,
            "auth_bindings": default_identity_bindings(),
            "identity_bindings": default_identity_bindings(),
            "created_at": now,
            "updated_at": now,
            "run_mode": "standard"
        })
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    id: i64,
    user_id: i64,
    key: String,
    name: String,
    group_id: Option<i64>,
    status: String,
    ip_whitelist: Vec<String>,
    ip_blacklist: Vec<String>,
    quota: f64,
    quota_used: f64,
    rate_limit_5h: f64,
    rate_limit_1d: f64,
    rate_limit_7d: f64,
    usage_5h: f64,
    usage_1d: f64,
    usage_7d: f64,
    window_5h_start: Option<i64>,
    window_1d_start: Option<i64>,
    window_7d_start: Option<i64>,
}

impl ApiKeyRecord {
    fn public_json(&self) -> Value {
        let now = "2026-06-06T00:00:00Z";
        json!({
            "id": self.id,
            "user_id": self.user_id,
            "key": self.key,
            "name": self.name,
            "group_id": self.group_id,
            "status": self.status,
            "ip_whitelist": self.ip_whitelist,
            "ip_blacklist": self.ip_blacklist,
            "last_used_at": null,
            "quota": self.quota,
            "quota_used": self.quota_used,
            "expires_at": null,
            "created_at": now,
            "updated_at": now,
            "rate_limit_5h": self.rate_limit_5h,
            "rate_limit_1d": self.rate_limit_1d,
            "rate_limit_7d": self.rate_limit_7d,
            "usage_5h": self.usage_5h,
            "usage_1d": self.usage_1d,
            "usage_7d": self.usage_7d,
            "window_5h_start": self.window_5h_start,
            "window_1d_start": self.window_1d_start,
            "window_7d_start": self.window_7d_start,
            "reset_5h_at": self.window_5h_start.map(|value| value + RATE_LIMIT_WINDOW_5H_SECONDS),
            "reset_1d_at": self.window_1d_start.map(|value| value + RATE_LIMIT_WINDOW_1D_SECONDS),
            "reset_7d_at": self.window_7d_start.map(|value| value + RATE_LIMIT_WINDOW_7D_SECONDS)
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiKeyUsageSnapshot {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub status: String,
    pub quota: f64,
    pub quota_used: f64,
    pub rate_limit_5h: f64,
    pub rate_limit_1d: f64,
    pub rate_limit_7d: f64,
    pub usage_5h: f64,
    pub usage_1d: f64,
    pub usage_7d: f64,
    pub window_5h_start: Option<i64>,
    pub window_1d_start: Option<i64>,
    pub window_7d_start: Option<i64>,
}

impl From<&ApiKeyRecord> for ApiKeyUsageSnapshot {
    fn from(record: &ApiKeyRecord) -> Self {
        Self {
            id: record.id,
            user_id: record.user_id,
            name: record.name.clone(),
            group_id: record.group_id,
            status: record.status.clone(),
            quota: record.quota,
            quota_used: record.quota_used,
            rate_limit_5h: record.rate_limit_5h,
            rate_limit_1d: record.rate_limit_1d,
            rate_limit_7d: record.rate_limit_7d,
            usage_5h: record.usage_5h,
            usage_1d: record.usage_1d,
            usage_7d: record.usage_7d,
            window_5h_start: record.window_5h_start,
            window_1d_start: record.window_1d_start,
            window_7d_start: record.window_7d_start,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayApiKeyIdentity {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub group_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PendingOAuthSession {
    pub token: String,
    pub provider: String,
    pub provider_key: String,
    pub provider_subject: String,
    pub intent: String,
    pub redirect: String,
    pub browser_session_key: String,
    pub resolved_email: Option<String>,
    pub target_user_id: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
    pub consumed_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct OAuthResolvedIdentity {
    pub provider_key: String,
    pub provider_subject: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthIdentityRecord {
    pub provider: String,
    pub provider_key: String,
    pub provider_subject: String,
    pub user_id: i64,
    pub email: Option<String>,
    pub bound_at: i64,
}

#[derive(Debug, Clone)]
struct OAuthBindToken {
    user_id: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
}

pub struct AuthService {
    next_user_id: AtomicI64,
    next_api_key_id: AtomicI64,
    now_override: RwLock<Option<i64>>,
    users_by_email: RwLock<HashMap<String, UserRecord>>,
    api_keys_by_id: RwLock<HashMap<i64, ApiKeyRecord>>,
    access_sessions: RwLock<HashMap<String, i64>>,
    refresh_sessions: RwLock<HashMap<String, i64>>,
    oauth_pending_sessions: RwLock<HashMap<String, PendingOAuthSession>>,
    oauth_identities: RwLock<HashMap<String, OAuthIdentityRecord>>,
    oauth_bind_tokens: RwLock<HashMap<String, OAuthBindToken>>,
    verification_codes: DynVerificationCodeStore,
    password_resets: DynPasswordResetStore,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedPasswordReset {
    pub email: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedVerificationCode {
    pub user_id: i64,
    pub target: String,
    pub code: String,
    pub purpose: VerificationCodePurpose,
}

impl AuthService {
    pub fn new() -> Self {
        Self::with_auth_stores(
            MemoryVerificationCodeStore::shared(),
            MemoryPasswordResetStore::shared(),
        )
    }

    pub(crate) fn with_auth_stores(
        verification_codes: DynVerificationCodeStore,
        password_resets: DynPasswordResetStore,
    ) -> Self {
        Self {
            next_user_id: AtomicI64::new(2),
            next_api_key_id: AtomicI64::new(1),
            now_override: RwLock::new(None),
            users_by_email: RwLock::new(HashMap::new()),
            api_keys_by_id: RwLock::new(HashMap::new()),
            access_sessions: RwLock::new(HashMap::new()),
            refresh_sessions: RwLock::new(HashMap::new()),
            oauth_pending_sessions: RwLock::new(HashMap::new()),
            oauth_identities: RwLock::new(HashMap::new()),
            oauth_bind_tokens: RwLock::new(HashMap::new()),
            verification_codes,
            password_resets,
        }
    }

    pub fn insert_seed_user(&self, user: UserRecord) {
        self.users_by_email
            .write()
            .expect("users lock")
            .insert(normalize_email(&user.email), user);
    }

    pub fn register(&self, request: RegisterRequest) -> Result<(AuthTokens, Value), ApiError> {
        let email = normalize_email(&request.email);
        validate_email(&email)?;
        validate_password(&request.password)?;

        let mut users = self.users_by_email.write().expect("users lock");
        if users.contains_key(&email) {
            return Err(ApiError::conflict("email already registered"));
        }

        let id = self.next_user_id.fetch_add(1, Ordering::SeqCst);
        let username = request
            .username
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_owned());
        let user = UserRecord::new(id, email.clone(), username, "user", request.password);
        let public_user = user.public_json();
        users.insert(email, user);
        drop(users);

        let tokens = self.issue_tokens(id);
        Ok((tokens, public_user))
    }

    pub fn login(&self, request: LoginRequest) -> Result<(AuthTokens, Value), ApiError> {
        let email = normalize_email(&request.email);
        let users = self.users_by_email.read().expect("users lock");
        let user = users
            .get(&email)
            .ok_or_else(|| ApiError::unauthorized("invalid email or password"))?;
        if !Sha256PasswordHasher::verify_password(&request.password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid email or password"));
        }

        let tokens = self.issue_tokens(user.id);
        Ok((tokens, user.public_json()))
    }

    pub fn verify_login_user(&self, email: &str, password: &str) -> Result<(i64, Value), ApiError> {
        let email = normalize_email(email);
        let users = self.users_by_email.read().expect("users lock");
        let user = users
            .get(&email)
            .ok_or_else(|| ApiError::unauthorized("invalid email or password"))?;
        if !Sha256PasswordHasher::verify_password(password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid email or password"));
        }
        Ok((user.id, user.public_json()))
    }

    pub fn login_2fa(&self, request: Login2FARequest) -> Result<(AuthTokens, Value), ApiError> {
        let code = request
            .totp_code
            .or(request.code)
            .unwrap_or_else(|| "123456".to_owned());
        if code.trim().is_empty() {
            return Err(ApiError::bad_request("2fa code is required"));
        }
        let _ = request.temp_token;
        let user_id = 1;
        let user = self.public_user_by_id(user_id)?;
        Ok((self.issue_tokens(user_id), user))
    }

    pub fn issue_tokens_for_user(&self, user_id: i64) -> Result<(AuthTokens, Value), ApiError> {
        let user = self.public_user_by_id(user_id)?;
        Ok((self.issue_tokens(user_id), user))
    }

    pub fn issue_tokens_for_repository_user(&self, user_id: i64) -> AuthTokens {
        self.issue_tokens(user_id)
    }

    pub fn send_verify_code(&self, request: EmailCodeRequest) -> Result<Value, ApiError> {
        validate_email(&request.email)?;
        let _ = request.turnstile_token;
        Ok(json!({
            "message": "Verification code sent",
            "countdown": 60,
            "expires_in": 600
        }))
    }

    pub fn forgot_password_response(&self, request: EmailCodeRequest) -> Result<Value, ApiError> {
        validate_email(&request.email)?;
        let _ = request.turnstile_token;
        Ok(Self::password_reset_requested_response())
    }

    pub fn password_reset_requested_response() -> Value {
        password_reset_requested_response()
    }

    pub(crate) fn prepare_password_reset(
        &self,
        request: EmailCodeRequest,
    ) -> Result<Option<PreparedPasswordReset>, ApiError> {
        let email = normalize_email(&request.email);
        validate_email(&email)?;
        let _ = request.turnstile_token;
        if !self
            .users_by_email
            .read()
            .expect("users lock")
            .contains_key(&email)
        {
            return Ok(None);
        }

        let now = self.now_seconds();
        if self.password_resets.is_email_in_cooldown(&email, now)? {
            return Ok(None);
        }
        let token = self
            .password_resets
            .reusable_token(&email, now)?
            .unwrap_or_else(generate_reset_token);
        Ok(Some(PreparedPasswordReset { email, token }))
    }

    pub(crate) fn prepare_password_reset_for_known_email(
        &self,
        email: &str,
    ) -> Result<Option<PreparedPasswordReset>, ApiError> {
        let email = normalize_email(email);
        validate_email(&email)?;
        let now = self.now_seconds();
        if self.password_resets.is_email_in_cooldown(&email, now)? {
            return Ok(None);
        }
        let token = self
            .password_resets
            .reusable_token(&email, now)?
            .unwrap_or_else(generate_reset_token);
        Ok(Some(PreparedPasswordReset { email, token }))
    }

    pub(crate) fn commit_prepared_password_reset(
        &self,
        prepared: &PreparedPasswordReset,
    ) -> Result<Value, ApiError> {
        let now = self.now_seconds();
        self.password_resets
            .save_token(&prepared.email, &prepared.token, now)?;
        self.password_resets.mark_email_sent(&prepared.email, now)?;
        Ok(password_reset_requested_response())
    }

    pub fn reset_repository_password_token(
        &self,
        email: &str,
        token: &str,
        new_password: &str,
    ) -> Result<String, ApiError> {
        validate_password(new_password)?;
        if token.trim().is_empty() {
            return Err(ApiError::bad_request("reset token is required"));
        }
        let email = normalize_email(email);
        validate_email(&email)?;
        self.password_resets
            .consume_token(&email, token, self.now_seconds())?;
        Ok(Sha256PasswordHasher::hash_password(new_password))
    }

    pub fn replace_memory_password_hash_if_present(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Option<i64> {
        let email = normalize_email(email);
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users.get_mut(&email)?;
        user.password_hash = password_hash.to_owned();
        Some(user.id)
    }

    pub fn reset_password(&self, request: ResetPasswordRequest) -> Result<Value, ApiError> {
        validate_password(&request.new_password)?;
        if request.token.trim().is_empty() {
            return Err(ApiError::bad_request("reset token is required"));
        }
        let email = normalize_email(&request.email);
        validate_email(&email)?;
        self.password_resets
            .consume_token(&email, &request.token, self.now_seconds())?;
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users.get_mut(&email).ok_or_else(|| invalid_reset_token())?;
        let user_id = user.id;
        user.password_hash = Sha256PasswordHasher::hash_password(&request.new_password);
        drop(users);
        self.revoke_all_user_sessions(user_id);
        Ok(json!({
            "message": "Your password has been reset successfully. You can now log in with your new password."
        }))
    }

    pub fn refresh(&self, request: RefreshRequest) -> Result<AuthTokens, ApiError> {
        let user_id = self
            .refresh_sessions
            .read()
            .expect("refresh lock")
            .get(&request.refresh_token)
            .copied()
            .ok_or_else(|| ApiError::unauthorized("invalid refresh token"))?;

        self.revoke_refresh(&request.refresh_token);
        Ok(self.issue_tokens(user_id))
    }

    pub fn logout(&self, request: LogoutRequest) {
        if let Some(refresh_token) = request.refresh_token {
            self.revoke_refresh(&refresh_token);
        }
    }

    pub fn revoke_tokens(&self, tokens: &AuthTokens) {
        self.access_sessions
            .write()
            .expect("access lock")
            .remove(&tokens.access_token);
        self.revoke_refresh(&tokens.refresh_token);
    }

    pub fn revoke_all_user_sessions(&self, user_id: i64) {
        self.access_sessions
            .write()
            .expect("access lock")
            .retain(|_, session_user_id| *session_user_id != user_id);
        self.refresh_sessions
            .write()
            .expect("refresh lock")
            .retain(|_, session_user_id| *session_user_id != user_id);
    }

    pub fn revoke_all_sessions_from_headers(&self, headers: &HeaderMap) -> Result<Value, ApiError> {
        let user_id = self.user_id_from_headers(headers)?;
        self.revoke_all_user_sessions(user_id);
        Ok(json!({ "message": "All sessions revoked" }))
    }

    pub fn prepare_oauth_bind_token(&self, headers: &HeaderMap) -> Result<Value, ApiError> {
        let user_id = self.user_id_from_headers(headers)?;
        let token = format!("bind-{user_id}-{}", Uuid::new_v4());
        let now = self.now_seconds();
        self.oauth_bind_tokens
            .write()
            .expect("oauth bind token lock")
            .insert(
                token.clone(),
                OAuthBindToken {
                    user_id,
                    expires_at: now + 600,
                    consumed_at: None,
                },
            );
        Ok(json!({
            "message": "OAuth bind token prepared",
            "bind_token": token
        }))
    }

    pub fn oauth_start_url(&self, provider: &str, bind: bool, payment: bool) -> Value {
        json!({
            "provider": provider,
            "auth_url": format!("/api/v1/auth/oauth/{provider}/callback?code=demo-code&state=demo-state"),
            "redirect_url": format!("/api/v1/auth/oauth/{provider}/callback"),
            "state": "demo-state",
            "intent": if bind { "bind_current_user" } else if payment { "payment" } else { "login" }
        })
    }

    pub fn oauth_callback(&self, provider: &str) -> Value {
        json!({
            "provider": provider,
            "auth_result": "pending",
            "pending_token": format!("pending-{provider}-{}", Uuid::new_v4()),
            "adoption_required": false,
            "suggested_display_name": format!("{provider}-user"),
            "suggested_avatar_url": null
        })
    }

    pub fn create_pending_oauth_session(
        &self,
        provider: &str,
        redirect: &str,
        intent: &str,
        bind_token: Option<&str>,
    ) -> Result<PendingOAuthSession, ApiError> {
        let provider = normalize_oauth_provider(provider)?;
        self.create_pending_oauth_session_with_identity(
            &provider,
            redirect,
            intent,
            bind_token,
            OAuthResolvedIdentity {
                provider_key: provider.clone(),
                provider_subject: format!("{provider}-subject-{}", Uuid::new_v4().simple()),
                email: None,
            },
        )
    }

    pub fn create_pending_oauth_session_with_identity(
        &self,
        provider: &str,
        redirect: &str,
        intent: &str,
        bind_token: Option<&str>,
        identity: OAuthResolvedIdentity,
    ) -> Result<PendingOAuthSession, ApiError> {
        let provider = normalize_oauth_provider(provider)?;
        let intent = normalize_oauth_intent(intent);
        let now = self.now_seconds();
        let mut target_user_id = None;
        if intent == "bind_current_user" {
            let token = bind_token
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::unauthorized("oauth bind token is required"))?;
            target_user_id = Some(self.consume_oauth_bind_token(token, now)?);
        }
        let session = PendingOAuthSession {
            token: format!("pending-{provider}-{}", Uuid::new_v4().simple()),
            provider: provider.clone(),
            provider_key: identity.provider_key,
            provider_subject: identity.provider_subject,
            intent,
            redirect: sanitize_auth_redirect(redirect),
            browser_session_key: format!("browser-{}", Uuid::new_v4().simple()),
            resolved_email: identity.email,
            target_user_id,
            created_at: now,
            expires_at: now + 600,
            consumed_at: None,
        };
        self.oauth_pending_sessions
            .write()
            .expect("oauth pending session lock")
            .insert(session.token.clone(), session.clone());
        Ok(session)
    }

    pub fn pending_oauth_session(
        &self,
        token: &str,
        browser_session_key: Option<&str>,
    ) -> Result<PendingOAuthSession, ApiError> {
        let sessions = self
            .oauth_pending_sessions
            .read()
            .expect("oauth pending session lock");
        let session = sessions
            .get(token.trim())
            .ok_or_else(|| ApiError::not_found("pending auth session not found"))?;
        self.validate_pending_oauth_session(session, browser_session_key)?;
        Ok(session.clone())
    }

    pub fn oauth_session_from_completion_request(
        &self,
        request: &OAuthCompletionRequest,
        browser_session_key: Option<&str>,
    ) -> Result<PendingOAuthSession, ApiError> {
        let token = pending_token_from_request(request)?;
        self.pending_oauth_session(&token, browser_session_key)
    }

    pub fn oauth_session_from_exchange_request(
        &self,
        request: &PendingOAuthExchangeRequest,
        session_token: Option<&str>,
        browser_session_key: Option<&str>,
    ) -> Result<PendingOAuthSession, ApiError> {
        let token = pending_token_from_parts(
            request.pending_token.clone(),
            request.pending_auth_token.clone(),
            request.pending_oauth_token.clone(),
        )
        .or_else(|_| {
            session_token
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| ApiError::bad_request("pending auth token is required"))
        })?;
        self.pending_oauth_session(&token, browser_session_key)
    }

    pub fn demo_email_oauth_tokens(&self, provider: &str) -> Result<(AuthTokens, Value), ApiError> {
        let email = format!("{provider}-oauth@example.com");
        let mut users = self.users_by_email.write().expect("users lock");
        let user_id = if let Some(user) = users.get(&email) {
            user.id
        } else {
            let id = self.next_user_id.fetch_add(1, Ordering::SeqCst);
            users.insert(
                email.clone(),
                UserRecord::new(
                    id,
                    email,
                    format!("{provider}-oauth"),
                    "user",
                    "oauth-password",
                ),
            );
            id
        };
        drop(users);
        Ok((self.issue_tokens(user_id), self.public_user_by_id(user_id)?))
    }

    pub fn pending_oauth_exchange(
        &self,
        request: PendingOAuthExchangeRequest,
    ) -> Result<Value, ApiError> {
        self.pending_oauth_exchange_with_session(request, None, None)
    }

    pub fn pending_oauth_exchange_with_session(
        &self,
        request: PendingOAuthExchangeRequest,
        session_token: Option<&str>,
        browser_session_key: Option<&str>,
    ) -> Result<Value, ApiError> {
        let _ = (request.adopt_display_name, request.adopt_avatar);
        let token = pending_token_from_parts(
            request.pending_token,
            request.pending_auth_token,
            request.pending_oauth_token,
        )
        .or_else(|_| {
            session_token
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| ApiError::bad_request("pending auth token is required"))
        })?;
        let session = self.pending_oauth_session(&token, browser_session_key)?;
        if let Some(user_id) = session.target_user_id {
            self.bind_oauth_identity_to_user(&session, user_id, session.resolved_email.clone())?;
            let consumed = self.consume_pending_oauth_session(&token, browser_session_key)?;
            let tokens = self.issue_tokens(user_id);
            let mut payload = auth_response(tokens, self.public_user_by_id(user_id)?);
            payload["provider"] = json!(consumed.provider);
            payload["auth_result"] = json!("login");
            return Ok(payload);
        }
        Ok(pending_oauth_session_status(&session))
    }

    pub fn pending_oauth_send_verify_code(
        &self,
        request: EmailCodeRequest,
    ) -> Result<Value, ApiError> {
        let mut response = self.send_verify_code(request)?;
        response["auth_result"] = json!("verify_code_sent");
        response["provider"] = json!("pending");
        response["redirect"] = json!("/dashboard");
        Ok(response)
    }

    pub fn complete_oauth_registration(
        &self,
        provider: &str,
        request: OAuthCompletionRequest,
    ) -> Result<(AuthTokens, Value), ApiError> {
        let _ = (
            request.invitation_code,
            request.aff_code,
            request.adopt_display_name,
            request.adopt_avatar,
        );
        let email = format!("{provider}-oauth@example.com");
        let mut users = self.users_by_email.write().expect("users lock");
        let user_id = if let Some(user) = users.get(&email) {
            user.id
        } else {
            let id = self.next_user_id.fetch_add(1, Ordering::SeqCst);
            users.insert(
                email.clone(),
                UserRecord::new(
                    id,
                    email,
                    format!("{provider}-oauth"),
                    "user",
                    "oauth-password",
                ),
            );
            id
        };
        drop(users);
        Ok((self.issue_tokens(user_id), self.public_user_by_id(user_id)?))
    }

    pub fn create_pending_oauth_account(
        &self,
        provider: &str,
        request: OAuthCompletionRequest,
    ) -> Result<(AuthTokens, Value), ApiError> {
        self.create_pending_oauth_account_with_session(provider, request, None)
    }

    pub fn create_pending_oauth_account_with_session(
        &self,
        provider: &str,
        request: OAuthCompletionRequest,
        browser_session_key: Option<&str>,
    ) -> Result<(AuthTokens, Value), ApiError> {
        let token = pending_token_from_request(&request)?;
        let session = self.pending_oauth_session(&token, browser_session_key)?;
        ensure_pending_provider(&session, provider)?;
        if session.intent != "login" {
            return Err(ApiError::bad_request(
                "pending auth session cannot create account",
            ));
        }
        let email = normalize_email(
            request
                .email
                .as_deref()
                .or(session.resolved_email.as_deref())
                .ok_or_else(|| ApiError::bad_request("email is required"))?,
        );
        validate_email(&email)?;
        let password = request
            .password
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("password is required"))?;
        validate_password(password)?;
        self.ensure_oauth_identity_available(&session, None)?;

        let mut users = self.users_by_email.write().expect("users lock");
        if users.contains_key(&email) {
            return Err(ApiError::conflict("email already registered"));
        }
        let id = self.next_user_id.fetch_add(1, Ordering::SeqCst);
        let username = email.split('@').next().unwrap_or("user").to_owned();
        let user = UserRecord::new(id, email.clone(), username, "user", password);
        let public_user = user.public_json();
        users.insert(email.clone(), user);
        drop(users);
        self.bind_oauth_identity_to_user(&session, id, Some(email))?;
        self.consume_pending_oauth_session(&token, browser_session_key)?;
        Ok((self.issue_tokens(id), public_user))
    }

    pub fn oauth_bind_login(
        &self,
        provider: &str,
        request: OAuthCompletionRequest,
    ) -> Result<(AuthTokens, Value), ApiError> {
        self.oauth_bind_login_with_session(provider, request, None)
    }

    pub fn oauth_bind_login_with_session(
        &self,
        provider: &str,
        request: OAuthCompletionRequest,
        browser_session_key: Option<&str>,
    ) -> Result<(AuthTokens, Value), ApiError> {
        let token = pending_token_from_request(&request)?;
        let session = self.pending_oauth_session(&token, browser_session_key)?;
        ensure_pending_provider(&session, provider)?;
        let email = request
            .email
            .as_deref()
            .map(normalize_email)
            .ok_or_else(|| ApiError::bad_request("email is required"))?;
        validate_email(&email)?;
        let password = request
            .password
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("password is required"))?;

        let users = self.users_by_email.read().expect("users lock");
        let user = users
            .get(&email)
            .ok_or_else(|| ApiError::unauthorized("invalid email or password"))?;
        if !Sha256PasswordHasher::verify_password(password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid email or password"));
        }
        if let Some(target_user_id) = session.target_user_id {
            if target_user_id != user.id {
                return Err(ApiError::conflict(
                    "pending auth session must be completed by the targeted user",
                ));
            }
        }
        let user_id = user.id;
        let user_json = user.public_json();
        drop(users);
        self.bind_oauth_identity_to_user(&session, user_id, Some(email))?;
        self.consume_pending_oauth_session(&token, browser_session_key)?;
        Ok((self.issue_tokens(user_id), user_json))
    }

    pub fn oauth_bind_login_preview(&self, provider: &str) -> Value {
        json!({
            "auth_result": "bind",
            "provider": provider,
            "redirect": "/profile"
        })
    }

    pub fn oauth_identity_for_user(
        &self,
        user_id: i64,
        provider: &str,
    ) -> Option<OAuthIdentityRecord> {
        self.oauth_identities
            .read()
            .expect("oauth identities lock")
            .values()
            .find(|identity| identity.user_id == user_id && identity.provider == provider)
            .cloned()
    }

    pub fn current_user_from_headers(&self, headers: &HeaderMap) -> Result<Value, ApiError> {
        let user_id = self.user_id_from_headers(headers)?;
        self.public_user_by_id(user_id)
    }

    pub fn admin_user_from_headers(&self, headers: &HeaderMap) -> Result<Value, ApiError> {
        let user = self.current_user_from_headers(headers)?;
        if user.get("role").and_then(Value::as_str) != Some("admin") {
            return Err(ApiError::forbidden("Admin access required"));
        }
        Ok(user)
    }

    pub fn update_profile_from_headers(
        &self,
        headers: &HeaderMap,
        request: UpdateProfileRequest,
    ) -> Result<Value, ApiError> {
        let user_id = self.user_id_from_headers(headers)?;
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if let Some(username) = request.username.filter(|value| !value.trim().is_empty()) {
            user.username = username;
        }
        if let Some(avatar_url) = request.avatar_url {
            user.avatar_url = normalize_avatar_url(&avatar_url)?;
        }
        if let Some(enabled) = request.balance_notify_enabled {
            user.balance_notify_enabled = enabled;
        }
        if request.balance_notify_threshold.is_some() {
            user.balance_notify_threshold = request.balance_notify_threshold;
        }
        if let Some(extra_emails) = request.balance_notify_extra_emails {
            user.balance_notify_extra_emails = extra_emails;
        }
        Ok(user.public_json())
    }

    pub fn send_notify_email_code(&self, user_id: i64, email: String) -> Result<Value, ApiError> {
        let prepared = self.prepare_notify_email_code(user_id, email)?;
        self.commit_prepared_verification_code(&prepared)
    }

    pub(crate) fn prepare_notify_email_code(
        &self,
        user_id: i64,
        email: String,
    ) -> Result<PreparedVerificationCode, ApiError> {
        let email = normalize_email(&email);
        validate_email(&email)?;
        self.prepare_verification_code(user_id, email, VerificationCodePurpose::NotifyEmail)
    }

    pub fn send_email_binding_code(&self, user_id: i64, email: String) -> Result<Value, ApiError> {
        let prepared = self.prepare_email_binding_code(user_id, email)?;
        self.commit_prepared_verification_code(&prepared)
    }

    pub(crate) fn prepare_email_binding_code(
        &self,
        user_id: i64,
        email: String,
    ) -> Result<PreparedVerificationCode, ApiError> {
        let email = normalize_email(&email);
        validate_email(&email)?;
        self.public_user_by_id(user_id)?;
        if self.email_is_used_by_other_user(user_id, &email) {
            return Err(ApiError::conflict("email already registered"));
        }
        self.prepare_verification_code(user_id, email, VerificationCodePurpose::EmailBinding)
    }

    pub fn send_totp_email_code(&self, user_id: i64) -> Result<Value, ApiError> {
        let prepared = self.prepare_totp_email_code(user_id)?;
        self.commit_prepared_verification_code(&prepared)
    }

    pub(crate) fn prepare_totp_email_code(
        &self,
        user_id: i64,
    ) -> Result<PreparedVerificationCode, ApiError> {
        let email = self.email_by_user_id(user_id)?;
        self.prepare_verification_code(user_id, email, VerificationCodePurpose::Totp)
    }

    pub(crate) fn commit_prepared_verification_code(
        &self,
        prepared: &PreparedVerificationCode,
    ) -> Result<Value, ApiError> {
        self.verification_codes.save_code(
            prepared.purpose,
            prepared.user_id,
            &prepared.target,
            &prepared.code,
            self.now_seconds(),
        )?;
        Ok(verification_code_response(
            "Verification code sent successfully",
        ))
    }

    pub fn verify_totp_email_code(&self, user_id: i64, code: &str) -> Result<(), ApiError> {
        let email = self.email_by_user_id(user_id)?;
        self.verification_codes.verify(
            VerificationCodePurpose::Totp,
            &email,
            code,
            self.now_seconds(),
        )?;
        self.verification_codes
            .remove(VerificationCodePurpose::Totp, &email)?;
        Ok(())
    }

    pub fn verify_user_password(&self, user_id: i64, password: &str) -> Result<(), ApiError> {
        if password.trim().is_empty() {
            return Err(ApiError::bad_request("password is required"));
        }
        let users = self.users_by_email.read().expect("users lock");
        let user = users
            .values()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if !Sha256PasswordHasher::verify_password(password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid password"));
        }
        Ok(())
    }

    pub fn verify_and_add_notify_email(
        &self,
        user_id: i64,
        email: String,
        code: String,
    ) -> Result<Value, ApiError> {
        let email = normalize_email(&email);
        validate_email(&email)?;
        self.verification_codes.verify(
            VerificationCodePurpose::NotifyEmail,
            &email,
            &code,
            self.now_seconds(),
        )?;
        self.verification_codes
            .remove(VerificationCodePurpose::NotifyEmail, &email)?;
        self.add_notify_email(user_id, email)
    }

    fn add_notify_email(&self, user_id: i64, email: String) -> Result<Value, ApiError> {
        validate_email(&email)?;
        let email = normalize_email(&email);
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if let Some(entry) = user
            .balance_notify_extra_emails
            .iter_mut()
            .find(|entry| entry.email == email)
        {
            entry.verified = true;
            entry.disabled = false;
        } else {
            if user.balance_notify_extra_emails.len() >= MAX_NOTIFY_EMAILS {
                return Err(ApiError::bad_request(format!(
                    "TOO_MANY_NOTIFY_EMAILS: maximum {MAX_NOTIFY_EMAILS} notification emails allowed"
                )));
            }
            user.balance_notify_extra_emails.push(NotifyEmailEntry {
                email,
                disabled: false,
                verified: true,
            });
        }
        Ok(user.public_json())
    }

    pub fn toggle_notify_email(
        &self,
        user_id: i64,
        email: String,
        disabled: bool,
    ) -> Result<Value, ApiError> {
        validate_email(&email)?;
        let email = normalize_email(&email);
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if let Some(entry) = user
            .balance_notify_extra_emails
            .iter_mut()
            .find(|entry| entry.email == email)
        {
            entry.disabled = disabled;
        } else {
            return Err(ApiError::bad_request(
                "EMAIL_NOT_FOUND: notification email not found",
            ));
        }
        Ok(user.public_json())
    }

    pub fn remove_notify_email(&self, user_id: i64, email: String) -> Result<Value, ApiError> {
        validate_email(&email)?;
        let email = normalize_email(&email);
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        let before = user.balance_notify_extra_emails.len();
        user.balance_notify_extra_emails
            .retain(|entry| entry.email != email);
        if user.balance_notify_extra_emails.len() == before {
            return Err(ApiError::bad_request(
                "EMAIL_NOT_FOUND: notification email not found",
            ));
        }
        Ok(user.public_json())
    }

    pub fn bind_email_identity(
        &self,
        user_id: i64,
        email: String,
        verify_code: String,
        password: String,
    ) -> Result<Value, ApiError> {
        validate_email(&email)?;
        let email = normalize_email(&email);
        self.verification_codes.verify(
            VerificationCodePurpose::EmailBinding,
            &email,
            &verify_code,
            self.now_seconds(),
        )?;
        let mut users = self.users_by_email.write().expect("users lock");
        if users
            .values()
            .any(|candidate| candidate.id != user_id && normalize_email(&candidate.email) == email)
        {
            return Err(ApiError::conflict("email already registered"));
        }
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if !Sha256PasswordHasher::verify_password(&password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid password"));
        }
        let bound_email = email.clone();
        user.email = bound_email.clone();
        self.verification_codes
            .remove(VerificationCodePurpose::EmailBinding, &bound_email)?;
        Ok(user.public_json())
    }

    pub fn public_user(&self, user_id: i64) -> Result<Value, ApiError> {
        self.public_user_by_id(user_id)
    }

    pub fn change_password_from_headers(
        &self,
        headers: &HeaderMap,
        request: ChangePasswordRequest,
    ) -> Result<Value, ApiError> {
        validate_password(&request.new_password)?;
        let user_id = self.user_id_from_headers(headers)?;
        let mut users = self.users_by_email.write().expect("users lock");
        let user = users
            .values_mut()
            .find(|user| user.id == user_id)
            .ok_or_else(|| ApiError::unauthorized("user not found"))?;
        if !Sha256PasswordHasher::verify_password(&request.old_password, &user.password_hash) {
            return Err(ApiError::unauthorized("invalid old password"));
        }
        user.password_hash = Sha256PasswordHasher::hash_password(&request.new_password);
        Ok(json!({ "message": "Password changed successfully" }))
    }

    pub fn user_id_from_headers(&self, headers: &HeaderMap) -> Result<i64, ApiError> {
        let token = bearer_token(headers).ok_or_else(|| ApiError::unauthorized("missing token"))?;
        self.access_sessions
            .read()
            .expect("access lock")
            .get(token)
            .copied()
            .ok_or_else(|| ApiError::unauthorized("invalid token"))
    }

    pub fn list_api_keys(&self, user_id: i64) -> Value {
        let mut items: Vec<Value> = self
            .api_keys_by_id
            .read()
            .expect("api key lock")
            .values()
            .filter(|key| key.user_id == user_id)
            .map(ApiKeyRecord::public_json)
            .collect();
        items.sort_by_key(|item| item["id"].as_i64().unwrap_or_default());
        let total = items.len();
        json!({
            "items": items,
            "total": total,
            "page": 1,
            "page_size": 10,
            "pages": if total == 0 { 0 } else { 1 }
        })
    }

    pub fn list_api_keys_for_admin(&self, user_id: i64, page: i64, page_size: i64) -> Value {
        let page = page.max(1);
        let page_size = page_size.clamp(1, 200);
        let mut items: Vec<Value> = self
            .api_keys_by_id
            .read()
            .expect("api key lock")
            .values()
            .filter(|key| key.user_id == user_id)
            .map(ApiKeyRecord::public_json)
            .collect();
        items.sort_by_key(|item| item["id"].as_i64().unwrap_or_default());
        items.reverse();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        let page_items = items
            .into_iter()
            .skip(start)
            .take(page_size as usize)
            .collect::<Vec<_>>();
        let pages = if total == 0 {
            0
        } else {
            ((total as f64) / (page_size as f64)).ceil() as i64
        };
        json!({
            "items": page_items,
            "total": total,
            "page": page,
            "page_size": page_size,
            "pages": pages,
            "total_pages": pages
        })
    }

    pub fn replace_user_group_keys(
        &self,
        user_id: i64,
        old_group_id: i64,
        new_group_id: i64,
    ) -> i64 {
        let mut migrated = 0;
        for record in self
            .api_keys_by_id
            .write()
            .expect("api key lock")
            .values_mut()
            .filter(|record| record.user_id == user_id && record.group_id == Some(old_group_id))
        {
            record.group_id = Some(new_group_id);
            migrated += 1;
        }
        migrated
    }

    pub fn create_api_key(
        &self,
        user_id: i64,
        request: CreateApiKeyRequest,
    ) -> Result<Value, ApiError> {
        let name = request.name.trim();
        if name.is_empty() {
            return Err(ApiError::bad_request("name is required"));
        }
        let id = self.next_api_key_id.fetch_add(1, Ordering::SeqCst);
        let key = request
            .custom_key
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("sk-dev-{}", Uuid::new_v4()));
        let record = ApiKeyRecord {
            id,
            user_id,
            key,
            name: name.to_owned(),
            group_id: request.group_id,
            status: "active".to_owned(),
            ip_whitelist: request.ip_whitelist.unwrap_or_default(),
            ip_blacklist: request.ip_blacklist.unwrap_or_default(),
            quota: request.quota.unwrap_or(0.0),
            quota_used: 0.0,
            rate_limit_5h: request.rate_limit_5h.unwrap_or(0.0),
            rate_limit_1d: request.rate_limit_1d.unwrap_or(0.0),
            rate_limit_7d: request.rate_limit_7d.unwrap_or(0.0),
            usage_5h: 0.0,
            usage_1d: 0.0,
            usage_7d: 0.0,
            window_5h_start: None,
            window_1d_start: None,
            window_7d_start: None,
        };
        let response = record.public_json();
        self.api_keys_by_id
            .write()
            .expect("api key lock")
            .insert(id, record);
        Ok(response)
    }

    pub fn get_api_key(&self, user_id: i64, id: i64) -> Result<Value, ApiError> {
        let keys = self.api_keys_by_id.read().expect("api key lock");
        let record = keys
            .get(&id)
            .filter(|record| record.user_id == user_id)
            .ok_or_else(|| ApiError::not_found("api key not found"))?;
        Ok(record.public_json())
    }

    pub fn get_api_key_for_admin(&self, id: i64) -> Result<Value, ApiError> {
        let keys = self.api_keys_by_id.read().expect("api key lock");
        let record = keys
            .get(&id)
            .ok_or_else(|| ApiError::not_found("api key not found"))?;
        Ok(record.public_json())
    }

    pub fn update_api_key(
        &self,
        user_id: i64,
        id: i64,
        request: UpdateApiKeyRequest,
    ) -> Result<Value, ApiError> {
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let record = keys
            .get_mut(&id)
            .filter(|record| record.user_id == user_id)
            .ok_or_else(|| ApiError::not_found("api key not found"))?;
        if let Some(name) = request.name.filter(|value| !value.trim().is_empty()) {
            record.name = name;
        }
        if request.group_id.is_some() {
            record.group_id = request.group_id;
        }
        if let Some(status) = request.status {
            if status != "active" && status != "inactive" {
                return Err(ApiError::bad_request("invalid api key status"));
            }
            record.status = status;
        }
        if let Some(ip_whitelist) = request.ip_whitelist {
            record.ip_whitelist = ip_whitelist;
        }
        if let Some(ip_blacklist) = request.ip_blacklist {
            record.ip_blacklist = ip_blacklist;
        }
        if let Some(quota) = request.quota {
            record.quota = quota;
        }
        if request.reset_quota.unwrap_or(false) {
            record.quota_used = 0.0;
        }
        if let Some(rate_limit) = request.rate_limit_5h {
            record.rate_limit_5h = rate_limit;
        }
        if let Some(rate_limit) = request.rate_limit_1d {
            record.rate_limit_1d = rate_limit;
        }
        if let Some(rate_limit) = request.rate_limit_7d {
            record.rate_limit_7d = rate_limit;
        }
        if request.reset_rate_limit_usage.unwrap_or(false) {
            record.usage_5h = 0.0;
            record.usage_1d = 0.0;
            record.usage_7d = 0.0;
            record.window_5h_start = None;
            record.window_1d_start = None;
            record.window_7d_start = None;
        }
        Ok(record.public_json())
    }

    pub fn admin_update_api_key(
        &self,
        id: i64,
        group_id: Option<Option<i64>>,
        reset_rate_limit_usage: bool,
    ) -> Result<Value, ApiError> {
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let record = keys
            .get_mut(&id)
            .ok_or_else(|| ApiError::not_found("api key not found"))?;
        if let Some(group_id) = group_id {
            record.group_id = group_id;
        }
        if reset_rate_limit_usage {
            record.usage_5h = 0.0;
            record.usage_1d = 0.0;
            record.usage_7d = 0.0;
            record.window_5h_start = None;
            record.window_1d_start = None;
            record.window_7d_start = None;
        }
        Ok(record.public_json())
    }

    pub fn delete_api_key(&self, user_id: i64, id: i64) -> Result<Value, ApiError> {
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let can_delete = keys
            .get(&id)
            .is_some_and(|record| record.user_id == user_id);
        if !can_delete {
            return Err(ApiError::not_found("api key not found"));
        }
        keys.remove(&id);
        Ok(json!({ "message": "API key deleted successfully" }))
    }

    pub fn validate_gateway_api_key(&self, key: &str) -> Result<GatewayApiKeyIdentity, ApiError> {
        let key = key.trim();
        let keys = self.api_keys_by_id.read().expect("api key lock");
        let record = keys
            .values()
            .find(|record| record.key == key)
            .ok_or_else(|| ApiError::unauthorized("invalid API key"))?;
        if record.status != "active" {
            return Err(ApiError::unauthorized("API key is not active"));
        }

        Ok(GatewayApiKeyIdentity {
            id: record.id,
            user_id: record.user_id,
            name: record.name.clone(),
            group_id: record.group_id,
        })
    }

    pub fn api_key_usage_snapshot(&self, id: i64) -> Option<ApiKeyUsageSnapshot> {
        self.api_keys_by_id
            .read()
            .expect("api key lock")
            .get(&id)
            .map(ApiKeyUsageSnapshot::from)
    }

    pub fn increment_api_key_quota_used(
        &self,
        id: i64,
        amount: f64,
    ) -> Option<ApiKeyUsageSnapshot> {
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let record = keys.get_mut(&id)?;
        if amount.is_finite() && amount > 0.0 {
            record.quota_used += amount;
            if record.quota > 0.0 && record.quota_used >= record.quota {
                record.status = "quota_exhausted".to_owned();
            }
        }
        Some(ApiKeyUsageSnapshot::from(&*record))
    }

    pub fn check_api_key_rate_limits(&self, id: i64) -> Result<(), ApiError> {
        let now = self.now_seconds();
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let Some(record) = keys.get_mut(&id) else {
            return Ok(());
        };
        reset_expired_rate_limit_windows(record, now);
        if record.rate_limit_5h > 0.0 && record.usage_5h >= record.rate_limit_5h {
            return Err(ApiError::too_many_requests(
                "API key 5h rate limit exceeded",
            ));
        }
        if record.rate_limit_1d > 0.0 && record.usage_1d >= record.rate_limit_1d {
            return Err(ApiError::too_many_requests(
                "API key 1d rate limit exceeded",
            ));
        }
        if record.rate_limit_7d > 0.0 && record.usage_7d >= record.rate_limit_7d {
            return Err(ApiError::too_many_requests(
                "API key 7d rate limit exceeded",
            ));
        }
        Ok(())
    }

    pub fn increment_api_key_rate_limit_usage(
        &self,
        id: i64,
        amount: f64,
    ) -> Option<ApiKeyUsageSnapshot> {
        let now = self.now_seconds();
        let mut keys = self.api_keys_by_id.write().expect("api key lock");
        let record = keys.get_mut(&id)?;
        reset_expired_rate_limit_windows(record, now);
        if amount.is_finite() && amount > 0.0 {
            if record.window_5h_start.is_none() {
                record.window_5h_start = Some(now);
            }
            if record.window_1d_start.is_none() {
                record.window_1d_start = Some(now);
            }
            if record.window_7d_start.is_none() {
                record.window_7d_start = Some(now);
            }
            record.usage_5h += amount;
            record.usage_1d += amount;
            record.usage_7d += amount;
        }
        Some(ApiKeyUsageSnapshot::from(&*record))
    }

    #[cfg(test)]
    pub fn set_now_for_tests(&self, now: i64) {
        *self.now_override.write().expect("now override lock") = Some(now);
    }

    #[cfg(test)]
    pub fn notify_code_for_tests(&self, email: &str) -> Option<String> {
        self.verification_codes.code_for_tests(
            VerificationCodePurpose::NotifyEmail,
            &normalize_email(email),
        )
    }

    #[cfg(test)]
    pub fn email_bind_code_for_tests(&self, email: &str) -> Option<String> {
        self.verification_codes.code_for_tests(
            VerificationCodePurpose::EmailBinding,
            &normalize_email(email),
        )
    }

    #[cfg(test)]
    pub fn totp_email_code_for_tests(&self, user_id: i64) -> Option<String> {
        self.email_by_user_id(user_id).ok().and_then(|email| {
            self.verification_codes
                .code_for_tests(VerificationCodePurpose::Totp, &email)
        })
    }

    #[cfg(test)]
    pub fn password_reset_token_for_tests(&self, email: &str) -> Option<String> {
        self.password_resets
            .token_for_tests(&normalize_email(email))
    }

    fn now_seconds(&self) -> i64 {
        if let Some(now) = *self.now_override.read().expect("now override lock") {
            return now;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0)
    }

    fn consume_oauth_bind_token(&self, token: &str, now: i64) -> Result<i64, ApiError> {
        let mut tokens = self
            .oauth_bind_tokens
            .write()
            .expect("oauth bind token lock");
        let record = tokens
            .get_mut(token)
            .ok_or_else(|| ApiError::unauthorized("invalid oauth bind token"))?;
        if record.consumed_at.is_some() {
            return Err(ApiError::unauthorized("oauth bind token already used"));
        }
        if record.expires_at <= now {
            return Err(ApiError::unauthorized("oauth bind token expired"));
        }
        record.consumed_at = Some(now);
        Ok(record.user_id)
    }

    fn validate_pending_oauth_session(
        &self,
        session: &PendingOAuthSession,
        browser_session_key: Option<&str>,
    ) -> Result<(), ApiError> {
        if session.consumed_at.is_some() {
            return Err(ApiError::unauthorized(
                "pending auth session has already been used",
            ));
        }
        if session.expires_at <= self.now_seconds() {
            return Err(ApiError::unauthorized("pending auth session has expired"));
        }
        let expected = session.browser_session_key.trim();
        if !expected.is_empty()
            && browser_session_key
                .map(str::trim)
                .filter(|value| !value.is_empty())
                != Some(expected)
        {
            return Err(ApiError::unauthorized(
                "pending auth completion code does not match this browser session",
            ));
        }
        Ok(())
    }

    fn consume_pending_oauth_session(
        &self,
        token: &str,
        browser_session_key: Option<&str>,
    ) -> Result<PendingOAuthSession, ApiError> {
        let mut sessions = self
            .oauth_pending_sessions
            .write()
            .expect("oauth pending session lock");
        let session = sessions
            .get_mut(token.trim())
            .ok_or_else(|| ApiError::not_found("pending auth session not found"))?;
        self.validate_pending_oauth_session(session, browser_session_key)?;
        session.consumed_at = Some(self.now_seconds());
        Ok(session.clone())
    }

    fn ensure_oauth_identity_available(
        &self,
        session: &PendingOAuthSession,
        user_id: Option<i64>,
    ) -> Result<(), ApiError> {
        let key = oauth_identity_key(
            &session.provider,
            &session.provider_key,
            &session.provider_subject,
        );
        if let Some(existing) = self
            .oauth_identities
            .read()
            .expect("oauth identities lock")
            .get(&key)
        {
            if user_id != Some(existing.user_id) {
                return Err(ApiError::conflict(
                    "oauth identity is already bound to another user",
                ));
            }
        }
        Ok(())
    }

    fn bind_oauth_identity_to_user(
        &self,
        session: &PendingOAuthSession,
        user_id: i64,
        email: Option<String>,
    ) -> Result<(), ApiError> {
        self.ensure_oauth_identity_available(session, Some(user_id))?;
        self.oauth_identities
            .write()
            .expect("oauth identities lock")
            .insert(
                oauth_identity_key(
                    &session.provider,
                    &session.provider_key,
                    &session.provider_subject,
                ),
                OAuthIdentityRecord {
                    provider: session.provider.clone(),
                    provider_key: session.provider_key.clone(),
                    provider_subject: session.provider_subject.clone(),
                    user_id,
                    email,
                    bound_at: self.now_seconds(),
                },
            );
        Ok(())
    }

    fn public_user_by_id(&self, user_id: i64) -> Result<Value, ApiError> {
        let users = self.users_by_email.read().expect("users lock");
        users
            .values()
            .find(|user| user.id == user_id)
            .map(UserRecord::public_json)
            .ok_or_else(|| ApiError::unauthorized("user not found"))
    }

    fn email_by_user_id(&self, user_id: i64) -> Result<String, ApiError> {
        let users = self.users_by_email.read().expect("users lock");
        users
            .values()
            .find(|user| user.id == user_id)
            .map(|user| normalize_email(&user.email))
            .filter(|email| !email.is_empty())
            .ok_or_else(|| ApiError::unauthorized("user not found"))
    }

    fn email_is_used_by_other_user(&self, user_id: i64, email: &str) -> bool {
        self.users_by_email
            .read()
            .expect("users lock")
            .values()
            .any(|candidate| candidate.id != user_id && normalize_email(&candidate.email) == email)
    }

    fn issue_tokens(&self, user_id: i64) -> AuthTokens {
        let access_token = format!("{ACCESS_TOKEN_PREFIX}{}", Uuid::new_v4());
        let refresh_token = format!("{REFRESH_TOKEN_PREFIX}{}", Uuid::new_v4());
        self.access_sessions
            .write()
            .expect("access lock")
            .insert(access_token.clone(), user_id);
        self.refresh_sessions
            .write()
            .expect("refresh lock")
            .insert(refresh_token.clone(), user_id);
        AuthTokens {
            access_token,
            refresh_token,
            expires_in: ACCESS_TOKEN_EXPIRES_IN_SECONDS,
            token_type: "Bearer",
        }
    }

    fn revoke_refresh(&self, refresh_token: &str) {
        self.refresh_sessions
            .write()
            .expect("refresh lock")
            .remove(refresh_token);
    }

    fn prepare_verification_code(
        &self,
        user_id: i64,
        target: String,
        purpose: VerificationCodePurpose,
    ) -> Result<PreparedVerificationCode, ApiError> {
        let now = self.now_seconds();
        self.verification_codes.can_create(purpose, &target, now)?;
        Ok(PreparedVerificationCode {
            user_id,
            code: generate_prepared_code(user_id, &target, now),
            target,
            purpose,
        })
    }
}

fn verification_code_response(message: &str) -> Value {
    json!({
        "message": message,
        "countdown": CODE_COOLDOWN_SECONDS,
        "expires_in": CODE_TTL_SECONDS
    })
}

fn password_reset_requested_response() -> Value {
    json!({
        "message": "If your email is registered, you will receive a password reset link shortly."
    })
}

fn invalid_reset_token() -> ApiError {
    ApiError::bad_request("INVALID_RESET_TOKEN: invalid or expired password reset token")
}

fn reset_expired_rate_limit_windows(record: &mut ApiKeyRecord, now: i64) {
    if window_expired(record.window_5h_start, now, RATE_LIMIT_WINDOW_5H_SECONDS) {
        record.usage_5h = 0.0;
        record.window_5h_start = None;
    }
    if window_expired(record.window_1d_start, now, RATE_LIMIT_WINDOW_1D_SECONDS) {
        record.usage_1d = 0.0;
        record.window_1d_start = None;
    }
    if window_expired(record.window_7d_start, now, RATE_LIMIT_WINDOW_7D_SECONDS) {
        record.usage_7d = 0.0;
        record.window_7d_start = None;
    }
}

fn window_expired(start: Option<i64>, now: i64, window_seconds: i64) -> bool {
    start.is_some_and(|start| start + window_seconds <= now)
}

trait PasswordHasher {
    fn hash_password(password: &str) -> String;
    fn verify_password(password: &str, hash: &str) -> bool;
}

struct Sha256PasswordHasher;

impl PasswordHasher for Sha256PasswordHasher {
    fn hash_password(password: &str) -> String {
        let digest = Sha256::digest(password.as_bytes());
        format!("sha256:{digest:x}")
    }

    fn verify_password(password: &str, hash: &str) -> bool {
        Self::hash_password(password) == hash
    }
}

pub fn auth_response(tokens: AuthTokens, user: Value) -> Value {
    json!({
        "access_token": tokens.access_token,
        "refresh_token": tokens.refresh_token,
        "expires_in": tokens.expires_in,
        "token_type": tokens.token_type,
        "user": user
    })
}

pub fn hash_password_for_repository(password: &str) -> Result<String, ApiError> {
    validate_password(password)?;
    Ok(Sha256PasswordHasher::hash_password(password))
}

pub fn verify_repository_password(password: &str, hash: &str) -> bool {
    Sha256PasswordHasher::verify_password(password, hash)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn normalize_oauth_provider(provider: &str) -> Result<String, ApiError> {
    let provider = provider.trim().to_ascii_lowercase();
    if matches!(
        provider.as_str(),
        "linuxdo" | "github" | "google" | "wechat" | "oidc" | "dingtalk" | "pending"
    ) {
        return Ok(provider);
    }
    Err(ApiError::bad_request("unsupported oauth provider"))
}

fn normalize_oauth_intent(intent: &str) -> String {
    match intent.trim() {
        "bind_current_user" => "bind_current_user".to_owned(),
        "payment" => "payment".to_owned(),
        _ => "login".to_owned(),
    }
}

fn sanitize_auth_redirect(value: &str) -> String {
    let value = value.trim();
    if value.is_empty()
        || !value.starts_with('/')
        || value.starts_with("//")
        || value.contains("://")
        || value.contains('\r')
        || value.contains('\n')
    {
        "/dashboard".to_owned()
    } else {
        value.to_owned()
    }
}

fn pending_token_from_request(request: &OAuthCompletionRequest) -> Result<String, ApiError> {
    pending_token_from_parts(
        request.pending_token.clone(),
        request.pending_auth_token.clone(),
        request.pending_oauth_token.clone(),
    )
}

fn pending_token_from_parts(
    pending_token: Option<String>,
    pending_auth_token: Option<String>,
    pending_oauth_token: Option<String>,
) -> Result<String, ApiError> {
    pending_token
        .or(pending_auth_token)
        .or(pending_oauth_token)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("pending auth token is required"))
}

fn ensure_pending_provider(session: &PendingOAuthSession, provider: &str) -> Result<(), ApiError> {
    let provider = normalize_oauth_provider(provider)?;
    if provider != "pending" && session.provider != provider {
        return Err(ApiError::bad_request(
            "pending oauth session provider mismatch",
        ));
    }
    Ok(())
}

fn oauth_identity_key(provider: &str, provider_key: &str, provider_subject: &str) -> String {
    format!(
        "{}:{}:{}",
        provider.trim().to_ascii_lowercase(),
        provider_key.trim().to_ascii_lowercase(),
        provider_subject.trim()
    )
}

fn pending_oauth_session_status(session: &PendingOAuthSession) -> Value {
    json!({
        "auth_result": "pending_session",
        "provider": session.provider,
        "intent": session.intent,
        "redirect": session.redirect,
        "email": session.resolved_email,
        "adoption_required": false,
        "suggested_display_name": format!("{}-user", session.provider),
        "suggested_avatar_url": null
    })
}

fn validate_email(email: &str) -> Result<(), ApiError> {
    if email.contains('@') && email.len() <= 254 {
        return Ok(());
    }
    Err(ApiError::bad_request("invalid email"))
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() >= 8 && password.len() <= 128 {
        return Ok(());
    }
    Err(ApiError::bad_request("password must be 8-128 characters"))
}

fn normalize_avatar_url(raw: &str) -> Result<Option<String>, ApiError> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if is_valid_remote_avatar_url(value) || is_valid_inline_avatar_url(value) {
        return Ok(Some(value.to_owned()));
    }
    Err(ApiError::bad_request("invalid avatar_url"))
}

fn is_valid_remote_avatar_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.trim().is_empty()
}

fn is_valid_inline_avatar_url(value: &str) -> bool {
    let Some(body) = value.strip_prefix("data:") else {
        return false;
    };
    let Some((metadata, encoded)) = body.split_once(',') else {
        return false;
    };
    let metadata = metadata.trim();
    !encoded.trim().is_empty()
        && metadata.len() > ";base64".len()
        && metadata.to_ascii_lowercase().starts_with("image/")
        && metadata.to_ascii_lowercase().ends_with(";base64")
}

fn default_identity_bindings() -> Value {
    json!({
        "email": {
            "provider": "email",
            "provider_key": "email",
            "bound": true,
            "bound_count": 1,
            "can_bind": false,
            "can_unbind": false
        },
        "linuxdo": provider_binding("linuxdo"),
        "oidc": provider_binding("oidc"),
        "wechat": provider_binding("wechat"),
        "github": provider_binding("github"),
        "google": provider_binding("google"),
        "dingtalk": provider_binding("dingtalk")
    })
}

fn provider_binding(provider: &str) -> Value {
    json!({
        "provider": provider,
        "bound": false,
        "bound_count": 0,
        "can_bind": true,
        "can_unbind": false
    })
}
