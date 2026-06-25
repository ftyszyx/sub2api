use domain::{AccountGroupBinding, Group, Provider};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use uuid::Uuid;

use crate::response::ApiError;

const NOW: &str = "2026-06-06T00:00:00Z";
const START_DATE: &str = "2026-06-01";
const END_DATE: &str = "2026-06-06";

#[derive(Debug, Deserialize)]
pub struct BatchApiKeysUsageRequest {
    pub api_key_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct RedeemRequest {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct EmailRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyNotifyEmailRequest {
    pub email: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct ToggleNotifyEmailRequest {
    pub email: String,
    pub disabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct BindEmailIdentityRequest {
    pub email: String,
    pub verify_code: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct StartIdentityBindingRequest {
    pub provider: String,
    pub redirect_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TotpSetupRequest {
    pub email_code: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TotpEnableRequest {
    pub totp_code: String,
    pub setup_token: String,
}

#[derive(Debug, Deserialize)]
pub struct TotpDisableRequest {
    pub email_code: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone)]
struct TotpState {
    enabled: bool,
    enabled_at: Option<i64>,
    setup_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AffiliateTransferClaim {
    pub amount: f64,
    pub available_quota_after: f64,
    pub frozen_quota_after: f64,
    pub history_quota_after: f64,
}

#[derive(Debug, Clone)]
struct AffiliateProfile {
    user_id: i64,
    aff_code: String,
    inviter_id: Option<i64>,
    aff_count: i64,
    aff_quota: f64,
    aff_frozen_quota: f64,
    aff_history_quota: f64,
    effective_rebate_rate_percent: f64,
    invitees: Vec<Value>,
}

impl AffiliateProfile {
    fn default_for_user(user_id: i64) -> Self {
        Self {
            user_id,
            aff_code: format!("AFF{user_id:04}"),
            inviter_id: None,
            aff_count: 0,
            aff_quota: 0.0,
            aff_frozen_quota: 0.0,
            aff_history_quota: 0.0,
            effective_rebate_rate_percent: 0.0,
            invitees: Vec::new(),
        }
    }

    fn detail_json(&self) -> Value {
        json!({
            "user_id": self.user_id,
            "aff_code": self.aff_code,
            "inviter_id": self.inviter_id,
            "aff_count": self.aff_count,
            "aff_quota": self.aff_quota,
            "aff_frozen_quota": self.aff_frozen_quota,
            "aff_history_quota": self.aff_history_quota,
            "effective_rebate_rate_percent": self.effective_rebate_rate_percent,
            "invitees": self.invitees
        })
    }
}

#[derive(Default)]
pub struct UserPortalService {
    totp_by_user: RwLock<HashMap<i64, TotpState>>,
    affiliates_by_user: RwLock<HashMap<i64, AffiliateProfile>>,
    affiliate_transfers: RwLock<Vec<Value>>,
}

impl UserPortalService {
    pub fn new() -> Self {
        Self {
            totp_by_user: RwLock::new(HashMap::new()),
            affiliates_by_user: RwLock::new(HashMap::new()),
            affiliate_transfers: RwLock::new(Vec::new()),
        }
    }

    pub fn affiliate_detail(&self, user_id: i64) -> Value {
        self.ensure_affiliate_profile(user_id).detail_json()
    }

    pub fn claim_affiliate_quota(&self, user_id: i64) -> Result<AffiliateTransferClaim, ApiError> {
        let mut profiles = self
            .affiliates_by_user
            .write()
            .expect("affiliate profiles lock");
        let profile = profiles
            .entry(user_id)
            .or_insert_with(|| AffiliateProfile::default_for_user(user_id));
        if profile.aff_quota <= f64::EPSILON {
            return Err(ApiError::bad_request(
                "no affiliate quota available to transfer",
            ));
        }
        let amount = profile.aff_quota;
        profile.aff_quota = 0.0;
        Ok(AffiliateTransferClaim {
            amount,
            available_quota_after: profile.aff_quota,
            frozen_quota_after: profile.aff_frozen_quota,
            history_quota_after: profile.aff_history_quota,
        })
    }

    pub fn rollback_affiliate_quota_claim(&self, user_id: i64, claim: &AffiliateTransferClaim) {
        let mut profiles = self
            .affiliates_by_user
            .write()
            .expect("affiliate profiles lock");
        let profile = profiles
            .entry(user_id)
            .or_insert_with(|| AffiliateProfile::default_for_user(user_id));
        profile.aff_quota += claim.amount;
    }

    pub fn commit_affiliate_transfer(
        &self,
        user_id: i64,
        claim: &AffiliateTransferClaim,
        balance: f64,
    ) -> Value {
        let transfer = json!({
            "user_id": user_id,
            "amount": claim.amount,
            "balance_after": balance,
            "available_quota_after": claim.available_quota_after,
            "frozen_quota_after": claim.frozen_quota_after,
            "history_quota_after": claim.history_quota_after,
            "snapshot_available": true,
            "created_at": NOW
        });
        self.affiliate_transfers
            .write()
            .expect("affiliate transfers lock")
            .push(transfer);
        json!({
            "transferred_quota": claim.amount,
            "balance": balance
        })
    }

    pub fn seed_affiliate_quota_for_tests(&self, user_id: i64, available_quota: f64) {
        let mut profiles = self
            .affiliates_by_user
            .write()
            .expect("affiliate profiles lock");
        let profile = profiles
            .entry(user_id)
            .or_insert_with(|| AffiliateProfile::default_for_user(user_id));
        profile.aff_quota = available_quota;
        profile.aff_history_quota = profile.aff_history_quota.max(available_quota);
    }

    fn ensure_affiliate_profile(&self, user_id: i64) -> AffiliateProfile {
        let mut profiles = self
            .affiliates_by_user
            .write()
            .expect("affiliate profiles lock");
        profiles
            .entry(user_id)
            .or_insert_with(|| AffiliateProfile::default_for_user(user_id))
            .clone()
    }

    pub fn send_email_response(&self) -> Value {
        json!({
            "message": "Verification code sent successfully"
        })
    }

    pub fn start_identity_binding(
        &self,
        request: StartIdentityBindingRequest,
    ) -> Result<Value, ApiError> {
        let provider = normalize_bindable_oauth_provider(&request.provider)
            .ok_or_else(|| ApiError::bad_request("invalid identity provider"))?;
        let redirect_to = request.redirect_to.unwrap_or_else(|| "/profile".to_owned());
        Ok(json!({
            "provider": provider,
            "authorize_url": format!("/api/v1/auth/oauth/{provider}/bind/start?intent=bind_current_user&redirect={redirect_to}"),
            "redirect_to": redirect_to,
            "method": "GET",
            "use_browser_redirect": true
        }))
    }

    pub fn totp_status(&self, user_id: i64) -> Value {
        let state = self
            .totp_by_user
            .read()
            .expect("totp lock")
            .get(&user_id)
            .cloned()
            .unwrap_or(TotpState {
                enabled: false,
                enabled_at: None,
                setup_token: None,
            });
        json!({
            "enabled": state.enabled,
            "enabled_at": state.enabled_at,
            "feature_enabled": true
        })
    }

    pub fn totp_verification_method(&self) -> Value {
        json!({ "method": "password" })
    }

    pub fn totp_send_code(&self) -> Value {
        json!({ "success": true })
    }

    pub fn totp_setup(&self, user_id: i64, _request: TotpSetupRequest) -> Value {
        let setup_token = format!("totp-setup-{}", Uuid::new_v4());
        let secret = "JBSWY3DPEHPK3PXP".to_owned();
        self.totp_by_user
            .write()
            .expect("totp lock")
            .entry(user_id)
            .and_modify(|state| state.setup_token = Some(setup_token.clone()))
            .or_insert(TotpState {
                enabled: false,
                enabled_at: None,
                setup_token: Some(setup_token.clone()),
            });
        json!({
            "secret": secret,
            "qr_code_url": format!("otpauth://totp/sub2api:user-{user_id}?secret={secret}&issuer=sub2api"),
            "setup_token": setup_token,
            "countdown": 300
        })
    }

    pub fn totp_enable(&self, user_id: i64, request: TotpEnableRequest) -> Result<Value, ApiError> {
        if request.totp_code.trim().len() != 6 {
            return Err(ApiError::bad_request("totp_code must be 6 digits"));
        }
        let mut states = self.totp_by_user.write().expect("totp lock");
        let state = states
            .get_mut(&user_id)
            .ok_or_else(|| ApiError::bad_request("totp setup is required"))?;
        if state.setup_token.as_deref() != Some(request.setup_token.as_str()) {
            return Err(ApiError::bad_request("invalid setup token"));
        }
        state.enabled = true;
        state.enabled_at = Some(1_780_704_000);
        state.setup_token = None;
        Ok(json!({ "success": true }))
    }

    pub fn totp_disable(&self, user_id: i64, _request: TotpDisableRequest) -> Value {
        self.totp_by_user.write().expect("totp lock").insert(
            user_id,
            TotpState {
                enabled: false,
                enabled_at: None,
                setup_token: None,
            },
        );
        json!({ "success": true })
    }

    pub fn available_groups(&self, groups: Vec<Group>) -> Value {
        json!(groups
            .into_iter()
            .map(group_to_available_group)
            .collect::<Vec<_>>())
    }

    pub fn group_rates(&self) -> Value {
        json!({})
    }

    pub fn available_channels(
        &self,
        groups: Vec<Group>,
        bindings_by_group: Vec<(i64, Vec<AccountGroupBinding>)>,
    ) -> Value {
        let groups_by_id = groups
            .into_iter()
            .map(|group| (group.id.0, group))
            .collect::<BTreeMap<_, _>>();
        let mut platform_groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut platform_models: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();

        for (group_id, bindings) in bindings_by_group {
            let Some(group) = groups_by_id.get(&group_id) else {
                continue;
            };
            let mut group_platforms = bindings
                .iter()
                .map(|binding| binding.account.provider.as_str().to_owned())
                .collect::<Vec<_>>();
            group_platforms.sort();
            group_platforms.dedup();

            for platform in group_platforms {
                platform_groups
                    .entry(platform.clone())
                    .or_default()
                    .push(group_to_channel_group(group, &platform));
            }

            for binding in bindings {
                let platform = binding.account.provider.as_str().to_owned();
                let models = platform_models.entry(platform.clone()).or_default();
                for rule in &binding.account.model_mapping {
                    models
                        .entry(rule.source.clone())
                        .or_insert_with(|| supported_model(&rule.source, &platform));
                }
                if binding.account.model_mapping.is_empty() {
                    let fallback = default_model_for_provider(binding.account.provider);
                    models
                        .entry(fallback.to_owned())
                        .or_insert_with(|| supported_model(fallback, &platform));
                }
            }
        }

        if platform_groups.is_empty() {
            return json!([]);
        }

        let platforms = platform_groups
            .into_iter()
            .map(|(platform, mut groups)| {
                groups.sort_by_key(|group| group["id"].as_i64().unwrap_or_default());
                groups.dedup_by_key(|group| group["id"].as_i64().unwrap_or_default());
                let supported_models = platform_models
                    .remove(&platform)
                    .map(|models| models.into_values().collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![supported_model(default_model_name(), &platform)]);

                json!({
                    "platform": platform,
                    "groups": groups,
                    "supported_models": supported_models
                })
            })
            .collect::<Vec<_>>();

        json!([
            {
                "name": "Repository",
                "description": "Active groups and accounts from repository",
                "platforms": platforms
            }
        ])
    }

    pub fn usage_list(&self) -> Value {
        json!({
            "items": [],
            "total": 0,
            "page": 1,
            "page_size": 20,
            "pages": 0
        })
    }

    pub fn usage_stats(&self) -> Value {
        json!({
            "period": "today",
            "total_requests": 0,
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_tokens": 0,
            "total_tokens": 0,
            "total_cost": 0.0,
            "total_actual_cost": 0.0,
            "average_duration_ms": 0,
            "models": {}
        })
    }

    pub fn dashboard_stats(&self) -> Value {
        json!({
            "total_api_keys": 0,
            "active_api_keys": 0,
            "total_requests": 0,
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_creation_tokens": 0,
            "total_cache_read_tokens": 0,
            "total_tokens": 0,
            "total_cost": 0.0,
            "total_actual_cost": 0.0,
            "today_requests": 0,
            "today_input_tokens": 0,
            "today_output_tokens": 0,
            "today_cache_creation_tokens": 0,
            "today_cache_read_tokens": 0,
            "today_tokens": 0,
            "today_cost": 0.0,
            "today_actual_cost": 0.0,
            "average_duration_ms": 0,
            "rpm": 0,
            "tpm": 0,
            "by_platform": []
        })
    }

    pub fn dashboard_trend(&self) -> Value {
        json!({
            "trend": [],
            "start_date": START_DATE,
            "end_date": END_DATE,
            "granularity": "day"
        })
    }

    pub fn dashboard_models(&self) -> Value {
        json!({
            "models": [],
            "start_date": START_DATE,
            "end_date": END_DATE
        })
    }

    pub fn dashboard_api_keys_usage(&self, request: BatchApiKeysUsageRequest) -> Value {
        let stats = request
            .api_key_ids
            .into_iter()
            .map(|id| {
                (
                    id.to_string(),
                    json!({
                        "api_key_id": id,
                        "today_actual_cost": 0.0,
                        "total_actual_cost": 0.0
                    }),
                )
            })
            .collect::<serde_json::Map<String, Value>>();
        json!({ "stats": stats })
    }

    pub fn api_key_daily_usage(&self, days: i64) -> Value {
        json!({
            "items": [],
            "days": days,
            "start_date": START_DATE,
            "end_date": END_DATE
        })
    }

    pub fn usage_by_id(&self, id: i64) -> Result<Value, ApiError> {
        Err(ApiError::not_found(format!("usage log {id} not found")))
    }

    pub fn announcements(&self) -> Value {
        json!([
            {
                "id": 1,
                "title": "Welcome",
                "content": "Welcome to sub2api.",
                "notify_mode": "none",
                "starts_at": null,
                "ends_at": null,
                "read_at": null,
                "created_at": NOW,
                "updated_at": NOW
            }
        ])
    }

    pub fn mark_announcement_read(&self, _id: i64) -> Value {
        json!({ "message": "Announcement marked as read" })
    }

    pub fn redeem_history(&self) -> Value {
        json!([])
    }

    pub fn subscriptions(&self) -> Value {
        json!([demo_subscription()])
    }

    pub fn active_subscriptions(&self) -> Value {
        self.subscriptions()
    }

    pub fn subscription_progress(&self) -> Value {
        json!([demo_subscription_progress()])
    }

    pub fn subscription_summary(&self) -> Value {
        json!({
            "active_count": 1,
            "total_used_usd": 0.0,
            "subscriptions": [
                {
                    "id": 1,
                    "group_id": 1,
                    "group_name": "Default OpenAI",
                    "status": "active",
                    "daily_progress": null,
                    "weekly_progress": null,
                    "monthly_progress": null,
                    "daily_used_usd": 0.0,
                    "weekly_used_usd": 0.0,
                    "monthly_used_usd": 0.0,
                    "expires_at": null,
                    "days_remaining": null
                }
            ]
        })
    }

    pub fn channel_monitors(&self) -> Value {
        json!({
            "items": [],
            "total": 0,
            "page": 1,
            "page_size": 20,
            "pages": 0
        })
    }

    pub fn channel_monitor_status(&self, id: i64) -> Value {
        json!({
            "id": id,
            "name": format!("monitor-{id}"),
            "status": "unknown",
            "last_checked_at": null,
            "latency_ms": null,
            "available": false
        })
    }

    pub fn platform_quotas(&self) -> Value {
        json!({
            "platform_quotas": []
        })
    }
}

fn normalize_bindable_oauth_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "linuxdo" => Some("linuxdo"),
        "oidc" => Some("oidc"),
        "wechat" => Some("wechat"),
        "dingtalk" => Some("dingtalk"),
        _ => None,
    }
}

fn demo_group() -> Value {
    json!({
        "id": 1,
        "name": "Default OpenAI",
        "description": "Default OpenAI-compatible group",
        "platform": "openai",
        "rate_multiplier": 1.0,
        "rpm_limit": 0,
        "is_exclusive": false,
        "status": "active",
        "subscription_type": "standard",
        "daily_limit_usd": null,
        "weekly_limit_usd": null,
        "monthly_limit_usd": null,
        "allow_image_generation": false,
        "image_rate_independent": false,
        "image_rate_multiplier": 0.0,
        "image_price_1k": null,
        "image_price_2k": null,
        "image_price_4k": null,
        "claude_code_only": false,
        "fallback_group_id": null,
        "fallback_group_id_on_invalid_request": null,
        "allow_messages_dispatch": false,
        "default_mapped_model": null,
        "messages_dispatch_model_config": null,
        "require_oauth_only": false,
        "require_privacy_set": false,
        "created_at": NOW,
        "updated_at": NOW
    })
}

fn group_to_available_group(group: Group) -> Value {
    json!({
        "id": group.id.0,
        "name": group.name,
        "description": "",
        "platform": "multi",
        "rate_multiplier": 1.0,
        "rpm_limit": 0,
        "is_exclusive": false,
        "status": "active",
        "subscription_type": "standard",
        "daily_limit_usd": null,
        "weekly_limit_usd": null,
        "monthly_limit_usd": null,
        "allow_image_generation": false,
        "image_rate_independent": false,
        "image_rate_multiplier": 0.0,
        "image_price_1k": null,
        "image_price_2k": null,
        "image_price_4k": null,
        "claude_code_only": false,
        "fallback_group_id": null,
        "fallback_group_id_on_invalid_request": null,
        "allow_messages_dispatch": false,
        "default_mapped_model": null,
        "messages_dispatch_model_config": null,
        "require_oauth_only": false,
        "require_privacy_set": false,
        "created_at": NOW,
        "updated_at": NOW
    })
}

fn group_to_channel_group(group: &Group, platform: &str) -> Value {
    json!({
        "id": group.id.0,
        "name": group.name,
        "platform": platform,
        "subscription_type": "standard",
        "rate_multiplier": 1.0,
        "is_exclusive": false
    })
}

fn supported_model(name: &str, platform: &str) -> Value {
    json!({
        "name": name,
        "platform": platform,
        "pricing": {
            "billing_mode": "token",
            "input_price": 0.0,
            "output_price": 0.0,
            "cache_write_price": 0.0,
            "cache_read_price": 0.0,
            "image_output_price": null,
            "per_request_price": null,
            "intervals": []
        }
    })
}

fn default_model_for_provider(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic | Provider::Antigravity => "claude-sonnet-4-5",
        Provider::Gemini | Provider::Vertex => "gemini-2.5-pro",
        Provider::DeepSeek => "deepseek-chat",
        Provider::OpenAi => default_model_name(),
    }
}

fn default_model_name() -> &'static str {
    "gpt-5.4"
}

fn demo_subscription() -> Value {
    json!({
        "id": 1,
        "user_id": 1,
        "group_id": 1,
        "status": "active",
        "starts_at": NOW,
        "daily_usage_usd": 0.0,
        "weekly_usage_usd": 0.0,
        "monthly_usage_usd": 0.0,
        "daily_window_start": null,
        "weekly_window_start": null,
        "monthly_window_start": null,
        "created_at": NOW,
        "updated_at": NOW,
        "expires_at": null,
        "group": demo_group()
    })
}

fn demo_subscription_progress() -> Value {
    json!({
        "subscription_id": 1,
        "daily": null,
        "weekly": null,
        "monthly": null,
        "expires_at": null,
        "days_remaining": null
    })
}
