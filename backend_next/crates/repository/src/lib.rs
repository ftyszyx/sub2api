use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use domain::{
    Account, AccountGroupBinding, AccountId, ApiKey, ApiKeyId, ApiKeyStatus, Group, GroupId,
    GroupStatus, ModelMappingRule, Provider, UpstreamProtocol,
};
use protocol::DownstreamProtocol;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    RwLock,
};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    #[error("{entity} {id} not found")]
    NotFound { entity: &'static str, id: i64 },

    #[error("duplicate {entity}: {key}")]
    Duplicate { entity: &'static str, key: String },

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("invalid repository input: {0}")]
    InvalidInput(String),

    #[error("database error: {0}")]
    Database(String),
}

pub type RepositoryResult<T> = Result<T, RepositoryError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRecord {
    pub id: i64,
    pub email: String,
    pub username: String,
    pub role: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: i64,
    pub user_id: i64,
    pub api_key_id: i64,
    pub group_id: Option<GroupId>,
    pub account_id: Option<AccountId>,
    pub downstream_protocol: DownstreamProtocol,
    pub upstream_protocol: String,
    pub provider: String,
    pub endpoint: String,
    pub requested_model: String,
    pub upstream_model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub actual_cost: f64,
    pub status: String,
    pub created_at_unix: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCleanupFilter {
    pub start_time_unix: i64,
    pub end_time_unix: i64,
    pub user_id: Option<i64>,
    pub api_key_id: Option<ApiKeyId>,
    pub group_id: Option<GroupId>,
    pub account_id: Option<AccountId>,
    pub model: Option<String>,
    pub request_type: Option<String>,
    pub stream: Option<bool>,
    pub billing_type: Option<i8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageCleanupTaskRecord {
    pub id: i64,
    pub status: String,
    pub filters: UsageCleanupFilter,
    pub created_by: i64,
    pub deleted_rows: i64,
    pub error_message: Option<String>,
    pub canceled_by: Option<i64>,
    pub canceled_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentOrderRecord {
    pub id: i64,
    pub user_id: i64,
    pub amount: f64,
    pub pay_amount: f64,
    pub currency: String,
    pub fee_rate: f64,
    pub payment_type: String,
    pub out_trade_no: String,
    pub status: String,
    pub order_type: String,
    pub refund_amount: f64,
    pub refund_reason: Option<String>,
    pub refund_request_reason: Option<String>,
    pub plan_id: Option<i64>,
    pub provider_instance_id: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub paid_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub refund_requested_at: Option<String>,
    pub refunded_at: Option<String>,
    pub webhook_count: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentAuditRecord {
    pub id: i64,
    pub order_id: String,
    pub action: String,
    pub detail: String,
    pub operator: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserBalanceRecord {
    pub user_id: i64,
    pub balance: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceTransactionRecord {
    pub id: i64,
    pub user_id: i64,
    pub order_id: String,
    pub transaction_type: String,
    pub amount: f64,
    pub balance_after: f64,
    pub created_at: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSubscriptionRecord {
    pub id: i64,
    pub user_id: i64,
    pub group_id: i64,
    pub plan_id: Option<i64>,
    pub status: String,
    pub starts_at: String,
    pub expires_at: String,
    pub daily_usage_usd: f64,
    pub weekly_usage_usd: f64,
    pub monthly_usage_usd: f64,
    pub daily_window_start: Option<String>,
    pub weekly_window_start: Option<String>,
    pub monthly_window_start: Option<String>,
    pub source_order_id: String,
    pub created_at: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPlatformQuotaRecord {
    pub id: i64,
    pub user_id: i64,
    pub platform: String,
    pub daily_limit_usd: Option<f64>,
    pub weekly_limit_usd: Option<f64>,
    pub monthly_limit_usd: Option<f64>,
    pub daily_usage_usd: f64,
    pub weekly_usage_usd: f64,
    pub monthly_usage_usd: f64,
    pub daily_window_start: Option<String>,
    pub weekly_window_start: Option<String>,
    pub monthly_window_start: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserGroupRateRecord {
    pub user_id: i64,
    pub group_id: i64,
    pub rate_multiplier: Option<f64>,
    pub rpm_override: Option<i32>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAttributeValueRecord {
    pub user_id: i64,
    pub attribute_id: i64,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMonitorHistoryRecord {
    pub id: i64,
    pub monitor_id: i64,
    pub model: String,
    pub status: String,
    pub latency_ms: Option<i64>,
    pub ping_latency_ms: Option<i64>,
    pub message: String,
    pub checked_at: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentModerationLogRecord {
    pub id: i64,
    pub request_id: String,
    pub user_id: Option<i64>,
    pub user_email: String,
    pub api_key_id: Option<i64>,
    pub api_key_name: String,
    pub group_id: Option<i64>,
    pub group_name: String,
    pub endpoint: String,
    pub provider: String,
    pub model: String,
    pub mode: String,
    pub action: String,
    pub flagged: bool,
    pub highest_category: String,
    pub highest_score: f64,
    pub category_scores: Value,
    pub threshold_snapshot: Value,
    pub input_excerpt: String,
    pub upstream_latency_ms: Option<i64>,
    pub error: String,
    pub violation_count: i64,
    pub auto_banned: bool,
    pub email_sent: bool,
    pub user_status: String,
    pub queue_delay_ms: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockLeaseRecord {
    pub name: String,
    pub owner: String,
    pub fencing_token: i64,
    pub expires_at: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdempotentJobRecord {
    pub id: i64,
    pub job_type: String,
    pub idempotency_key: String,
    pub status: String,
    pub payload: Value,
    pub result: Option<Value>,
    pub attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdempotentJobCreateResult {
    pub job: IdempotentJobRecord,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountConcurrencySlotRecord {
    pub account_id: i64,
    pub request_id: String,
    pub expires_at: String,
    pub metadata: Value,
    pub in_flight: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConcurrencySnapshotRecord {
    pub account_id: i64,
    pub in_flight: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub scope: String,
    pub count: i64,
    pub limit: i64,
    pub remaining: i64,
    pub window_start_unix: i64,
    pub window_seconds: i64,
    pub reset_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitUsageRecord {
    pub scope: String,
    pub usage: f64,
    pub limit: f64,
    pub remaining: f64,
    pub window_start_unix: i64,
    pub window_seconds: i64,
    pub reset_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentProviderInstanceRecord {
    pub id: i64,
    pub provider_key: String,
    pub name: String,
    pub config: Value,
    pub supported_types: Vec<String>,
    pub enabled: bool,
    pub payment_mode: String,
    pub sort_order: i32,
    pub limits: Value,
    pub refund_enabled: bool,
    pub allow_user_refund: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentPlanRecord {
    pub id: i64,
    pub group_id: i64,
    pub name: String,
    pub description: String,
    pub price: f64,
    pub original_price: Option<f64>,
    pub validity_days: i32,
    pub validity_unit: String,
    pub features: Value,
    pub product_name: String,
    pub for_sale: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminCollectionItemRecord {
    pub collection: String,
    pub id: i64,
    pub item: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSettingRecord {
    pub namespace: String,
    pub key: String,
    pub value: Value,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailQueueTaskRecord {
    pub id: i64,
    pub task_type: String,
    pub status: String,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthIdentityRecord {
    pub id: i64,
    pub user_id: i64,
    pub provider: String,
    pub provider_key: String,
    pub provider_subject: String,
    pub email: Option<String>,
    pub bound_at_unix: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthSessionRecord {
    pub id: i64,
    pub user_id: i64,
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at_unix: i64,
    pub refresh_expires_at_unix: i64,
    pub revoked_at_unix: Option<i64>,
    pub created_at_unix: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthCredentialRecord {
    pub user_id: i64,
    pub email: String,
    pub password_hash: String,
    pub status: String,
    pub updated_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFilter {
    pub user_id: Option<i64>,
    pub api_key_id: Option<ApiKeyId>,
    pub group_id: Option<GroupId>,
    pub account_id: Option<AccountId>,
    pub downstream_protocol: Option<DownstreamProtocol>,
    pub model_contains: Option<String>,
    pub request_type: Option<String>,
    pub stream: Option<bool>,
    pub status: Option<String>,
    pub billing_mode: Option<String>,
    pub billing_type: Option<i8>,
    pub created_at_unix_gte: Option<i64>,
    pub created_at_unix_lt: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    pub page: i64,
    pub page_size: i64,
}

impl Pagination {
    pub fn new(page: i64, page_size: i64) -> Self {
        Self {
            page: page.max(1),
            page_size: page_size.clamp(1, 200),
        }
    }

    pub fn offset(self) -> i64 {
        (self.page - 1) * self.page_size
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaginatedRecords<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentModerationLogFilter {
    pub result: Option<String>,
    pub group_id: Option<i64>,
    pub endpoint: Option<String>,
    pub search: Option<String>,
    pub created_at_gte: Option<String>,
    pub created_at_lte: Option<String>,
}

impl ContentModerationLogFilter {
    pub fn all() -> Self {
        Self {
            result: None,
            group_id: None,
            endpoint: None,
            search: None,
            created_at_gte: None,
            created_at_lte: None,
        }
    }

    fn matches(&self, record: &ContentModerationLogRecord) -> bool {
        content_moderation_result_matches(self.result.as_deref(), record)
            && self
                .group_id
                .map(|expected| record.group_id == Some(expected))
                .unwrap_or(true)
            && self
                .endpoint
                .as_ref()
                .map(|expected| record.endpoint == *expected)
                .unwrap_or(true)
            && self
                .search
                .as_ref()
                .map(|needle| {
                    let needle = needle.to_ascii_lowercase();
                    [
                        record.request_id.as_str(),
                        record.user_email.as_str(),
                        record.api_key_name.as_str(),
                        record.model.as_str(),
                        record.input_excerpt.as_str(),
                    ]
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&needle))
                })
                .unwrap_or(true)
            && self
                .created_at_gte
                .as_ref()
                .map(|expected| record.created_at.as_str() >= expected.as_str())
                .unwrap_or(true)
            && self
                .created_at_lte
                .as_ref()
                .map(|expected| record.created_at.as_str() <= expected.as_str())
                .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentOrderFilter {
    pub user_id: Option<i64>,
    pub status: Option<String>,
    pub payment_type: Option<String>,
    pub provider_instance_id: Option<String>,
    pub out_trade_no_contains: Option<String>,
}

impl PaymentOrderFilter {
    pub fn all() -> Self {
        Self {
            user_id: None,
            status: None,
            payment_type: None,
            provider_instance_id: None,
            out_trade_no_contains: None,
        }
    }

    fn matches(&self, record: &PaymentOrderRecord) -> bool {
        self.user_id
            .map(|expected| record.user_id == expected)
            .unwrap_or(true)
            && self
                .status
                .as_ref()
                .map(|expected| record.status.eq_ignore_ascii_case(expected))
                .unwrap_or(true)
            && self
                .payment_type
                .as_ref()
                .map(|expected| record.payment_type.eq_ignore_ascii_case(expected))
                .unwrap_or(true)
            && self
                .provider_instance_id
                .as_ref()
                .map(|expected| record.provider_instance_id.as_ref() == Some(expected))
                .unwrap_or(true)
            && self
                .out_trade_no_contains
                .as_ref()
                .map(|needle| {
                    record
                        .out_trade_no
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
                .unwrap_or(true)
    }
}

impl UsageFilter {
    pub fn all() -> Self {
        Self {
            user_id: None,
            api_key_id: None,
            group_id: None,
            account_id: None,
            downstream_protocol: None,
            model_contains: None,
            request_type: None,
            stream: None,
            status: None,
            billing_mode: None,
            billing_type: None,
            created_at_unix_gte: None,
            created_at_unix_lt: None,
        }
    }

    fn matches(&self, record: &UsageRecord) -> bool {
        let model_matches = self.model_contains.as_ref().map(|needle| {
            let needle = needle.to_ascii_lowercase();
            record
                .requested_model
                .to_ascii_lowercase()
                .contains(&needle)
                || record.upstream_model.to_ascii_lowercase().contains(&needle)
                || record
                    .metadata
                    .get("model_mapping_chain")
                    .map(|value| value.to_string().to_ascii_lowercase().contains(&needle))
                    .unwrap_or(false)
        });
        self.user_id
            .map(|expected| record.user_id == expected)
            .unwrap_or(true)
            && self
                .api_key_id
                .map(|expected| record.api_key_id == expected.0)
                .unwrap_or(true)
            && self
                .group_id
                .map(|expected| record.group_id == Some(expected))
                .unwrap_or(true)
            && self
                .account_id
                .map(|expected| record.account_id == Some(expected))
                .unwrap_or(true)
            && self
                .downstream_protocol
                .map(|expected| record.downstream_protocol == expected)
                .unwrap_or(true)
            && model_matches.unwrap_or(true)
            && self
                .request_type
                .as_ref()
                .map(|expected| {
                    record
                        .metadata
                        .get("request_type")
                        .and_then(Value::as_str)
                        .map(|actual| actual.eq_ignore_ascii_case(expected))
                        .unwrap_or(false)
                })
                .unwrap_or(true)
            && self
                .stream
                .map(|expected| {
                    record
                        .metadata
                        .get("stream")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        == expected
                })
                .unwrap_or(true)
            && self
                .status
                .as_ref()
                .map(|expected| record.status.eq_ignore_ascii_case(expected))
                .unwrap_or(true)
            && self
                .billing_mode
                .as_ref()
                .map(|expected| {
                    record
                        .metadata
                        .get("billing_mode")
                        .and_then(Value::as_str)
                        .map(|actual| actual.eq_ignore_ascii_case(expected))
                        .unwrap_or(false)
                })
                .unwrap_or(true)
            && self
                .billing_type
                .map(|expected| {
                    record
                        .metadata
                        .get("billing_type")
                        .and_then(Value::as_i64)
                        .map(|actual| actual == i64::from(expected))
                        .unwrap_or(false)
                })
                .unwrap_or(true)
            && self
                .created_at_unix_gte
                .map(|expected| record.created_at_unix >= expected)
                .unwrap_or(true)
            && self
                .created_at_unix_lt
                .map(|expected| record.created_at_unix < expected)
                .unwrap_or(true)
    }
}

impl UsageCleanupFilter {
    pub fn to_usage_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            api_key_id: self.api_key_id,
            group_id: self.group_id,
            account_id: self.account_id,
            downstream_protocol: None,
            model_contains: self.model.clone(),
            request_type: self.request_type.clone(),
            stream: self.stream,
            status: None,
            billing_mode: None,
            billing_type: self.billing_type,
            created_at_unix_gte: Some(self.start_time_unix),
            created_at_unix_lt: Some(self.end_time_unix),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageSummary {
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub actual_cost: f64,
}

impl UsageSummary {
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }

    fn add_record(&mut self, record: &UsageRecord) {
        self.requests += 1;
        self.input_tokens += record.input_tokens;
        self.output_tokens += record.output_tokens;
        self.cache_creation_tokens += record.cache_creation_tokens;
        self.cache_read_tokens += record.cache_read_tokens;
        self.actual_cost += record.actual_cost;
    }
}

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn upsert_user(&self, user: UserRecord) -> RepositoryResult<UserRecord>;
    async fn get_user(&self, id: i64) -> RepositoryResult<UserRecord>;
    async fn get_user_by_email(&self, email: &str) -> RepositoryResult<UserRecord>;
    async fn list_users(&self) -> RepositoryResult<Vec<UserRecord>>;
    async fn delete_user(&self, id: i64) -> RepositoryResult<()>;
}

#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    async fn upsert_api_key(&self, api_key: ApiKey) -> RepositoryResult<ApiKey>;
    async fn get_api_key(&self, id: ApiKeyId) -> RepositoryResult<ApiKey>;
    async fn get_api_key_by_key(&self, key: &str) -> RepositoryResult<ApiKey>;
    async fn list_api_keys_by_user(&self, user_id: i64) -> RepositoryResult<Vec<ApiKey>>;
    async fn list_api_keys_by_group(&self, group_id: GroupId) -> RepositoryResult<Vec<ApiKey>>;
    async fn delete_api_key(&self, id: ApiKeyId) -> RepositoryResult<()>;
}

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn upsert_group(&self, group: Group) -> RepositoryResult<Group>;
    async fn get_group(&self, id: GroupId) -> RepositoryResult<Group>;
    async fn list_groups(&self) -> RepositoryResult<Vec<Group>>;
    async fn list_active_groups(&self) -> RepositoryResult<Vec<Group>>;
    async fn delete_group(&self, id: GroupId) -> RepositoryResult<()>;
}

#[async_trait]
pub trait AccountRepository: Send + Sync {
    async fn upsert_account(&self, account: Account) -> RepositoryResult<Account>;
    async fn get_account(&self, id: AccountId) -> RepositoryResult<Account>;
    async fn list_accounts(&self) -> RepositoryResult<Vec<Account>>;
    async fn delete_account(&self, id: AccountId) -> RepositoryResult<()>;
    async fn bind_account_to_group(
        &self,
        binding: AccountGroupBinding,
    ) -> RepositoryResult<AccountGroupBinding>;
    async fn list_bindings_by_group(
        &self,
        group_id: GroupId,
    ) -> RepositoryResult<Vec<AccountGroupBinding>>;
}

#[async_trait]
pub trait UsageRepository: Send + Sync {
    async fn insert_usage(&self, record: UsageRecord) -> RepositoryResult<UsageRecord>;
    async fn list_usage(&self, filter: UsageFilter) -> RepositoryResult<Vec<UsageRecord>>;
    async fn summarize_usage(&self, filter: UsageFilter) -> RepositoryResult<UsageSummary>;
    async fn create_usage_cleanup_task(
        &self,
        task: UsageCleanupTaskRecord,
    ) -> RepositoryResult<UsageCleanupTaskRecord>;
    async fn list_usage_cleanup_tasks(
        &self,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<UsageCleanupTaskRecord>>;
    async fn get_usage_cleanup_task_status(&self, task_id: i64) -> RepositoryResult<String>;
    async fn claim_next_usage_cleanup_task(
        &self,
        stale_running_after_seconds: i64,
    ) -> RepositoryResult<Option<UsageCleanupTaskRecord>>;
    async fn update_usage_cleanup_task_progress(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<()>;
    async fn cancel_usage_cleanup_task(
        &self,
        task_id: i64,
        canceled_by: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord>;
    async fn mark_usage_cleanup_task_succeeded(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord>;
    async fn mark_usage_cleanup_task_failed(
        &self,
        task_id: i64,
        deleted_rows: i64,
        error_message: String,
    ) -> RepositoryResult<UsageCleanupTaskRecord>;
    async fn delete_usage_batch(
        &self,
        filter: UsageCleanupFilter,
        limit: i64,
    ) -> RepositoryResult<i64>;
}

#[async_trait]
pub trait PaymentOrderRepository: Send + Sync {
    async fn upsert_payment_order(
        &self,
        order: PaymentOrderRecord,
    ) -> RepositoryResult<PaymentOrderRecord>;
    async fn get_payment_order(&self, id: i64) -> RepositoryResult<PaymentOrderRecord>;
    async fn get_payment_order_by_trade_no(
        &self,
        out_trade_no: &str,
    ) -> RepositoryResult<PaymentOrderRecord>;
    async fn list_payment_orders(
        &self,
        filter: PaymentOrderFilter,
    ) -> RepositoryResult<Vec<PaymentOrderRecord>>;
}

#[async_trait]
pub trait PaymentAuditRepository: Send + Sync {
    async fn insert_payment_audit(
        &self,
        record: PaymentAuditRecord,
    ) -> RepositoryResult<PaymentAuditRecord>;
    async fn list_payment_audits(
        &self,
        order_id: &str,
    ) -> RepositoryResult<Vec<PaymentAuditRecord>>;
}

#[async_trait]
pub trait BalanceRepository: Send + Sync {
    async fn get_user_balance(&self, user_id: i64) -> RepositoryResult<UserBalanceRecord>;
    async fn apply_balance_transaction(
        &self,
        record: BalanceTransactionRecord,
    ) -> RepositoryResult<BalanceTransactionRecord>;
    async fn list_balance_transactions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<BalanceTransactionRecord>>;
}

#[async_trait]
pub trait SubscriptionRepository: Send + Sync {
    async fn upsert_user_subscription(
        &self,
        record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord>;
    async fn get_user_subscription(&self, id: i64) -> RepositoryResult<UserSubscriptionRecord>;
    async fn update_user_subscription(
        &self,
        record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord>;
    async fn delete_user_subscription(&self, id: i64) -> RepositoryResult<()>;
    async fn update_subscription_status_by_source_order(
        &self,
        source_order_id: &str,
        status: &str,
        metadata: Value,
    ) -> RepositoryResult<UserSubscriptionRecord>;
    async fn get_subscription_by_source_order(
        &self,
        source_order_id: &str,
    ) -> RepositoryResult<UserSubscriptionRecord>;
    async fn list_user_subscriptions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserSubscriptionRecord>>;
    async fn list_subscriptions(&self) -> RepositoryResult<Vec<UserSubscriptionRecord>>;
}

#[async_trait]
pub trait UserPlatformQuotaRepository: Send + Sync {
    async fn list_user_platform_quotas(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>>;
    async fn replace_user_platform_quotas(
        &self,
        user_id: i64,
        records: Vec<UserPlatformQuotaRecord>,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>>;
    async fn increment_user_platform_quota_usage(
        &self,
        user_id: i64,
        platform: &str,
        cost: f64,
        daily_window_start: String,
        weekly_window_start: String,
        monthly_window_start: String,
    ) -> RepositoryResult<UserPlatformQuotaRecord>;
}

#[async_trait]
pub trait UserGroupRateRepository: Send + Sync {
    async fn list_user_group_rates(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>>;
    async fn list_group_rate_overrides(
        &self,
        group_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>>;
    async fn replace_group_rate_multipliers(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>>;
    async fn clear_group_rate_multipliers(&self, group_id: i64) -> RepositoryResult<()>;
    async fn replace_group_rpm_overrides(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>>;
    async fn clear_group_rpm_overrides(&self, group_id: i64) -> RepositoryResult<()>;
}

#[async_trait]
pub trait UserAttributeValueRepository: Send + Sync {
    async fn list_user_attribute_values(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>>;
    async fn replace_user_attribute_values(
        &self,
        user_id: i64,
        records: Vec<UserAttributeValueRecord>,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>>;
}

#[async_trait]
pub trait ChannelMonitorHistoryRepository: Send + Sync {
    async fn insert_channel_monitor_history(
        &self,
        record: ChannelMonitorHistoryRecord,
    ) -> RepositoryResult<ChannelMonitorHistoryRecord>;
    async fn list_channel_monitor_history(
        &self,
        monitor_id: i64,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ChannelMonitorHistoryRecord>>;
}

#[async_trait]
pub trait ContentModerationLogRepository: Send + Sync {
    async fn insert_content_moderation_log(
        &self,
        record: ContentModerationLogRecord,
    ) -> RepositoryResult<ContentModerationLogRecord>;
    async fn list_content_moderation_logs(
        &self,
        filter: ContentModerationLogFilter,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ContentModerationLogRecord>>;
}

#[async_trait]
pub trait PaymentProviderRepository: Send + Sync {
    async fn upsert_payment_provider_instance(
        &self,
        record: PaymentProviderInstanceRecord,
    ) -> RepositoryResult<PaymentProviderInstanceRecord>;
    async fn get_payment_provider_instance(
        &self,
        id: i64,
    ) -> RepositoryResult<PaymentProviderInstanceRecord>;
    async fn list_payment_provider_instances(
        &self,
    ) -> RepositoryResult<Vec<PaymentProviderInstanceRecord>>;
    async fn delete_payment_provider_instance(&self, id: i64) -> RepositoryResult<()>;
}

#[async_trait]
pub trait PaymentPlanRepository: Send + Sync {
    async fn upsert_payment_plan(
        &self,
        record: PaymentPlanRecord,
    ) -> RepositoryResult<PaymentPlanRecord>;
    async fn get_payment_plan(&self, id: i64) -> RepositoryResult<PaymentPlanRecord>;
    async fn list_payment_plans(&self) -> RepositoryResult<Vec<PaymentPlanRecord>>;
    async fn list_payment_plans_for_sale(&self) -> RepositoryResult<Vec<PaymentPlanRecord>>;
    async fn delete_payment_plan(&self, id: i64) -> RepositoryResult<()>;
}

#[async_trait]
pub trait AdminCollectionRepository: Send + Sync {
    async fn upsert_admin_collection_item(
        &self,
        record: AdminCollectionItemRecord,
    ) -> RepositoryResult<AdminCollectionItemRecord>;
    async fn get_admin_collection_item(
        &self,
        collection: &str,
        id: i64,
    ) -> RepositoryResult<AdminCollectionItemRecord>;
    async fn list_admin_collection_items(
        &self,
        collection: &str,
    ) -> RepositoryResult<Vec<AdminCollectionItemRecord>>;
    async fn delete_admin_collection_item(&self, collection: &str, id: i64)
        -> RepositoryResult<()>;
}

#[async_trait]
pub trait SystemSettingRepository: Send + Sync {
    async fn upsert_system_setting(
        &self,
        record: SystemSettingRecord,
    ) -> RepositoryResult<SystemSettingRecord>;
    async fn get_system_setting(
        &self,
        namespace: &str,
        key: &str,
    ) -> RepositoryResult<SystemSettingRecord>;
    async fn list_system_settings(
        &self,
        namespace: &str,
    ) -> RepositoryResult<Vec<SystemSettingRecord>>;
    async fn delete_system_setting(&self, namespace: &str, key: &str) -> RepositoryResult<()>;
}

#[async_trait]
pub trait EmailQueueTaskRepository: Send + Sync {
    async fn enqueue_email_task(
        &self,
        record: EmailQueueTaskRecord,
    ) -> RepositoryResult<EmailQueueTaskRecord>;
    async fn list_pending_email_tasks(
        &self,
        limit: i64,
    ) -> RepositoryResult<Vec<EmailQueueTaskRecord>>;
    async fn mark_email_task_processing(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord>;
    async fn mark_email_task_sent(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord>;
    async fn mark_email_task_failed(
        &self,
        id: i64,
        last_error: String,
    ) -> RepositoryResult<EmailQueueTaskRecord>;
}

#[async_trait]
pub trait OAuthIdentityRepository: Send + Sync {
    async fn upsert_oauth_identity(
        &self,
        record: OAuthIdentityRecord,
    ) -> RepositoryResult<OAuthIdentityRecord>;
    async fn get_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<OAuthIdentityRecord>;
    async fn list_oauth_identities_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<OAuthIdentityRecord>>;
    async fn delete_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<()>;
}

#[async_trait]
pub trait AuthSessionRepository: Send + Sync {
    async fn upsert_auth_session(
        &self,
        record: AuthSessionRecord,
    ) -> RepositoryResult<AuthSessionRecord>;
    async fn get_auth_session_by_access_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord>;
    async fn get_auth_session_by_refresh_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord>;
    async fn list_auth_sessions_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<AuthSessionRecord>>;
    async fn revoke_auth_session_by_refresh_token(
        &self,
        token: &str,
        revoked_at_unix: i64,
    ) -> RepositoryResult<AuthSessionRecord>;
    async fn revoke_auth_sessions_by_user(
        &self,
        user_id: i64,
        revoked_at_unix: i64,
    ) -> RepositoryResult<i64>;
}

#[async_trait]
pub trait AuthCredentialRepository: Send + Sync {
    async fn upsert_auth_credential(
        &self,
        record: AuthCredentialRecord,
    ) -> RepositoryResult<AuthCredentialRecord>;
    async fn get_auth_credential_by_email(
        &self,
        email: &str,
    ) -> RepositoryResult<AuthCredentialRecord>;
    async fn get_auth_credential_by_user_id(
        &self,
        user_id: i64,
    ) -> RepositoryResult<AuthCredentialRecord>;
    async fn delete_auth_credential(&self, user_id: i64) -> RepositoryResult<()>;
}

#[async_trait]
pub trait ConsistencyRepository: Send + Sync {
    fn uses_shared_consistency_backend(&self) -> bool {
        false
    }

    async fn try_acquire_lock(
        &self,
        name: &str,
        owner: &str,
        ttl_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<LockLeaseRecord>>;
    async fn renew_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
        ttl_seconds: i64,
    ) -> RepositoryResult<bool>;
    async fn release_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
    ) -> RepositoryResult<bool>;
    async fn create_idempotent_job(
        &self,
        job_type: &str,
        idempotency_key: &str,
        payload: Value,
    ) -> RepositoryResult<IdempotentJobCreateResult>;
    async fn claim_next_idempotent_job(
        &self,
        job_type: &str,
        owner: &str,
        lease_seconds: i64,
    ) -> RepositoryResult<Option<IdempotentJobRecord>>;
    async fn complete_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        result: Value,
    ) -> RepositoryResult<IdempotentJobRecord>;
    async fn fail_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        error: String,
    ) -> RepositoryResult<IdempotentJobRecord>;
    async fn acquire_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
        max_concurrent: i64,
        lease_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<AccountConcurrencySlotRecord>>;
    async fn release_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
    ) -> RepositoryResult<bool>;
    async fn list_account_concurrency_snapshots(
        &self,
    ) -> RepositoryResult<Vec<AccountConcurrencySnapshotRecord>>;
    async fn hit_rate_limit_fixed_window(
        &self,
        scope: &str,
        limit: i64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitDecision>;
    async fn get_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord>;
    async fn add_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        amount: f64,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord>;
}

pub trait AppRepository:
    UserRepository
    + ApiKeyRepository
    + GroupRepository
    + AccountRepository
    + UsageRepository
    + PaymentOrderRepository
    + PaymentAuditRepository
    + BalanceRepository
    + SubscriptionRepository
    + UserPlatformQuotaRepository
    + UserGroupRateRepository
    + UserAttributeValueRepository
    + ChannelMonitorHistoryRepository
    + ContentModerationLogRepository
    + PaymentProviderRepository
    + PaymentPlanRepository
    + AdminCollectionRepository
    + SystemSettingRepository
    + EmailQueueTaskRepository
    + OAuthIdentityRepository
    + AuthSessionRepository
    + AuthCredentialRepository
    + ConsistencyRepository
{
}

impl<T> AppRepository for T where
    T: UserRepository
        + ApiKeyRepository
        + GroupRepository
        + AccountRepository
        + UsageRepository
        + PaymentOrderRepository
        + PaymentAuditRepository
        + BalanceRepository
        + SubscriptionRepository
        + UserPlatformQuotaRepository
        + UserGroupRateRepository
        + UserAttributeValueRepository
        + ChannelMonitorHistoryRepository
        + ContentModerationLogRepository
        + PaymentProviderRepository
        + PaymentPlanRepository
        + AdminCollectionRepository
        + SystemSettingRepository
        + EmailQueueTaskRepository
        + OAuthIdentityRepository
        + AuthSessionRepository
        + AuthCredentialRepository
        + ConsistencyRepository
{
}

#[derive(Default)]
pub struct InMemoryRepository {
    next_usage_id: AtomicI64,
    next_payment_order_id: AtomicI64,
    next_payment_audit_id: AtomicI64,
    next_balance_transaction_id: AtomicI64,
    next_subscription_id: AtomicI64,
    next_payment_provider_instance_id: AtomicI64,
    next_payment_plan_id: AtomicI64,
    next_usage_cleanup_task_id: AtomicI64,
    next_channel_monitor_history_id: AtomicI64,
    next_content_moderation_log_id: AtomicI64,
    next_email_queue_task_id: AtomicI64,
    next_oauth_identity_id: AtomicI64,
    next_auth_session_id: AtomicI64,
    next_idempotent_job_id: AtomicI64,
    users: RwLock<HashMap<i64, UserRecord>>,
    api_keys: RwLock<HashMap<ApiKeyId, ApiKey>>,
    groups: RwLock<HashMap<GroupId, Group>>,
    accounts: RwLock<HashMap<AccountId, Account>>,
    bindings: RwLock<HashMap<GroupId, Vec<AccountGroupBinding>>>,
    usage: RwLock<Vec<UsageRecord>>,
    usage_cleanup_tasks: RwLock<HashMap<i64, UsageCleanupTaskRecord>>,
    payment_orders: RwLock<HashMap<i64, PaymentOrderRecord>>,
    payment_order_id_by_trade_no: RwLock<HashMap<String, i64>>,
    payment_audits: RwLock<Vec<PaymentAuditRecord>>,
    user_balances: RwLock<HashMap<i64, UserBalanceRecord>>,
    balance_transactions: RwLock<Vec<BalanceTransactionRecord>>,
    user_subscriptions: RwLock<HashMap<i64, UserSubscriptionRecord>>,
    subscription_id_by_source_order: RwLock<HashMap<String, i64>>,
    user_platform_quotas: RwLock<HashMap<(i64, String), UserPlatformQuotaRecord>>,
    user_group_rates: RwLock<HashMap<(i64, i64), UserGroupRateRecord>>,
    user_attribute_values: RwLock<HashMap<(i64, i64), UserAttributeValueRecord>>,
    channel_monitor_history: RwLock<HashMap<i64, ChannelMonitorHistoryRecord>>,
    content_moderation_logs: RwLock<HashMap<i64, ContentModerationLogRecord>>,
    payment_provider_instances: RwLock<HashMap<i64, PaymentProviderInstanceRecord>>,
    payment_plans: RwLock<HashMap<i64, PaymentPlanRecord>>,
    admin_collection_items: RwLock<HashMap<String, HashMap<i64, AdminCollectionItemRecord>>>,
    system_settings: RwLock<HashMap<(String, String), SystemSettingRecord>>,
    email_queue_tasks: RwLock<HashMap<i64, EmailQueueTaskRecord>>,
    oauth_identities: RwLock<HashMap<String, OAuthIdentityRecord>>,
    auth_sessions: RwLock<HashMap<i64, AuthSessionRecord>>,
    auth_session_id_by_access_token: RwLock<HashMap<String, i64>>,
    auth_session_id_by_refresh_token: RwLock<HashMap<String, i64>>,
    auth_credentials: RwLock<HashMap<i64, AuthCredentialRecord>>,
    auth_credential_user_id_by_email: RwLock<HashMap<String, i64>>,
    distributed_locks: RwLock<HashMap<String, LockLeaseRecord>>,
    idempotent_jobs: RwLock<HashMap<i64, IdempotentJobRecord>>,
    idempotent_job_id_by_key: RwLock<HashMap<(String, String), i64>>,
    account_concurrency_slots: RwLock<HashMap<(i64, String), AccountConcurrencySlotRecord>>,
    rate_limit_counters: RwLock<HashMap<(String, i64), RateLimitDecision>>,
    rate_limit_usage_counters: RwLock<HashMap<(String, i64), RateLimitUsageRecord>>,
}

impl InMemoryRepository {
    pub fn new() -> Self {
        Self {
            next_usage_id: AtomicI64::new(1),
            next_payment_order_id: AtomicI64::new(1),
            next_payment_audit_id: AtomicI64::new(1),
            next_balance_transaction_id: AtomicI64::new(1),
            next_subscription_id: AtomicI64::new(1),
            next_payment_provider_instance_id: AtomicI64::new(1),
            next_payment_plan_id: AtomicI64::new(1),
            next_usage_cleanup_task_id: AtomicI64::new(1),
            next_channel_monitor_history_id: AtomicI64::new(1),
            next_content_moderation_log_id: AtomicI64::new(1),
            next_email_queue_task_id: AtomicI64::new(1),
            next_oauth_identity_id: AtomicI64::new(1),
            next_auth_session_id: AtomicI64::new(1),
            next_idempotent_job_id: AtomicI64::new(1),
            ..Self::default()
        }
    }

    pub fn seed_gateway_principals(
        &self,
        user: UserRecord,
        group: Group,
        api_key: ApiKey,
        credential: Option<AuthCredentialRecord>,
    ) {
        self.users
            .write()
            .expect("user repository lock")
            .insert(user.id, user.clone());
        self.groups
            .write()
            .expect("group repository lock")
            .insert(group.id, group);
        self.api_keys
            .write()
            .expect("api key repository lock")
            .insert(api_key.id, api_key);
        if let Some(mut credential) = credential {
            if normalize_auth_credential_record(&mut credential).is_ok() {
                self.auth_credential_user_id_by_email
                    .write()
                    .expect("auth credential email index lock")
                    .insert(credential.email.clone(), credential.user_id);
                self.auth_credentials
                    .write()
                    .expect("auth credential repository lock")
                    .insert(credential.user_id, credential);
            }
        }
    }
}

#[derive(Clone)]
pub struct PostgresRepository {
    pool: sqlx::PgPool,
}

impl PostgresRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str) -> RepositoryResult<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(db_error)?;
        let repository = Self::new(pool);
        repository.run_migrations().await?;
        Ok(repository)
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    pub async fn run_migrations(&self) -> RepositoryResult<()> {
        for statement in core_gateway_migration_sql()
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    }
}

pub fn core_gateway_migration_sql() -> &'static str {
    include_str!("../../../migrations/0001_core_gateway_schema.sql")
}

#[async_trait]
impl UserRepository for InMemoryRepository {
    async fn upsert_user(&self, user: UserRecord) -> RepositoryResult<UserRecord> {
        if user.email.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "email is required".to_owned(),
            ));
        }
        self.users
            .write()
            .expect("user repository lock")
            .insert(user.id, user.clone());
        Ok(user)
    }

    async fn get_user(&self, id: i64) -> RepositoryResult<UserRecord> {
        self.users
            .read()
            .expect("user repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound { entity: "user", id })
    }

    async fn get_user_by_email(&self, email: &str) -> RepositoryResult<UserRecord> {
        self.users
            .read()
            .expect("user repository lock")
            .values()
            .find(|user| user.email.eq_ignore_ascii_case(email))
            .cloned()
            .ok_or_else(|| RepositoryError::Duplicate {
                entity: "user email lookup",
                key: email.to_owned(),
            })
    }

    async fn list_users(&self) -> RepositoryResult<Vec<UserRecord>> {
        let mut users = self
            .users
            .read()
            .expect("user repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        users.sort_by_key(|user| user.id);
        Ok(users)
    }

    async fn delete_user(&self, id: i64) -> RepositoryResult<()> {
        self.users
            .write()
            .expect("user repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound { entity: "user", id })?;
        self.api_keys
            .write()
            .expect("api key repository lock")
            .retain(|_, api_key| api_key.user_id != id);
        self.oauth_identities
            .write()
            .expect("oauth identity repository lock")
            .retain(|_, identity| identity.user_id != id);
        let session_ids = {
            let sessions = self
                .auth_sessions
                .read()
                .expect("auth session repository lock");
            sessions
                .values()
                .filter(|session| session.user_id == id)
                .map(|session| session.id)
                .collect::<Vec<_>>()
        };
        if !session_ids.is_empty() {
            let mut sessions = self
                .auth_sessions
                .write()
                .expect("auth session repository lock");
            let mut access_index = self
                .auth_session_id_by_access_token
                .write()
                .expect("auth session access index lock");
            let mut refresh_index = self
                .auth_session_id_by_refresh_token
                .write()
                .expect("auth session refresh index lock");
            for session_id in session_ids {
                if let Some(session) = sessions.remove(&session_id) {
                    access_index.remove(&session.access_token);
                    refresh_index.remove(&session.refresh_token);
                }
            }
        }
        if let Some(credential) = self
            .auth_credentials
            .write()
            .expect("auth credential repository lock")
            .remove(&id)
        {
            self.auth_credential_user_id_by_email
                .write()
                .expect("auth credential email index lock")
                .remove(&credential.email);
        }
        Ok(())
    }
}

#[async_trait]
impl OAuthIdentityRepository for InMemoryRepository {
    async fn upsert_oauth_identity(
        &self,
        mut record: OAuthIdentityRecord,
    ) -> RepositoryResult<OAuthIdentityRecord> {
        normalize_oauth_identity_record(&mut record)?;
        let key = oauth_identity_repository_key(
            &record.provider,
            &record.provider_key,
            &record.provider_subject,
        )?;
        let mut identities = self
            .oauth_identities
            .write()
            .expect("oauth identity repository lock");
        if let Some(existing) = identities.get(&key) {
            if existing.user_id != record.user_id {
                return Err(RepositoryError::Conflict(
                    "oauth identity is already bound to another user".to_owned(),
                ));
            }
            if record.id <= 0 {
                record.id = existing.id;
            }
        } else if record.id <= 0 {
            record.id = self.next_oauth_identity_id.fetch_add(1, Ordering::SeqCst);
        }
        identities.insert(key, record.clone());
        Ok(record)
    }

    async fn get_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<OAuthIdentityRecord> {
        let key = oauth_identity_repository_key(provider, provider_key, provider_subject)?;
        self.oauth_identities
            .read()
            .expect("oauth identity repository lock")
            .get(&key)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "oauth_identity",
                id: 0,
            })
    }

    async fn list_oauth_identities_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<OAuthIdentityRecord>> {
        let mut identities = self
            .oauth_identities
            .read()
            .expect("oauth identity repository lock")
            .values()
            .filter(|identity| identity.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        identities.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then(left.provider_key.cmp(&right.provider_key))
                .then(left.provider_subject.cmp(&right.provider_subject))
        });
        Ok(identities)
    }

    async fn delete_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<()> {
        let key = oauth_identity_repository_key(provider, provider_key, provider_subject)?;
        self.oauth_identities
            .write()
            .expect("oauth identity repository lock")
            .remove(&key)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "oauth_identity",
                id: 0,
            })
    }
}

#[async_trait]
impl AuthSessionRepository for InMemoryRepository {
    async fn upsert_auth_session(
        &self,
        mut record: AuthSessionRecord,
    ) -> RepositoryResult<AuthSessionRecord> {
        normalize_auth_session_record(&mut record)?;
        let mut sessions = self
            .auth_sessions
            .write()
            .expect("auth session repository lock");
        let mut access_index = self
            .auth_session_id_by_access_token
            .write()
            .expect("auth session access index lock");
        let mut refresh_index = self
            .auth_session_id_by_refresh_token
            .write()
            .expect("auth session refresh index lock");
        if let Some(existing_id) = access_index.get(&record.access_token).copied() {
            if record.id <= 0 || record.id != existing_id {
                return Err(RepositoryError::Duplicate {
                    entity: "auth_session access_token",
                    key: record.access_token,
                });
            }
        }
        if let Some(existing_id) = refresh_index.get(&record.refresh_token).copied() {
            if record.id <= 0 || record.id != existing_id {
                return Err(RepositoryError::Duplicate {
                    entity: "auth_session refresh_token",
                    key: record.refresh_token,
                });
            }
        }
        if record.id <= 0 {
            record.id = self.next_auth_session_id.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(existing) = sessions.insert(record.id, record.clone()) {
            access_index.remove(&existing.access_token);
            refresh_index.remove(&existing.refresh_token);
        }
        access_index.insert(record.access_token.clone(), record.id);
        refresh_index.insert(record.refresh_token.clone(), record.id);
        Ok(record)
    }

    async fn get_auth_session_by_access_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "access_token")?;
        let id = self
            .auth_session_id_by_access_token
            .read()
            .expect("auth session access index lock")
            .get(&token)
            .copied()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })?;
        self.auth_sessions
            .read()
            .expect("auth session repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id,
            })
    }

    async fn get_auth_session_by_refresh_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "refresh_token")?;
        let id = self
            .auth_session_id_by_refresh_token
            .read()
            .expect("auth session refresh index lock")
            .get(&token)
            .copied()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })?;
        self.auth_sessions
            .read()
            .expect("auth session repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id,
            })
    }

    async fn list_auth_sessions_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<AuthSessionRecord>> {
        let mut sessions = self
            .auth_sessions
            .read()
            .expect("auth session repository lock")
            .values()
            .filter(|session| session.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.id);
        Ok(sessions)
    }

    async fn revoke_auth_session_by_refresh_token(
        &self,
        token: &str,
        revoked_at_unix: i64,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "refresh_token")?;
        let id = self
            .auth_session_id_by_refresh_token
            .read()
            .expect("auth session refresh index lock")
            .get(&token)
            .copied()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })?;
        let mut sessions = self
            .auth_sessions
            .write()
            .expect("auth session repository lock");
        let session = sessions.get_mut(&id).ok_or(RepositoryError::NotFound {
            entity: "auth_session",
            id,
        })?;
        session.revoked_at_unix = Some(revoked_at_unix.max(1));
        Ok(session.clone())
    }

    async fn revoke_auth_sessions_by_user(
        &self,
        user_id: i64,
        revoked_at_unix: i64,
    ) -> RepositoryResult<i64> {
        let mut revoked = 0;
        let mut sessions = self
            .auth_sessions
            .write()
            .expect("auth session repository lock");
        for session in sessions.values_mut() {
            if session.user_id == user_id && session.revoked_at_unix.is_none() {
                session.revoked_at_unix = Some(revoked_at_unix.max(1));
                revoked += 1;
            }
        }
        Ok(revoked)
    }
}

#[async_trait]
impl AuthCredentialRepository for InMemoryRepository {
    async fn upsert_auth_credential(
        &self,
        mut record: AuthCredentialRecord,
    ) -> RepositoryResult<AuthCredentialRecord> {
        normalize_auth_credential_record(&mut record)?;
        let mut credentials = self
            .auth_credentials
            .write()
            .expect("auth credential repository lock");
        let mut email_index = self
            .auth_credential_user_id_by_email
            .write()
            .expect("auth credential email index lock");
        if let Some(existing_user_id) = email_index.get(&record.email).copied() {
            if existing_user_id != record.user_id {
                return Err(RepositoryError::Duplicate {
                    entity: "auth_credential email",
                    key: record.email,
                });
            }
        }
        if let Some(existing) = credentials.insert(record.user_id, record.clone()) {
            if existing.email != record.email {
                email_index.remove(&existing.email);
            }
        }
        email_index.insert(record.email.clone(), record.user_id);
        Ok(record)
    }

    async fn get_auth_credential_by_email(
        &self,
        email: &str,
    ) -> RepositoryResult<AuthCredentialRecord> {
        let email = normalize_auth_credential_email(email)?;
        let user_id = self
            .auth_credential_user_id_by_email
            .read()
            .expect("auth credential email index lock")
            .get(&email)
            .copied()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_credential",
                id: 0,
            })?;
        self.get_auth_credential_by_user_id(user_id).await
    }

    async fn get_auth_credential_by_user_id(
        &self,
        user_id: i64,
    ) -> RepositoryResult<AuthCredentialRecord> {
        self.auth_credentials
            .read()
            .expect("auth credential repository lock")
            .get(&user_id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "auth_credential",
                id: user_id,
            })
    }

    async fn delete_auth_credential(&self, user_id: i64) -> RepositoryResult<()> {
        let credential = self
            .auth_credentials
            .write()
            .expect("auth credential repository lock")
            .remove(&user_id)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_credential",
                id: user_id,
            })?;
        self.auth_credential_user_id_by_email
            .write()
            .expect("auth credential email index lock")
            .remove(&credential.email);
        Ok(())
    }
}

#[async_trait]
impl ApiKeyRepository for InMemoryRepository {
    async fn upsert_api_key(&self, api_key: ApiKey) -> RepositoryResult<ApiKey> {
        self.api_keys
            .write()
            .expect("api key repository lock")
            .insert(api_key.id, api_key.clone());
        Ok(api_key)
    }

    async fn get_api_key(&self, id: ApiKeyId) -> RepositoryResult<ApiKey> {
        self.api_keys
            .read()
            .expect("api key repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "api_key",
                id: id.0,
            })
    }

    async fn get_api_key_by_key(&self, key: &str) -> RepositoryResult<ApiKey> {
        self.api_keys
            .read()
            .expect("api key repository lock")
            .values()
            .find(|api_key| api_key.key == key)
            .cloned()
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "api_key",
                id: 0,
            })
    }

    async fn list_api_keys_by_user(&self, user_id: i64) -> RepositoryResult<Vec<ApiKey>> {
        let mut api_keys = self
            .api_keys
            .read()
            .expect("api key repository lock")
            .values()
            .filter(|api_key| api_key.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        api_keys.sort_by_key(|api_key| api_key.id.0);
        Ok(api_keys)
    }

    async fn list_api_keys_by_group(&self, group_id: GroupId) -> RepositoryResult<Vec<ApiKey>> {
        let mut api_keys = self
            .api_keys
            .read()
            .expect("api key repository lock")
            .values()
            .filter(|api_key| api_key.group_id == Some(group_id))
            .cloned()
            .collect::<Vec<_>>();
        api_keys.sort_by_key(|api_key| api_key.id.0);
        Ok(api_keys)
    }

    async fn delete_api_key(&self, id: ApiKeyId) -> RepositoryResult<()> {
        self.api_keys
            .write()
            .expect("api key repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "api_key",
                id: id.0,
            })
    }
}

#[async_trait]
impl GroupRepository for InMemoryRepository {
    async fn upsert_group(&self, group: Group) -> RepositoryResult<Group> {
        self.groups
            .write()
            .expect("group repository lock")
            .insert(group.id, group.clone());
        Ok(group)
    }

    async fn get_group(&self, id: GroupId) -> RepositoryResult<Group> {
        self.groups
            .read()
            .expect("group repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "group",
                id: id.0,
            })
    }

    async fn list_groups(&self) -> RepositoryResult<Vec<Group>> {
        let mut groups = self
            .groups
            .read()
            .expect("group repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| group.id.0);
        Ok(groups)
    }

    async fn list_active_groups(&self) -> RepositoryResult<Vec<Group>> {
        let mut groups = self
            .groups
            .read()
            .expect("group repository lock")
            .values()
            .filter(|group| group.status == GroupStatus::Active)
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| group.id.0);
        Ok(groups)
    }

    async fn delete_group(&self, id: GroupId) -> RepositoryResult<()> {
        self.groups
            .write()
            .expect("group repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "group",
                id: id.0,
            })?;
        self.bindings
            .write()
            .expect("binding repository lock")
            .remove(&id);
        for api_key in self
            .api_keys
            .write()
            .expect("api key repository lock")
            .values_mut()
        {
            if api_key.group_id == Some(id) {
                api_key.group_id = None;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl AccountRepository for InMemoryRepository {
    async fn upsert_account(&self, account: Account) -> RepositoryResult<Account> {
        self.accounts
            .write()
            .expect("account repository lock")
            .insert(account.id, account.clone());
        Ok(account)
    }

    async fn get_account(&self, id: AccountId) -> RepositoryResult<Account> {
        self.accounts
            .read()
            .expect("account repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "account",
                id: id.0,
            })
    }

    async fn list_accounts(&self) -> RepositoryResult<Vec<Account>> {
        let mut accounts = self
            .accounts
            .read()
            .expect("account repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        accounts.sort_by_key(|account| account.id.0);
        Ok(accounts)
    }

    async fn delete_account(&self, id: AccountId) -> RepositoryResult<()> {
        self.accounts
            .write()
            .expect("account repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "account",
                id: id.0,
            })?;
        for bindings in self
            .bindings
            .write()
            .expect("binding repository lock")
            .values_mut()
        {
            bindings.retain(|binding| binding.account.id != id);
        }
        Ok(())
    }

    async fn bind_account_to_group(
        &self,
        binding: AccountGroupBinding,
    ) -> RepositoryResult<AccountGroupBinding> {
        let account_id = binding.account.id;
        if !self
            .accounts
            .read()
            .expect("account repository lock")
            .contains_key(&account_id)
        {
            return Err(RepositoryError::NotFound {
                entity: "account",
                id: account_id.0,
            });
        }
        if !self
            .groups
            .read()
            .expect("group repository lock")
            .contains_key(&binding.group_id)
        {
            return Err(RepositoryError::NotFound {
                entity: "group",
                id: binding.group_id.0,
            });
        }
        let mut bindings = self.bindings.write().expect("binding repository lock");
        let group_bindings = bindings.entry(binding.group_id).or_default();
        group_bindings.retain(|existing| existing.account.id != account_id);
        group_bindings.push(binding.clone());
        group_bindings.sort_by_key(|binding| binding.priority);
        Ok(binding)
    }

    async fn list_bindings_by_group(
        &self,
        group_id: GroupId,
    ) -> RepositoryResult<Vec<AccountGroupBinding>> {
        let mut bindings = self
            .bindings
            .read()
            .expect("binding repository lock")
            .get(&group_id)
            .cloned()
            .unwrap_or_default();
        bindings.sort_by_key(|binding| binding.priority);
        Ok(bindings)
    }
}

#[async_trait]
impl UsageRepository for InMemoryRepository {
    async fn insert_usage(&self, mut record: UsageRecord) -> RepositoryResult<UsageRecord> {
        if record.id <= 0 {
            record.id = self.next_usage_id.fetch_add(1, Ordering::SeqCst);
        }
        self.usage
            .write()
            .expect("usage repository lock")
            .push(record.clone());
        Ok(record)
    }

    async fn list_usage(&self, filter: UsageFilter) -> RepositoryResult<Vec<UsageRecord>> {
        let mut records = self
            .usage
            .read()
            .expect("usage repository lock")
            .iter()
            .filter(|record| filter.matches(record))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }

    async fn summarize_usage(&self, filter: UsageFilter) -> RepositoryResult<UsageSummary> {
        let mut summary = UsageSummary::default();
        for record in self
            .usage
            .read()
            .expect("usage repository lock")
            .iter()
            .filter(|record| filter.matches(record))
        {
            summary.add_record(record);
        }
        Ok(summary)
    }

    async fn create_usage_cleanup_task(
        &self,
        mut task: UsageCleanupTaskRecord,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        validate_cleanup_task(&task)?;
        if task.id <= 0 {
            task.id = self
                .next_usage_cleanup_task_id
                .fetch_add(1, Ordering::SeqCst);
        }
        let now = repository_now();
        if task.created_at.trim().is_empty() {
            task.created_at = now.clone();
        }
        if task.updated_at.trim().is_empty() {
            task.updated_at = now;
        }
        self.usage_cleanup_tasks
            .write()
            .expect("usage cleanup task repository lock")
            .insert(task.id, task.clone());
        Ok(task)
    }

    async fn list_usage_cleanup_tasks(
        &self,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<UsageCleanupTaskRecord>> {
        let pagination = Pagination::new(pagination.page, pagination.page_size);
        let mut items = self
            .usage_cleanup_tasks
            .read()
            .expect("usage cleanup task repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let total = items.len() as i64;
        let page_items = items
            .into_iter()
            .skip(pagination.offset() as usize)
            .take(pagination.page_size as usize)
            .collect();
        Ok(PaginatedRecords {
            items: page_items,
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }

    async fn get_usage_cleanup_task_status(&self, task_id: i64) -> RepositoryResult<String> {
        self.usage_cleanup_tasks
            .read()
            .expect("usage cleanup task repository lock")
            .get(&task_id)
            .map(|task| task.status.clone())
            .ok_or(RepositoryError::NotFound {
                entity: "usage_cleanup_task",
                id: task_id,
            })
    }

    async fn claim_next_usage_cleanup_task(
        &self,
        stale_running_after_seconds: i64,
    ) -> RepositoryResult<Option<UsageCleanupTaskRecord>> {
        let stale_running_after_seconds = stale_running_after_seconds.max(1);
        let now = Utc::now();
        let mut tasks = self
            .usage_cleanup_tasks
            .write()
            .expect("usage cleanup task repository lock");
        let candidate_id = tasks
            .values()
            .filter(|task| {
                task.status == "pending"
                    || (task.status == "running"
                        && cleanup_started_before(
                            task,
                            now - Duration::seconds(stale_running_after_seconds),
                        ))
            })
            .min_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then(left.id.cmp(&right.id))
            })
            .map(|task| task.id);
        let Some(candidate_id) = candidate_id else {
            return Ok(None);
        };
        let task = tasks
            .get_mut(&candidate_id)
            .expect("candidate id should exist");
        task.status = "running".to_owned();
        task.started_at = Some(repository_now());
        task.finished_at = None;
        task.error_message = None;
        task.updated_at = repository_now();
        Ok(Some(task.clone()))
    }

    async fn update_usage_cleanup_task_progress(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<()> {
        let mut tasks = self
            .usage_cleanup_tasks
            .write()
            .expect("usage cleanup task repository lock");
        let task = tasks.get_mut(&task_id).ok_or(RepositoryError::NotFound {
            entity: "usage_cleanup_task",
            id: task_id,
        })?;
        task.deleted_rows = deleted_rows.max(0);
        task.updated_at = repository_now();
        Ok(())
    }

    async fn cancel_usage_cleanup_task(
        &self,
        task_id: i64,
        canceled_by: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        let mut tasks = self
            .usage_cleanup_tasks
            .write()
            .expect("usage cleanup task repository lock");
        let task = tasks.get_mut(&task_id).ok_or(RepositoryError::NotFound {
            entity: "usage_cleanup_task",
            id: task_id,
        })?;
        if task.status == "canceled" {
            return Ok(task.clone());
        }
        if task.status != "pending" && task.status != "running" {
            return Err(RepositoryError::Conflict(
                "cleanup task cannot be canceled in current status".to_owned(),
            ));
        }
        let now = repository_now();
        task.status = "canceled".to_owned();
        task.canceled_by = Some(canceled_by);
        task.canceled_at = Some(now.clone());
        task.finished_at = Some(now.clone());
        task.error_message = None;
        task.updated_at = now;
        Ok(task.clone())
    }

    async fn mark_usage_cleanup_task_succeeded(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        self.finish_cleanup_task(task_id, "succeeded", deleted_rows, None)
    }

    async fn mark_usage_cleanup_task_failed(
        &self,
        task_id: i64,
        deleted_rows: i64,
        error_message: String,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        self.finish_cleanup_task(task_id, "failed", deleted_rows, Some(error_message))
    }

    async fn delete_usage_batch(
        &self,
        filter: UsageCleanupFilter,
        limit: i64,
    ) -> RepositoryResult<i64> {
        validate_cleanup_filter(&filter)?;
        let limit = limit.max(1) as usize;
        let usage_filter = filter.to_usage_filter();
        let mut usage = self.usage.write().expect("usage repository lock");
        let mut deleted = 0usize;
        let mut next = Vec::with_capacity(usage.len());
        for record in usage.drain(..) {
            if deleted < limit && cleanup_filter_matches(&filter, &usage_filter, &record) {
                deleted += 1;
            } else {
                next.push(record);
            }
        }
        *usage = next;
        Ok(deleted as i64)
    }
}

impl InMemoryRepository {
    fn finish_cleanup_task(
        &self,
        task_id: i64,
        status: &str,
        deleted_rows: i64,
        error_message: Option<String>,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        let mut tasks = self
            .usage_cleanup_tasks
            .write()
            .expect("usage cleanup task repository lock");
        let task = tasks.get_mut(&task_id).ok_or(RepositoryError::NotFound {
            entity: "usage_cleanup_task",
            id: task_id,
        })?;
        task.status = status.to_owned();
        task.deleted_rows = deleted_rows.max(0);
        task.error_message = error_message;
        let now = repository_now();
        task.finished_at = Some(now.clone());
        task.updated_at = now;
        Ok(task.clone())
    }
}

#[async_trait]
impl PaymentOrderRepository for InMemoryRepository {
    async fn upsert_payment_order(
        &self,
        mut order: PaymentOrderRecord,
    ) -> RepositoryResult<PaymentOrderRecord> {
        if order.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "payment order user_id is required".to_owned(),
            ));
        }
        if order.out_trade_no.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment order out_trade_no is required".to_owned(),
            ));
        }
        if order.id <= 0 {
            order.id = self.next_payment_order_id.fetch_add(1, Ordering::SeqCst);
        }
        self.payment_orders
            .write()
            .expect("payment order repository lock")
            .insert(order.id, order.clone());
        self.payment_order_id_by_trade_no
            .write()
            .expect("payment order trade repository lock")
            .insert(order.out_trade_no.clone(), order.id);
        Ok(order)
    }

    async fn get_payment_order(&self, id: i64) -> RepositoryResult<PaymentOrderRecord> {
        self.payment_orders
            .read()
            .expect("payment order repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "payment_order",
                id,
            })
    }

    async fn get_payment_order_by_trade_no(
        &self,
        out_trade_no: &str,
    ) -> RepositoryResult<PaymentOrderRecord> {
        let id = self
            .payment_order_id_by_trade_no
            .read()
            .expect("payment order trade repository lock")
            .get(out_trade_no)
            .copied()
            .ok_or_else(|| RepositoryError::Duplicate {
                entity: "payment_order trade lookup",
                key: out_trade_no.to_owned(),
            })?;
        self.get_payment_order(id).await
    }

    async fn list_payment_orders(
        &self,
        filter: PaymentOrderFilter,
    ) -> RepositoryResult<Vec<PaymentOrderRecord>> {
        let mut records = self
            .payment_orders
            .read()
            .expect("payment order repository lock")
            .values()
            .filter(|record| filter.matches(record))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }
}

#[async_trait]
impl PaymentAuditRepository for InMemoryRepository {
    async fn insert_payment_audit(
        &self,
        mut record: PaymentAuditRecord,
    ) -> RepositoryResult<PaymentAuditRecord> {
        if record.order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment audit order_id is required".to_owned(),
            ));
        }
        if record.action.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment audit action is required".to_owned(),
            ));
        }
        if record.id <= 0 {
            record.id = self.next_payment_audit_id.fetch_add(1, Ordering::SeqCst);
        }
        self.payment_audits
            .write()
            .expect("payment audit repository lock")
            .push(record.clone());
        Ok(record)
    }

    async fn list_payment_audits(
        &self,
        order_id: &str,
    ) -> RepositoryResult<Vec<PaymentAuditRecord>> {
        let mut records = self
            .payment_audits
            .read()
            .expect("payment audit repository lock")
            .iter()
            .filter(|record| record.order_id == order_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }
}

#[async_trait]
impl BalanceRepository for InMemoryRepository {
    async fn get_user_balance(&self, user_id: i64) -> RepositoryResult<UserBalanceRecord> {
        Ok(self
            .user_balances
            .read()
            .expect("user balance repository lock")
            .get(&user_id)
            .cloned()
            .unwrap_or(UserBalanceRecord {
                user_id,
                balance: 0.0,
                updated_at: String::new(),
            }))
    }

    async fn apply_balance_transaction(
        &self,
        mut record: BalanceTransactionRecord,
    ) -> RepositoryResult<BalanceTransactionRecord> {
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "balance transaction user_id is required".to_owned(),
            ));
        }
        if record.order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "balance transaction order_id is required".to_owned(),
            ));
        }
        if let Some(existing) = self
            .balance_transactions
            .read()
            .expect("balance transaction repository lock")
            .iter()
            .find(|existing| {
                existing.order_id == record.order_id
                    && existing.transaction_type == record.transaction_type
            })
            .cloned()
        {
            return Ok(existing);
        }

        if record.id <= 0 {
            record.id = self
                .next_balance_transaction_id
                .fetch_add(1, Ordering::SeqCst);
        }
        let mut balances = self
            .user_balances
            .write()
            .expect("user balance repository lock");
        let current = balances
            .get(&record.user_id)
            .map(|balance| balance.balance)
            .unwrap_or(0.0);
        record.balance_after = current + record.amount;
        balances.insert(
            record.user_id,
            UserBalanceRecord {
                user_id: record.user_id,
                balance: record.balance_after,
                updated_at: record.created_at.clone(),
            },
        );
        self.balance_transactions
            .write()
            .expect("balance transaction repository lock")
            .push(record.clone());
        Ok(record)
    }

    async fn list_balance_transactions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<BalanceTransactionRecord>> {
        let mut records = self
            .balance_transactions
            .read()
            .expect("balance transaction repository lock")
            .iter()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }
}

#[async_trait]
impl SubscriptionRepository for InMemoryRepository {
    async fn upsert_user_subscription(
        &self,
        mut record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription user_id is required".to_owned(),
            ));
        }
        if record.group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription group_id is required".to_owned(),
            ));
        }
        if record.source_order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription source_order_id is required".to_owned(),
            ));
        }
        if let Some(id) = self
            .subscription_id_by_source_order
            .read()
            .expect("subscription source order repository lock")
            .get(&record.source_order_id)
            .copied()
        {
            if let Some(existing) = self
                .user_subscriptions
                .read()
                .expect("subscription repository lock")
                .get(&id)
                .cloned()
            {
                return Ok(existing);
            }
        }

        if record.id <= 0 {
            record.id = self.next_subscription_id.fetch_add(1, Ordering::SeqCst);
        }
        self.user_subscriptions
            .write()
            .expect("subscription repository lock")
            .insert(record.id, record.clone());
        self.subscription_id_by_source_order
            .write()
            .expect("subscription source order repository lock")
            .insert(record.source_order_id.clone(), record.id);
        Ok(record)
    }

    async fn get_user_subscription(&self, id: i64) -> RepositoryResult<UserSubscriptionRecord> {
        self.user_subscriptions
            .read()
            .expect("subscription repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id,
            })
    }

    async fn update_user_subscription(
        &self,
        record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if record.id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription id is required".to_owned(),
            ));
        }
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription user_id is required".to_owned(),
            ));
        }
        if record.group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription group_id is required".to_owned(),
            ));
        }
        if record.source_order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription source_order_id is required".to_owned(),
            ));
        }

        let mut subscriptions = self
            .user_subscriptions
            .write()
            .expect("subscription repository lock");
        let existing = subscriptions
            .get(&record.id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id: record.id,
            })?;
        subscriptions.insert(record.id, record.clone());
        let mut source_orders = self
            .subscription_id_by_source_order
            .write()
            .expect("subscription source order repository lock");
        source_orders.remove(&existing.source_order_id);
        source_orders.insert(record.source_order_id.clone(), record.id);
        Ok(record)
    }

    async fn delete_user_subscription(&self, id: i64) -> RepositoryResult<()> {
        let removed = self
            .user_subscriptions
            .write()
            .expect("subscription repository lock")
            .remove(&id)
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id,
            })?;
        self.subscription_id_by_source_order
            .write()
            .expect("subscription source order repository lock")
            .remove(&removed.source_order_id);
        Ok(())
    }

    async fn update_subscription_status_by_source_order(
        &self,
        source_order_id: &str,
        status: &str,
        metadata: Value,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if status.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription status is required".to_owned(),
            ));
        }
        let id = self
            .subscription_id_by_source_order
            .read()
            .expect("subscription source order repository lock")
            .get(source_order_id)
            .copied()
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "subscription",
                id: 0,
            })?;
        let mut subscriptions = self
            .user_subscriptions
            .write()
            .expect("subscription repository lock");
        let subscription = subscriptions
            .get_mut(&id)
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id,
            })?;
        subscription.status = status.to_owned();
        subscription.metadata = merge_json(subscription.metadata.clone(), metadata);
        Ok(subscription.clone())
    }

    async fn get_subscription_by_source_order(
        &self,
        source_order_id: &str,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        let id = self
            .subscription_id_by_source_order
            .read()
            .expect("subscription source order repository lock")
            .get(source_order_id)
            .copied()
            .ok_or_else(|| RepositoryError::Duplicate {
                entity: "subscription source order lookup",
                key: source_order_id.to_owned(),
            })?;
        self.user_subscriptions
            .read()
            .expect("subscription repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id,
            })
    }

    async fn list_user_subscriptions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserSubscriptionRecord>> {
        let mut records = self
            .user_subscriptions
            .read()
            .expect("subscription repository lock")
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }

    async fn list_subscriptions(&self) -> RepositoryResult<Vec<UserSubscriptionRecord>> {
        let mut records = self
            .user_subscriptions
            .read()
            .expect("subscription repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }
}

#[async_trait]
impl UserPlatformQuotaRepository for InMemoryRepository {
    async fn list_user_platform_quotas(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>> {
        let mut records = self
            .user_platform_quotas
            .read()
            .expect("user platform quota repository lock")
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.platform.cmp(&right.platform));
        Ok(records)
    }

    async fn replace_user_platform_quotas(
        &self,
        user_id: i64,
        records: Vec<UserPlatformQuotaRecord>,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "quota user_id is required".to_owned(),
            ));
        }
        validate_user_platform_quota_records(&records)?;
        let mut quotas = self
            .user_platform_quotas
            .write()
            .expect("user platform quota repository lock");
        quotas.retain(|(existing_user_id, _), _| *existing_user_id != user_id);
        for mut record in records {
            record.user_id = user_id;
            record.platform = normalize_quota_platform(&record.platform)?;
            record.id = record.id.max(0);
            quotas.insert((user_id, record.platform.clone()), record);
        }
        let mut saved = quotas
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        saved.sort_by(|left, right| left.platform.cmp(&right.platform));
        Ok(saved)
    }

    async fn increment_user_platform_quota_usage(
        &self,
        user_id: i64,
        platform: &str,
        cost: f64,
        daily_window_start: String,
        weekly_window_start: String,
        monthly_window_start: String,
    ) -> RepositoryResult<UserPlatformQuotaRecord> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "quota user_id is required".to_owned(),
            ));
        }
        let platform = normalize_quota_platform(platform)?;
        let mut quotas = self
            .user_platform_quotas
            .write()
            .expect("user platform quota repository lock");
        let record = quotas
            .entry((user_id, platform.clone()))
            .or_insert_with(|| UserPlatformQuotaRecord {
                id: 0,
                user_id,
                platform,
                daily_limit_usd: None,
                weekly_limit_usd: None,
                monthly_limit_usd: None,
                daily_usage_usd: 0.0,
                weekly_usage_usd: 0.0,
                monthly_usage_usd: 0.0,
                daily_window_start: None,
                weekly_window_start: None,
                monthly_window_start: None,
            });
        if record.daily_window_start.as_deref() != Some(daily_window_start.as_str()) {
            record.daily_usage_usd = cost;
            record.daily_window_start = Some(daily_window_start);
        } else {
            record.daily_usage_usd += cost;
        }
        if record.weekly_window_start.as_deref() != Some(weekly_window_start.as_str()) {
            record.weekly_usage_usd = cost;
            record.weekly_window_start = Some(weekly_window_start);
        } else {
            record.weekly_usage_usd += cost;
        }
        if monthly_quota_expired(
            record.monthly_window_start.as_deref(),
            &monthly_window_start,
        ) {
            record.monthly_usage_usd = cost;
            record.monthly_window_start = Some(monthly_window_start);
        } else {
            record.monthly_usage_usd += cost;
        }
        Ok(record.clone())
    }
}

#[async_trait]
impl UserGroupRateRepository for InMemoryRepository {
    async fn list_user_group_rates(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "user_id is required".to_owned(),
            ));
        }
        let mut records = self
            .user_group_rates
            .read()
            .expect("user group rate repository lock")
            .values()
            .filter(|record| record.user_id == user_id && record.rate_multiplier.is_some())
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.group_id);
        Ok(records)
    }

    async fn list_group_rate_overrides(
        &self,
        group_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let mut records = self
            .user_group_rates
            .read()
            .expect("user group rate repository lock")
            .values()
            .filter(|record| {
                record.group_id == group_id
                    && (record.rate_multiplier.is_some() || record.rpm_override.is_some())
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.user_id);
        Ok(records)
    }

    async fn replace_group_rate_multipliers(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let mut rates = self
            .user_group_rates
            .write()
            .expect("user group rate repository lock");
        let now = Utc::now().to_rfc3339();
        for record in rates
            .values_mut()
            .filter(|record| record.group_id == group_id)
        {
            record.rate_multiplier = None;
            record.updated_at = now.clone();
        }
        rates.retain(|(_, existing_group_id), record| {
            *existing_group_id != group_id
                || record.rate_multiplier.is_some()
                || record.rpm_override.is_some()
        });
        for record in validate_user_group_rate_records(group_id, records, true)? {
            let key = (record.user_id, group_id);
            let mut merged = rates.remove(&key).unwrap_or(UserGroupRateRecord {
                user_id: record.user_id,
                group_id,
                rate_multiplier: None,
                rpm_override: None,
                updated_at: record.updated_at.clone(),
            });
            merged.rate_multiplier = record.rate_multiplier;
            merged.updated_at = record.updated_at;
            rates.insert(key, merged);
        }
        Ok(sorted_group_rate_overrides(&rates, group_id))
    }

    async fn clear_group_rate_multipliers(&self, group_id: i64) -> RepositoryResult<()> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let mut rates = self
            .user_group_rates
            .write()
            .expect("user group rate repository lock");
        let now = Utc::now().to_rfc3339();
        for record in rates
            .values_mut()
            .filter(|record| record.group_id == group_id)
        {
            record.rate_multiplier = None;
            record.updated_at = now.clone();
        }
        rates.retain(|(_, existing_group_id), record| {
            *existing_group_id != group_id
                || record.rate_multiplier.is_some()
                || record.rpm_override.is_some()
        });
        Ok(())
    }

    async fn replace_group_rpm_overrides(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let mut rates = self
            .user_group_rates
            .write()
            .expect("user group rate repository lock");
        let now = Utc::now().to_rfc3339();
        for record in rates
            .values_mut()
            .filter(|record| record.group_id == group_id)
        {
            record.rpm_override = None;
            record.updated_at = now.clone();
        }
        rates.retain(|(_, existing_group_id), record| {
            *existing_group_id != group_id
                || record.rate_multiplier.is_some()
                || record.rpm_override.is_some()
        });
        for record in validate_user_group_rate_records(group_id, records, false)? {
            let key = (record.user_id, group_id);
            let mut merged = rates.remove(&key).unwrap_or(UserGroupRateRecord {
                user_id: record.user_id,
                group_id,
                rate_multiplier: None,
                rpm_override: None,
                updated_at: record.updated_at.clone(),
            });
            merged.rpm_override = record.rpm_override;
            merged.updated_at = record.updated_at;
            if merged.rate_multiplier.is_some() || merged.rpm_override.is_some() {
                rates.insert(key, merged);
            }
        }
        Ok(sorted_group_rate_overrides(&rates, group_id))
    }

    async fn clear_group_rpm_overrides(&self, group_id: i64) -> RepositoryResult<()> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let mut rates = self
            .user_group_rates
            .write()
            .expect("user group rate repository lock");
        let now = Utc::now().to_rfc3339();
        for record in rates
            .values_mut()
            .filter(|record| record.group_id == group_id)
        {
            record.rpm_override = None;
            record.updated_at = now.clone();
        }
        rates.retain(|(_, existing_group_id), record| {
            *existing_group_id != group_id
                || record.rate_multiplier.is_some()
                || record.rpm_override.is_some()
        });
        Ok(())
    }
}

#[async_trait]
impl UserAttributeValueRepository for InMemoryRepository {
    async fn list_user_attribute_values(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "user_id is required".to_owned(),
            ));
        }
        let mut records = self
            .user_attribute_values
            .read()
            .expect("user attribute value repository lock")
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.attribute_id);
        Ok(records)
    }

    async fn replace_user_attribute_values(
        &self,
        user_id: i64,
        records: Vec<UserAttributeValueRecord>,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>> {
        let records = validate_user_attribute_value_records(user_id, records)?;
        let mut values = self
            .user_attribute_values
            .write()
            .expect("user attribute value repository lock");
        values.retain(|(existing_user_id, _), _| *existing_user_id != user_id);
        for record in records {
            values.insert((user_id, record.attribute_id), record);
        }
        let mut records = values
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.attribute_id);
        Ok(records)
    }
}

#[async_trait]
impl ChannelMonitorHistoryRepository for InMemoryRepository {
    async fn insert_channel_monitor_history(
        &self,
        mut record: ChannelMonitorHistoryRecord,
    ) -> RepositoryResult<ChannelMonitorHistoryRecord> {
        record = validate_channel_monitor_history_record(record)?;
        let id = if record.id > 0 {
            record.id
        } else {
            self.next_channel_monitor_history_id
                .fetch_add(1, Ordering::SeqCst)
        };
        record.id = id;
        self.channel_monitor_history
            .write()
            .expect("channel monitor history repository lock")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn list_channel_monitor_history(
        &self,
        monitor_id: i64,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ChannelMonitorHistoryRecord>> {
        if monitor_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "monitor_id is required".to_owned(),
            ));
        }
        let mut records = self
            .channel_monitor_history
            .read()
            .expect("channel monitor history repository lock")
            .values()
            .filter(|record| record.monitor_id == monitor_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .checked_at
                .cmp(&left.checked_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let total = records.len() as i64;
        let items = records
            .into_iter()
            .skip(pagination.offset() as usize)
            .take(pagination.page_size as usize)
            .collect();
        Ok(PaginatedRecords {
            items,
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }
}

#[async_trait]
impl ContentModerationLogRepository for InMemoryRepository {
    async fn insert_content_moderation_log(
        &self,
        mut record: ContentModerationLogRecord,
    ) -> RepositoryResult<ContentModerationLogRecord> {
        record = validate_content_moderation_log_record(record)?;
        let id = if record.id > 0 {
            record.id
        } else {
            self.next_content_moderation_log_id
                .fetch_add(1, Ordering::SeqCst)
        };
        record.id = id;
        self.content_moderation_logs
            .write()
            .expect("content moderation log repository lock")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn list_content_moderation_logs(
        &self,
        filter: ContentModerationLogFilter,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ContentModerationLogRecord>> {
        let mut records = self
            .content_moderation_logs
            .read()
            .expect("content moderation log repository lock")
            .values()
            .filter(|record| filter.matches(record))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let total = records.len() as i64;
        let items = records
            .into_iter()
            .skip(pagination.offset() as usize)
            .take(pagination.page_size as usize)
            .collect();
        Ok(PaginatedRecords {
            items,
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }
}

#[async_trait]
impl PaymentProviderRepository for InMemoryRepository {
    async fn upsert_payment_provider_instance(
        &self,
        mut record: PaymentProviderInstanceRecord,
    ) -> RepositoryResult<PaymentProviderInstanceRecord> {
        record.provider_key = normalize_repository_string(&record.provider_key);
        record.supported_types = normalize_repository_strings(&record.supported_types);
        if record.provider_key.is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment provider provider_key is required".to_owned(),
            ));
        }
        if record.name.trim().is_empty() {
            record.name = record.provider_key.clone();
        }
        if record.id <= 0 {
            record.id = self
                .next_payment_provider_instance_id
                .fetch_add(1, Ordering::SeqCst);
        }
        self.payment_provider_instances
            .write()
            .expect("payment provider repository lock")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn get_payment_provider_instance(
        &self,
        id: i64,
    ) -> RepositoryResult<PaymentProviderInstanceRecord> {
        self.payment_provider_instances
            .read()
            .expect("payment provider repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "payment_provider_instance",
                id,
            })
    }

    async fn list_payment_provider_instances(
        &self,
    ) -> RepositoryResult<Vec<PaymentProviderInstanceRecord>> {
        let mut records = self
            .payment_provider_instances
            .read()
            .expect("payment provider repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| (record.sort_order, record.id));
        Ok(records)
    }

    async fn delete_payment_provider_instance(&self, id: i64) -> RepositoryResult<()> {
        self.payment_provider_instances
            .write()
            .expect("payment provider repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "payment_provider_instance",
                id,
            })
    }
}

#[async_trait]
impl PaymentPlanRepository for InMemoryRepository {
    async fn upsert_payment_plan(
        &self,
        mut record: PaymentPlanRecord,
    ) -> RepositoryResult<PaymentPlanRecord> {
        validate_payment_plan(&record, true)?;
        record.name = record.name.trim().to_owned();
        record.validity_unit = record.validity_unit.trim().to_owned();
        record.features = normalize_plan_features(record.features);
        if record.id <= 0 {
            record.id = self.next_payment_plan_id.fetch_add(1, Ordering::SeqCst);
        }
        self.payment_plans
            .write()
            .expect("payment plan repository lock")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn get_payment_plan(&self, id: i64) -> RepositoryResult<PaymentPlanRecord> {
        self.payment_plans
            .read()
            .expect("payment plan repository lock")
            .get(&id)
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "payment_plan",
                id,
            })
    }

    async fn list_payment_plans(&self) -> RepositoryResult<Vec<PaymentPlanRecord>> {
        let mut records = self
            .payment_plans
            .read()
            .expect("payment plan repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        sort_payment_plans(&mut records);
        Ok(records)
    }

    async fn list_payment_plans_for_sale(&self) -> RepositoryResult<Vec<PaymentPlanRecord>> {
        let mut records = self
            .payment_plans
            .read()
            .expect("payment plan repository lock")
            .values()
            .filter(|record| record.for_sale)
            .cloned()
            .collect::<Vec<_>>();
        sort_payment_plans(&mut records);
        Ok(records)
    }

    async fn delete_payment_plan(&self, id: i64) -> RepositoryResult<()> {
        self.payment_plans
            .write()
            .expect("payment plan repository lock")
            .remove(&id)
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "payment_plan",
                id,
            })
    }
}

#[async_trait]
impl AdminCollectionRepository for InMemoryRepository {
    async fn upsert_admin_collection_item(
        &self,
        record: AdminCollectionItemRecord,
    ) -> RepositoryResult<AdminCollectionItemRecord> {
        let collection = normalize_admin_collection_name(&record.collection)?;
        if record.id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "admin collection item id is required".to_owned(),
            ));
        }
        let record = AdminCollectionItemRecord {
            collection: collection.clone(),
            id: record.id,
            item: record.item,
        };
        self.admin_collection_items
            .write()
            .expect("admin collection repository lock")
            .entry(collection)
            .or_default()
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn get_admin_collection_item(
        &self,
        collection: &str,
        id: i64,
    ) -> RepositoryResult<AdminCollectionItemRecord> {
        let collection = normalize_admin_collection_name(collection)?;
        self.admin_collection_items
            .read()
            .expect("admin collection repository lock")
            .get(&collection)
            .and_then(|items| items.get(&id))
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "admin_collection_item",
                id,
            })
    }

    async fn list_admin_collection_items(
        &self,
        collection: &str,
    ) -> RepositoryResult<Vec<AdminCollectionItemRecord>> {
        let collection = normalize_admin_collection_name(collection)?;
        let mut records = self
            .admin_collection_items
            .read()
            .expect("admin collection repository lock")
            .get(&collection)
            .map(|items| items.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }

    async fn delete_admin_collection_item(
        &self,
        collection: &str,
        id: i64,
    ) -> RepositoryResult<()> {
        let collection = normalize_admin_collection_name(collection)?;
        self.admin_collection_items
            .write()
            .expect("admin collection repository lock")
            .get_mut(&collection)
            .and_then(|items| items.remove(&id))
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "admin_collection_item",
                id,
            })
    }
}

#[async_trait]
impl SystemSettingRepository for InMemoryRepository {
    async fn upsert_system_setting(
        &self,
        record: SystemSettingRecord,
    ) -> RepositoryResult<SystemSettingRecord> {
        let namespace = normalize_setting_namespace(&record.namespace)?;
        let key = normalize_setting_key(&record.key)?;
        let record = SystemSettingRecord {
            namespace: namespace.clone(),
            key: key.clone(),
            value: record.value,
            updated_at: record.updated_at,
        };
        self.system_settings
            .write()
            .expect("system setting repository lock")
            .insert((namespace, key), record.clone());
        Ok(record)
    }

    async fn get_system_setting(
        &self,
        namespace: &str,
        key: &str,
    ) -> RepositoryResult<SystemSettingRecord> {
        let namespace = normalize_setting_namespace(namespace)?;
        let key = normalize_setting_key(key)?;
        self.system_settings
            .read()
            .expect("system setting repository lock")
            .get(&(namespace, key))
            .cloned()
            .ok_or(RepositoryError::NotFound {
                entity: "system_setting",
                id: 0,
            })
    }

    async fn list_system_settings(
        &self,
        namespace: &str,
    ) -> RepositoryResult<Vec<SystemSettingRecord>> {
        let namespace = normalize_setting_namespace(namespace)?;
        let mut records = self
            .system_settings
            .read()
            .expect("system setting repository lock")
            .values()
            .filter(|record| record.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(records)
    }

    async fn delete_system_setting(&self, namespace: &str, key: &str) -> RepositoryResult<()> {
        let namespace = normalize_setting_namespace(namespace)?;
        let key = normalize_setting_key(key)?;
        self.system_settings
            .write()
            .expect("system setting repository lock")
            .remove(&(namespace, key))
            .map(|_| ())
            .ok_or(RepositoryError::NotFound {
                entity: "system_setting",
                id: 0,
            })
    }
}

#[async_trait]
impl EmailQueueTaskRepository for InMemoryRepository {
    async fn enqueue_email_task(
        &self,
        mut record: EmailQueueTaskRecord,
    ) -> RepositoryResult<EmailQueueTaskRecord> {
        normalize_email_queue_task_record(&mut record)?;
        if record.id <= 0 {
            record.id = self.next_email_queue_task_id.fetch_add(1, Ordering::SeqCst);
        }
        self.email_queue_tasks
            .write()
            .expect("email queue task repository lock")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn list_pending_email_tasks(
        &self,
        limit: i64,
    ) -> RepositoryResult<Vec<EmailQueueTaskRecord>> {
        let limit = limit.clamp(1, 1000) as usize;
        let mut records = self
            .email_queue_tasks
            .read()
            .expect("email queue task repository lock")
            .values()
            .filter(|record| matches!(record.status.as_str(), "pending" | "processing"))
            .filter(|record| record.attempts < record.max_attempts)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.id);
        records.truncate(limit);
        Ok(records)
    }

    async fn mark_email_task_processing(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord> {
        let mut tasks = self
            .email_queue_tasks
            .write()
            .expect("email queue task repository lock");
        let task = tasks.get_mut(&id).ok_or(RepositoryError::NotFound {
            entity: "email_queue_task",
            id,
        })?;
        task.status = "processing".to_owned();
        task.attempts += 1;
        task.updated_at = Utc::now().to_rfc3339();
        Ok(task.clone())
    }

    async fn mark_email_task_sent(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord> {
        let mut tasks = self
            .email_queue_tasks
            .write()
            .expect("email queue task repository lock");
        let task = tasks.get_mut(&id).ok_or(RepositoryError::NotFound {
            entity: "email_queue_task",
            id,
        })?;
        task.status = "sent".to_owned();
        task.last_error = None;
        task.updated_at = Utc::now().to_rfc3339();
        Ok(task.clone())
    }

    async fn mark_email_task_failed(
        &self,
        id: i64,
        last_error: String,
    ) -> RepositoryResult<EmailQueueTaskRecord> {
        let mut tasks = self
            .email_queue_tasks
            .write()
            .expect("email queue task repository lock");
        let task = tasks.get_mut(&id).ok_or(RepositoryError::NotFound {
            entity: "email_queue_task",
            id,
        })?;
        task.status = "failed".to_owned();
        task.last_error = Some(last_error);
        task.updated_at = Utc::now().to_rfc3339();
        Ok(task.clone())
    }
}

#[async_trait]
impl UserRepository for PostgresRepository {
    async fn upsert_user(&self, user: UserRecord) -> RepositoryResult<UserRecord> {
        if user.email.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "email is required".to_owned(),
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO users (id, email, username, role, status)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO UPDATE SET
                email = EXCLUDED.email,
                username = EXCLUDED.username,
                role = EXCLUDED.role,
                status = EXCLUDED.status,
                updated_at = NOW()
            "#,
        )
        .bind(user.id)
        .bind(&user.email)
        .bind(&user.username)
        .bind(&user.role)
        .bind(&user.status)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(user)
    }

    async fn get_user(&self, id: i64) -> RepositoryResult<UserRecord> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, username, role, status FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserRecord::from)
            .ok_or(RepositoryError::NotFound { entity: "user", id })
    }

    async fn get_user_by_email(&self, email: &str) -> RepositoryResult<UserRecord> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, username, role, status FROM users WHERE lower(email) = lower($1)",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserRecord::from)
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "user",
                id: 0,
            })
    }

    async fn list_users(&self) -> RepositoryResult<Vec<UserRecord>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, username, role, status FROM users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(UserRecord::from).collect())
    }

    async fn delete_user(&self, id: i64) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound { entity: "user", id });
        }
        Ok(())
    }
}

#[async_trait]
impl OAuthIdentityRepository for PostgresRepository {
    async fn upsert_oauth_identity(
        &self,
        mut record: OAuthIdentityRecord,
    ) -> RepositoryResult<OAuthIdentityRecord> {
        normalize_oauth_identity_record(&mut record)?;
        let row = if record.id > 0 {
            sqlx::query_as::<_, OAuthIdentityRow>(
                r#"
                INSERT INTO oauth_identities (
                    id, user_id, provider, provider_key, provider_subject,
                    email, bound_at_unix, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (provider, provider_key, provider_subject) DO UPDATE SET
                    email = EXCLUDED.email,
                    bound_at_unix = EXCLUDED.bound_at_unix,
                    metadata = EXCLUDED.metadata,
                    updated_at = NOW()
                WHERE oauth_identities.user_id = EXCLUDED.user_id
                RETURNING id, user_id, provider, provider_key, provider_subject,
                    email, bound_at_unix, metadata
                "#,
            )
            .bind(record.id)
            .bind(record.user_id)
            .bind(&record.provider)
            .bind(&record.provider_key)
            .bind(&record.provider_subject)
            .bind(&record.email)
            .bind(record.bound_at_unix)
            .bind(record.metadata.clone())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, OAuthIdentityRow>(
                r#"
                INSERT INTO oauth_identities (
                    user_id, provider, provider_key, provider_subject,
                    email, bound_at_unix, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (provider, provider_key, provider_subject) DO UPDATE SET
                    email = EXCLUDED.email,
                    bound_at_unix = EXCLUDED.bound_at_unix,
                    metadata = EXCLUDED.metadata,
                    updated_at = NOW()
                WHERE oauth_identities.user_id = EXCLUDED.user_id
                RETURNING id, user_id, provider, provider_key, provider_subject,
                    email, bound_at_unix, metadata
                "#,
            )
            .bind(record.user_id)
            .bind(&record.provider)
            .bind(&record.provider_key)
            .bind(&record.provider_subject)
            .bind(&record.email)
            .bind(record.bound_at_unix)
            .bind(record.metadata.clone())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
        };
        row.map(OAuthIdentityRecord::from).ok_or_else(|| {
            RepositoryError::Conflict("oauth identity is already bound to another user".to_owned())
        })
    }

    async fn get_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<OAuthIdentityRecord> {
        let key = oauth_identity_repository_key(provider, provider_key, provider_subject)?;
        let (provider, provider_key, provider_subject) = split_oauth_identity_repository_key(&key)?;
        let row = sqlx::query_as::<_, OAuthIdentityRow>(
            r#"
            SELECT id, user_id, provider, provider_key, provider_subject,
                email, bound_at_unix, metadata
            FROM oauth_identities
            WHERE provider = $1 AND provider_key = $2 AND provider_subject = $3
            "#,
        )
        .bind(provider)
        .bind(provider_key)
        .bind(provider_subject)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(OAuthIdentityRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "oauth_identity",
                id: 0,
            })
    }

    async fn list_oauth_identities_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<OAuthIdentityRecord>> {
        let rows = sqlx::query_as::<_, OAuthIdentityRow>(
            r#"
            SELECT id, user_id, provider, provider_key, provider_subject,
                email, bound_at_unix, metadata
            FROM oauth_identities
            WHERE user_id = $1
            ORDER BY provider, provider_key, provider_subject
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(OAuthIdentityRecord::from).collect())
    }

    async fn delete_oauth_identity(
        &self,
        provider: &str,
        provider_key: &str,
        provider_subject: &str,
    ) -> RepositoryResult<()> {
        let key = oauth_identity_repository_key(provider, provider_key, provider_subject)?;
        let (provider, provider_key, provider_subject) = split_oauth_identity_repository_key(&key)?;
        let result = sqlx::query(
            r#"
            DELETE FROM oauth_identities
            WHERE provider = $1 AND provider_key = $2 AND provider_subject = $3
            "#,
        )
        .bind(provider)
        .bind(provider_key)
        .bind(provider_subject)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "oauth_identity",
                id: 0,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl AuthSessionRepository for PostgresRepository {
    async fn upsert_auth_session(
        &self,
        mut record: AuthSessionRecord,
    ) -> RepositoryResult<AuthSessionRecord> {
        normalize_auth_session_record(&mut record)?;
        let row = if record.id > 0 {
            sqlx::query_as::<_, AuthSessionRow>(
                r#"
                INSERT INTO auth_sessions (
                    id, user_id, access_token, refresh_token,
                    access_expires_at_unix, refresh_expires_at_unix,
                    revoked_at_unix, created_at_unix, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (id) DO UPDATE SET
                    access_token = EXCLUDED.access_token,
                    refresh_token = EXCLUDED.refresh_token,
                    access_expires_at_unix = EXCLUDED.access_expires_at_unix,
                    refresh_expires_at_unix = EXCLUDED.refresh_expires_at_unix,
                    revoked_at_unix = EXCLUDED.revoked_at_unix,
                    metadata = EXCLUDED.metadata,
                    updated_at = NOW()
                RETURNING id, user_id, access_token, refresh_token,
                    access_expires_at_unix, refresh_expires_at_unix,
                    revoked_at_unix, created_at_unix, metadata
                "#,
            )
            .bind(record.id)
            .bind(record.user_id)
            .bind(&record.access_token)
            .bind(&record.refresh_token)
            .bind(record.access_expires_at_unix)
            .bind(record.refresh_expires_at_unix)
            .bind(record.revoked_at_unix)
            .bind(record.created_at_unix)
            .bind(record.metadata.clone())
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, AuthSessionRow>(
                r#"
                INSERT INTO auth_sessions (
                    user_id, access_token, refresh_token,
                    access_expires_at_unix, refresh_expires_at_unix,
                    revoked_at_unix, created_at_unix, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id, user_id, access_token, refresh_token,
                    access_expires_at_unix, refresh_expires_at_unix,
                    revoked_at_unix, created_at_unix, metadata
                "#,
            )
            .bind(record.user_id)
            .bind(&record.access_token)
            .bind(&record.refresh_token)
            .bind(record.access_expires_at_unix)
            .bind(record.refresh_expires_at_unix)
            .bind(record.revoked_at_unix)
            .bind(record.created_at_unix)
            .bind(record.metadata.clone())
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        Ok(row.into())
    }

    async fn get_auth_session_by_access_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "access_token")?;
        let row = sqlx::query_as::<_, AuthSessionRow>(
            r#"
            SELECT id, user_id, access_token, refresh_token,
                access_expires_at_unix, refresh_expires_at_unix,
                revoked_at_unix, created_at_unix, metadata
            FROM auth_sessions
            WHERE access_token = $1
            "#,
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AuthSessionRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })
    }

    async fn get_auth_session_by_refresh_token(
        &self,
        token: &str,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "refresh_token")?;
        let row = sqlx::query_as::<_, AuthSessionRow>(
            r#"
            SELECT id, user_id, access_token, refresh_token,
                access_expires_at_unix, refresh_expires_at_unix,
                revoked_at_unix, created_at_unix, metadata
            FROM auth_sessions
            WHERE refresh_token = $1
            "#,
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AuthSessionRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })
    }

    async fn list_auth_sessions_by_user(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<AuthSessionRecord>> {
        let rows = sqlx::query_as::<_, AuthSessionRow>(
            r#"
            SELECT id, user_id, access_token, refresh_token,
                access_expires_at_unix, refresh_expires_at_unix,
                revoked_at_unix, created_at_unix, metadata
            FROM auth_sessions
            WHERE user_id = $1
            ORDER BY id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(AuthSessionRecord::from).collect())
    }

    async fn revoke_auth_session_by_refresh_token(
        &self,
        token: &str,
        revoked_at_unix: i64,
    ) -> RepositoryResult<AuthSessionRecord> {
        let token = normalize_auth_session_token(token, "refresh_token")?;
        let row = sqlx::query_as::<_, AuthSessionRow>(
            r#"
            UPDATE auth_sessions
            SET revoked_at_unix = $2, updated_at = NOW()
            WHERE refresh_token = $1
            RETURNING id, user_id, access_token, refresh_token,
                access_expires_at_unix, refresh_expires_at_unix,
                revoked_at_unix, created_at_unix, metadata
            "#,
        )
        .bind(token)
        .bind(revoked_at_unix.max(1))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AuthSessionRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_session",
                id: 0,
            })
    }

    async fn revoke_auth_sessions_by_user(
        &self,
        user_id: i64,
        revoked_at_unix: i64,
    ) -> RepositoryResult<i64> {
        let result = sqlx::query(
            r#"
            UPDATE auth_sessions
            SET revoked_at_unix = $2, updated_at = NOW()
            WHERE user_id = $1 AND revoked_at_unix IS NULL
            "#,
        )
        .bind(user_id)
        .bind(revoked_at_unix.max(1))
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected().min(i64::MAX as u64) as i64)
    }
}

#[async_trait]
impl AuthCredentialRepository for PostgresRepository {
    async fn upsert_auth_credential(
        &self,
        mut record: AuthCredentialRecord,
    ) -> RepositoryResult<AuthCredentialRecord> {
        normalize_auth_credential_record(&mut record)?;
        let row = sqlx::query_as::<_, AuthCredentialRow>(
            r#"
            INSERT INTO auth_credentials (
                user_id, email, password_hash, status, updated_at_unix
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id) DO UPDATE SET
                email = EXCLUDED.email,
                password_hash = EXCLUDED.password_hash,
                status = EXCLUDED.status,
                updated_at_unix = EXCLUDED.updated_at_unix,
                updated_at = NOW()
            RETURNING user_id, email, password_hash, status, updated_at_unix
            "#,
        )
        .bind(record.user_id)
        .bind(&record.email)
        .bind(&record.password_hash)
        .bind(&record.status)
        .bind(record.updated_at_unix)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row.into())
    }

    async fn get_auth_credential_by_email(
        &self,
        email: &str,
    ) -> RepositoryResult<AuthCredentialRecord> {
        let email = normalize_auth_credential_email(email)?;
        let row = sqlx::query_as::<_, AuthCredentialRow>(
            r#"
            SELECT user_id, email, password_hash, status, updated_at_unix
            FROM auth_credentials
            WHERE email = $1
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AuthCredentialRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_credential",
                id: 0,
            })
    }

    async fn get_auth_credential_by_user_id(
        &self,
        user_id: i64,
    ) -> RepositoryResult<AuthCredentialRecord> {
        let row = sqlx::query_as::<_, AuthCredentialRow>(
            r#"
            SELECT user_id, email, password_hash, status, updated_at_unix
            FROM auth_credentials
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AuthCredentialRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "auth_credential",
                id: user_id,
            })
    }

    async fn delete_auth_credential(&self, user_id: i64) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM auth_credentials WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "auth_credential",
                id: user_id,
            });
        }
        Ok(())
    }
}

const API_KEY_SELECT: &str = r#"
SELECT
    id, user_id, key, name, group_id, status,
    quota, quota_used, rate_limit_5h, rate_limit_1d, rate_limit_7d,
    usage_5h, usage_1d, usage_7d,
    window_5h_start, window_1d_start, window_7d_start
FROM api_keys
"#;

#[async_trait]
impl ApiKeyRepository for PostgresRepository {
    async fn upsert_api_key(&self, api_key: ApiKey) -> RepositoryResult<ApiKey> {
        sqlx::query(
            r#"
            INSERT INTO api_keys (
                id, user_id, key, name, group_id, status,
                quota, quota_used, rate_limit_5h, rate_limit_1d, rate_limit_7d,
                usage_5h, usage_1d, usage_7d,
                window_5h_start, window_1d_start, window_7d_start
            )
            VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, $11,
                $12, $13, $14,
                $15, $16, $17
            )
            ON CONFLICT (id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                key = EXCLUDED.key,
                name = EXCLUDED.name,
                group_id = EXCLUDED.group_id,
                status = EXCLUDED.status,
                quota = EXCLUDED.quota,
                quota_used = EXCLUDED.quota_used,
                rate_limit_5h = EXCLUDED.rate_limit_5h,
                rate_limit_1d = EXCLUDED.rate_limit_1d,
                rate_limit_7d = EXCLUDED.rate_limit_7d,
                usage_5h = EXCLUDED.usage_5h,
                usage_1d = EXCLUDED.usage_1d,
                usage_7d = EXCLUDED.usage_7d,
                window_5h_start = EXCLUDED.window_5h_start,
                window_1d_start = EXCLUDED.window_1d_start,
                window_7d_start = EXCLUDED.window_7d_start,
                updated_at = NOW()
            "#,
        )
        .bind(api_key.id.0)
        .bind(api_key.user_id)
        .bind(&api_key.key)
        .bind(&api_key.name)
        .bind(api_key.group_id.map(|id| id.0))
        .bind(api_key_status_to_str(api_key.status))
        .bind(api_key.quota)
        .bind(api_key.quota_used)
        .bind(api_key.rate_limit_5h)
        .bind(api_key.rate_limit_1d)
        .bind(api_key.rate_limit_7d)
        .bind(api_key.usage_5h)
        .bind(api_key.usage_1d)
        .bind(api_key.usage_7d)
        .bind(api_key.window_5h_start)
        .bind(api_key.window_1d_start)
        .bind(api_key.window_7d_start)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(api_key)
    }

    async fn get_api_key(&self, id: ApiKeyId) -> RepositoryResult<ApiKey> {
        let row = sqlx::query_as::<_, ApiKeyRow>(&format!("{API_KEY_SELECT} WHERE id = $1"))
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(try_api_key_from_row)
            .transpose()?
            .ok_or(RepositoryError::NotFound {
                entity: "api_key",
                id: id.0,
            })
    }

    async fn get_api_key_by_key(&self, key: &str) -> RepositoryResult<ApiKey> {
        let row = sqlx::query_as::<_, ApiKeyRow>(&format!("{API_KEY_SELECT} WHERE key = $1"))
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(try_api_key_from_row)
            .transpose()?
            .ok_or(RepositoryError::NotFound {
                entity: "api_key",
                id: 0,
            })
    }

    async fn list_api_keys_by_user(&self, user_id: i64) -> RepositoryResult<Vec<ApiKey>> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(&format!(
            "{API_KEY_SELECT} WHERE user_id = $1 ORDER BY id"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_api_key_from_row).collect()
    }

    async fn list_api_keys_by_group(&self, group_id: GroupId) -> RepositoryResult<Vec<ApiKey>> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(&format!(
            "{API_KEY_SELECT} WHERE group_id = $1 ORDER BY id"
        ))
        .bind(group_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_api_key_from_row).collect()
    }

    async fn delete_api_key(&self, id: ApiKeyId) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "api_key",
                id: id.0,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl GroupRepository for PostgresRepository {
    async fn upsert_group(&self, group: Group) -> RepositoryResult<Group> {
        sqlx::query(
            r#"
            INSERT INTO groups (id, name, status)
            VALUES ($1, $2, $3)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                status = EXCLUDED.status,
                updated_at = NOW()
            "#,
        )
        .bind(group.id.0)
        .bind(&group.name)
        .bind(group_status_to_str(group.status))
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(group)
    }

    async fn get_group(&self, id: GroupId) -> RepositoryResult<Group> {
        let row =
            sqlx::query_as::<_, GroupRow>("SELECT id, name, status FROM groups WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_error)?;
        row.map(try_group_from_row)
            .transpose()?
            .ok_or(RepositoryError::NotFound {
                entity: "group",
                id: id.0,
            })
    }

    async fn list_groups(&self) -> RepositoryResult<Vec<Group>> {
        let rows = sqlx::query_as::<_, GroupRow>("SELECT id, name, status FROM groups ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        rows.into_iter().map(try_group_from_row).collect()
    }

    async fn list_active_groups(&self) -> RepositoryResult<Vec<Group>> {
        let rows = sqlx::query_as::<_, GroupRow>(
            "SELECT id, name, status FROM groups WHERE status = 'active' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_group_from_row).collect()
    }

    async fn delete_group(&self, id: GroupId) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM groups WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "group",
                id: id.0,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl AccountRepository for PostgresRepository {
    async fn upsert_account(&self, account: Account) -> RepositoryResult<Account> {
        let mapping = serde_json::to_value(&account.model_mapping)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO accounts (
                id, name, provider, default_upstream_protocol, base_url, api_key,
                model_mapping, extra, enabled
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                provider = EXCLUDED.provider,
                default_upstream_protocol = EXCLUDED.default_upstream_protocol,
                base_url = EXCLUDED.base_url,
                api_key = EXCLUDED.api_key,
                model_mapping = EXCLUDED.model_mapping,
                extra = EXCLUDED.extra,
                enabled = EXCLUDED.enabled,
                updated_at = NOW()
            "#,
        )
        .bind(account.id.0)
        .bind(&account.name)
        .bind(provider_to_str(account.provider))
        .bind(upstream_protocol_to_str(account.default_upstream_protocol))
        .bind(&account.base_url)
        .bind(&account.api_key)
        .bind(mapping)
        .bind(account.extra.clone())
        .bind(account.enabled)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(account)
    }

    async fn get_account(&self, id: AccountId) -> RepositoryResult<Account> {
        let row = sqlx::query_as::<_, AccountRow>(
            r#"
            SELECT
                id, name, provider, default_upstream_protocol, base_url,
                api_key, model_mapping, extra, enabled
            FROM accounts
            WHERE id = $1
            "#,
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(try_account_from_row)
            .transpose()?
            .ok_or(RepositoryError::NotFound {
                entity: "account",
                id: id.0,
            })
    }

    async fn list_accounts(&self) -> RepositoryResult<Vec<Account>> {
        let rows = sqlx::query_as::<_, AccountRow>(
            r#"
            SELECT
                id, name, provider, default_upstream_protocol, base_url,
                api_key, model_mapping, extra, enabled
            FROM accounts
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_account_from_row).collect()
    }

    async fn delete_account(&self, id: AccountId) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "account",
                id: id.0,
            });
        }
        Ok(())
    }

    async fn bind_account_to_group(
        &self,
        binding: AccountGroupBinding,
    ) -> RepositoryResult<AccountGroupBinding> {
        let protocols = binding
            .supported_downstream_protocols
            .iter()
            .map(|protocol| protocol.as_str().to_owned())
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO account_groups (
                account_id, group_id, supported_downstream_protocols,
                upstream_protocol_override, priority
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (account_id, group_id) DO UPDATE SET
                supported_downstream_protocols = EXCLUDED.supported_downstream_protocols,
                upstream_protocol_override = EXCLUDED.upstream_protocol_override,
                priority = EXCLUDED.priority,
                updated_at = NOW()
            "#,
        )
        .bind(binding.account.id.0)
        .bind(binding.group_id.0)
        .bind(protocols)
        .bind(
            binding
                .upstream_protocol_override
                .map(upstream_protocol_to_str),
        )
        .bind(binding.priority)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(binding)
    }

    async fn list_bindings_by_group(
        &self,
        group_id: GroupId,
    ) -> RepositoryResult<Vec<AccountGroupBinding>> {
        let rows = sqlx::query_as::<_, AccountBindingRow>(
            r#"
            SELECT
                a.id,
                a.name,
                a.provider,
                a.default_upstream_protocol,
                a.base_url,
                a.api_key,
                a.model_mapping,
                a.extra,
                a.enabled,
                ag.group_id,
                ag.supported_downstream_protocols,
                ag.upstream_protocol_override,
                ag.priority
            FROM account_groups ag
            JOIN accounts a ON a.id = ag.account_id
            WHERE ag.group_id = $1
            ORDER BY ag.priority, a.id
            "#,
        )
        .bind(group_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_binding_from_row).collect()
    }
}

#[async_trait]
impl UsageRepository for PostgresRepository {
    async fn insert_usage(&self, record: UsageRecord) -> RepositoryResult<UsageRecord> {
        let query = if record.id > 0 {
            r#"
            INSERT INTO usage_logs (
                id, user_id, api_key_id, group_id, account_id, downstream_protocol,
                upstream_protocol, provider, endpoint, requested_model, upstream_model,
                input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
                actual_cost, status, created_at_unix, metadata
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19
            )
            RETURNING id
            "#
        } else {
            r#"
            INSERT INTO usage_logs (
                user_id, api_key_id, group_id, account_id, downstream_protocol,
                upstream_protocol, provider, endpoint, requested_model, upstream_model,
                input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
                actual_cost, status, created_at_unix, metadata
            )
            VALUES (
                $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19
            )
            RETURNING id
            "#
        };
        let id: i64 = sqlx::query_scalar(query)
            .bind(record.id)
            .bind(record.user_id)
            .bind(record.api_key_id)
            .bind(record.group_id.map(|id| id.0))
            .bind(record.account_id.map(|id| id.0))
            .bind(record.downstream_protocol.as_str())
            .bind(&record.upstream_protocol)
            .bind(&record.provider)
            .bind(&record.endpoint)
            .bind(&record.requested_model)
            .bind(&record.upstream_model)
            .bind(record.input_tokens)
            .bind(record.output_tokens)
            .bind(record.cache_creation_tokens)
            .bind(record.cache_read_tokens)
            .bind(record.actual_cost)
            .bind(&record.status)
            .bind(record.created_at_unix)
            .bind(record.metadata.clone())
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(UsageRecord { id, ..record })
    }

    async fn list_usage(&self, filter: UsageFilter) -> RepositoryResult<Vec<UsageRecord>> {
        let rows = sqlx::query_as::<_, UsageRow>(
            r#"
            SELECT
                id, user_id, api_key_id, group_id, account_id, downstream_protocol,
                upstream_protocol, provider, endpoint, requested_model, upstream_model,
                input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
                actual_cost, status, created_at_unix, metadata
            FROM usage_logs
            WHERE ($1::BIGINT IS NULL OR user_id = $1)
              AND ($2::BIGINT IS NULL OR api_key_id = $2)
              AND ($3::BIGINT IS NULL OR group_id = $3)
              AND ($4::BIGINT IS NULL OR account_id = $4)
              AND ($5::TEXT IS NULL OR downstream_protocol = $5)
              AND ($6::TEXT IS NULL OR requested_model ILIKE ('%' || $6 || '%') OR upstream_model ILIKE ('%' || $6 || '%') OR metadata::TEXT ILIKE ('%' || $6 || '%'))
              AND ($7::TEXT IS NULL OR LOWER(metadata->>'request_type') = LOWER($7))
              AND ($8::BOOLEAN IS NULL OR COALESCE((metadata->>'stream')::BOOLEAN, FALSE) = $8)
              AND ($9::TEXT IS NULL OR LOWER(status) = LOWER($9))
              AND ($10::TEXT IS NULL OR LOWER(metadata->>'billing_mode') = LOWER($10))
              AND ($11::SMALLINT IS NULL OR (metadata->>'billing_type') ~ '^-?[0-9]+$' AND (metadata->>'billing_type')::SMALLINT = $11)
              AND ($12::BIGINT IS NULL OR created_at_unix >= $12)
              AND ($13::BIGINT IS NULL OR created_at_unix < $13)
            ORDER BY id
            "#,
        )
        .bind(filter.user_id)
        .bind(filter.api_key_id.map(|id| id.0))
        .bind(filter.group_id.map(|id| id.0))
        .bind(filter.account_id.map(|id| id.0))
        .bind(filter.downstream_protocol.map(|protocol| protocol.as_str()))
        .bind(filter.model_contains.as_deref())
        .bind(filter.request_type.as_deref())
        .bind(filter.stream)
        .bind(filter.status.as_deref())
        .bind(filter.billing_mode.as_deref())
        .bind(filter.billing_type.map(i16::from))
        .bind(filter.created_at_unix_gte)
        .bind(filter.created_at_unix_lt)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(try_usage_from_row).collect()
    }

    async fn summarize_usage(&self, filter: UsageFilter) -> RepositoryResult<UsageSummary> {
        let row = sqlx::query_as::<_, UsageSummaryRow>(
            r#"
            SELECT
                COUNT(*)::BIGINT AS requests,
                COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
                COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
                COALESCE(SUM(actual_cost), 0)::DOUBLE PRECISION AS actual_cost
            FROM usage_logs
            WHERE ($1::BIGINT IS NULL OR user_id = $1)
              AND ($2::BIGINT IS NULL OR api_key_id = $2)
              AND ($3::BIGINT IS NULL OR group_id = $3)
              AND ($4::BIGINT IS NULL OR account_id = $4)
              AND ($5::TEXT IS NULL OR downstream_protocol = $5)
              AND ($6::TEXT IS NULL OR requested_model ILIKE ('%' || $6 || '%') OR upstream_model ILIKE ('%' || $6 || '%') OR metadata::TEXT ILIKE ('%' || $6 || '%'))
              AND ($7::TEXT IS NULL OR LOWER(metadata->>'request_type') = LOWER($7))
              AND ($8::BOOLEAN IS NULL OR COALESCE((metadata->>'stream')::BOOLEAN, FALSE) = $8)
              AND ($9::TEXT IS NULL OR LOWER(status) = LOWER($9))
              AND ($10::TEXT IS NULL OR LOWER(metadata->>'billing_mode') = LOWER($10))
              AND ($11::SMALLINT IS NULL OR (metadata->>'billing_type') ~ '^-?[0-9]+$' AND (metadata->>'billing_type')::SMALLINT = $11)
              AND ($12::BIGINT IS NULL OR created_at_unix >= $12)
              AND ($13::BIGINT IS NULL OR created_at_unix < $13)
            "#,
        )
        .bind(filter.user_id)
        .bind(filter.api_key_id.map(|id| id.0))
        .bind(filter.group_id.map(|id| id.0))
        .bind(filter.account_id.map(|id| id.0))
        .bind(filter.downstream_protocol.map(|protocol| protocol.as_str()))
        .bind(filter.model_contains.as_deref())
        .bind(filter.request_type.as_deref())
        .bind(filter.stream)
        .bind(filter.status.as_deref())
        .bind(filter.billing_mode.as_deref())
        .bind(filter.billing_type.map(i16::from))
        .bind(filter.created_at_unix_gte)
        .bind(filter.created_at_unix_lt)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(UsageSummary {
            requests: row.requests,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            actual_cost: row.actual_cost,
        })
    }

    async fn create_usage_cleanup_task(
        &self,
        mut task: UsageCleanupTaskRecord,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        validate_cleanup_task(&task)?;
        let now = repository_now();
        if task.created_at.trim().is_empty() {
            task.created_at = now.clone();
        }
        if task.updated_at.trim().is_empty() {
            task.updated_at = now;
        }
        let row = if task.id > 0 {
            sqlx::query_as::<_, UsageCleanupTaskRow>(
                r#"
            INSERT INTO usage_cleanup_tasks (
                id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            "#,
            )
            .bind(task.id)
            .bind(&task.status)
            .bind(serde_json::to_value(&task.filters).map_err(|error| {
                RepositoryError::InvalidInput(format!("invalid cleanup filters: {error}"))
            })?)
            .bind(task.created_by)
            .bind(task.deleted_rows)
            .bind(task.error_message.as_deref())
            .bind(task.canceled_by)
            .bind(task.canceled_at.as_deref())
            .bind(task.started_at.as_deref())
            .bind(task.finished_at.as_deref())
            .bind(&task.created_at)
            .bind(&task.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, UsageCleanupTaskRow>(
                r#"
            INSERT INTO usage_cleanup_tasks (
                status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            "#,
            )
            .bind(&task.status)
            .bind(serde_json::to_value(&task.filters).map_err(|error| {
                RepositoryError::InvalidInput(format!("invalid cleanup filters: {error}"))
            })?)
            .bind(task.created_by)
            .bind(task.deleted_rows)
            .bind(task.error_message.as_deref())
            .bind(task.canceled_by)
            .bind(task.canceled_at.as_deref())
            .bind(task.started_at.as_deref())
            .bind(task.finished_at.as_deref())
            .bind(&task.created_at)
            .bind(&task.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        try_usage_cleanup_task_from_row(row)
    }

    async fn list_usage_cleanup_tasks(
        &self,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<UsageCleanupTaskRecord>> {
        let pagination = Pagination::new(pagination.page, pagination.page_size);
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM usage_cleanup_tasks")
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        let rows = sqlx::query_as::<_, UsageCleanupTaskRow>(
            r#"
            SELECT id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            FROM usage_cleanup_tasks
            ORDER BY created_at DESC, id DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(pagination.page_size)
        .bind(pagination.offset())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let items = rows
            .into_iter()
            .map(try_usage_cleanup_task_from_row)
            .collect::<RepositoryResult<Vec<_>>>()?;
        Ok(PaginatedRecords {
            items,
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }

    async fn get_usage_cleanup_task_status(&self, task_id: i64) -> RepositoryResult<String> {
        sqlx::query_scalar::<_, String>("SELECT status FROM usage_cleanup_tasks WHERE id = $1")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?
            .ok_or(RepositoryError::NotFound {
                entity: "usage_cleanup_task",
                id: task_id,
            })
    }

    async fn claim_next_usage_cleanup_task(
        &self,
        stale_running_after_seconds: i64,
    ) -> RepositoryResult<Option<UsageCleanupTaskRecord>> {
        let stale_running_after_seconds = stale_running_after_seconds.max(1);
        let now = repository_now();
        let row = sqlx::query_as::<_, UsageCleanupTaskRow>(
            r#"
            WITH next AS (
                SELECT id
                FROM usage_cleanup_tasks
                WHERE status = 'pending'
                   OR (
                       status = 'running'
                       AND started_at IS NOT NULL
                       AND started_at < NOW() - ($1 * interval '1 second')
                   )
                ORDER BY created_at ASC, id ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE usage_cleanup_tasks AS task
            SET status = 'running',
                started_at = NOW(),
                started_at_text = $2,
                finished_at = NULL,
                finished_at_text = NULL,
                error_message = NULL,
                updated_at = NOW(),
                updated_at_text = $2
            FROM next
            WHERE task.id = next.id
            RETURNING task.id, task.status, task.filters, task.created_by, task.deleted_rows,
                task.error_message, task.canceled_by, task.canceled_at_text,
                task.started_at_text, task.finished_at_text, task.created_at_text,
                task.updated_at_text
            "#,
        )
        .bind(stale_running_after_seconds)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(try_usage_cleanup_task_from_row).transpose()
    }

    async fn update_usage_cleanup_task_progress(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<()> {
        let rows = sqlx::query(
            r#"
            UPDATE usage_cleanup_tasks
            SET deleted_rows = $1,
                updated_at = NOW(),
                updated_at_text = $2
            WHERE id = $3
            "#,
        )
        .bind(deleted_rows.max(0))
        .bind(repository_now())
        .bind(task_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        if rows == 0 {
            return Err(RepositoryError::NotFound {
                entity: "usage_cleanup_task",
                id: task_id,
            });
        }
        Ok(())
    }

    async fn cancel_usage_cleanup_task(
        &self,
        task_id: i64,
        canceled_by: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        if self.get_usage_cleanup_task_status(task_id).await? == "canceled" {
            return self.usage_cleanup_task_by_id(task_id).await;
        }
        let now = repository_now();
        let row = sqlx::query_as::<_, UsageCleanupTaskRow>(
            r#"
            UPDATE usage_cleanup_tasks
            SET status = 'canceled',
                canceled_by = $1,
                canceled_at = NOW(),
                canceled_at_text = $2,
                finished_at = NOW(),
                finished_at_text = $2,
                error_message = NULL,
                updated_at = NOW(),
                updated_at_text = $2
            WHERE id = $3
              AND status IN ('pending', 'running')
            RETURNING id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            "#,
        )
        .bind(canceled_by)
        .bind(&now)
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        match row {
            Some(row) => try_usage_cleanup_task_from_row(row),
            None => Err(RepositoryError::Conflict(
                "cleanup task cannot be canceled in current status".to_owned(),
            )),
        }
    }

    async fn mark_usage_cleanup_task_succeeded(
        &self,
        task_id: i64,
        deleted_rows: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        self.finish_usage_cleanup_task(task_id, "succeeded", deleted_rows, None)
            .await
    }

    async fn mark_usage_cleanup_task_failed(
        &self,
        task_id: i64,
        deleted_rows: i64,
        error_message: String,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        self.finish_usage_cleanup_task(task_id, "failed", deleted_rows, Some(error_message))
            .await
    }

    async fn delete_usage_batch(
        &self,
        filter: UsageCleanupFilter,
        limit: i64,
    ) -> RepositoryResult<i64> {
        validate_cleanup_filter(&filter)?;
        let rows = sqlx::query(
            r#"
            WITH target AS (
                SELECT id
                FROM usage_logs
                WHERE created_at_unix >= $1
                  AND created_at_unix < $2
                  AND ($3::BIGINT IS NULL OR user_id = $3)
                  AND ($4::BIGINT IS NULL OR api_key_id = $4)
                  AND ($5::BIGINT IS NULL OR group_id = $5)
                  AND ($6::BIGINT IS NULL OR account_id = $6)
                  AND ($7::TEXT IS NULL OR requested_model = $7 OR upstream_model = $7)
                  AND ($8::TEXT IS NULL OR LOWER(metadata->>'request_type') = LOWER($8))
                  AND ($9::BOOLEAN IS NULL OR COALESCE((metadata->>'stream')::BOOLEAN, FALSE) = $9)
                  AND ($10::SMALLINT IS NULL OR (metadata->>'billing_type')::SMALLINT = $10)
                ORDER BY created_at_unix ASC, id ASC
                LIMIT $11
            )
            DELETE FROM usage_logs
            WHERE id IN (SELECT id FROM target)
            "#,
        )
        .bind(filter.start_time_unix)
        .bind(filter.end_time_unix)
        .bind(filter.user_id)
        .bind(filter.api_key_id.map(|id| id.0))
        .bind(filter.group_id.map(|id| id.0))
        .bind(filter.account_id.map(|id| id.0))
        .bind(filter.model.as_deref())
        .bind(filter.request_type.as_deref())
        .bind(filter.stream)
        .bind(filter.billing_type.map(i16::from))
        .bind(limit.max(1))
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        Ok(rows as i64)
    }
}

impl PostgresRepository {
    async fn usage_cleanup_task_by_id(
        &self,
        task_id: i64,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        let row = sqlx::query_as::<_, UsageCleanupTaskRow>(
            r#"
            SELECT id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            FROM usage_cleanup_tasks
            WHERE id = $1
            "#,
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        .ok_or(RepositoryError::NotFound {
            entity: "usage_cleanup_task",
            id: task_id,
        })?;
        try_usage_cleanup_task_from_row(row)
    }

    async fn finish_usage_cleanup_task(
        &self,
        task_id: i64,
        status: &str,
        deleted_rows: i64,
        error_message: Option<String>,
    ) -> RepositoryResult<UsageCleanupTaskRecord> {
        let now = repository_now();
        let row = sqlx::query_as::<_, UsageCleanupTaskRow>(
            r#"
            UPDATE usage_cleanup_tasks
            SET status = $1,
                deleted_rows = $2,
                error_message = $3,
                finished_at = NOW(),
                finished_at_text = $4,
                updated_at = NOW(),
                updated_at_text = $4
            WHERE id = $5
            RETURNING id, status, filters, created_by, deleted_rows, error_message,
                canceled_by, canceled_at_text, started_at_text, finished_at_text,
                created_at_text, updated_at_text
            "#,
        )
        .bind(status)
        .bind(deleted_rows.max(0))
        .bind(error_message.as_deref())
        .bind(&now)
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        .ok_or(RepositoryError::NotFound {
            entity: "usage_cleanup_task",
            id: task_id,
        })?;
        try_usage_cleanup_task_from_row(row)
    }
}

#[async_trait]
impl PaymentOrderRepository for PostgresRepository {
    async fn upsert_payment_order(
        &self,
        order: PaymentOrderRecord,
    ) -> RepositoryResult<PaymentOrderRecord> {
        if order.out_trade_no.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment order out_trade_no is required".to_owned(),
            ));
        }
        let query = if order.id > 0 {
            r#"
            INSERT INTO payment_orders (
                id, user_id, amount, pay_amount, currency, fee_rate, payment_type,
                out_trade_no, status, order_type, refund_amount, refund_reason,
                refund_request_reason, plan_id, provider_instance_id, created_at_text,
                expires_at_text, paid_at_text, completed_at_text, cancelled_at_text,
                refund_requested_at_text, refunded_at_text, webhook_count, metadata
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24
            )
            ON CONFLICT (id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                amount = EXCLUDED.amount,
                pay_amount = EXCLUDED.pay_amount,
                currency = EXCLUDED.currency,
                fee_rate = EXCLUDED.fee_rate,
                payment_type = EXCLUDED.payment_type,
                out_trade_no = EXCLUDED.out_trade_no,
                status = EXCLUDED.status,
                order_type = EXCLUDED.order_type,
                refund_amount = EXCLUDED.refund_amount,
                refund_reason = EXCLUDED.refund_reason,
                refund_request_reason = EXCLUDED.refund_request_reason,
                plan_id = EXCLUDED.plan_id,
                provider_instance_id = EXCLUDED.provider_instance_id,
                created_at_text = EXCLUDED.created_at_text,
                expires_at_text = EXCLUDED.expires_at_text,
                paid_at_text = EXCLUDED.paid_at_text,
                completed_at_text = EXCLUDED.completed_at_text,
                cancelled_at_text = EXCLUDED.cancelled_at_text,
                refund_requested_at_text = EXCLUDED.refund_requested_at_text,
                refunded_at_text = EXCLUDED.refunded_at_text,
                webhook_count = EXCLUDED.webhook_count,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            RETURNING id
            "#
        } else {
            r#"
            INSERT INTO payment_orders (
                user_id, amount, pay_amount, currency, fee_rate, payment_type,
                out_trade_no, status, order_type, refund_amount, refund_reason,
                refund_request_reason, plan_id, provider_instance_id, created_at_text,
                expires_at_text, paid_at_text, completed_at_text, cancelled_at_text,
                refund_requested_at_text, refunded_at_text, webhook_count, metadata
            )
            VALUES (
                $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24
            )
            ON CONFLICT (out_trade_no) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                amount = EXCLUDED.amount,
                pay_amount = EXCLUDED.pay_amount,
                currency = EXCLUDED.currency,
                fee_rate = EXCLUDED.fee_rate,
                payment_type = EXCLUDED.payment_type,
                status = EXCLUDED.status,
                order_type = EXCLUDED.order_type,
                refund_amount = EXCLUDED.refund_amount,
                refund_reason = EXCLUDED.refund_reason,
                refund_request_reason = EXCLUDED.refund_request_reason,
                plan_id = EXCLUDED.plan_id,
                provider_instance_id = EXCLUDED.provider_instance_id,
                created_at_text = EXCLUDED.created_at_text,
                expires_at_text = EXCLUDED.expires_at_text,
                paid_at_text = EXCLUDED.paid_at_text,
                completed_at_text = EXCLUDED.completed_at_text,
                cancelled_at_text = EXCLUDED.cancelled_at_text,
                refund_requested_at_text = EXCLUDED.refund_requested_at_text,
                refunded_at_text = EXCLUDED.refunded_at_text,
                webhook_count = EXCLUDED.webhook_count,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            RETURNING id
            "#
        };
        let id: i64 = sqlx::query_scalar(query)
            .bind(order.id)
            .bind(order.user_id)
            .bind(order.amount)
            .bind(order.pay_amount)
            .bind(&order.currency)
            .bind(order.fee_rate)
            .bind(&order.payment_type)
            .bind(&order.out_trade_no)
            .bind(&order.status)
            .bind(&order.order_type)
            .bind(order.refund_amount)
            .bind(&order.refund_reason)
            .bind(&order.refund_request_reason)
            .bind(order.plan_id)
            .bind(&order.provider_instance_id)
            .bind(&order.created_at)
            .bind(&order.expires_at)
            .bind(&order.paid_at)
            .bind(&order.completed_at)
            .bind(&order.cancelled_at)
            .bind(&order.refund_requested_at)
            .bind(&order.refunded_at)
            .bind(order.webhook_count)
            .bind(order.metadata.clone())
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(PaymentOrderRecord { id, ..order })
    }

    async fn get_payment_order(&self, id: i64) -> RepositoryResult<PaymentOrderRecord> {
        let row = sqlx::query_as::<_, PaymentOrderRow>(PAYMENT_ORDER_SELECT_BY_ID)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(PaymentOrderRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "payment_order",
                id,
            })
    }

    async fn get_payment_order_by_trade_no(
        &self,
        out_trade_no: &str,
    ) -> RepositoryResult<PaymentOrderRecord> {
        let row = sqlx::query_as::<_, PaymentOrderRow>(PAYMENT_ORDER_SELECT_BY_TRADE_NO)
            .bind(out_trade_no)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        row.map(PaymentOrderRecord::from)
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "payment_order",
                id: 0,
            })
    }

    async fn list_payment_orders(
        &self,
        filter: PaymentOrderFilter,
    ) -> RepositoryResult<Vec<PaymentOrderRecord>> {
        let rows = sqlx::query_as::<_, PaymentOrderRow>(
            r#"
            SELECT
                id, user_id, amount, pay_amount, currency, fee_rate, payment_type,
                out_trade_no, status, order_type, refund_amount, refund_reason,
                refund_request_reason, plan_id, provider_instance_id, created_at_text,
                expires_at_text, paid_at_text, completed_at_text, cancelled_at_text,
                refund_requested_at_text, refunded_at_text, webhook_count, metadata
            FROM payment_orders
            WHERE ($1::BIGINT IS NULL OR user_id = $1)
              AND ($2::TEXT IS NULL OR upper(status) = upper($2))
              AND ($3::TEXT IS NULL OR lower(payment_type) = lower($3))
              AND ($4::TEXT IS NULL OR provider_instance_id = $4)
              AND ($5::TEXT IS NULL OR lower(out_trade_no) LIKE '%' || lower($5) || '%')
            ORDER BY id
            "#,
        )
        .bind(filter.user_id)
        .bind(filter.status)
        .bind(filter.payment_type)
        .bind(filter.provider_instance_id)
        .bind(filter.out_trade_no_contains)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(PaymentOrderRecord::from).collect())
    }
}

#[async_trait]
impl PaymentAuditRepository for PostgresRepository {
    async fn insert_payment_audit(
        &self,
        record: PaymentAuditRecord,
    ) -> RepositoryResult<PaymentAuditRecord> {
        if record.order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment audit order_id is required".to_owned(),
            ));
        }
        let query = if record.id > 0 {
            r#"
            INSERT INTO payment_audit_logs (id, order_id, action, detail, operator, created_at_text)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#
        } else {
            r#"
            INSERT INTO payment_audit_logs (order_id, action, detail, operator, created_at_text)
            VALUES ($2, $3, $4, $5, $6)
            RETURNING id
            "#
        };
        let id: i64 = sqlx::query_scalar(query)
            .bind(record.id)
            .bind(&record.order_id)
            .bind(&record.action)
            .bind(&record.detail)
            .bind(&record.operator)
            .bind(&record.created_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(PaymentAuditRecord { id, ..record })
    }

    async fn list_payment_audits(
        &self,
        order_id: &str,
    ) -> RepositoryResult<Vec<PaymentAuditRecord>> {
        let rows = sqlx::query_as::<_, PaymentAuditRow>(
            r#"
            SELECT id, order_id, action, detail, operator, created_at_text
            FROM payment_audit_logs
            WHERE order_id = $1
            ORDER BY id
            "#,
        )
        .bind(order_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(PaymentAuditRecord::from).collect())
    }
}

#[async_trait]
impl BalanceRepository for PostgresRepository {
    async fn get_user_balance(&self, user_id: i64) -> RepositoryResult<UserBalanceRecord> {
        let row = sqlx::query_as::<_, UserBalanceRow>(
            r#"
            SELECT user_id, balance, updated_at_text
            FROM user_balances
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row
            .map(UserBalanceRecord::from)
            .unwrap_or(UserBalanceRecord {
                user_id,
                balance: 0.0,
                updated_at: String::new(),
            }))
    }

    async fn apply_balance_transaction(
        &self,
        record: BalanceTransactionRecord,
    ) -> RepositoryResult<BalanceTransactionRecord> {
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "balance transaction user_id is required".to_owned(),
            ));
        }
        if record.order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "balance transaction order_id is required".to_owned(),
            ));
        }
        if let Some(existing) = sqlx::query_as::<_, BalanceTransactionRow>(
            r#"
            SELECT id, user_id, order_id, transaction_type, amount, balance_after,
                   created_at_text, metadata
            FROM balance_transactions
            WHERE order_id = $1 AND transaction_type = $2
            "#,
        )
        .bind(&record.order_id)
        .bind(&record.transaction_type)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?
        {
            return Ok(BalanceTransactionRecord::from(existing));
        }

        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let inserted_id: Option<i64> = sqlx::query_scalar(
            r#"
            INSERT INTO balance_transactions (
                user_id, order_id, transaction_type, amount, balance_after,
                created_at_text, metadata
            )
            VALUES ($1, $2, $3, $4, 0, $5, $6)
            ON CONFLICT (order_id, transaction_type) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(record.user_id)
        .bind(&record.order_id)
        .bind(&record.transaction_type)
        .bind(record.amount)
        .bind(&record.created_at)
        .bind(record.metadata.clone())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(id) = inserted_id else {
            tx.rollback().await.map_err(db_error)?;
            let existing = sqlx::query_as::<_, BalanceTransactionRow>(
                r#"
                SELECT id, user_id, order_id, transaction_type, amount, balance_after,
                       created_at_text, metadata
                FROM balance_transactions
                WHERE order_id = $1 AND transaction_type = $2
                "#,
            )
            .bind(&record.order_id)
            .bind(&record.transaction_type)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
            return Ok(BalanceTransactionRecord::from(existing));
        };
        let balance_after: f64 = sqlx::query_scalar(
            r#"
            INSERT INTO user_balances (user_id, balance, updated_at_text)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id) DO UPDATE SET
                balance = user_balances.balance + EXCLUDED.balance,
                updated_at_text = EXCLUDED.updated_at_text,
                updated_at = NOW()
            RETURNING balance
            "#,
        )
        .bind(record.user_id)
        .bind(record.amount)
        .bind(&record.created_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query(
            r#"
            UPDATE balance_transactions
            SET balance_after = $1
            WHERE id = $2
            "#,
        )
        .bind(balance_after)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;

        Ok(BalanceTransactionRecord {
            id,
            balance_after,
            ..record
        })
    }

    async fn list_balance_transactions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<BalanceTransactionRecord>> {
        let rows = sqlx::query_as::<_, BalanceTransactionRow>(
            r#"
            SELECT id, user_id, order_id, transaction_type, amount, balance_after,
                   created_at_text, metadata
            FROM balance_transactions
            WHERE user_id = $1
            ORDER BY id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(BalanceTransactionRecord::from)
            .collect())
    }
}

#[async_trait]
impl SubscriptionRepository for PostgresRepository {
    async fn upsert_user_subscription(
        &self,
        record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription user_id is required".to_owned(),
            ));
        }
        if record.group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription group_id is required".to_owned(),
            ));
        }
        if record.source_order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription source_order_id is required".to_owned(),
            ));
        }
        let query = if record.id > 0 {
            r#"
            INSERT INTO user_subscriptions (
                id, user_id, group_id, plan_id, status, starts_at_text,
                expires_at_text, daily_usage_usd, weekly_usage_usd,
                monthly_usage_usd, daily_window_start_text,
                weekly_window_start_text, monthly_window_start_text,
                source_order_id, created_at_text, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (source_order_id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                group_id = EXCLUDED.group_id,
                plan_id = EXCLUDED.plan_id,
                status = EXCLUDED.status,
                starts_at_text = EXCLUDED.starts_at_text,
                expires_at_text = EXCLUDED.expires_at_text,
                daily_usage_usd = EXCLUDED.daily_usage_usd,
                weekly_usage_usd = EXCLUDED.weekly_usage_usd,
                monthly_usage_usd = EXCLUDED.monthly_usage_usd,
                daily_window_start_text = EXCLUDED.daily_window_start_text,
                weekly_window_start_text = EXCLUDED.weekly_window_start_text,
                monthly_window_start_text = EXCLUDED.monthly_window_start_text,
                created_at_text = EXCLUDED.created_at_text,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            RETURNING id
            "#
        } else {
            r#"
            INSERT INTO user_subscriptions (
                user_id, group_id, plan_id, status, starts_at_text,
                expires_at_text, daily_usage_usd, weekly_usage_usd,
                monthly_usage_usd, daily_window_start_text,
                weekly_window_start_text, monthly_window_start_text,
                source_order_id, created_at_text, metadata
            )
            VALUES ($2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (source_order_id) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                group_id = EXCLUDED.group_id,
                plan_id = EXCLUDED.plan_id,
                status = EXCLUDED.status,
                starts_at_text = EXCLUDED.starts_at_text,
                expires_at_text = EXCLUDED.expires_at_text,
                daily_usage_usd = EXCLUDED.daily_usage_usd,
                weekly_usage_usd = EXCLUDED.weekly_usage_usd,
                monthly_usage_usd = EXCLUDED.monthly_usage_usd,
                daily_window_start_text = EXCLUDED.daily_window_start_text,
                weekly_window_start_text = EXCLUDED.weekly_window_start_text,
                monthly_window_start_text = EXCLUDED.monthly_window_start_text,
                created_at_text = EXCLUDED.created_at_text,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            RETURNING id
            "#
        };
        let id: i64 = sqlx::query_scalar(query)
            .bind(record.id)
            .bind(record.user_id)
            .bind(record.group_id)
            .bind(record.plan_id)
            .bind(&record.status)
            .bind(&record.starts_at)
            .bind(&record.expires_at)
            .bind(record.daily_usage_usd)
            .bind(record.weekly_usage_usd)
            .bind(record.monthly_usage_usd)
            .bind(&record.daily_window_start)
            .bind(&record.weekly_window_start)
            .bind(&record.monthly_window_start)
            .bind(&record.source_order_id)
            .bind(&record.created_at)
            .bind(record.metadata.clone())
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(UserSubscriptionRecord { id, ..record })
    }

    async fn get_user_subscription(&self, id: i64) -> RepositoryResult<UserSubscriptionRecord> {
        let row = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            SELECT id, user_id, group_id, plan_id, status, starts_at_text,
                   expires_at_text, daily_usage_usd, weekly_usage_usd,
                   monthly_usage_usd, daily_window_start_text,
                   weekly_window_start_text, monthly_window_start_text,
                   source_order_id, created_at_text, metadata
            FROM user_subscriptions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserSubscriptionRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id,
            })
    }

    async fn update_user_subscription(
        &self,
        record: UserSubscriptionRecord,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if record.id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription id is required".to_owned(),
            ));
        }
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription user_id is required".to_owned(),
            ));
        }
        if record.group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "subscription group_id is required".to_owned(),
            ));
        }
        if record.source_order_id.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription source_order_id is required".to_owned(),
            ));
        }
        let row = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            UPDATE user_subscriptions
            SET user_id = $2,
                group_id = $3,
                plan_id = $4,
                status = $5,
                starts_at_text = $6,
                expires_at_text = $7,
                daily_usage_usd = $8,
                weekly_usage_usd = $9,
                monthly_usage_usd = $10,
                daily_window_start_text = $11,
                weekly_window_start_text = $12,
                monthly_window_start_text = $13,
                source_order_id = $14,
                created_at_text = $15,
                metadata = $16,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, user_id, group_id, plan_id, status, starts_at_text,
                      expires_at_text, daily_usage_usd, weekly_usage_usd,
                      monthly_usage_usd, daily_window_start_text,
                      weekly_window_start_text, monthly_window_start_text,
                      source_order_id, created_at_text, metadata
            "#,
        )
        .bind(record.id)
        .bind(record.user_id)
        .bind(record.group_id)
        .bind(record.plan_id)
        .bind(&record.status)
        .bind(&record.starts_at)
        .bind(&record.expires_at)
        .bind(record.daily_usage_usd)
        .bind(record.weekly_usage_usd)
        .bind(record.monthly_usage_usd)
        .bind(&record.daily_window_start)
        .bind(&record.weekly_window_start)
        .bind(&record.monthly_window_start)
        .bind(&record.source_order_id)
        .bind(&record.created_at)
        .bind(record.metadata)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserSubscriptionRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "subscription",
                id: record.id,
            })
    }

    async fn delete_user_subscription(&self, id: i64) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM user_subscriptions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "subscription",
                id,
            });
        }
        Ok(())
    }

    async fn update_subscription_status_by_source_order(
        &self,
        source_order_id: &str,
        status: &str,
        metadata: Value,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        if status.trim().is_empty() {
            return Err(RepositoryError::InvalidInput(
                "subscription status is required".to_owned(),
            ));
        }
        let row = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            UPDATE user_subscriptions
            SET status = $2,
                metadata = metadata || $3::jsonb,
                updated_at = NOW()
            WHERE source_order_id = $1
            RETURNING id, user_id, group_id, plan_id, status, starts_at_text,
                      expires_at_text, daily_usage_usd, weekly_usage_usd,
                      monthly_usage_usd, daily_window_start_text,
                      weekly_window_start_text, monthly_window_start_text,
                      source_order_id, created_at_text, metadata
            "#,
        )
        .bind(source_order_id)
        .bind(status)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserSubscriptionRecord::from)
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "subscription",
                id: 0,
            })
    }

    async fn get_subscription_by_source_order(
        &self,
        source_order_id: &str,
    ) -> RepositoryResult<UserSubscriptionRecord> {
        let row = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            SELECT id, user_id, group_id, plan_id, status, starts_at_text,
                   expires_at_text, daily_usage_usd, weekly_usage_usd,
                   monthly_usage_usd, daily_window_start_text,
                   weekly_window_start_text, monthly_window_start_text,
                   source_order_id, created_at_text, metadata
            FROM user_subscriptions
            WHERE source_order_id = $1
            "#,
        )
        .bind(source_order_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(UserSubscriptionRecord::from)
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "subscription",
                id: 0,
            })
    }

    async fn list_user_subscriptions(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserSubscriptionRecord>> {
        let rows = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            SELECT id, user_id, group_id, plan_id, status, starts_at_text,
                   expires_at_text, daily_usage_usd, weekly_usage_usd,
                   monthly_usage_usd, daily_window_start_text,
                   weekly_window_start_text, monthly_window_start_text,
                   source_order_id, created_at_text, metadata
            FROM user_subscriptions
            WHERE user_id = $1
            ORDER BY id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(UserSubscriptionRecord::from).collect())
    }

    async fn list_subscriptions(&self) -> RepositoryResult<Vec<UserSubscriptionRecord>> {
        let rows = sqlx::query_as::<_, UserSubscriptionRow>(
            r#"
            SELECT id, user_id, group_id, plan_id, status, starts_at_text,
                   expires_at_text, daily_usage_usd, weekly_usage_usd,
                   monthly_usage_usd, daily_window_start_text,
                   weekly_window_start_text, monthly_window_start_text,
                   source_order_id, created_at_text, metadata
            FROM user_subscriptions
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(UserSubscriptionRecord::from).collect())
    }
}

#[async_trait]
impl UserPlatformQuotaRepository for PostgresRepository {
    async fn list_user_platform_quotas(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>> {
        let rows = sqlx::query_as::<_, UserPlatformQuotaRow>(
            r#"
            SELECT id, user_id, platform, daily_limit_usd, weekly_limit_usd,
                   monthly_limit_usd, daily_usage_usd, weekly_usage_usd,
                   monthly_usage_usd, daily_window_start_text,
                   weekly_window_start_text, monthly_window_start_text
            FROM user_platform_quotas
            WHERE user_id = $1 AND deleted_at IS NULL
            ORDER BY platform
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(UserPlatformQuotaRecord::from)
            .collect())
    }

    async fn replace_user_platform_quotas(
        &self,
        user_id: i64,
        records: Vec<UserPlatformQuotaRecord>,
    ) -> RepositoryResult<Vec<UserPlatformQuotaRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "quota user_id is required".to_owned(),
            ));
        }
        validate_user_platform_quota_records(&records)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            r#"
            UPDATE user_platform_quotas
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE user_id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        for record in records {
            let platform = normalize_quota_platform(&record.platform)?;
            sqlx::query(
                r#"
                INSERT INTO user_platform_quotas (
                    user_id, platform, daily_limit_usd, weekly_limit_usd,
                    monthly_limit_usd, daily_usage_usd, weekly_usage_usd,
                    monthly_usage_usd, daily_window_start_text,
                    weekly_window_start_text, monthly_window_start_text
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (user_id, platform) WHERE deleted_at IS NULL
                DO UPDATE SET
                    daily_limit_usd = EXCLUDED.daily_limit_usd,
                    weekly_limit_usd = EXCLUDED.weekly_limit_usd,
                    monthly_limit_usd = EXCLUDED.monthly_limit_usd,
                    daily_usage_usd = EXCLUDED.daily_usage_usd,
                    weekly_usage_usd = EXCLUDED.weekly_usage_usd,
                    monthly_usage_usd = EXCLUDED.monthly_usage_usd,
                    daily_window_start_text = EXCLUDED.daily_window_start_text,
                    weekly_window_start_text = EXCLUDED.weekly_window_start_text,
                    monthly_window_start_text = EXCLUDED.monthly_window_start_text,
                    deleted_at = NULL,
                    updated_at = NOW()
                "#,
            )
            .bind(user_id)
            .bind(platform)
            .bind(record.daily_limit_usd)
            .bind(record.weekly_limit_usd)
            .bind(record.monthly_limit_usd)
            .bind(record.daily_usage_usd)
            .bind(record.weekly_usage_usd)
            .bind(record.monthly_usage_usd)
            .bind(record.daily_window_start)
            .bind(record.weekly_window_start)
            .bind(record.monthly_window_start)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        self.list_user_platform_quotas(user_id).await
    }

    async fn increment_user_platform_quota_usage(
        &self,
        user_id: i64,
        platform: &str,
        cost: f64,
        daily_window_start: String,
        weekly_window_start: String,
        monthly_window_start: String,
    ) -> RepositoryResult<UserPlatformQuotaRecord> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "quota user_id is required".to_owned(),
            ));
        }
        let platform = normalize_quota_platform(platform)?;
        let row = sqlx::query_as::<_, UserPlatformQuotaRow>(
            r#"
            INSERT INTO user_platform_quotas (
                user_id, platform, daily_usage_usd, weekly_usage_usd, monthly_usage_usd,
                daily_window_start_text, weekly_window_start_text, monthly_window_start_text
            )
            VALUES ($1, $2, $3, $3, $3, $4, $5, $6)
            ON CONFLICT (user_id, platform) WHERE deleted_at IS NULL
            DO UPDATE SET
                daily_usage_usd = CASE
                    WHEN user_platform_quotas.daily_window_start_text = EXCLUDED.daily_window_start_text
                    THEN user_platform_quotas.daily_usage_usd + EXCLUDED.daily_usage_usd
                    ELSE EXCLUDED.daily_usage_usd
                END,
                weekly_usage_usd = CASE
                    WHEN user_platform_quotas.weekly_window_start_text = EXCLUDED.weekly_window_start_text
                    THEN user_platform_quotas.weekly_usage_usd + EXCLUDED.weekly_usage_usd
                    ELSE EXCLUDED.weekly_usage_usd
                END,
                monthly_usage_usd = CASE
                    WHEN user_platform_quotas.monthly_window_start_text IS NOT NULL
                         AND EXCLUDED.monthly_window_start_text IS NOT NULL
                         AND (EXCLUDED.monthly_window_start_text::timestamptz - user_platform_quotas.monthly_window_start_text::timestamptz) < INTERVAL '30 days'
                    THEN user_platform_quotas.monthly_usage_usd + EXCLUDED.monthly_usage_usd
                    ELSE EXCLUDED.monthly_usage_usd
                END,
                daily_window_start_text = EXCLUDED.daily_window_start_text,
                weekly_window_start_text = EXCLUDED.weekly_window_start_text,
                monthly_window_start_text = CASE
                    WHEN user_platform_quotas.monthly_window_start_text IS NOT NULL
                         AND EXCLUDED.monthly_window_start_text IS NOT NULL
                         AND (EXCLUDED.monthly_window_start_text::timestamptz - user_platform_quotas.monthly_window_start_text::timestamptz) < INTERVAL '30 days'
                    THEN user_platform_quotas.monthly_window_start_text
                    ELSE EXCLUDED.monthly_window_start_text
                END,
                updated_at = NOW()
            RETURNING id, user_id, platform, daily_limit_usd, weekly_limit_usd,
                      monthly_limit_usd, daily_usage_usd, weekly_usage_usd,
                      monthly_usage_usd, daily_window_start_text,
                      weekly_window_start_text, monthly_window_start_text
            "#,
        )
        .bind(user_id)
        .bind(platform)
        .bind(cost)
        .bind(daily_window_start)
        .bind(weekly_window_start)
        .bind(monthly_window_start)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(UserPlatformQuotaRecord::from(row))
    }
}

#[async_trait]
impl UserGroupRateRepository for PostgresRepository {
    async fn list_user_group_rates(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "user_id is required".to_owned(),
            ));
        }
        let rows = sqlx::query_as::<_, UserGroupRateRow>(
            r#"
            SELECT user_id, group_id, rate_multiplier, rpm_override, updated_at_text
            FROM user_group_rates
            WHERE user_id = $1 AND rate_multiplier IS NOT NULL
            ORDER BY group_id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(UserGroupRateRecord::from).collect())
    }

    async fn list_group_rate_overrides(
        &self,
        group_id: i64,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let rows = sqlx::query_as::<_, UserGroupRateRow>(
            r#"
            SELECT user_id, group_id, rate_multiplier, rpm_override, updated_at_text
            FROM user_group_rates
            WHERE group_id = $1 AND (rate_multiplier IS NOT NULL OR rpm_override IS NOT NULL)
            ORDER BY user_id
            "#,
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(UserGroupRateRecord::from).collect())
    }

    async fn replace_group_rate_multipliers(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let records = validate_user_group_rate_records(group_id, records, true)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            r#"
            UPDATE user_group_rates
            SET rate_multiplier = NULL, updated_at_text = $2, updated_at = NOW()
            WHERE group_id = $1
            "#,
        )
        .bind(group_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        for record in records {
            sqlx::query(
                r#"
                INSERT INTO user_group_rates (
                    user_id, group_id, rate_multiplier, rpm_override, updated_at_text
                )
                VALUES ($1, $2, $3, NULL, $4)
                ON CONFLICT (user_id, group_id) DO UPDATE SET
                    rate_multiplier = EXCLUDED.rate_multiplier,
                    updated_at_text = EXCLUDED.updated_at_text,
                    updated_at = NOW()
                "#,
            )
            .bind(record.user_id)
            .bind(group_id)
            .bind(record.rate_multiplier)
            .bind(record.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        sqlx::query(
            "DELETE FROM user_group_rates WHERE group_id = $1 AND rate_multiplier IS NULL AND rpm_override IS NULL",
        )
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.list_group_rate_overrides(group_id).await
    }

    async fn clear_group_rate_multipliers(&self, group_id: i64) -> RepositoryResult<()> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        sqlx::query(
            r#"
            UPDATE user_group_rates
            SET rate_multiplier = NULL, updated_at_text = $2, updated_at = NOW()
            WHERE group_id = $1
            "#,
        )
        .bind(group_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        sqlx::query(
            "DELETE FROM user_group_rates WHERE group_id = $1 AND rate_multiplier IS NULL AND rpm_override IS NULL",
        )
        .bind(group_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn replace_group_rpm_overrides(
        &self,
        group_id: i64,
        records: Vec<UserGroupRateRecord>,
    ) -> RepositoryResult<Vec<UserGroupRateRecord>> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        let records = validate_user_group_rate_records(group_id, records, false)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query(
            r#"
            UPDATE user_group_rates
            SET rpm_override = NULL, updated_at_text = $2, updated_at = NOW()
            WHERE group_id = $1
            "#,
        )
        .bind(group_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        for record in records {
            sqlx::query(
                r#"
                INSERT INTO user_group_rates (
                    user_id, group_id, rate_multiplier, rpm_override, updated_at_text
                )
                VALUES ($1, $2, NULL, $3, $4)
                ON CONFLICT (user_id, group_id) DO UPDATE SET
                    rpm_override = EXCLUDED.rpm_override,
                    updated_at_text = EXCLUDED.updated_at_text,
                    updated_at = NOW()
                "#,
            )
            .bind(record.user_id)
            .bind(group_id)
            .bind(record.rpm_override)
            .bind(record.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        sqlx::query(
            "DELETE FROM user_group_rates WHERE group_id = $1 AND rate_multiplier IS NULL AND rpm_override IS NULL",
        )
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.list_group_rate_overrides(group_id).await
    }

    async fn clear_group_rpm_overrides(&self, group_id: i64) -> RepositoryResult<()> {
        if group_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "group_id is required".to_owned(),
            ));
        }
        sqlx::query(
            r#"
            UPDATE user_group_rates
            SET rpm_override = NULL, updated_at_text = $2, updated_at = NOW()
            WHERE group_id = $1
            "#,
        )
        .bind(group_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        sqlx::query(
            "DELETE FROM user_group_rates WHERE group_id = $1 AND rate_multiplier IS NULL AND rpm_override IS NULL",
        )
        .bind(group_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[async_trait]
impl UserAttributeValueRepository for PostgresRepository {
    async fn list_user_attribute_values(
        &self,
        user_id: i64,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>> {
        if user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "user_id is required".to_owned(),
            ));
        }
        let rows = sqlx::query_as::<_, UserAttributeValueRow>(
            r#"
            SELECT user_id, attribute_id, value, created_at_text, updated_at_text
            FROM user_attribute_values
            WHERE user_id = $1
            ORDER BY attribute_id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(UserAttributeValueRecord::from)
            .collect())
    }

    async fn replace_user_attribute_values(
        &self,
        user_id: i64,
        records: Vec<UserAttributeValueRecord>,
    ) -> RepositoryResult<Vec<UserAttributeValueRecord>> {
        let records = validate_user_attribute_value_records(user_id, records)?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        sqlx::query("DELETE FROM user_attribute_values WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        for record in records {
            sqlx::query(
                r#"
                INSERT INTO user_attribute_values (
                    user_id, attribute_id, value, created_at_text, updated_at_text
                )
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (user_id, attribute_id) DO UPDATE SET
                    value = EXCLUDED.value,
                    updated_at_text = EXCLUDED.updated_at_text,
                    updated_at = NOW()
                "#,
            )
            .bind(user_id)
            .bind(record.attribute_id)
            .bind(record.value)
            .bind(record.created_at)
            .bind(record.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        self.list_user_attribute_values(user_id).await
    }
}

#[async_trait]
impl ChannelMonitorHistoryRepository for PostgresRepository {
    async fn insert_channel_monitor_history(
        &self,
        record: ChannelMonitorHistoryRecord,
    ) -> RepositoryResult<ChannelMonitorHistoryRecord> {
        let record = validate_channel_monitor_history_record(record)?;
        let row = if record.id > 0 {
            sqlx::query_as::<_, ChannelMonitorHistoryRow>(
                r#"
                INSERT INTO channel_monitor_history (
                    id, monitor_id, model, status, latency_ms, ping_latency_ms,
                    message, checked_at_text, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (id) DO UPDATE SET
                    monitor_id = EXCLUDED.monitor_id,
                    model = EXCLUDED.model,
                    status = EXCLUDED.status,
                    latency_ms = EXCLUDED.latency_ms,
                    ping_latency_ms = EXCLUDED.ping_latency_ms,
                    message = EXCLUDED.message,
                    checked_at_text = EXCLUDED.checked_at_text,
                    metadata = EXCLUDED.metadata
                RETURNING id, monitor_id, model, status, latency_ms, ping_latency_ms,
                    message, checked_at_text, metadata
                "#,
            )
            .bind(record.id)
            .bind(record.monitor_id)
            .bind(record.model)
            .bind(record.status)
            .bind(record.latency_ms)
            .bind(record.ping_latency_ms)
            .bind(record.message)
            .bind(record.checked_at)
            .bind(record.metadata)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, ChannelMonitorHistoryRow>(
                r#"
                INSERT INTO channel_monitor_history (
                    monitor_id, model, status, latency_ms, ping_latency_ms,
                    message, checked_at_text, metadata
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id, monitor_id, model, status, latency_ms, ping_latency_ms,
                    message, checked_at_text, metadata
                "#,
            )
            .bind(record.monitor_id)
            .bind(record.model)
            .bind(record.status)
            .bind(record.latency_ms)
            .bind(record.ping_latency_ms)
            .bind(record.message)
            .bind(record.checked_at)
            .bind(record.metadata)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        Ok(ChannelMonitorHistoryRecord::from(row))
    }

    async fn list_channel_monitor_history(
        &self,
        monitor_id: i64,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ChannelMonitorHistoryRecord>> {
        if monitor_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "monitor_id is required".to_owned(),
            ));
        }
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM channel_monitor_history WHERE monitor_id = $1",
        )
        .bind(monitor_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let rows = sqlx::query_as::<_, ChannelMonitorHistoryRow>(
            r#"
            SELECT id, monitor_id, model, status, latency_ms, ping_latency_ms,
                message, checked_at_text, metadata
            FROM channel_monitor_history
            WHERE monitor_id = $1
            ORDER BY checked_at_text DESC, id DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(monitor_id)
        .bind(pagination.page_size)
        .bind(pagination.offset())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(PaginatedRecords {
            items: rows
                .into_iter()
                .map(ChannelMonitorHistoryRecord::from)
                .collect(),
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }
}

#[async_trait]
impl ContentModerationLogRepository for PostgresRepository {
    async fn insert_content_moderation_log(
        &self,
        record: ContentModerationLogRecord,
    ) -> RepositoryResult<ContentModerationLogRecord> {
        let record = validate_content_moderation_log_record(record)?;
        let row = if record.id > 0 {
            sqlx::query_as::<_, ContentModerationLogRow>(
                r#"
                INSERT INTO content_moderation_logs (
                    id, request_id, user_id, user_email, api_key_id, api_key_name,
                    group_id, group_name, endpoint, provider, model, mode, action,
                    flagged, highest_category, highest_score, category_scores,
                    threshold_snapshot, input_excerpt, upstream_latency_ms, error,
                    violation_count, auto_banned, email_sent, queue_delay_ms, created_at_text
                )
                VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                    $21, $22, $23, $24, $25, $26
                )
                ON CONFLICT (id) DO UPDATE SET
                    request_id = EXCLUDED.request_id,
                    user_id = EXCLUDED.user_id,
                    user_email = EXCLUDED.user_email,
                    api_key_id = EXCLUDED.api_key_id,
                    api_key_name = EXCLUDED.api_key_name,
                    group_id = EXCLUDED.group_id,
                    group_name = EXCLUDED.group_name,
                    endpoint = EXCLUDED.endpoint,
                    provider = EXCLUDED.provider,
                    model = EXCLUDED.model,
                    mode = EXCLUDED.mode,
                    action = EXCLUDED.action,
                    flagged = EXCLUDED.flagged,
                    highest_category = EXCLUDED.highest_category,
                    highest_score = EXCLUDED.highest_score,
                    category_scores = EXCLUDED.category_scores,
                    threshold_snapshot = EXCLUDED.threshold_snapshot,
                    input_excerpt = EXCLUDED.input_excerpt,
                    upstream_latency_ms = EXCLUDED.upstream_latency_ms,
                    error = EXCLUDED.error,
                    violation_count = EXCLUDED.violation_count,
                    auto_banned = EXCLUDED.auto_banned,
                    email_sent = EXCLUDED.email_sent,
                    queue_delay_ms = EXCLUDED.queue_delay_ms,
                    created_at_text = EXCLUDED.created_at_text
                RETURNING id, request_id, user_id, user_email, api_key_id, api_key_name,
                    group_id, group_name, endpoint, provider, model, mode, action,
                    flagged, highest_category, highest_score, category_scores,
                    threshold_snapshot, input_excerpt, upstream_latency_ms, error,
                    violation_count, auto_banned, email_sent, ''::TEXT AS user_status,
                    queue_delay_ms, created_at_text
                "#,
            )
            .bind(record.id)
            .bind(&record.request_id)
            .bind(record.user_id)
            .bind(&record.user_email)
            .bind(record.api_key_id)
            .bind(&record.api_key_name)
            .bind(record.group_id)
            .bind(&record.group_name)
            .bind(&record.endpoint)
            .bind(&record.provider)
            .bind(&record.model)
            .bind(&record.mode)
            .bind(&record.action)
            .bind(record.flagged)
            .bind(&record.highest_category)
            .bind(record.highest_score)
            .bind(record.category_scores.clone())
            .bind(record.threshold_snapshot.clone())
            .bind(&record.input_excerpt)
            .bind(record.upstream_latency_ms)
            .bind(&record.error)
            .bind(record.violation_count)
            .bind(record.auto_banned)
            .bind(record.email_sent)
            .bind(record.queue_delay_ms)
            .bind(&record.created_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, ContentModerationLogRow>(
                r#"
                INSERT INTO content_moderation_logs (
                    request_id, user_id, user_email, api_key_id, api_key_name,
                    group_id, group_name, endpoint, provider, model, mode, action,
                    flagged, highest_category, highest_score, category_scores,
                    threshold_snapshot, input_excerpt, upstream_latency_ms, error,
                    violation_count, auto_banned, email_sent, queue_delay_ms, created_at_text
                )
                VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18, $19,
                    $20, $21, $22, $23, $24, $25
                )
                RETURNING id, request_id, user_id, user_email, api_key_id, api_key_name,
                    group_id, group_name, endpoint, provider, model, mode, action,
                    flagged, highest_category, highest_score, category_scores,
                    threshold_snapshot, input_excerpt, upstream_latency_ms, error,
                    violation_count, auto_banned, email_sent, ''::TEXT AS user_status,
                    queue_delay_ms, created_at_text
                "#,
            )
            .bind(&record.request_id)
            .bind(record.user_id)
            .bind(&record.user_email)
            .bind(record.api_key_id)
            .bind(&record.api_key_name)
            .bind(record.group_id)
            .bind(&record.group_name)
            .bind(&record.endpoint)
            .bind(&record.provider)
            .bind(&record.model)
            .bind(&record.mode)
            .bind(&record.action)
            .bind(record.flagged)
            .bind(&record.highest_category)
            .bind(record.highest_score)
            .bind(record.category_scores.clone())
            .bind(record.threshold_snapshot.clone())
            .bind(&record.input_excerpt)
            .bind(record.upstream_latency_ms)
            .bind(&record.error)
            .bind(record.violation_count)
            .bind(record.auto_banned)
            .bind(record.email_sent)
            .bind(record.queue_delay_ms)
            .bind(&record.created_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        Ok(ContentModerationLogRecord::from(row))
    }

    async fn list_content_moderation_logs(
        &self,
        filter: ContentModerationLogFilter,
        pagination: Pagination,
    ) -> RepositoryResult<PaginatedRecords<ContentModerationLogRecord>> {
        let result_filter = normalize_content_moderation_result_filter(filter.result.as_deref());
        let total = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM content_moderation_logs l
            WHERE ($1::TEXT IS NULL OR
                    (LOWER($1) IN ('hit', 'flagged') AND l.flagged = TRUE) OR
                    (LOWER($1) IN ('blocked', 'block') AND l.action IN ('block', 'keyword_block', 'hash_block')) OR
                    (LOWER($1) IN ('pass', 'allow') AND l.flagged = FALSE AND l.error = '') OR
                    (LOWER($1) = 'error' AND l.error <> ''))
              AND ($2::BIGINT IS NULL OR l.group_id = $2)
              AND ($3::TEXT IS NULL OR l.endpoint = $3)
              AND ($4::TEXT IS NULL OR l.request_id ILIKE ('%' || $4 || '%')
                    OR l.user_email ILIKE ('%' || $4 || '%')
                    OR l.api_key_name ILIKE ('%' || $4 || '%')
                    OR l.model ILIKE ('%' || $4 || '%')
                    OR l.input_excerpt ILIKE ('%' || $4 || '%'))
              AND ($5::TEXT IS NULL OR l.created_at_text >= $5)
              AND ($6::TEXT IS NULL OR l.created_at_text <= $6)
            "#,
        )
        .bind(result_filter.as_deref())
        .bind(filter.group_id)
        .bind(normalized_optional_filter(filter.endpoint.as_deref()))
        .bind(normalized_optional_filter(filter.search.as_deref()))
        .bind(normalized_optional_filter(filter.created_at_gte.as_deref()))
        .bind(normalized_optional_filter(filter.created_at_lte.as_deref()))
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let rows = sqlx::query_as::<_, ContentModerationLogRow>(
            r#"
            SELECT l.id, l.request_id, l.user_id, l.user_email, l.api_key_id, l.api_key_name,
                l.group_id, l.group_name, l.endpoint, l.provider, l.model, l.mode, l.action,
                l.flagged, l.highest_category, l.highest_score, l.category_scores,
                l.threshold_snapshot, l.input_excerpt, l.upstream_latency_ms, l.error,
                l.violation_count, l.auto_banned, l.email_sent, COALESCE(u.status, '') AS user_status,
                l.queue_delay_ms, l.created_at_text
            FROM content_moderation_logs l
            LEFT JOIN users u ON u.id = l.user_id
            WHERE ($1::TEXT IS NULL OR
                    (LOWER($1) IN ('hit', 'flagged') AND l.flagged = TRUE) OR
                    (LOWER($1) IN ('blocked', 'block') AND l.action IN ('block', 'keyword_block', 'hash_block')) OR
                    (LOWER($1) IN ('pass', 'allow') AND l.flagged = FALSE AND l.error = '') OR
                    (LOWER($1) = 'error' AND l.error <> ''))
              AND ($2::BIGINT IS NULL OR l.group_id = $2)
              AND ($3::TEXT IS NULL OR l.endpoint = $3)
              AND ($4::TEXT IS NULL OR l.request_id ILIKE ('%' || $4 || '%')
                    OR l.user_email ILIKE ('%' || $4 || '%')
                    OR l.api_key_name ILIKE ('%' || $4 || '%')
                    OR l.model ILIKE ('%' || $4 || '%')
                    OR l.input_excerpt ILIKE ('%' || $4 || '%'))
              AND ($5::TEXT IS NULL OR l.created_at_text >= $5)
              AND ($6::TEXT IS NULL OR l.created_at_text <= $6)
            ORDER BY l.created_at_text DESC, l.id DESC
            LIMIT $7 OFFSET $8
            "#,
        )
        .bind(result_filter.as_deref())
        .bind(filter.group_id)
        .bind(normalized_optional_filter(filter.endpoint.as_deref()))
        .bind(normalized_optional_filter(filter.search.as_deref()))
        .bind(normalized_optional_filter(filter.created_at_gte.as_deref()))
        .bind(normalized_optional_filter(filter.created_at_lte.as_deref()))
        .bind(pagination.page_size)
        .bind(pagination.offset())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(PaginatedRecords {
            items: rows
                .into_iter()
                .map(ContentModerationLogRecord::from)
                .collect(),
            total,
            page: pagination.page,
            page_size: pagination.page_size,
        })
    }
}

#[async_trait]
impl PaymentProviderRepository for PostgresRepository {
    async fn upsert_payment_provider_instance(
        &self,
        mut record: PaymentProviderInstanceRecord,
    ) -> RepositoryResult<PaymentProviderInstanceRecord> {
        record.provider_key = normalize_repository_string(&record.provider_key);
        record.supported_types = normalize_repository_strings(&record.supported_types);
        if record.provider_key.is_empty() {
            return Err(RepositoryError::InvalidInput(
                "payment provider provider_key is required".to_owned(),
            ));
        }
        if record.name.trim().is_empty() {
            record.name = record.provider_key.clone();
        }
        let query = if record.id > 0 {
            r#"
            INSERT INTO payment_provider_instances (
                id, provider_key, name, config, supported_types, enabled,
                payment_mode, sort_order, limits, refund_enabled, allow_user_refund,
                created_at_text, updated_at_text
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (id) DO UPDATE SET
                provider_key = EXCLUDED.provider_key,
                name = EXCLUDED.name,
                config = EXCLUDED.config,
                supported_types = EXCLUDED.supported_types,
                enabled = EXCLUDED.enabled,
                payment_mode = EXCLUDED.payment_mode,
                sort_order = EXCLUDED.sort_order,
                limits = EXCLUDED.limits,
                refund_enabled = EXCLUDED.refund_enabled,
                allow_user_refund = EXCLUDED.allow_user_refund,
                updated_at_text = EXCLUDED.updated_at_text,
                updated_at = NOW()
            RETURNING id
            "#
        } else {
            r#"
            INSERT INTO payment_provider_instances (
                provider_key, name, config, supported_types, enabled,
                payment_mode, sort_order, limits, refund_enabled, allow_user_refund,
                created_at_text, updated_at_text
            )
            VALUES ($2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING id
            "#
        };
        let id: i64 = sqlx::query_scalar(query)
            .bind(record.id)
            .bind(&record.provider_key)
            .bind(&record.name)
            .bind(record.config.clone())
            .bind(&record.supported_types)
            .bind(record.enabled)
            .bind(&record.payment_mode)
            .bind(record.sort_order)
            .bind(record.limits.clone())
            .bind(record.refund_enabled)
            .bind(record.allow_user_refund)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(PaymentProviderInstanceRecord { id, ..record })
    }

    async fn get_payment_provider_instance(
        &self,
        id: i64,
    ) -> RepositoryResult<PaymentProviderInstanceRecord> {
        let row = sqlx::query_as::<_, PaymentProviderInstanceRow>(
            r#"
            SELECT id, provider_key, name, config, supported_types, enabled,
                   payment_mode, sort_order, limits, refund_enabled, allow_user_refund,
                   created_at_text, updated_at_text
            FROM payment_provider_instances
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(PaymentProviderInstanceRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "payment_provider_instance",
                id,
            })
    }

    async fn list_payment_provider_instances(
        &self,
    ) -> RepositoryResult<Vec<PaymentProviderInstanceRecord>> {
        let rows = sqlx::query_as::<_, PaymentProviderInstanceRow>(
            r#"
            SELECT id, provider_key, name, config, supported_types, enabled,
                   payment_mode, sort_order, limits, refund_enabled, allow_user_refund,
                   created_at_text, updated_at_text
            FROM payment_provider_instances
            ORDER BY sort_order, id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(PaymentProviderInstanceRecord::from)
            .collect())
    }

    async fn delete_payment_provider_instance(&self, id: i64) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM payment_provider_instances WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "payment_provider_instance",
                id,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl PaymentPlanRepository for PostgresRepository {
    async fn upsert_payment_plan(
        &self,
        record: PaymentPlanRecord,
    ) -> RepositoryResult<PaymentPlanRecord> {
        validate_payment_plan(&record, true)?;
        let name = record.name.trim().to_owned();
        let validity_unit = record.validity_unit.trim().to_owned();
        let features = normalize_plan_features(record.features.clone());
        let row = if record.id > 0 {
            sqlx::query_as::<_, PaymentPlanRow>(
                r#"
                INSERT INTO payment_plans (
                    id, group_id, name, description, price, original_price,
                    validity_days, validity_unit, features, product_name,
                    for_sale, sort_order, created_at_text, updated_at_text
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                ON CONFLICT (id) DO UPDATE SET
                    group_id = EXCLUDED.group_id,
                    name = EXCLUDED.name,
                    description = EXCLUDED.description,
                    price = EXCLUDED.price,
                    original_price = EXCLUDED.original_price,
                    validity_days = EXCLUDED.validity_days,
                    validity_unit = EXCLUDED.validity_unit,
                    features = EXCLUDED.features,
                    product_name = EXCLUDED.product_name,
                    for_sale = EXCLUDED.for_sale,
                    sort_order = EXCLUDED.sort_order,
                    updated_at_text = EXCLUDED.updated_at_text,
                    updated_at = NOW()
                RETURNING id, group_id, name, description, price, original_price,
                          validity_days, validity_unit, features, product_name,
                          for_sale, sort_order, created_at_text, updated_at_text
                "#,
            )
            .bind(record.id)
            .bind(record.group_id)
            .bind(&name)
            .bind(&record.description)
            .bind(record.price)
            .bind(record.original_price)
            .bind(record.validity_days)
            .bind(&validity_unit)
            .bind(features.clone())
            .bind(&record.product_name)
            .bind(record.for_sale)
            .bind(record.sort_order)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, PaymentPlanRow>(
                r#"
                INSERT INTO payment_plans (
                    group_id, name, description, price, original_price,
                    validity_days, validity_unit, features, product_name,
                    for_sale, sort_order, created_at_text, updated_at_text
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                RETURNING id, group_id, name, description, price, original_price,
                          validity_days, validity_unit, features, product_name,
                          for_sale, sort_order, created_at_text, updated_at_text
                "#,
            )
            .bind(record.group_id)
            .bind(&name)
            .bind(&record.description)
            .bind(record.price)
            .bind(record.original_price)
            .bind(record.validity_days)
            .bind(&validity_unit)
            .bind(features)
            .bind(&record.product_name)
            .bind(record.for_sale)
            .bind(record.sort_order)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        Ok(PaymentPlanRecord::from(row))
    }

    async fn get_payment_plan(&self, id: i64) -> RepositoryResult<PaymentPlanRecord> {
        let row = sqlx::query_as::<_, PaymentPlanRow>(
            r#"
            SELECT id, group_id, name, description, price, original_price,
                   validity_days, validity_unit, features, product_name,
                   for_sale, sort_order, created_at_text, updated_at_text
            FROM payment_plans
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(PaymentPlanRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "payment_plan",
                id,
            })
    }

    async fn list_payment_plans(&self) -> RepositoryResult<Vec<PaymentPlanRecord>> {
        let rows = sqlx::query_as::<_, PaymentPlanRow>(
            r#"
            SELECT id, group_id, name, description, price, original_price,
                   validity_days, validity_unit, features, product_name,
                   for_sale, sort_order, created_at_text, updated_at_text
            FROM payment_plans
            ORDER BY sort_order, id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(PaymentPlanRecord::from).collect())
    }

    async fn list_payment_plans_for_sale(&self) -> RepositoryResult<Vec<PaymentPlanRecord>> {
        let rows = sqlx::query_as::<_, PaymentPlanRow>(
            r#"
            SELECT id, group_id, name, description, price, original_price,
                   validity_days, validity_unit, features, product_name,
                   for_sale, sort_order, created_at_text, updated_at_text
            FROM payment_plans
            WHERE for_sale = TRUE
            ORDER BY sort_order, id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(PaymentPlanRecord::from).collect())
    }

    async fn delete_payment_plan(&self, id: i64) -> RepositoryResult<()> {
        let result = sqlx::query("DELETE FROM payment_plans WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "payment_plan",
                id,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl AdminCollectionRepository for PostgresRepository {
    async fn upsert_admin_collection_item(
        &self,
        record: AdminCollectionItemRecord,
    ) -> RepositoryResult<AdminCollectionItemRecord> {
        let collection = normalize_admin_collection_name(&record.collection)?;
        if record.id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "admin collection item id is required".to_owned(),
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO admin_collection_items (collection, id, item)
            VALUES ($1, $2, $3)
            ON CONFLICT (collection, id) DO UPDATE SET
                item = EXCLUDED.item,
                updated_at = NOW()
            "#,
        )
        .bind(&collection)
        .bind(record.id)
        .bind(record.item.clone())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(AdminCollectionItemRecord {
            collection,
            id: record.id,
            item: record.item,
        })
    }

    async fn get_admin_collection_item(
        &self,
        collection: &str,
        id: i64,
    ) -> RepositoryResult<AdminCollectionItemRecord> {
        let collection = normalize_admin_collection_name(collection)?;
        let row = sqlx::query_as::<_, AdminCollectionItemRow>(
            r#"
            SELECT collection, id, item
            FROM admin_collection_items
            WHERE collection = $1 AND id = $2
            "#,
        )
        .bind(&collection)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(AdminCollectionItemRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "admin_collection_item",
                id,
            })
    }

    async fn list_admin_collection_items(
        &self,
        collection: &str,
    ) -> RepositoryResult<Vec<AdminCollectionItemRecord>> {
        let collection = normalize_admin_collection_name(collection)?;
        let rows = sqlx::query_as::<_, AdminCollectionItemRow>(
            r#"
            SELECT collection, id, item
            FROM admin_collection_items
            WHERE collection = $1
            ORDER BY id
            "#,
        )
        .bind(&collection)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(AdminCollectionItemRecord::from)
            .collect())
    }

    async fn delete_admin_collection_item(
        &self,
        collection: &str,
        id: i64,
    ) -> RepositoryResult<()> {
        let collection = normalize_admin_collection_name(collection)?;
        let result =
            sqlx::query("DELETE FROM admin_collection_items WHERE collection = $1 AND id = $2")
                .bind(&collection)
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "admin_collection_item",
                id,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl SystemSettingRepository for PostgresRepository {
    async fn upsert_system_setting(
        &self,
        record: SystemSettingRecord,
    ) -> RepositoryResult<SystemSettingRecord> {
        let namespace = normalize_setting_namespace(&record.namespace)?;
        let key = normalize_setting_key(&record.key)?;
        sqlx::query(
            r#"
            INSERT INTO system_settings (namespace, key, value, updated_at_text)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (namespace, key) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at_text = EXCLUDED.updated_at_text,
                updated_at = NOW()
            "#,
        )
        .bind(&namespace)
        .bind(&key)
        .bind(record.value.clone())
        .bind(&record.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(SystemSettingRecord {
            namespace,
            key,
            value: record.value,
            updated_at: record.updated_at,
        })
    }

    async fn get_system_setting(
        &self,
        namespace: &str,
        key: &str,
    ) -> RepositoryResult<SystemSettingRecord> {
        let namespace = normalize_setting_namespace(namespace)?;
        let key = normalize_setting_key(key)?;
        let row = sqlx::query_as::<_, SystemSettingRow>(
            r#"
            SELECT namespace, key, value, updated_at_text
            FROM system_settings
            WHERE namespace = $1 AND key = $2
            "#,
        )
        .bind(&namespace)
        .bind(&key)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(SystemSettingRecord::from)
            .ok_or(RepositoryError::NotFound {
                entity: "system_setting",
                id: 0,
            })
    }

    async fn list_system_settings(
        &self,
        namespace: &str,
    ) -> RepositoryResult<Vec<SystemSettingRecord>> {
        let namespace = normalize_setting_namespace(namespace)?;
        let rows = sqlx::query_as::<_, SystemSettingRow>(
            r#"
            SELECT namespace, key, value, updated_at_text
            FROM system_settings
            WHERE namespace = $1
            ORDER BY key
            "#,
        )
        .bind(&namespace)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(SystemSettingRecord::from).collect())
    }

    async fn delete_system_setting(&self, namespace: &str, key: &str) -> RepositoryResult<()> {
        let namespace = normalize_setting_namespace(namespace)?;
        let key = normalize_setting_key(key)?;
        let result = sqlx::query("DELETE FROM system_settings WHERE namespace = $1 AND key = $2")
            .bind(&namespace)
            .bind(&key)
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                entity: "system_setting",
                id: 0,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl EmailQueueTaskRepository for PostgresRepository {
    async fn enqueue_email_task(
        &self,
        mut record: EmailQueueTaskRecord,
    ) -> RepositoryResult<EmailQueueTaskRecord> {
        normalize_email_queue_task_record(&mut record)?;
        let row = if record.id > 0 {
            sqlx::query_as::<_, EmailQueueTaskRow>(
                r#"
                INSERT INTO email_queue_tasks (
                    id, task_type, status, payload, attempts, max_attempts,
                    last_error, created_at_text, updated_at_text
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (id) DO UPDATE SET
                    task_type = EXCLUDED.task_type,
                    status = EXCLUDED.status,
                    payload = EXCLUDED.payload,
                    attempts = EXCLUDED.attempts,
                    max_attempts = EXCLUDED.max_attempts,
                    last_error = EXCLUDED.last_error,
                    updated_at_text = EXCLUDED.updated_at_text,
                    updated_at = NOW()
                RETURNING id, task_type, status, payload, attempts, max_attempts,
                    last_error, created_at_text, updated_at_text
                "#,
            )
            .bind(record.id)
            .bind(&record.task_type)
            .bind(&record.status)
            .bind(record.payload.clone())
            .bind(record.attempts)
            .bind(record.max_attempts)
            .bind(&record.last_error)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            sqlx::query_as::<_, EmailQueueTaskRow>(
                r#"
                INSERT INTO email_queue_tasks (
                    task_type, status, payload, attempts, max_attempts,
                    last_error, created_at_text, updated_at_text
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id, task_type, status, payload, attempts, max_attempts,
                    last_error, created_at_text, updated_at_text
                "#,
            )
            .bind(&record.task_type)
            .bind(&record.status)
            .bind(record.payload.clone())
            .bind(record.attempts)
            .bind(record.max_attempts)
            .bind(&record.last_error)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?
        };
        Ok(row.into())
    }

    async fn list_pending_email_tasks(
        &self,
        limit: i64,
    ) -> RepositoryResult<Vec<EmailQueueTaskRecord>> {
        let rows = sqlx::query_as::<_, EmailQueueTaskRow>(
            r#"
            SELECT id, task_type, status, payload, attempts, max_attempts,
                last_error, created_at_text, updated_at_text
            FROM email_queue_tasks
            WHERE status IN ('pending', 'processing')
                AND attempts < max_attempts
            ORDER BY id
            LIMIT $1
            "#,
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(EmailQueueTaskRecord::from).collect())
    }

    async fn mark_email_task_processing(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord> {
        update_email_task_status(&self.pool, id, "processing", None, true).await
    }

    async fn mark_email_task_sent(&self, id: i64) -> RepositoryResult<EmailQueueTaskRecord> {
        update_email_task_status(&self.pool, id, "sent", None, false).await
    }

    async fn mark_email_task_failed(
        &self,
        id: i64,
        last_error: String,
    ) -> RepositoryResult<EmailQueueTaskRecord> {
        update_email_task_status(&self.pool, id, "failed", Some(last_error), false).await
    }
}

#[async_trait]
impl ConsistencyRepository for InMemoryRepository {
    async fn try_acquire_lock(
        &self,
        name: &str,
        owner: &str,
        ttl_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<LockLeaseRecord>> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let expires_at = Utc::now() + Duration::seconds(ttl_seconds.max(1));
        let expires_at_text = expires_at.to_rfc3339();
        let mut locks = self
            .distributed_locks
            .write()
            .expect("distributed lock repository lock");
        if let Some(existing) = locks.get(&name) {
            if parse_rfc3339_utc(&existing.expires_at)
                .map(|value| value > Utc::now())
                .unwrap_or(false)
                && existing.owner != owner
            {
                return Ok(None);
            }
        }
        let fencing_token = locks
            .get(&name)
            .map(|record| record.fencing_token.saturating_add(1))
            .unwrap_or(1);
        let record = LockLeaseRecord {
            name: name.clone(),
            owner,
            fencing_token,
            expires_at: expires_at_text,
            metadata,
        };
        locks.insert(name, record.clone());
        Ok(Some(record))
    }

    async fn renew_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
        ttl_seconds: i64,
    ) -> RepositoryResult<bool> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let mut locks = self
            .distributed_locks
            .write()
            .expect("distributed lock repository lock");
        let Some(record) = locks.get_mut(&name) else {
            return Ok(false);
        };
        if record.owner != owner || record.fencing_token != fencing_token {
            return Ok(false);
        }
        record.expires_at = (Utc::now() + Duration::seconds(ttl_seconds.max(1))).to_rfc3339();
        Ok(true)
    }

    async fn release_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
    ) -> RepositoryResult<bool> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let mut locks = self
            .distributed_locks
            .write()
            .expect("distributed lock repository lock");
        let should_remove = locks
            .get(&name)
            .map(|record| record.owner == owner && record.fencing_token == fencing_token)
            .unwrap_or(false);
        if should_remove {
            locks.remove(&name);
        }
        Ok(should_remove)
    }

    async fn create_idempotent_job(
        &self,
        job_type: &str,
        idempotency_key: &str,
        payload: Value,
    ) -> RepositoryResult<IdempotentJobCreateResult> {
        let job_type = normalize_consistency_name(job_type, "job type")?;
        let idempotency_key = normalize_consistency_name(idempotency_key, "idempotency key")?;
        let key = (job_type.clone(), idempotency_key.clone());
        if let Some(id) = self
            .idempotent_job_id_by_key
            .read()
            .expect("idempotent job key repository lock")
            .get(&key)
            .copied()
        {
            let job = self
                .idempotent_jobs
                .read()
                .expect("idempotent job repository lock")
                .get(&id)
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    entity: "idempotent_job",
                    id,
                })?;
            return Ok(IdempotentJobCreateResult {
                job,
                created: false,
            });
        }
        let now = repository_now();
        let id = self.next_idempotent_job_id.fetch_add(1, Ordering::SeqCst);
        let job = IdempotentJobRecord {
            id,
            job_type: job_type.clone(),
            idempotency_key: idempotency_key.clone(),
            status: "pending".to_owned(),
            payload,
            result: None,
            attempts: 0,
            lease_owner: None,
            lease_expires_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.idempotent_jobs
            .write()
            .expect("idempotent job repository lock")
            .insert(id, job.clone());
        self.idempotent_job_id_by_key
            .write()
            .expect("idempotent job key repository lock")
            .insert(key, id);
        Ok(IdempotentJobCreateResult { job, created: true })
    }

    async fn claim_next_idempotent_job(
        &self,
        job_type: &str,
        owner: &str,
        lease_seconds: i64,
    ) -> RepositoryResult<Option<IdempotentJobRecord>> {
        let job_type = normalize_consistency_name(job_type, "job type")?;
        let owner = normalize_consistency_name(owner, "job owner")?;
        let now = Utc::now();
        let mut jobs = self
            .idempotent_jobs
            .write()
            .expect("idempotent job repository lock");
        let candidate_id = jobs
            .values()
            .filter(|job| {
                job.job_type == job_type
                    && (job.status == "pending"
                        || (job.status == "running"
                            && job
                                .lease_expires_at
                                .as_deref()
                                .and_then(parse_rfc3339_utc)
                                .map(|expires| expires <= now)
                                .unwrap_or(true)))
            })
            .min_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then(left.id.cmp(&right.id))
            })
            .map(|job| job.id);
        let Some(candidate_id) = candidate_id else {
            return Ok(None);
        };
        let job = jobs
            .get_mut(&candidate_id)
            .expect("candidate job id should exist");
        job.status = "running".to_owned();
        job.lease_owner = Some(owner);
        job.lease_expires_at = Some((now + Duration::seconds(lease_seconds.max(1))).to_rfc3339());
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = None;
        job.updated_at = repository_now();
        Ok(Some(job.clone()))
    }

    async fn complete_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        result: Value,
    ) -> RepositoryResult<IdempotentJobRecord> {
        self.finish_idempotent_job(job_id, owner, "completed", Some(result), None)
    }

    async fn fail_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        error: String,
    ) -> RepositoryResult<IdempotentJobRecord> {
        self.finish_idempotent_job(job_id, owner, "failed", None, Some(error))
    }

    async fn acquire_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
        max_concurrent: i64,
        lease_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<AccountConcurrencySlotRecord>> {
        let request_id = normalize_consistency_name(request_id, "request id")?;
        let now = Utc::now();
        let mut slots = self
            .account_concurrency_slots
            .write()
            .expect("account concurrency repository lock");
        slots.retain(|_, slot| {
            parse_rfc3339_utc(&slot.expires_at)
                .map(|expires| expires > now)
                .unwrap_or(false)
        });
        let current = slots
            .values()
            .filter(|slot| slot.account_id == account_id)
            .count() as i64;
        let key = (account_id, request_id.clone());
        if !slots.contains_key(&key) && current >= max_concurrent.max(1) {
            return Ok(None);
        }
        let record = AccountConcurrencySlotRecord {
            account_id,
            request_id: request_id.clone(),
            expires_at: (now + Duration::seconds(lease_seconds.max(1))).to_rfc3339(),
            metadata,
            in_flight: if slots.contains_key(&key) {
                current
            } else {
                current + 1
            },
        };
        slots.insert(key, record.clone());
        Ok(Some(record))
    }

    async fn release_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
    ) -> RepositoryResult<bool> {
        let request_id = normalize_consistency_name(request_id, "request id")?;
        Ok(self
            .account_concurrency_slots
            .write()
            .expect("account concurrency repository lock")
            .remove(&(account_id, request_id))
            .is_some())
    }

    async fn list_account_concurrency_snapshots(
        &self,
    ) -> RepositoryResult<Vec<AccountConcurrencySnapshotRecord>> {
        let now = Utc::now();
        let mut counts: HashMap<i64, i64> = HashMap::new();
        for slot in self
            .account_concurrency_slots
            .read()
            .expect("account concurrency repository lock")
            .values()
        {
            if parse_rfc3339_utc(&slot.expires_at)
                .map(|expires| expires > now)
                .unwrap_or(false)
            {
                *counts.entry(slot.account_id).or_default() += 1;
            }
        }
        let mut snapshots = counts
            .into_iter()
            .map(|(account_id, in_flight)| AccountConcurrencySnapshotRecord {
                account_id,
                in_flight,
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.account_id);
        Ok(snapshots)
    }

    async fn hit_rate_limit_fixed_window(
        &self,
        scope: &str,
        limit: i64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitDecision> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let limit = limit.max(1);
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        let mut counters = self
            .rate_limit_counters
            .write()
            .expect("rate limit repository lock");
        let key = (scope.clone(), window_start_unix);
        let count = counters
            .get(&key)
            .map(|decision| decision.count)
            .unwrap_or(0)
            + 1;
        let decision = RateLimitDecision {
            allowed: count <= limit,
            scope,
            count,
            limit,
            remaining: (limit - count).max(0),
            window_start_unix,
            window_seconds,
            reset_at_unix,
        };
        counters.insert(key, decision.clone());
        Ok(decision)
    }

    async fn get_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let limit = normalize_rate_limit_usage_limit(limit)?;
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        let counters = self
            .rate_limit_usage_counters
            .read()
            .expect("rate limit usage repository lock");
        let usage = counters
            .get(&(scope.clone(), window_start_unix))
            .map(|record| record.usage)
            .unwrap_or(0.0);
        Ok(RateLimitUsageRecord {
            scope,
            usage,
            limit,
            remaining: (limit - usage).max(0.0),
            window_start_unix,
            window_seconds,
            reset_at_unix,
        })
    }

    async fn add_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        amount: f64,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let amount = normalize_rate_limit_usage_amount(amount)?;
        let limit = normalize_rate_limit_usage_limit(limit)?;
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        let mut counters = self
            .rate_limit_usage_counters
            .write()
            .expect("rate limit usage repository lock");
        let key = (scope.clone(), window_start_unix);
        let usage = counters.get(&key).map(|record| record.usage).unwrap_or(0.0) + amount;
        let record = RateLimitUsageRecord {
            scope,
            usage,
            limit,
            remaining: (limit - usage).max(0.0),
            window_start_unix,
            window_seconds,
            reset_at_unix,
        };
        counters.insert(key, record.clone());
        Ok(record)
    }
}

impl InMemoryRepository {
    fn finish_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        status: &str,
        result: Option<Value>,
        last_error: Option<String>,
    ) -> RepositoryResult<IdempotentJobRecord> {
        let owner = normalize_consistency_name(owner, "job owner")?;
        let mut jobs = self
            .idempotent_jobs
            .write()
            .expect("idempotent job repository lock");
        let job = jobs.get_mut(&job_id).ok_or(RepositoryError::NotFound {
            entity: "idempotent_job",
            id: job_id,
        })?;
        if job.status != "running" || job.lease_owner.as_deref() != Some(owner.as_str()) {
            return Err(RepositoryError::Conflict(
                "idempotent job lease is not held by owner".to_owned(),
            ));
        }
        job.status = status.to_owned();
        job.result = result;
        job.last_error = last_error;
        job.lease_owner = None;
        job.lease_expires_at = None;
        job.updated_at = repository_now();
        Ok(job.clone())
    }
}

#[async_trait]
impl ConsistencyRepository for PostgresRepository {
    fn uses_shared_consistency_backend(&self) -> bool {
        true
    }

    async fn try_acquire_lock(
        &self,
        name: &str,
        owner: &str,
        ttl_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<LockLeaseRecord>> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let ttl_seconds = ttl_seconds.max(1);
        let row = sqlx::query_as::<_, LockLeaseRow>(
            r#"
            INSERT INTO distributed_locks (
                name, owner, fencing_token, expires_at, metadata
            )
            VALUES ($1, $2, 1, NOW() + ($3 * interval '1 second'), $4)
            ON CONFLICT (name) DO UPDATE SET
                owner = EXCLUDED.owner,
                fencing_token = distributed_locks.fencing_token + 1,
                expires_at = EXCLUDED.expires_at,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            WHERE distributed_locks.expires_at <= NOW()
               OR distributed_locks.owner = EXCLUDED.owner
            RETURNING name, owner, fencing_token, expires_at::TEXT AS expires_at, metadata
            "#,
        )
        .bind(&name)
        .bind(&owner)
        .bind(ttl_seconds)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row.map(LockLeaseRecord::from))
    }

    async fn renew_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
        ttl_seconds: i64,
    ) -> RepositoryResult<bool> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let rows = sqlx::query(
            r#"
            UPDATE distributed_locks
            SET expires_at = NOW() + ($4 * interval '1 second'),
                updated_at = NOW()
            WHERE name = $1
              AND owner = $2
              AND fencing_token = $3
              AND expires_at > NOW()
            "#,
        )
        .bind(&name)
        .bind(&owner)
        .bind(fencing_token)
        .bind(ttl_seconds.max(1))
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        Ok(rows > 0)
    }

    async fn release_lock(
        &self,
        name: &str,
        owner: &str,
        fencing_token: i64,
    ) -> RepositoryResult<bool> {
        let name = normalize_consistency_name(name, "lock name")?;
        let owner = normalize_consistency_name(owner, "lock owner")?;
        let rows = sqlx::query(
            "DELETE FROM distributed_locks WHERE name = $1 AND owner = $2 AND fencing_token = $3",
        )
        .bind(&name)
        .bind(&owner)
        .bind(fencing_token)
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        Ok(rows > 0)
    }

    async fn create_idempotent_job(
        &self,
        job_type: &str,
        idempotency_key: &str,
        payload: Value,
    ) -> RepositoryResult<IdempotentJobCreateResult> {
        let job_type = normalize_consistency_name(job_type, "job type")?;
        let idempotency_key = normalize_consistency_name(idempotency_key, "idempotency key")?;
        let row = sqlx::query_as::<_, IdempotentJobCreateRow>(
            r#"
            WITH inserted AS (
                INSERT INTO idempotent_jobs (job_type, idempotency_key, payload)
                VALUES ($1, $2, $3)
                ON CONFLICT (job_type, idempotency_key) DO NOTHING
                RETURNING *, TRUE AS created
            )
            SELECT
                id, job_type, idempotency_key, status, payload, result, attempts,
                lease_owner, lease_expires_at::TEXT AS lease_expires_at,
                last_error, created_at::TEXT AS created_at, updated_at::TEXT AS updated_at,
                created
            FROM inserted
            UNION ALL
            SELECT
                id, job_type, idempotency_key, status, payload, result, attempts,
                lease_owner, lease_expires_at::TEXT AS lease_expires_at,
                last_error, created_at::TEXT AS created_at, updated_at::TEXT AS updated_at,
                FALSE AS created
            FROM idempotent_jobs
            WHERE job_type = $1 AND idempotency_key = $2
              AND NOT EXISTS (SELECT 1 FROM inserted)
            LIMIT 1
            "#,
        )
        .bind(&job_type)
        .bind(&idempotency_key)
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(IdempotentJobCreateResult {
            created: row.created,
            job: IdempotentJobRecord::from(row),
        })
    }

    async fn claim_next_idempotent_job(
        &self,
        job_type: &str,
        owner: &str,
        lease_seconds: i64,
    ) -> RepositoryResult<Option<IdempotentJobRecord>> {
        let job_type = normalize_consistency_name(job_type, "job type")?;
        let owner = normalize_consistency_name(owner, "job owner")?;
        let row = sqlx::query_as::<_, IdempotentJobRow>(
            r#"
            WITH next AS (
                SELECT id
                FROM idempotent_jobs
                WHERE job_type = $1
                  AND (
                    status = 'pending'
                    OR (
                        status = 'running'
                        AND (lease_expires_at IS NULL OR lease_expires_at <= NOW())
                    )
                  )
                ORDER BY created_at ASC, id ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE idempotent_jobs AS job
            SET status = 'running',
                attempts = job.attempts + 1,
                lease_owner = $2,
                lease_expires_at = NOW() + ($3 * interval '1 second'),
                last_error = NULL,
                updated_at = NOW()
            FROM next
            WHERE job.id = next.id
            RETURNING job.id, job.job_type, job.idempotency_key, job.status,
                job.payload, job.result, job.attempts, job.lease_owner,
                job.lease_expires_at::TEXT AS lease_expires_at,
                job.last_error, job.created_at::TEXT AS created_at,
                job.updated_at::TEXT AS updated_at
            "#,
        )
        .bind(&job_type)
        .bind(&owner)
        .bind(lease_seconds.max(1))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row.map(IdempotentJobRecord::from))
    }

    async fn complete_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        result: Value,
    ) -> RepositoryResult<IdempotentJobRecord> {
        finish_postgres_idempotent_job(&self.pool, job_id, owner, "completed", Some(result), None)
            .await
    }

    async fn fail_idempotent_job(
        &self,
        job_id: i64,
        owner: &str,
        error: String,
    ) -> RepositoryResult<IdempotentJobRecord> {
        finish_postgres_idempotent_job(&self.pool, job_id, owner, "failed", None, Some(error)).await
    }

    async fn acquire_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
        max_concurrent: i64,
        lease_seconds: i64,
        metadata: Value,
    ) -> RepositoryResult<Option<AccountConcurrencySlotRecord>> {
        let request_id = normalize_consistency_name(request_id, "request id")?;
        let row = sqlx::query_as::<_, AccountConcurrencySlotRow>(
            r#"
            WITH cleanup AS (
                DELETE FROM account_concurrency_slots
                WHERE expires_at <= NOW()
            ),
            current_count AS (
                SELECT COUNT(*)::BIGINT AS in_flight
                FROM account_concurrency_slots
                WHERE account_id = $1
            ),
            existing_slot AS (
                SELECT EXISTS (
                    SELECT 1
                    FROM account_concurrency_slots
                    WHERE account_id = $1 AND request_id = $2
                ) AS exists
            ),
            upserted AS (
                INSERT INTO account_concurrency_slots (
                    account_id, request_id, expires_at, metadata
                )
                SELECT
                    $1, $2, NOW() + ($4 * interval '1 second'), $5
                FROM current_count, existing_slot
                WHERE current_count.in_flight < $3
                   OR existing_slot.exists
                ON CONFLICT (account_id, request_id) DO UPDATE SET
                    expires_at = EXCLUDED.expires_at,
                    metadata = EXCLUDED.metadata,
                    updated_at = NOW()
                RETURNING account_id, request_id, expires_at::TEXT AS expires_at, metadata
            )
            SELECT
                upserted.account_id,
                upserted.request_id,
                upserted.expires_at,
                upserted.metadata,
                CASE
                    WHEN existing_slot.exists THEN current_count.in_flight
                    ELSE current_count.in_flight + 1
                END AS in_flight
            FROM upserted, current_count, existing_slot
            "#,
        )
        .bind(account_id)
        .bind(&request_id)
        .bind(max_concurrent.max(1))
        .bind(lease_seconds.max(1))
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row.map(AccountConcurrencySlotRecord::from))
    }

    async fn release_account_concurrency_slot(
        &self,
        account_id: i64,
        request_id: &str,
    ) -> RepositoryResult<bool> {
        let request_id = normalize_consistency_name(request_id, "request id")?;
        let rows = sqlx::query(
            "DELETE FROM account_concurrency_slots WHERE account_id = $1 AND request_id = $2",
        )
        .bind(account_id)
        .bind(&request_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?
        .rows_affected();
        Ok(rows > 0)
    }

    async fn list_account_concurrency_snapshots(
        &self,
    ) -> RepositoryResult<Vec<AccountConcurrencySnapshotRecord>> {
        sqlx::query("DELETE FROM account_concurrency_slots WHERE expires_at <= NOW()")
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        let rows = sqlx::query_as::<_, AccountConcurrencySnapshotRow>(
            r#"
            SELECT account_id, COUNT(*)::BIGINT AS in_flight
            FROM account_concurrency_slots
            WHERE expires_at > NOW()
            GROUP BY account_id
            ORDER BY account_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(AccountConcurrencySnapshotRecord::from)
            .collect())
    }

    async fn hit_rate_limit_fixed_window(
        &self,
        scope: &str,
        limit: i64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitDecision> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let limit = limit.max(1);
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        let row = sqlx::query_as::<_, RateLimitCounterRow>(
            r#"
            WITH cleanup AS (
                DELETE FROM rate_limit_counters
                WHERE expires_at <= NOW()
            )
            INSERT INTO rate_limit_counters (
                scope, window_start_unix, window_seconds, count, expires_at
            )
            VALUES (
                $1, $2, $3, 1, to_timestamp($4)
            )
            ON CONFLICT (scope, window_start_unix) DO UPDATE SET
                count = rate_limit_counters.count + 1,
                window_seconds = EXCLUDED.window_seconds,
                expires_at = EXCLUDED.expires_at,
                updated_at = NOW()
            RETURNING scope, window_start_unix, window_seconds, count
            "#,
        )
        .bind(&scope)
        .bind(window_start_unix)
        .bind(window_seconds)
        .bind(reset_at_unix)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let count = row.count;
        Ok(RateLimitDecision {
            allowed: count <= limit,
            scope: row.scope,
            count,
            limit,
            remaining: (limit - count).max(0),
            window_start_unix: row.window_start_unix,
            window_seconds: row.window_seconds,
            reset_at_unix,
        })
    }

    async fn get_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let limit = normalize_rate_limit_usage_limit(limit)?;
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        sqlx::query("DELETE FROM rate_limit_counters WHERE expires_at <= NOW()")
            .execute(&self.pool)
            .await
            .map_err(db_error)?;
        let row = sqlx::query_as::<_, RateLimitUsageRow>(
            r#"
            SELECT scope, window_start_unix, window_seconds, usage
            FROM rate_limit_counters
            WHERE scope = $1 AND window_start_unix = $2
            "#,
        )
        .bind(&scope)
        .bind(window_start_unix)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        let usage = row.as_ref().map(|row| row.usage).unwrap_or(0.0);
        Ok(RateLimitUsageRecord {
            scope: row.map(|row| row.scope).unwrap_or(scope),
            usage,
            limit,
            remaining: (limit - usage).max(0.0),
            window_start_unix,
            window_seconds,
            reset_at_unix,
        })
    }

    async fn add_rate_limit_usage_fixed_window(
        &self,
        scope: &str,
        amount: f64,
        limit: f64,
        window_start_unix: i64,
        window_seconds: i64,
    ) -> RepositoryResult<RateLimitUsageRecord> {
        let scope = normalize_consistency_name(scope, "rate limit scope")?;
        let amount = normalize_rate_limit_usage_amount(amount)?;
        let limit = normalize_rate_limit_usage_limit(limit)?;
        let window_seconds = window_seconds.max(1);
        let reset_at_unix = window_start_unix.saturating_add(window_seconds);
        let row = sqlx::query_as::<_, RateLimitUsageRow>(
            r#"
            WITH cleanup AS (
                DELETE FROM rate_limit_counters
                WHERE expires_at <= NOW()
            )
            INSERT INTO rate_limit_counters (
                scope, window_start_unix, window_seconds, count, usage, expires_at
            )
            VALUES (
                $1, $2, $3, 0, $4, to_timestamp($5)
            )
            ON CONFLICT (scope, window_start_unix) DO UPDATE SET
                usage = rate_limit_counters.usage + EXCLUDED.usage,
                window_seconds = EXCLUDED.window_seconds,
                expires_at = EXCLUDED.expires_at,
                updated_at = NOW()
            RETURNING scope, window_start_unix, window_seconds, usage
            "#,
        )
        .bind(&scope)
        .bind(window_start_unix)
        .bind(window_seconds)
        .bind(amount)
        .bind(reset_at_unix)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        let usage = row.usage;
        Ok(RateLimitUsageRecord {
            scope: row.scope,
            usage,
            limit,
            remaining: (limit - usage).max(0.0),
            window_start_unix: row.window_start_unix,
            window_seconds: row.window_seconds,
            reset_at_unix,
        })
    }
}

const PAYMENT_ORDER_SELECT_BY_ID: &str = r#"
SELECT
    id, user_id, amount, pay_amount, currency, fee_rate, payment_type,
    out_trade_no, status, order_type, refund_amount, refund_reason,
    refund_request_reason, plan_id, provider_instance_id, created_at_text,
    expires_at_text, paid_at_text, completed_at_text, cancelled_at_text,
    refund_requested_at_text, refunded_at_text, webhook_count, metadata
FROM payment_orders
WHERE id = $1
"#;

const PAYMENT_ORDER_SELECT_BY_TRADE_NO: &str = r#"
SELECT
    id, user_id, amount, pay_amount, currency, fee_rate, payment_type,
    out_trade_no, status, order_type, refund_amount, refund_reason,
    refund_request_reason, plan_id, provider_instance_id, created_at_text,
    expires_at_text, paid_at_text, completed_at_text, cancelled_at_text,
    refund_requested_at_text, refunded_at_text, webhook_count, metadata
FROM payment_orders
WHERE out_trade_no = $1
"#;

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    email: String,
    username: String,
    role: String,
    status: String,
}

impl From<UserRow> for UserRecord {
    fn from(row: UserRow) -> Self {
        Self {
            id: row.id,
            email: row.email,
            username: row.username,
            role: row.role,
            status: row.status,
        }
    }
}

#[derive(sqlx::FromRow)]
struct OAuthIdentityRow {
    id: i64,
    user_id: i64,
    provider: String,
    provider_key: String,
    provider_subject: String,
    email: Option<String>,
    bound_at_unix: i64,
    metadata: Value,
}

impl From<OAuthIdentityRow> for OAuthIdentityRecord {
    fn from(row: OAuthIdentityRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            provider: row.provider,
            provider_key: row.provider_key,
            provider_subject: row.provider_subject,
            email: row.email,
            bound_at_unix: row.bound_at_unix,
            metadata: row.metadata,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AuthSessionRow {
    id: i64,
    user_id: i64,
    access_token: String,
    refresh_token: String,
    access_expires_at_unix: i64,
    refresh_expires_at_unix: i64,
    revoked_at_unix: Option<i64>,
    created_at_unix: i64,
    metadata: Value,
}

impl From<AuthSessionRow> for AuthSessionRecord {
    fn from(row: AuthSessionRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            access_token: row.access_token,
            refresh_token: row.refresh_token,
            access_expires_at_unix: row.access_expires_at_unix,
            refresh_expires_at_unix: row.refresh_expires_at_unix,
            revoked_at_unix: row.revoked_at_unix,
            created_at_unix: row.created_at_unix,
            metadata: row.metadata,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AuthCredentialRow {
    user_id: i64,
    email: String,
    password_hash: String,
    status: String,
    updated_at_unix: i64,
}

impl From<AuthCredentialRow> for AuthCredentialRecord {
    fn from(row: AuthCredentialRow) -> Self {
        Self {
            user_id: row.user_id,
            email: row.email,
            password_hash: row.password_hash,
            status: row.status,
            updated_at_unix: row.updated_at_unix,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: i64,
    user_id: i64,
    key: String,
    name: String,
    group_id: Option<i64>,
    status: String,
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

#[derive(sqlx::FromRow)]
struct GroupRow {
    id: i64,
    name: String,
    status: String,
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    provider: String,
    default_upstream_protocol: String,
    base_url: Option<String>,
    api_key: Option<String>,
    model_mapping: Value,
    extra: Value,
    enabled: bool,
}

#[derive(sqlx::FromRow)]
struct AccountBindingRow {
    id: i64,
    name: String,
    provider: String,
    default_upstream_protocol: String,
    base_url: Option<String>,
    api_key: Option<String>,
    model_mapping: Value,
    extra: Value,
    enabled: bool,
    group_id: i64,
    supported_downstream_protocols: Vec<String>,
    upstream_protocol_override: Option<String>,
    priority: i32,
}

#[derive(sqlx::FromRow)]
struct UsageRow {
    id: i64,
    user_id: i64,
    api_key_id: i64,
    group_id: Option<i64>,
    account_id: Option<i64>,
    downstream_protocol: String,
    upstream_protocol: String,
    provider: String,
    endpoint: String,
    requested_model: String,
    upstream_model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    actual_cost: f64,
    status: String,
    created_at_unix: i64,
    metadata: Value,
}

#[derive(sqlx::FromRow)]
struct UsageSummaryRow {
    requests: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    actual_cost: f64,
}

#[derive(sqlx::FromRow)]
struct UsageCleanupTaskRow {
    id: i64,
    status: String,
    filters: Value,
    created_by: i64,
    deleted_rows: i64,
    error_message: Option<String>,
    canceled_by: Option<i64>,
    canceled_at_text: Option<String>,
    started_at_text: Option<String>,
    finished_at_text: Option<String>,
    created_at_text: String,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct PaymentOrderRow {
    id: i64,
    user_id: i64,
    amount: f64,
    pay_amount: f64,
    currency: String,
    fee_rate: f64,
    payment_type: String,
    out_trade_no: String,
    status: String,
    order_type: String,
    refund_amount: f64,
    refund_reason: Option<String>,
    refund_request_reason: Option<String>,
    plan_id: Option<i64>,
    provider_instance_id: Option<String>,
    created_at_text: String,
    expires_at_text: String,
    paid_at_text: Option<String>,
    completed_at_text: Option<String>,
    cancelled_at_text: Option<String>,
    refund_requested_at_text: Option<String>,
    refunded_at_text: Option<String>,
    webhook_count: i64,
    metadata: Value,
}

#[derive(sqlx::FromRow)]
struct PaymentAuditRow {
    id: i64,
    order_id: String,
    action: String,
    detail: String,
    operator: String,
    created_at_text: String,
}

#[derive(sqlx::FromRow)]
struct UserBalanceRow {
    user_id: i64,
    balance: f64,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct BalanceTransactionRow {
    id: i64,
    user_id: i64,
    order_id: String,
    transaction_type: String,
    amount: f64,
    balance_after: f64,
    created_at_text: String,
    metadata: Value,
}

#[derive(sqlx::FromRow)]
struct UserSubscriptionRow {
    id: i64,
    user_id: i64,
    group_id: i64,
    plan_id: Option<i64>,
    status: String,
    starts_at_text: String,
    expires_at_text: String,
    daily_usage_usd: f64,
    weekly_usage_usd: f64,
    monthly_usage_usd: f64,
    daily_window_start_text: Option<String>,
    weekly_window_start_text: Option<String>,
    monthly_window_start_text: Option<String>,
    source_order_id: String,
    created_at_text: String,
    metadata: Value,
}

#[derive(sqlx::FromRow)]
struct UserPlatformQuotaRow {
    id: i64,
    user_id: i64,
    platform: String,
    daily_limit_usd: Option<f64>,
    weekly_limit_usd: Option<f64>,
    monthly_limit_usd: Option<f64>,
    daily_usage_usd: f64,
    weekly_usage_usd: f64,
    monthly_usage_usd: f64,
    daily_window_start_text: Option<String>,
    weekly_window_start_text: Option<String>,
    monthly_window_start_text: Option<String>,
}

#[derive(sqlx::FromRow)]
struct UserGroupRateRow {
    user_id: i64,
    group_id: i64,
    rate_multiplier: Option<f64>,
    rpm_override: Option<i32>,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct UserAttributeValueRow {
    user_id: i64,
    attribute_id: i64,
    value: String,
    created_at_text: String,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct PaymentProviderInstanceRow {
    id: i64,
    provider_key: String,
    name: String,
    config: Value,
    supported_types: Vec<String>,
    enabled: bool,
    payment_mode: String,
    sort_order: i32,
    limits: Value,
    refund_enabled: bool,
    allow_user_refund: bool,
    created_at_text: String,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct PaymentPlanRow {
    id: i64,
    group_id: i64,
    name: String,
    description: String,
    price: f64,
    original_price: Option<f64>,
    validity_days: i32,
    validity_unit: String,
    features: Value,
    product_name: String,
    for_sale: bool,
    sort_order: i32,
    created_at_text: String,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct AdminCollectionItemRow {
    collection: String,
    id: i64,
    item: Value,
}

#[derive(sqlx::FromRow)]
struct SystemSettingRow {
    namespace: String,
    key: String,
    value: Value,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct EmailQueueTaskRow {
    id: i64,
    task_type: String,
    status: String,
    payload: Value,
    attempts: i32,
    max_attempts: i32,
    last_error: Option<String>,
    created_at_text: String,
    updated_at_text: String,
}

#[derive(sqlx::FromRow)]
struct LockLeaseRow {
    name: String,
    owner: String,
    fencing_token: i64,
    expires_at: String,
    metadata: Value,
}

impl From<LockLeaseRow> for LockLeaseRecord {
    fn from(row: LockLeaseRow) -> Self {
        Self {
            name: row.name,
            owner: row.owner,
            fencing_token: row.fencing_token,
            expires_at: row.expires_at,
            metadata: row.metadata,
        }
    }
}

#[derive(sqlx::FromRow)]
struct IdempotentJobRow {
    id: i64,
    job_type: String,
    idempotency_key: String,
    status: String,
    payload: Value,
    result: Option<Value>,
    attempts: i32,
    lease_owner: Option<String>,
    lease_expires_at: Option<String>,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct IdempotentJobCreateRow {
    id: i64,
    job_type: String,
    idempotency_key: String,
    status: String,
    payload: Value,
    result: Option<Value>,
    attempts: i32,
    lease_owner: Option<String>,
    lease_expires_at: Option<String>,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
    created: bool,
}

impl From<IdempotentJobRow> for IdempotentJobRecord {
    fn from(row: IdempotentJobRow) -> Self {
        Self {
            id: row.id,
            job_type: row.job_type,
            idempotency_key: row.idempotency_key,
            status: row.status,
            payload: row.payload,
            result: row.result,
            attempts: row.attempts,
            lease_owner: row.lease_owner,
            lease_expires_at: row.lease_expires_at,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<IdempotentJobCreateRow> for IdempotentJobRecord {
    fn from(row: IdempotentJobCreateRow) -> Self {
        Self {
            id: row.id,
            job_type: row.job_type,
            idempotency_key: row.idempotency_key,
            status: row.status,
            payload: row.payload,
            result: row.result,
            attempts: row.attempts,
            lease_owner: row.lease_owner,
            lease_expires_at: row.lease_expires_at,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AccountConcurrencySlotRow {
    account_id: i64,
    request_id: String,
    expires_at: String,
    metadata: Value,
    in_flight: i64,
}

impl From<AccountConcurrencySlotRow> for AccountConcurrencySlotRecord {
    fn from(row: AccountConcurrencySlotRow) -> Self {
        Self {
            account_id: row.account_id,
            request_id: row.request_id,
            expires_at: row.expires_at,
            metadata: row.metadata,
            in_flight: row.in_flight,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AccountConcurrencySnapshotRow {
    account_id: i64,
    in_flight: i64,
}

impl From<AccountConcurrencySnapshotRow> for AccountConcurrencySnapshotRecord {
    fn from(row: AccountConcurrencySnapshotRow) -> Self {
        Self {
            account_id: row.account_id,
            in_flight: row.in_flight,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RateLimitCounterRow {
    scope: String,
    window_start_unix: i64,
    window_seconds: i64,
    count: i64,
}

#[derive(sqlx::FromRow)]
struct RateLimitUsageRow {
    scope: String,
    window_start_unix: i64,
    window_seconds: i64,
    usage: f64,
}

#[derive(sqlx::FromRow)]
struct ChannelMonitorHistoryRow {
    id: i64,
    monitor_id: i64,
    model: String,
    status: String,
    latency_ms: Option<i64>,
    ping_latency_ms: Option<i64>,
    message: String,
    checked_at_text: String,
    metadata: Value,
}

#[derive(sqlx::FromRow)]
struct ContentModerationLogRow {
    id: i64,
    request_id: String,
    user_id: Option<i64>,
    user_email: String,
    api_key_id: Option<i64>,
    api_key_name: String,
    group_id: Option<i64>,
    group_name: String,
    endpoint: String,
    provider: String,
    model: String,
    mode: String,
    action: String,
    flagged: bool,
    highest_category: String,
    highest_score: f64,
    category_scores: Value,
    threshold_snapshot: Value,
    input_excerpt: String,
    upstream_latency_ms: Option<i64>,
    error: String,
    violation_count: i64,
    auto_banned: bool,
    email_sent: bool,
    user_status: String,
    queue_delay_ms: Option<i64>,
    created_at_text: String,
}

impl From<AdminCollectionItemRow> for AdminCollectionItemRecord {
    fn from(row: AdminCollectionItemRow) -> Self {
        Self {
            collection: row.collection,
            id: row.id,
            item: row.item,
        }
    }
}

impl From<SystemSettingRow> for SystemSettingRecord {
    fn from(row: SystemSettingRow) -> Self {
        Self {
            namespace: row.namespace,
            key: row.key,
            value: row.value,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<EmailQueueTaskRow> for EmailQueueTaskRecord {
    fn from(row: EmailQueueTaskRow) -> Self {
        Self {
            id: row.id,
            task_type: row.task_type,
            status: row.status,
            payload: row.payload,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            last_error: row.last_error,
            created_at: row.created_at_text,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<ChannelMonitorHistoryRow> for ChannelMonitorHistoryRecord {
    fn from(row: ChannelMonitorHistoryRow) -> Self {
        Self {
            id: row.id,
            monitor_id: row.monitor_id,
            model: row.model,
            status: row.status,
            latency_ms: row.latency_ms,
            ping_latency_ms: row.ping_latency_ms,
            message: row.message,
            checked_at: row.checked_at_text,
            metadata: row.metadata,
        }
    }
}

impl From<ContentModerationLogRow> for ContentModerationLogRecord {
    fn from(row: ContentModerationLogRow) -> Self {
        Self {
            id: row.id,
            request_id: row.request_id,
            user_id: row.user_id,
            user_email: row.user_email,
            api_key_id: row.api_key_id,
            api_key_name: row.api_key_name,
            group_id: row.group_id,
            group_name: row.group_name,
            endpoint: row.endpoint,
            provider: row.provider,
            model: row.model,
            mode: row.mode,
            action: row.action,
            flagged: row.flagged,
            highest_category: row.highest_category,
            highest_score: row.highest_score,
            category_scores: row.category_scores,
            threshold_snapshot: row.threshold_snapshot,
            input_excerpt: row.input_excerpt,
            upstream_latency_ms: row.upstream_latency_ms,
            error: row.error,
            violation_count: row.violation_count,
            auto_banned: row.auto_banned,
            email_sent: row.email_sent,
            user_status: row.user_status,
            queue_delay_ms: row.queue_delay_ms,
            created_at: row.created_at_text,
        }
    }
}

impl From<PaymentProviderInstanceRow> for PaymentProviderInstanceRecord {
    fn from(row: PaymentProviderInstanceRow) -> Self {
        Self {
            id: row.id,
            provider_key: row.provider_key,
            name: row.name,
            config: row.config,
            supported_types: normalize_repository_strings(&row.supported_types),
            enabled: row.enabled,
            payment_mode: row.payment_mode,
            sort_order: row.sort_order,
            limits: row.limits,
            refund_enabled: row.refund_enabled,
            allow_user_refund: row.allow_user_refund,
            created_at: row.created_at_text,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<PaymentPlanRow> for PaymentPlanRecord {
    fn from(row: PaymentPlanRow) -> Self {
        Self {
            id: row.id,
            group_id: row.group_id,
            name: row.name,
            description: row.description,
            price: row.price,
            original_price: row.original_price,
            validity_days: row.validity_days,
            validity_unit: row.validity_unit,
            features: normalize_plan_features(row.features),
            product_name: row.product_name,
            for_sale: row.for_sale,
            sort_order: row.sort_order,
            created_at: row.created_at_text,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<PaymentAuditRow> for PaymentAuditRecord {
    fn from(row: PaymentAuditRow) -> Self {
        Self {
            id: row.id,
            order_id: row.order_id,
            action: row.action,
            detail: row.detail,
            operator: row.operator,
            created_at: row.created_at_text,
        }
    }
}

impl From<UserBalanceRow> for UserBalanceRecord {
    fn from(row: UserBalanceRow) -> Self {
        Self {
            user_id: row.user_id,
            balance: row.balance,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<BalanceTransactionRow> for BalanceTransactionRecord {
    fn from(row: BalanceTransactionRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            order_id: row.order_id,
            transaction_type: row.transaction_type,
            amount: row.amount,
            balance_after: row.balance_after,
            created_at: row.created_at_text,
            metadata: row.metadata,
        }
    }
}

impl From<UserSubscriptionRow> for UserSubscriptionRecord {
    fn from(row: UserSubscriptionRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            group_id: row.group_id,
            plan_id: row.plan_id,
            status: row.status,
            starts_at: row.starts_at_text,
            expires_at: row.expires_at_text,
            daily_usage_usd: row.daily_usage_usd,
            weekly_usage_usd: row.weekly_usage_usd,
            monthly_usage_usd: row.monthly_usage_usd,
            daily_window_start: row.daily_window_start_text,
            weekly_window_start: row.weekly_window_start_text,
            monthly_window_start: row.monthly_window_start_text,
            source_order_id: row.source_order_id,
            created_at: row.created_at_text,
            metadata: row.metadata,
        }
    }
}

impl From<UserPlatformQuotaRow> for UserPlatformQuotaRecord {
    fn from(row: UserPlatformQuotaRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            platform: row.platform,
            daily_limit_usd: row.daily_limit_usd,
            weekly_limit_usd: row.weekly_limit_usd,
            monthly_limit_usd: row.monthly_limit_usd,
            daily_usage_usd: row.daily_usage_usd,
            weekly_usage_usd: row.weekly_usage_usd,
            monthly_usage_usd: row.monthly_usage_usd,
            daily_window_start: row.daily_window_start_text,
            weekly_window_start: row.weekly_window_start_text,
            monthly_window_start: row.monthly_window_start_text,
        }
    }
}

impl From<UserGroupRateRow> for UserGroupRateRecord {
    fn from(row: UserGroupRateRow) -> Self {
        Self {
            user_id: row.user_id,
            group_id: row.group_id,
            rate_multiplier: row.rate_multiplier,
            rpm_override: row.rpm_override,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<UserAttributeValueRow> for UserAttributeValueRecord {
    fn from(row: UserAttributeValueRow) -> Self {
        Self {
            user_id: row.user_id,
            attribute_id: row.attribute_id,
            value: row.value,
            created_at: row.created_at_text,
            updated_at: row.updated_at_text,
        }
    }
}

impl From<PaymentOrderRow> for PaymentOrderRecord {
    fn from(row: PaymentOrderRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            amount: row.amount,
            pay_amount: row.pay_amount,
            currency: row.currency,
            fee_rate: row.fee_rate,
            payment_type: row.payment_type,
            out_trade_no: row.out_trade_no,
            status: row.status,
            order_type: row.order_type,
            refund_amount: row.refund_amount,
            refund_reason: row.refund_reason,
            refund_request_reason: row.refund_request_reason,
            plan_id: row.plan_id,
            provider_instance_id: row.provider_instance_id,
            created_at: row.created_at_text,
            expires_at: row.expires_at_text,
            paid_at: row.paid_at_text,
            completed_at: row.completed_at_text,
            cancelled_at: row.cancelled_at_text,
            refund_requested_at: row.refund_requested_at_text,
            refunded_at: row.refunded_at_text,
            webhook_count: row.webhook_count,
            metadata: row.metadata,
        }
    }
}

fn try_usage_cleanup_task_from_row(
    row: UsageCleanupTaskRow,
) -> RepositoryResult<UsageCleanupTaskRecord> {
    let filters = serde_json::from_value::<UsageCleanupFilter>(row.filters)
        .map_err(|error| RepositoryError::Database(error.to_string()))?;
    Ok(UsageCleanupTaskRecord {
        id: row.id,
        status: row.status,
        filters,
        created_by: row.created_by,
        deleted_rows: row.deleted_rows,
        error_message: row.error_message,
        canceled_by: row.canceled_by,
        canceled_at: row.canceled_at_text,
        started_at: row.started_at_text,
        finished_at: row.finished_at_text,
        created_at: row.created_at_text,
        updated_at: row.updated_at_text,
    })
}

fn try_api_key_from_row(row: ApiKeyRow) -> RepositoryResult<ApiKey> {
    Ok(ApiKey {
        id: ApiKeyId(row.id),
        user_id: row.user_id,
        key: row.key,
        name: row.name,
        group_id: row.group_id.map(GroupId),
        status: parse_api_key_status(&row.status)?,
        quota: row.quota,
        quota_used: row.quota_used,
        rate_limit_5h: row.rate_limit_5h,
        rate_limit_1d: row.rate_limit_1d,
        rate_limit_7d: row.rate_limit_7d,
        usage_5h: row.usage_5h,
        usage_1d: row.usage_1d,
        usage_7d: row.usage_7d,
        window_5h_start: row.window_5h_start,
        window_1d_start: row.window_1d_start,
        window_7d_start: row.window_7d_start,
    })
}

fn try_group_from_row(row: GroupRow) -> RepositoryResult<Group> {
    Ok(Group {
        id: GroupId(row.id),
        name: row.name,
        status: parse_group_status(&row.status)?,
    })
}

fn try_account_from_row(row: AccountRow) -> RepositoryResult<Account> {
    let model_mapping = serde_json::from_value::<Vec<ModelMappingRule>>(row.model_mapping)
        .map_err(|error| RepositoryError::Database(error.to_string()))?;
    Ok(Account {
        id: AccountId(row.id),
        name: row.name,
        provider: parse_provider(&row.provider)?,
        default_upstream_protocol: parse_upstream_protocol(&row.default_upstream_protocol)?,
        base_url: row.base_url,
        api_key: row.api_key,
        model_mapping,
        extra: row.extra,
        enabled: row.enabled,
    })
}

fn try_binding_from_row(row: AccountBindingRow) -> RepositoryResult<AccountGroupBinding> {
    let model_mapping = serde_json::from_value::<Vec<ModelMappingRule>>(row.model_mapping)
        .map_err(|error| RepositoryError::Database(error.to_string()))?;
    let supported_downstream_protocols = row
        .supported_downstream_protocols
        .iter()
        .map(|value| {
            DownstreamProtocol::from_str(value)
                .map_err(|error| RepositoryError::Database(error.to_string()))
        })
        .collect::<RepositoryResult<Vec<_>>>()?;
    Ok(AccountGroupBinding {
        account: Account {
            id: AccountId(row.id),
            name: row.name,
            provider: parse_provider(&row.provider)?,
            default_upstream_protocol: parse_upstream_protocol(&row.default_upstream_protocol)?,
            base_url: row.base_url,
            api_key: row.api_key,
            model_mapping,
            extra: row.extra,
            enabled: row.enabled,
        },
        group_id: GroupId(row.group_id),
        supported_downstream_protocols,
        upstream_protocol_override: row
            .upstream_protocol_override
            .as_deref()
            .map(parse_upstream_protocol)
            .transpose()?,
        priority: row.priority,
    })
}

fn try_usage_from_row(row: UsageRow) -> RepositoryResult<UsageRecord> {
    Ok(UsageRecord {
        id: row.id,
        user_id: row.user_id,
        api_key_id: row.api_key_id,
        group_id: row.group_id.map(GroupId),
        account_id: row.account_id.map(AccountId),
        downstream_protocol: DownstreamProtocol::from_str(&row.downstream_protocol)
            .map_err(|error| RepositoryError::Database(error.to_string()))?,
        upstream_protocol: row.upstream_protocol,
        provider: row.provider,
        endpoint: row.endpoint,
        requested_model: row.requested_model,
        upstream_model: row.upstream_model,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_creation_tokens: row.cache_creation_tokens,
        cache_read_tokens: row.cache_read_tokens,
        actual_cost: row.actual_cost,
        status: row.status,
        created_at_unix: row.created_at_unix,
        metadata: row.metadata,
    })
}

fn db_error(error: sqlx::Error) -> RepositoryError {
    RepositoryError::Database(error.to_string())
}

fn repository_now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn normalize_consistency_name(value: &str, label: &str) -> RepositoryResult<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} is required"
        )));
    }
    if normalized.len() > 256 {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} is too long"
        )));
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/' | '@'))
    {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} contains unsupported characters"
        )));
    }
    Ok(normalized.to_owned())
}

fn normalize_rate_limit_usage_amount(amount: f64) -> RepositoryResult<f64> {
    if !amount.is_finite() || amount < 0.0 {
        return Err(RepositoryError::InvalidInput(
            "rate limit usage amount must be finite and non-negative".to_owned(),
        ));
    }
    Ok(amount)
}

fn normalize_rate_limit_usage_limit(limit: f64) -> RepositoryResult<f64> {
    if !limit.is_finite() || limit < 0.0 {
        return Err(RepositoryError::InvalidInput(
            "rate limit usage limit must be finite and non-negative".to_owned(),
        ));
    }
    Ok(limit)
}

async fn finish_postgres_idempotent_job(
    pool: &sqlx::PgPool,
    job_id: i64,
    owner: &str,
    status: &str,
    result: Option<Value>,
    last_error: Option<String>,
) -> RepositoryResult<IdempotentJobRecord> {
    let owner = normalize_consistency_name(owner, "job owner")?;
    let row = sqlx::query_as::<_, IdempotentJobRow>(
        r#"
        UPDATE idempotent_jobs
        SET status = $1,
            result = $2,
            last_error = $3,
            lease_owner = NULL,
            lease_expires_at = NULL,
            updated_at = NOW()
        WHERE id = $4
          AND status = 'running'
          AND lease_owner = $5
          AND (lease_expires_at IS NULL OR lease_expires_at > NOW())
        RETURNING id, job_type, idempotency_key, status, payload, result,
            attempts, lease_owner, lease_expires_at::TEXT AS lease_expires_at,
            last_error, created_at::TEXT AS created_at, updated_at::TEXT AS updated_at
        "#,
    )
    .bind(status)
    .bind(result)
    .bind(last_error)
    .bind(job_id)
    .bind(&owner)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(IdempotentJobRecord::from).ok_or_else(|| {
        RepositoryError::Conflict("idempotent job lease is not held by owner".to_owned())
    })
}

fn validate_cleanup_task(task: &UsageCleanupTaskRecord) -> RepositoryResult<()> {
    if task.created_by <= 0 {
        return Err(RepositoryError::InvalidInput(
            "created_by is required".to_owned(),
        ));
    }
    if !matches!(
        task.status.as_str(),
        "pending" | "running" | "succeeded" | "failed" | "canceled"
    ) {
        return Err(RepositoryError::InvalidInput(
            "invalid cleanup task status".to_owned(),
        ));
    }
    validate_cleanup_filter(&task.filters)
}

fn validate_cleanup_filter(filter: &UsageCleanupFilter) -> RepositoryResult<()> {
    if filter.start_time_unix <= 0 || filter.end_time_unix <= 0 {
        return Err(RepositoryError::InvalidInput(
            "cleanup filters missing time range".to_owned(),
        ));
    }
    if filter.end_time_unix <= filter.start_time_unix {
        return Err(RepositoryError::InvalidInput(
            "end_date must be after start_date".to_owned(),
        ));
    }
    Ok(())
}

fn cleanup_started_before(task: &UsageCleanupTaskRecord, before: DateTime<Utc>) -> bool {
    task.started_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc) < before)
        .unwrap_or(false)
}

fn cleanup_filter_matches(
    cleanup_filter: &UsageCleanupFilter,
    usage_filter: &UsageFilter,
    record: &UsageRecord,
) -> bool {
    usage_filter.matches(record)
        && cleanup_filter
            .billing_type
            .map(|expected| {
                record
                    .metadata
                    .get("billing_type")
                    .and_then(Value::as_i64)
                    .map(|actual| actual == i64::from(expected))
                    .unwrap_or(false)
            })
            .unwrap_or(true)
}

fn normalize_repository_string(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_repository_strings(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let value = normalize_repository_string(value);
        if !value.is_empty() && !normalized.iter().any(|existing| existing == &value) {
            normalized.push(value);
        }
    }
    normalized
}

fn normalize_oauth_identity_record(record: &mut OAuthIdentityRecord) -> RepositoryResult<()> {
    if record.user_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "oauth identity user_id is required".to_owned(),
        ));
    }
    record.provider = normalize_setting_token(&record.provider, "oauth identity provider")?;
    record.provider_key =
        normalize_setting_token_with(&record.provider_key, "oauth identity provider_key", |ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/')
        })?;
    record.provider_subject = record.provider_subject.trim().to_owned();
    if record.provider_subject.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "oauth identity provider_subject is required".to_owned(),
        ));
    }
    if record.bound_at_unix <= 0 {
        record.bound_at_unix = Utc::now().timestamp();
    }
    record.email = record.email.as_deref().map(str::trim).and_then(|email| {
        if email.is_empty() {
            None
        } else {
            Some(email.to_ascii_lowercase())
        }
    });
    Ok(())
}

fn oauth_identity_repository_key(
    provider: &str,
    provider_key: &str,
    provider_subject: &str,
) -> RepositoryResult<String> {
    let provider = normalize_setting_token(provider, "oauth identity provider")?;
    let provider_key =
        normalize_setting_token_with(provider_key, "oauth identity provider_key", |ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/')
        })?;
    let provider_subject = provider_subject.trim();
    if provider_subject.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "oauth identity provider_subject is required".to_owned(),
        ));
    }
    Ok(format!("{provider}\n{provider_key}\n{provider_subject}"))
}

fn split_oauth_identity_repository_key(key: &str) -> RepositoryResult<(&str, &str, &str)> {
    let mut parts = key.splitn(3, '\n');
    let provider = parts.next().unwrap_or_default();
    let provider_key = parts.next().unwrap_or_default();
    let provider_subject = parts.next().unwrap_or_default();
    if provider.is_empty() || provider_key.is_empty() || provider_subject.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "oauth identity key is invalid".to_owned(),
        ));
    }
    Ok((provider, provider_key, provider_subject))
}

fn normalize_auth_session_record(record: &mut AuthSessionRecord) -> RepositoryResult<()> {
    if record.user_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "auth session user_id is required".to_owned(),
        ));
    }
    record.access_token = normalize_auth_session_token(&record.access_token, "access_token")?;
    record.refresh_token = normalize_auth_session_token(&record.refresh_token, "refresh_token")?;
    if record.access_expires_at_unix <= 0 {
        return Err(RepositoryError::InvalidInput(
            "auth session access expiry is required".to_owned(),
        ));
    }
    if record.refresh_expires_at_unix <= 0 {
        return Err(RepositoryError::InvalidInput(
            "auth session refresh expiry is required".to_owned(),
        ));
    }
    if record.refresh_expires_at_unix < record.access_expires_at_unix {
        return Err(RepositoryError::InvalidInput(
            "auth session refresh expiry must not precede access expiry".to_owned(),
        ));
    }
    if record.created_at_unix <= 0 {
        record.created_at_unix = Utc::now().timestamp();
    }
    Ok(())
}

fn normalize_auth_session_token(value: &str, label: &str) -> RepositoryResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} is required"
        )));
    }
    if value.len() > 1024 || value.chars().any(char::is_whitespace) {
        return Err(RepositoryError::InvalidInput(format!("{label} is invalid")));
    }
    Ok(value.to_owned())
}

fn normalize_auth_credential_record(record: &mut AuthCredentialRecord) -> RepositoryResult<()> {
    if record.user_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "auth credential user_id is required".to_owned(),
        ));
    }
    record.email = normalize_auth_credential_email(&record.email)?;
    record.password_hash = record.password_hash.trim().to_owned();
    if record.password_hash.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "auth credential password_hash is required".to_owned(),
        ));
    }
    record.status = normalize_repository_string(&record.status);
    if record.status.is_empty() {
        record.status = "active".to_owned();
    }
    if !matches!(record.status.as_str(), "active" | "disabled" | "deleted") {
        return Err(RepositoryError::InvalidInput(
            "auth credential status is invalid".to_owned(),
        ));
    }
    if record.updated_at_unix <= 0 {
        record.updated_at_unix = Utc::now().timestamp();
    }
    Ok(())
}

fn normalize_auth_credential_email(email: &str) -> RepositoryResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "auth credential email is required".to_owned(),
        ));
    }
    if !email.contains('@') || email.chars().any(char::is_whitespace) {
        return Err(RepositoryError::InvalidInput(
            "auth credential email is invalid".to_owned(),
        ));
    }
    Ok(email)
}

fn normalize_setting_namespace(value: &str) -> RepositoryResult<String> {
    normalize_setting_token(value, "system setting namespace")
}

fn normalize_setting_key(value: &str) -> RepositoryResult<String> {
    normalize_setting_token_with(value, "system setting key", |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')
    })
}

fn normalize_setting_token(value: &str, label: &str) -> RepositoryResult<String> {
    normalize_setting_token_with(value, label, |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
    })
}

fn normalize_setting_token_with(
    value: &str,
    label: &str,
    allowed: impl Fn(char) -> bool,
) -> RepositoryResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} is required"
        )));
    }
    if !value.chars().all(allowed) {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} contains unsupported characters"
        )));
    }
    Ok(value)
}

fn validate_payment_plan(record: &PaymentPlanRecord, require_all: bool) -> RepositoryResult<()> {
    if require_all && record.name.trim().is_empty() {
        return Err(RepositoryError::InvalidInput(
            "plan name is required".to_owned(),
        ));
    }
    if record.group_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "group is required".to_owned(),
        ));
    }
    if !record.price.is_finite() || record.price <= 0.0 {
        return Err(RepositoryError::InvalidInput(
            "price must be > 0".to_owned(),
        ));
    }
    if let Some(original_price) = record.original_price {
        if !original_price.is_finite() || original_price < 0.0 {
            return Err(RepositoryError::InvalidInput(
                "original price must be >= 0".to_owned(),
            ));
        }
    }
    if record.validity_days <= 0 {
        return Err(RepositoryError::InvalidInput(
            "validity days must be > 0".to_owned(),
        ));
    }
    if record.validity_unit.trim().is_empty() {
        return Err(RepositoryError::InvalidInput(
            "validity unit is required".to_owned(),
        ));
    }
    Ok(())
}

fn sort_payment_plans(records: &mut [PaymentPlanRecord]) {
    records.sort_by_key(|record| (record.sort_order, record.id));
}

fn normalize_plan_features(value: Value) -> Value {
    match value {
        Value::Array(_) => value,
        Value::String(text) => {
            let items = text
                .split('\n')
                .flat_map(|line| line.split(','))
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_owned()))
                .collect::<Vec<_>>();
            Value::Array(items)
        }
        Value::Null => Value::Array(Vec::new()),
        other => other,
    }
}

fn normalize_quota_platform(value: &str) -> RepositoryResult<String> {
    let value = normalize_repository_string(value);
    match value.as_str() {
        "anthropic" | "openai" | "gemini" | "antigravity" => Ok(value),
        _ => Err(RepositoryError::InvalidInput(format!(
            "invalid platform: {value}"
        ))),
    }
}

fn monthly_quota_expired(existing_start: Option<&str>, candidate_start: &str) -> bool {
    let Some(existing_start) = existing_start
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        return true;
    };
    let Some(candidate_start) = DateTime::parse_from_rfc3339(candidate_start)
        .ok()
        .map(|value| value.with_timezone(&Utc))
    else {
        return true;
    };
    candidate_start.signed_duration_since(existing_start) >= Duration::days(30)
}

fn validate_user_platform_quota_records(
    records: &[UserPlatformQuotaRecord],
) -> RepositoryResult<()> {
    if records.len() > 4 {
        return Err(RepositoryError::InvalidInput(
            "quotas length must be <= 4".to_owned(),
        ));
    }
    let mut seen = Vec::new();
    for record in records {
        let platform = normalize_quota_platform(&record.platform)?;
        if seen.iter().any(|existing| existing == &platform) {
            return Err(RepositoryError::InvalidInput(format!(
                "duplicate platform: {platform}"
            )));
        }
        seen.push(platform);
        for (name, value) in [
            ("daily_limit_usd", record.daily_limit_usd),
            ("weekly_limit_usd", record.weekly_limit_usd),
            ("monthly_limit_usd", record.monthly_limit_usd),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || value < 0.0 {
                    return Err(RepositoryError::InvalidInput(format!(
                        "{name} must be a finite number >= 0"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_user_group_rate_records(
    group_id: i64,
    records: Vec<UserGroupRateRecord>,
    require_rate_multiplier: bool,
) -> RepositoryResult<Vec<UserGroupRateRecord>> {
    if group_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "group_id is required".to_owned(),
        ));
    }
    if records.len() > 10_000 {
        return Err(RepositoryError::InvalidInput(
            "user group rate records length must be <= 10000".to_owned(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let mut normalized = Vec::with_capacity(records.len());
    for mut record in records {
        if record.user_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "user_id is required".to_owned(),
            ));
        }
        record.group_id = group_id;
        if require_rate_multiplier && record.rate_multiplier.is_none() {
            return Err(RepositoryError::InvalidInput(
                "rate_multiplier is required".to_owned(),
            ));
        }
        if let Some(rate_multiplier) = record.rate_multiplier {
            if !rate_multiplier.is_finite() || rate_multiplier <= 0.0 {
                return Err(RepositoryError::InvalidInput(
                    "rate_multiplier must be > 0".to_owned(),
                ));
            }
        }
        if let Some(rpm_override) = record.rpm_override {
            if rpm_override < 0 {
                return Err(RepositoryError::InvalidInput(
                    "rpm_override must be >= 0".to_owned(),
                ));
            }
        }
        if record.updated_at.trim().is_empty() {
            record.updated_at = now.clone();
        }
        normalized.push(record);
    }
    normalized.sort_by_key(|record| record.user_id);
    normalized.dedup_by_key(|record| record.user_id);
    Ok(normalized)
}

fn sorted_group_rate_overrides(
    rates: &HashMap<(i64, i64), UserGroupRateRecord>,
    group_id: i64,
) -> Vec<UserGroupRateRecord> {
    let mut records = rates
        .values()
        .filter(|record| {
            record.group_id == group_id
                && (record.rate_multiplier.is_some() || record.rpm_override.is_some())
        })
        .cloned()
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.user_id);
    records
}

fn validate_user_attribute_value_records(
    user_id: i64,
    records: Vec<UserAttributeValueRecord>,
) -> RepositoryResult<Vec<UserAttributeValueRecord>> {
    if user_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "user_id is required".to_owned(),
        ));
    }
    if records.len() > 10_000 {
        return Err(RepositoryError::InvalidInput(
            "user attribute values length must be <= 10000".to_owned(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let mut by_attribute = HashMap::<i64, UserAttributeValueRecord>::new();
    for mut record in records {
        if record.attribute_id <= 0 {
            return Err(RepositoryError::InvalidInput(
                "attribute_id is required".to_owned(),
            ));
        }
        record.user_id = user_id;
        record.value = record.value.trim().to_owned();
        if record.value.is_empty() {
            by_attribute.remove(&record.attribute_id);
            continue;
        }
        if record.created_at.trim().is_empty() {
            record.created_at = now.clone();
        }
        if record.updated_at.trim().is_empty() {
            record.updated_at = now.clone();
        }
        by_attribute.insert(record.attribute_id, record);
    }
    let mut records = by_attribute.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| record.attribute_id);
    Ok(records)
}

fn validate_channel_monitor_history_record(
    mut record: ChannelMonitorHistoryRecord,
) -> RepositoryResult<ChannelMonitorHistoryRecord> {
    if record.monitor_id <= 0 {
        return Err(RepositoryError::InvalidInput(
            "monitor_id is required".to_owned(),
        ));
    }
    record.model = record.model.trim().to_owned();
    if record.model.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "model is required".to_owned(),
        ));
    }
    record.status = normalize_repository_string(&record.status);
    if record.status.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "status is required".to_owned(),
        ));
    }
    if record.latency_ms.map(|value| value < 0).unwrap_or(false)
        || record
            .ping_latency_ms
            .map(|value| value < 0)
            .unwrap_or(false)
    {
        return Err(RepositoryError::InvalidInput(
            "latency must be >= 0".to_owned(),
        ));
    }
    record.message = record.message.trim().to_owned();
    if record.checked_at.trim().is_empty() {
        record.checked_at = Utc::now().to_rfc3339();
    }
    if record.metadata.is_null() {
        record.metadata = Value::Object(Default::default());
    }
    Ok(record)
}

fn validate_content_moderation_log_record(
    mut record: ContentModerationLogRecord,
) -> RepositoryResult<ContentModerationLogRecord> {
    record.request_id = record.request_id.trim().to_owned();
    record.user_email = record.user_email.trim().to_owned();
    record.api_key_name = record.api_key_name.trim().to_owned();
    record.group_name = record.group_name.trim().to_owned();
    record.endpoint = record.endpoint.trim().to_owned();
    record.provider = normalize_repository_string(&record.provider);
    record.model = record.model.trim().to_owned();
    record.mode = normalize_repository_string(&record.mode);
    record.action = normalize_repository_string(&record.action);
    record.highest_category = record.highest_category.trim().to_owned();
    record.input_excerpt = record.input_excerpt.trim().to_owned();
    record.error = record.error.trim().to_owned();
    record.user_status = record.user_status.trim().to_owned();
    if record.user_id.map(|value| value <= 0).unwrap_or(false)
        || record.api_key_id.map(|value| value <= 0).unwrap_or(false)
        || record.group_id.map(|value| value <= 0).unwrap_or(false)
    {
        return Err(RepositoryError::InvalidInput(
            "content moderation log ids must be positive".to_owned(),
        ));
    }
    if !record.highest_score.is_finite() || record.highest_score < 0.0 {
        return Err(RepositoryError::InvalidInput(
            "highest_score must be a finite number >= 0".to_owned(),
        ));
    }
    if record
        .upstream_latency_ms
        .map(|value| value < 0)
        .unwrap_or(false)
        || record
            .queue_delay_ms
            .map(|value| value < 0)
            .unwrap_or(false)
    {
        return Err(RepositoryError::InvalidInput(
            "latency values must be >= 0".to_owned(),
        ));
    }
    if record.violation_count < 0 {
        return Err(RepositoryError::InvalidInput(
            "violation_count must be >= 0".to_owned(),
        ));
    }
    if record.category_scores.is_null() {
        record.category_scores = Value::Object(Default::default());
    }
    if record.threshold_snapshot.is_null() {
        record.threshold_snapshot = Value::Object(Default::default());
    }
    if record.created_at.trim().is_empty() {
        record.created_at = Utc::now().to_rfc3339();
    }
    Ok(record)
}

fn content_moderation_result_matches(
    result: Option<&str>,
    record: &ContentModerationLogRecord,
) -> bool {
    match result.map(normalize_repository_string).as_deref() {
        Some("hit") | Some("flagged") => record.flagged,
        Some("blocked") | Some("block") => {
            matches!(
                record.action.as_str(),
                "block" | "keyword_block" | "hash_block"
            )
        }
        Some("pass") | Some("allow") => !record.flagged && record.error.is_empty(),
        Some("error") => !record.error.is_empty(),
        _ => true,
    }
}

fn normalize_content_moderation_result_filter(value: Option<&str>) -> Option<String> {
    let value = normalize_repository_string(value?);
    match value.as_str() {
        "hit" | "flagged" | "blocked" | "block" | "pass" | "allow" | "error" => Some(value),
        _ => None,
    }
}

fn normalized_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_email_queue_task_record(record: &mut EmailQueueTaskRecord) -> RepositoryResult<()> {
    record.task_type = normalize_repository_string(&record.task_type);
    if record.task_type.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "email task type is required".to_owned(),
        ));
    }
    record.status = normalize_repository_string(&record.status);
    if record.status.is_empty() {
        record.status = "pending".to_owned();
    }
    if !matches!(
        record.status.as_str(),
        "pending" | "processing" | "sent" | "failed"
    ) {
        return Err(RepositoryError::InvalidInput(
            "email task status is invalid".to_owned(),
        ));
    }
    if record.payload.is_null() {
        record.payload = Value::Object(Default::default());
    }
    if record.attempts < 0 {
        return Err(RepositoryError::InvalidInput(
            "email task attempts must be >= 0".to_owned(),
        ));
    }
    if record.max_attempts <= 0 {
        record.max_attempts = 1;
    }
    let now = Utc::now().to_rfc3339();
    if record.created_at.trim().is_empty() {
        record.created_at = now.clone();
    }
    if record.updated_at.trim().is_empty() {
        record.updated_at = now;
    }
    Ok(())
}

fn normalize_admin_collection_name(value: &str) -> RepositoryResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(RepositoryError::InvalidInput(
            "admin collection name is required".to_owned(),
        ));
    }
    Ok(value)
}

fn merge_json(mut base: Value, patch: Value) -> Value {
    let Some(base_object) = base.as_object_mut() else {
        return patch;
    };
    if let Value::Object(patch_object) = patch {
        for (key, value) in patch_object {
            base_object.insert(key, value);
        }
    }
    base
}

fn api_key_status_to_str(status: ApiKeyStatus) -> &'static str {
    match status {
        ApiKeyStatus::Active => "active",
        ApiKeyStatus::Disabled => "disabled",
        ApiKeyStatus::QuotaExhausted => "quota_exhausted",
        ApiKeyStatus::Expired => "expired",
    }
}

fn parse_api_key_status(value: &str) -> RepositoryResult<ApiKeyStatus> {
    match value {
        "active" => Ok(ApiKeyStatus::Active),
        "disabled" | "inactive" => Ok(ApiKeyStatus::Disabled),
        "quota_exhausted" => Ok(ApiKeyStatus::QuotaExhausted),
        "expired" => Ok(ApiKeyStatus::Expired),
        other => Err(RepositoryError::Database(format!(
            "unknown api key status {other}"
        ))),
    }
}

fn group_status_to_str(status: GroupStatus) -> &'static str {
    match status {
        GroupStatus::Active => "active",
        GroupStatus::Disabled => "disabled",
        GroupStatus::Deleted => "deleted",
    }
}

fn parse_group_status(value: &str) -> RepositoryResult<GroupStatus> {
    match value {
        "active" => Ok(GroupStatus::Active),
        "disabled" => Ok(GroupStatus::Disabled),
        "deleted" => Ok(GroupStatus::Deleted),
        other => Err(RepositoryError::Database(format!(
            "unknown group status {other}"
        ))),
    }
}

fn provider_to_str(provider: Provider) -> &'static str {
    provider.as_str()
}

fn parse_provider(value: &str) -> RepositoryResult<Provider> {
    match value {
        "openai" => Ok(Provider::OpenAi),
        "deepseek" => Ok(Provider::DeepSeek),
        "anthropic" => Ok(Provider::Anthropic),
        "gemini" => Ok(Provider::Gemini),
        "vertex" => Ok(Provider::Vertex),
        "antigravity" => Ok(Provider::Antigravity),
        other => Err(RepositoryError::Database(format!(
            "unknown provider {other}"
        ))),
    }
}

fn upstream_protocol_to_str(protocol: UpstreamProtocol) -> &'static str {
    match protocol {
        UpstreamProtocol::OpenAiResponses => "openai_responses",
        UpstreamProtocol::OpenAiChatCompletions => "openai_chat_completions",
        UpstreamProtocol::AnthropicMessages => "anthropic_messages",
        UpstreamProtocol::GeminiGenerateContent => "gemini_generate_content",
    }
}

fn parse_upstream_protocol(value: &str) -> RepositoryResult<UpstreamProtocol> {
    match value {
        "openai_responses" => Ok(UpstreamProtocol::OpenAiResponses),
        "openai_chat_completions" => Ok(UpstreamProtocol::OpenAiChatCompletions),
        "anthropic_messages" => Ok(UpstreamProtocol::AnthropicMessages),
        "gemini_generate_content" => Ok(UpstreamProtocol::GeminiGenerateContent),
        other => Err(RepositoryError::Database(format!(
            "unknown upstream protocol {other}"
        ))),
    }
}

async fn update_email_task_status(
    pool: &sqlx::PgPool,
    id: i64,
    status: &str,
    last_error: Option<String>,
    increment_attempts: bool,
) -> RepositoryResult<EmailQueueTaskRecord> {
    let row = sqlx::query_as::<_, EmailQueueTaskRow>(
        r#"
        UPDATE email_queue_tasks
        SET
            status = $2,
            attempts = attempts + $3,
            last_error = CASE WHEN $4::TEXT IS NULL THEN last_error ELSE $4 END,
            updated_at_text = $5,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, task_type, status, payload, attempts, max_attempts,
            last_error, created_at_text, updated_at_text
        "#,
    )
    .bind(id)
    .bind(status)
    .bind(if increment_attempts { 1 } else { 0 })
    .bind(last_error)
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(pool)
    .await
    .map_err(db_error)?;
    row.map(EmailQueueTaskRecord::from)
        .ok_or(RepositoryError::NotFound {
            entity: "email_queue_task",
            id,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ApiKeyStatus, ModelMappingRule, Provider, UpstreamProtocol};

    fn user(id: i64, email: &str) -> UserRecord {
        UserRecord {
            id,
            email: email.to_owned(),
            username: format!("user-{id}"),
            role: "user".to_owned(),
            status: "active".to_owned(),
        }
    }

    fn group(id: i64, status: GroupStatus) -> Group {
        Group {
            id: GroupId(id),
            name: format!("group-{id}"),
            status,
        }
    }

    fn account(id: i64) -> Account {
        Account {
            id: AccountId(id),
            name: format!("account-{id}"),
            provider: Provider::DeepSeek,
            default_upstream_protocol: UpstreamProtocol::OpenAiChatCompletions,
            base_url: Some("https://api.deepseek.com".to_owned()),
            api_key: Some("sk-upstream".to_owned()),
            model_mapping: vec![ModelMappingRule {
                source: "gpt-5.4".to_owned(),
                target: "deepseek-v4-pro".to_owned(),
            }],
            extra: Value::Null,
            enabled: true,
        }
    }

    fn usage(api_key_id: i64, group_id: i64, protocol: DownstreamProtocol) -> UsageRecord {
        UsageRecord {
            id: 0,
            user_id: 1,
            api_key_id,
            group_id: Some(GroupId(group_id)),
            account_id: Some(AccountId(10)),
            downstream_protocol: protocol,
            upstream_protocol: "openai_chat_completions".to_owned(),
            provider: "deepseek".to_owned(),
            endpoint: "/responses".to_owned(),
            requested_model: "gpt-5.4".to_owned(),
            upstream_model: "deepseek-v4-pro".to_owned(),
            input_tokens: 11,
            output_tokens: 7,
            cache_creation_tokens: 2,
            cache_read_tokens: 3,
            actual_cost: 0.42,
            status: "success".to_owned(),
            created_at_unix: 1_780_704_000,
            metadata: Value::Null,
        }
    }

    fn cleanup_filter() -> UsageCleanupFilter {
        UsageCleanupFilter {
            start_time_unix: 1_780_790_000,
            end_time_unix: 1_780_800_000,
            user_id: Some(1),
            api_key_id: Some(ApiKeyId(7)),
            group_id: Some(GroupId(2)),
            account_id: Some(AccountId(10)),
            model: Some("gpt-5.4".to_owned()),
            request_type: Some("sync".to_owned()),
            stream: Some(false),
            billing_type: Some(1),
        }
    }

    fn cleanup_task(status: &str) -> UsageCleanupTaskRecord {
        UsageCleanupTaskRecord {
            id: 0,
            status: status.to_owned(),
            filters: cleanup_filter(),
            created_by: 1,
            deleted_rows: 0,
            error_message: None,
            canceled_by: None,
            canceled_at: None,
            started_at: None,
            finished_at: None,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    fn payment_order(id: i64, status: &str) -> PaymentOrderRecord {
        PaymentOrderRecord {
            id,
            user_id: 1,
            amount: 12.5,
            pay_amount: 12.5,
            currency: "CNY".to_owned(),
            fee_rate: 0.0,
            payment_type: "alipay".to_owned(),
            out_trade_no: format!("sub2_20260606{id:08}"),
            status: status.to_owned(),
            order_type: "balance".to_owned(),
            refund_amount: 0.0,
            refund_reason: None,
            refund_request_reason: None,
            plan_id: None,
            provider_instance_id: Some("alipay-default".to_owned()),
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            expires_at: "2026-06-06T01:00:00Z".to_owned(),
            paid_at: None,
            completed_at: None,
            cancelled_at: None,
            refund_requested_at: None,
            refunded_at: None,
            webhook_count: 0,
            metadata: Value::Null,
        }
    }

    fn payment_audit(order_id: &str, action: &str) -> PaymentAuditRecord {
        PaymentAuditRecord {
            id: 0,
            order_id: order_id.to_owned(),
            action: action.to_owned(),
            detail: "{}".to_owned(),
            operator: "system".to_owned(),
            created_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    fn balance_transaction(order_id: &str, amount: f64) -> BalanceTransactionRecord {
        BalanceTransactionRecord {
            id: 0,
            user_id: 1,
            order_id: order_id.to_owned(),
            transaction_type: "payment_recharge".to_owned(),
            amount,
            balance_after: 0.0,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            metadata: Value::Null,
        }
    }

    fn subscription(order_id: &str) -> UserSubscriptionRecord {
        UserSubscriptionRecord {
            id: 0,
            user_id: 1,
            group_id: 7,
            plan_id: Some(3),
            status: "active".to_owned(),
            starts_at: "2026-06-06T00:00:00Z".to_owned(),
            expires_at: "2026-07-06T00:00:00Z".to_owned(),
            daily_usage_usd: 0.1,
            weekly_usage_usd: 0.2,
            monthly_usage_usd: 0.3,
            daily_window_start: Some("2026-06-06T00:00:00Z".to_owned()),
            weekly_window_start: Some("2026-06-01T00:00:00Z".to_owned()),
            monthly_window_start: Some("2026-06-06T00:00:00Z".to_owned()),
            source_order_id: order_id.to_owned(),
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            metadata: Value::Null,
        }
    }

    fn platform_quota(platform: &str, daily: Option<f64>) -> UserPlatformQuotaRecord {
        UserPlatformQuotaRecord {
            id: 0,
            user_id: 1,
            platform: platform.to_owned(),
            daily_limit_usd: daily,
            weekly_limit_usd: Some(10.0),
            monthly_limit_usd: None,
            daily_usage_usd: 0.0,
            weekly_usage_usd: 0.0,
            monthly_usage_usd: 0.0,
            daily_window_start: None,
            weekly_window_start: None,
            monthly_window_start: None,
        }
    }

    fn payment_provider_instance(
        id: i64,
        provider_key: &str,
        supported_types: Vec<&str>,
        sort_order: i32,
    ) -> PaymentProviderInstanceRecord {
        PaymentProviderInstanceRecord {
            id,
            provider_key: provider_key.to_owned(),
            name: format!("{provider_key}-{id}"),
            config: serde_json::json!({ "webhookSecret": "secret" }),
            supported_types: supported_types.into_iter().map(ToOwned::to_owned).collect(),
            enabled: true,
            payment_mode: "redirect".to_owned(),
            sort_order,
            limits: serde_json::json!({}),
            refund_enabled: false,
            allow_user_refund: false,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    fn payment_plan(id: i64, name: &str, sort_order: i32, for_sale: bool) -> PaymentPlanRecord {
        PaymentPlanRecord {
            id,
            group_id: 1,
            name: name.to_owned(),
            description: format!("{name} plan"),
            price: 9.9,
            original_price: Some(19.9),
            validity_days: 30,
            validity_unit: "day".to_owned(),
            features: serde_json::json!("fast,stable"),
            product_name: name.to_owned(),
            for_sale,
            sort_order,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    fn moderation_log(
        id: i64,
        request_id: &str,
        action: &str,
        flagged: bool,
    ) -> ContentModerationLogRecord {
        ContentModerationLogRecord {
            id,
            request_id: request_id.to_owned(),
            user_id: Some(1),
            user_email: "admin@example.com".to_owned(),
            api_key_id: Some(7),
            api_key_name: "admin-key".to_owned(),
            group_id: Some(2),
            group_name: "codex".to_owned(),
            endpoint: "/responses".to_owned(),
            provider: "openai".to_owned(),
            model: "gpt-5.4".to_owned(),
            mode: "block".to_owned(),
            action: action.to_owned(),
            flagged,
            highest_category: if flagged { "violence" } else { "" }.to_owned(),
            highest_score: if flagged { 0.91 } else { 0.0 },
            category_scores: serde_json::json!({ "violence": 0.91 }),
            threshold_snapshot: serde_json::json!({ "violence": 0.7 }),
            input_excerpt: "hello moderation".to_owned(),
            upstream_latency_ms: Some(123),
            error: String::new(),
            violation_count: if flagged { 1 } else { 0 },
            auto_banned: false,
            email_sent: false,
            user_status: "active".to_owned(),
            queue_delay_ms: Some(4),
            created_at: format!("2026-06-0{}T00:00:00Z", id.max(1)),
        }
    }

    fn oauth_identity(user_id: i64, provider: &str, provider_subject: &str) -> OAuthIdentityRecord {
        OAuthIdentityRecord {
            id: 0,
            user_id,
            provider: provider.to_owned(),
            provider_key: provider.to_owned(),
            provider_subject: provider_subject.to_owned(),
            email: Some(format!("user-{user_id}@example.com")),
            bound_at_unix: 1_780_704_000,
            metadata: serde_json::json!({ "intent": "login" }),
        }
    }

    fn auth_session(user_id: i64, suffix: &str) -> AuthSessionRecord {
        AuthSessionRecord {
            id: 0,
            user_id,
            access_token: format!("dev-access-{suffix}"),
            refresh_token: format!("dev-refresh-{suffix}"),
            access_expires_at_unix: 1_780_704_000 + 3600,
            refresh_expires_at_unix: 1_780_704_000 + 30 * 24 * 3600,
            revoked_at_unix: None,
            created_at_unix: 1_780_704_000,
            metadata: serde_json::json!({ "source": "test" }),
        }
    }

    fn auth_credential(user_id: i64, email: &str, password_hash: &str) -> AuthCredentialRecord {
        AuthCredentialRecord {
            user_id,
            email: email.to_owned(),
            password_hash: password_hash.to_owned(),
            status: "active".to_owned(),
            updated_at_unix: 1_780_704_000,
        }
    }

    #[tokio::test]
    async fn stores_users_api_keys_and_active_groups() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "A@Example.com")).await.unwrap();
        repo.upsert_group(group(1, GroupStatus::Active))
            .await
            .unwrap();
        repo.upsert_group(group(2, GroupStatus::Disabled))
            .await
            .unwrap();
        repo.upsert_api_key(ApiKey {
            id: ApiKeyId(7),
            user_id: 2,
            key: "sk-repository-dev".to_owned(),
            name: "dev key".to_owned(),
            group_id: Some(GroupId(1)),
            status: ApiKeyStatus::Active,
            quota: 12.5,
            quota_used: 1.25,
            rate_limit_5h: 2.5,
            rate_limit_1d: 5.0,
            rate_limit_7d: 7.5,
            usage_5h: 0.5,
            usage_1d: 1.0,
            usage_7d: 1.5,
            window_5h_start: Some(1_000),
            window_1d_start: Some(2_000),
            window_7d_start: Some(3_000),
        })
        .await
        .unwrap();

        assert_eq!(repo.get_user_by_email("a@example.com").await.unwrap().id, 2);
        assert_eq!(repo.list_users().await.unwrap().len(), 1);
        assert_eq!(repo.list_active_groups().await.unwrap().len(), 1);
        assert_eq!(
            repo.list_api_keys_by_user(2).await.unwrap()[0].active_group_id(),
            Some(GroupId(1))
        );
        assert_eq!(
            repo.list_api_keys_by_group(GroupId(1)).await.unwrap()[0].id,
            ApiKeyId(7)
        );
        assert_eq!(
            repo.get_api_key_by_key("sk-repository-dev")
                .await
                .unwrap()
                .id,
            ApiKeyId(7)
        );
        let persisted_key = repo.get_api_key(ApiKeyId(7)).await.unwrap();
        assert_eq!(persisted_key.quota, 12.5);
        assert_eq!(persisted_key.quota_used, 1.25);
        assert_eq!(persisted_key.rate_limit_5h, 2.5);
        assert_eq!(persisted_key.usage_7d, 1.5);
        assert_eq!(persisted_key.window_1d_start, Some(2_000));
        assert!(repo
            .list_api_keys_by_group(GroupId(2))
            .await
            .unwrap()
            .is_empty());
        repo.delete_api_key(ApiKeyId(7)).await.unwrap();
        assert!(repo.get_api_key(ApiKeyId(7)).await.is_err());
        assert!(repo.list_api_keys_by_user(2).await.unwrap().is_empty());
        assert!(repo
            .list_api_keys_by_group(GroupId(1))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn stores_group_bindings_ordered_by_priority() {
        let repo = InMemoryRepository::new();
        repo.upsert_group(group(1, GroupStatus::Active))
            .await
            .unwrap();
        repo.upsert_account(account(10)).await.unwrap();
        repo.upsert_account(account(11)).await.unwrap();

        repo.bind_account_to_group(AccountGroupBinding {
            account: account(10),
            group_id: GroupId(1),
            supported_downstream_protocols: vec![DownstreamProtocol::OpenAiChatCompletions],
            upstream_protocol_override: None,
            priority: 50,
        })
        .await
        .unwrap();
        repo.bind_account_to_group(AccountGroupBinding {
            account: account(11),
            group_id: GroupId(1),
            supported_downstream_protocols: vec![DownstreamProtocol::OpenAiResponses],
            upstream_protocol_override: Some(UpstreamProtocol::OpenAiResponses),
            priority: 10,
        })
        .await
        .unwrap();

        let bindings = repo.list_bindings_by_group(GroupId(1)).await.unwrap();
        assert_eq!(bindings[0].account.id, AccountId(11));
        assert_eq!(
            bindings[0].supported_downstream_protocols,
            vec![DownstreamProtocol::OpenAiResponses]
        );
        assert_eq!(bindings[1].account.id, AccountId(10));
    }

    #[tokio::test]
    async fn deletes_groups_and_accounts_with_dependent_repository_state() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "owner@example.com"))
            .await
            .unwrap();
        repo.upsert_group(group(1, GroupStatus::Active))
            .await
            .unwrap();
        repo.upsert_group(group(2, GroupStatus::Disabled))
            .await
            .unwrap();
        repo.upsert_account(account(10)).await.unwrap();
        repo.upsert_api_key(ApiKey {
            id: ApiKeyId(7),
            user_id: 2,
            key: "sk-repository-dependent".to_owned(),
            name: "dev key".to_owned(),
            group_id: Some(GroupId(1)),
            status: ApiKeyStatus::Active,
            ..ApiKey::default()
        })
        .await
        .unwrap();
        repo.bind_account_to_group(AccountGroupBinding {
            account: account(10),
            group_id: GroupId(1),
            supported_downstream_protocols: vec![DownstreamProtocol::OpenAiResponses],
            upstream_protocol_override: Some(UpstreamProtocol::OpenAiChatCompletions),
            priority: 10,
        })
        .await
        .unwrap();

        assert_eq!(repo.list_groups().await.unwrap().len(), 2);
        assert_eq!(
            repo.get_account(AccountId(10)).await.unwrap().name,
            "account-10"
        );
        repo.delete_account(AccountId(10)).await.unwrap();
        assert!(repo.get_account(AccountId(10)).await.is_err());
        assert!(repo
            .list_bindings_by_group(GroupId(1))
            .await
            .unwrap()
            .is_empty());

        repo.delete_group(GroupId(1)).await.unwrap();
        assert!(repo.get_group(GroupId(1)).await.is_err());
        assert_eq!(repo.get_api_key(ApiKeyId(7)).await.unwrap().group_id, None);
        assert_eq!(repo.list_active_groups().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn deletes_users_and_owned_api_keys() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "owner@example.com"))
            .await
            .unwrap();
        repo.upsert_api_key(ApiKey {
            id: ApiKeyId(7),
            user_id: 2,
            key: "sk-repository-owned".to_owned(),
            name: "dev key".to_owned(),
            group_id: None,
            status: ApiKeyStatus::Active,
            ..ApiKey::default()
        })
        .await
        .unwrap();

        repo.delete_user(2).await.unwrap();

        assert!(repo.get_user(2).await.is_err());
        assert!(repo.list_api_keys_by_user(2).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stores_oauth_identities_and_rejects_cross_user_rebinds() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "owner@example.com"))
            .await
            .unwrap();
        repo.upsert_user(user(3, "other@example.com"))
            .await
            .unwrap();

        let stored = repo
            .upsert_oauth_identity(oauth_identity(2, "GitHub", "Subject-42"))
            .await
            .unwrap();
        assert_eq!(stored.id, 1);
        assert_eq!(stored.provider, "github");
        assert_eq!(stored.provider_key, "github");
        assert_eq!(stored.provider_subject, "Subject-42");
        assert_eq!(stored.email.as_deref(), Some("user-2@example.com"));

        let mut updated = oauth_identity(2, "github", "Subject-42");
        updated.email = Some("Updated@Example.com".to_owned());
        let updated = repo.upsert_oauth_identity(updated).await.unwrap();
        assert_eq!(updated.id, stored.id);
        assert_eq!(updated.email.as_deref(), Some("updated@example.com"));

        let conflict = repo
            .upsert_oauth_identity(oauth_identity(3, "github", "Subject-42"))
            .await
            .unwrap_err();
        assert_eq!(
            conflict,
            RepositoryError::Conflict("oauth identity is already bound to another user".to_owned())
        );

        assert_eq!(
            repo.get_oauth_identity("github", "github", "Subject-42")
                .await
                .unwrap()
                .user_id,
            2
        );
        assert_eq!(
            repo.list_oauth_identities_by_user(2).await.unwrap().len(),
            1
        );
        repo.delete_oauth_identity("github", "github", "Subject-42")
            .await
            .unwrap();
        assert!(repo
            .get_oauth_identity("github", "github", "Subject-42")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_auth_sessions_and_revokes_by_refresh_or_user() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "owner@example.com"))
            .await
            .unwrap();
        repo.upsert_user(user(3, "other@example.com"))
            .await
            .unwrap();

        let first = repo
            .upsert_auth_session(auth_session(2, "one"))
            .await
            .unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(
            repo.get_auth_session_by_access_token("dev-access-one")
                .await
                .unwrap()
                .user_id,
            2
        );
        assert_eq!(
            repo.get_auth_session_by_refresh_token("dev-refresh-one")
                .await
                .unwrap()
                .user_id,
            2
        );

        let duplicate = repo
            .upsert_auth_session(AuthSessionRecord {
                access_token: "dev-access-one".to_owned(),
                ..auth_session(3, "other")
            })
            .await
            .unwrap_err();
        assert!(matches!(
            duplicate,
            RepositoryError::Duplicate {
                entity: "auth_session access_token",
                ..
            }
        ));

        let revoked = repo
            .revoke_auth_session_by_refresh_token("dev-refresh-one", 1_780_705_000)
            .await
            .unwrap();
        assert_eq!(revoked.revoked_at_unix, Some(1_780_705_000));

        repo.upsert_auth_session(auth_session(2, "two"))
            .await
            .unwrap();
        repo.upsert_auth_session(auth_session(3, "three"))
            .await
            .unwrap();
        assert_eq!(
            repo.revoke_auth_sessions_by_user(2, 1_780_706_000)
                .await
                .unwrap(),
            1
        );
        let user_sessions = repo.list_auth_sessions_by_user(2).await.unwrap();
        assert_eq!(user_sessions.len(), 2);
        assert!(user_sessions
            .iter()
            .all(|session| session.revoked_at_unix.is_some()));
    }

    #[tokio::test]
    async fn stores_auth_credentials_by_user_and_normalized_email() {
        let repo = InMemoryRepository::new();
        repo.upsert_user(user(2, "owner@example.com"))
            .await
            .unwrap();
        repo.upsert_user(user(3, "other@example.com"))
            .await
            .unwrap();

        let stored = repo
            .upsert_auth_credential(auth_credential(2, "Owner@Example.com", "hash-one"))
            .await
            .unwrap();
        assert_eq!(stored.email, "owner@example.com");
        assert_eq!(
            repo.get_auth_credential_by_email("OWNER@example.com")
                .await
                .unwrap()
                .user_id,
            2
        );

        let updated = repo
            .upsert_auth_credential(auth_credential(2, "owner@example.com", "hash-two"))
            .await
            .unwrap();
        assert_eq!(updated.password_hash, "hash-two");
        assert_eq!(
            repo.get_auth_credential_by_user_id(2)
                .await
                .unwrap()
                .password_hash,
            "hash-two"
        );

        let duplicate = repo
            .upsert_auth_credential(auth_credential(3, "owner@example.com", "hash-three"))
            .await
            .unwrap_err();
        assert!(matches!(
            duplicate,
            RepositoryError::Duplicate {
                entity: "auth_credential email",
                ..
            }
        ));

        repo.delete_auth_credential(2).await.unwrap();
        assert!(repo.get_auth_credential_by_user_id(2).await.is_err());
        assert!(repo
            .get_auth_credential_by_email("owner@example.com")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_generic_admin_collection_items() {
        let repo = InMemoryRepository::new();
        repo.upsert_admin_collection_item(AdminCollectionItemRecord {
            collection: "Announcements".to_owned(),
            id: 5,
            item: serde_json::json!({ "id": 5, "title": "hello" }),
        })
        .await
        .unwrap();
        repo.upsert_admin_collection_item(AdminCollectionItemRecord {
            collection: "announcements".to_owned(),
            id: 7,
            item: serde_json::json!({ "id": 7, "title": "world" }),
        })
        .await
        .unwrap();

        let items = repo
            .list_admin_collection_items("announcements")
            .await
            .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].collection, "announcements");
        assert_eq!(items[0].item["title"], "hello");

        let item = repo
            .get_admin_collection_item("announcements", 7)
            .await
            .unwrap();
        assert_eq!(item.item["title"], "world");

        repo.delete_admin_collection_item("announcements", 5)
            .await
            .unwrap();
        assert!(repo
            .get_admin_collection_item("announcements", 5)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_system_settings_by_namespace_and_key() {
        let repo = InMemoryRepository::new();
        repo.upsert_system_setting(SystemSettingRecord {
            namespace: "Admin".to_owned(),
            key: "Settings".to_owned(),
            value: serde_json::json!({ "site_name": "Sub2API Next" }),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        })
        .await
        .unwrap();
        repo.upsert_system_setting(SystemSettingRecord {
            namespace: "admin".to_owned(),
            key: "payment_config".to_owned(),
            value: serde_json::json!({ "enabled": false }),
            updated_at: "2026-06-07T00:00:00Z".to_owned(),
        })
        .await
        .unwrap();

        let settings = repo.get_system_setting("admin", "settings").await.unwrap();
        assert_eq!(settings.value["site_name"], "Sub2API Next");
        assert_eq!(settings.namespace, "admin");
        assert_eq!(settings.key, "settings");

        let listed = repo.list_system_settings("admin").await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].key, "payment_config");
        assert_eq!(listed[1].key, "settings");

        repo.delete_system_setting("admin", "settings")
            .await
            .unwrap();
        assert!(repo.get_system_setting("admin", "settings").await.is_err());
        assert!(repo
            .upsert_system_setting(SystemSettingRecord {
                namespace: "bad namespace".to_owned(),
                key: "settings".to_owned(),
                value: serde_json::json!({}),
                updated_at: "2026-06-06T00:00:00Z".to_owned(),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_and_transitions_email_queue_tasks() {
        let repo = InMemoryRepository::new();
        let task = repo
            .enqueue_email_task(EmailQueueTaskRecord {
                id: 0,
                task_type: " Verify_Code ".to_owned(),
                status: String::new(),
                payload: serde_json::json!({ "email": "admin@example.com" }),
                attempts: 0,
                max_attempts: 2,
                last_error: None,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap();

        assert_eq!(task.id, 1);
        assert_eq!(task.task_type, "verify_code");
        assert_eq!(task.status, "pending");
        assert_eq!(repo.list_pending_email_tasks(10).await.unwrap().len(), 1);

        let processing = repo.mark_email_task_processing(task.id).await.unwrap();
        assert_eq!(processing.status, "processing");
        assert_eq!(processing.attempts, 1);

        let failed = repo
            .mark_email_task_failed(task.id, "smtp down".to_owned())
            .await
            .unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.last_error.as_deref(), Some("smtp down"));
        assert!(repo.list_pending_email_tasks(10).await.unwrap().is_empty());

        let sent = repo.mark_email_task_sent(task.id).await.unwrap();
        assert_eq!(sent.status, "sent");
        assert_eq!(sent.last_error, None);
    }

    #[tokio::test]
    async fn records_and_summarizes_usage_with_filters() {
        let repo = InMemoryRepository::new();
        repo.insert_usage(usage(7, 1, DownstreamProtocol::OpenAiResponses))
            .await
            .unwrap();
        repo.insert_usage(usage(8, 1, DownstreamProtocol::OpenAiChatCompletions))
            .await
            .unwrap();

        let records = repo
            .list_usage(UsageFilter {
                api_key_id: Some(ApiKeyId(7)),
                ..UsageFilter::all()
            })
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 1);

        let summary = repo
            .summarize_usage(UsageFilter {
                group_id: Some(GroupId(1)),
                ..UsageFilter::all()
            })
            .await
            .unwrap();
        assert_eq!(summary.requests, 2);
        assert_eq!(summary.total_tokens(), 46);
        assert!((summary.actual_cost - 0.84).abs() < 0.000_000_1);

        repo.insert_usage(UsageRecord {
            api_key_id: 9,
            requested_model: "gpt-5.5".to_owned(),
            upstream_model: "deepseek-reasoner".to_owned(),
            status: "failed".to_owned(),
            created_at_unix: 1_780_790_400,
            metadata: serde_json::json!({
                "request_type": "stream",
                "stream": true,
                "billing_mode": "token",
                "billing_type": 2,
                "model_mapping_chain": ["gpt-5.5", "deepseek-reasoner"]
            }),
            ..usage(9, 2, DownstreamProtocol::OpenAiChatCompletions)
        })
        .await
        .unwrap();

        let filtered = repo
            .list_usage(UsageFilter {
                model_contains: Some("reasoner".to_owned()),
                request_type: Some("stream".to_owned()),
                stream: Some(true),
                status: Some("FAILED".to_owned()),
                billing_mode: Some("TOKEN".to_owned()),
                billing_type: Some(2),
                created_at_unix_gte: Some(1_780_790_000),
                created_at_unix_lt: Some(1_780_800_000),
                ..UsageFilter::all()
            })
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].api_key_id, 9);

        let filtered_summary = repo
            .summarize_usage(UsageFilter {
                request_type: Some("stream".to_owned()),
                stream: Some(true),
                billing_type: Some(2),
                ..UsageFilter::all()
            })
            .await
            .unwrap();
        assert_eq!(filtered_summary.requests, 1);
        assert_eq!(filtered_summary.total_tokens(), 23);
    }

    #[tokio::test]
    async fn stores_and_executes_usage_cleanup_tasks() {
        let repo = InMemoryRepository::new();
        let mut matched = usage(7, 2, DownstreamProtocol::OpenAiResponses);
        matched.id = 1;
        matched.created_at_unix = 1_780_790_400;
        matched.metadata = serde_json::json!({
            "request_type": "sync",
            "stream": false,
            "billing_type": 1
        });
        let mut outside_range = matched.clone();
        outside_range.id = 2;
        outside_range.created_at_unix = 1_780_810_000;
        let mut other_model = matched.clone();
        other_model.id = 3;
        other_model.requested_model = "other-model".to_owned();
        repo.insert_usage(matched).await.unwrap();
        repo.insert_usage(outside_range).await.unwrap();
        repo.insert_usage(other_model).await.unwrap();

        let created = repo
            .create_usage_cleanup_task(cleanup_task("pending"))
            .await
            .unwrap();
        assert_eq!(created.status, "pending");
        assert_eq!(
            repo.list_usage_cleanup_tasks(Pagination::new(1, 20))
                .await
                .unwrap()
                .total,
            1
        );
        let claimed = repo
            .claim_next_usage_cleanup_task(1800)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, created.id);
        assert_eq!(claimed.status, "running");
        assert_eq!(
            repo.delete_usage_batch(cleanup_filter(), 10).await.unwrap(),
            1
        );
        repo.update_usage_cleanup_task_progress(created.id, 1)
            .await
            .unwrap();
        let finished = repo
            .mark_usage_cleanup_task_succeeded(created.id, 1)
            .await
            .unwrap();
        assert_eq!(finished.status, "succeeded");
        assert_eq!(finished.deleted_rows, 1);
        assert!(matches!(
            repo.cancel_usage_cleanup_task(created.id, 1).await,
            Err(RepositoryError::Conflict(_))
        ));
        assert_eq!(repo.list_usage(UsageFilter::all()).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn consistency_repository_coordinates_locks_jobs_slots_and_windows() {
        let repo = InMemoryRepository::new();

        let lock = repo
            .try_acquire_lock("backup:scheduler", "instance-a", 60, serde_json::json!({}))
            .await
            .unwrap()
            .expect("lock can be acquired");
        assert_eq!(lock.fencing_token, 1);
        assert!(repo
            .try_acquire_lock("backup:scheduler", "instance-b", 60, serde_json::json!({}))
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .renew_lock("backup:scheduler", "instance-a", lock.fencing_token, 60)
            .await
            .unwrap());
        assert!(repo
            .release_lock("backup:scheduler", "instance-a", lock.fencing_token)
            .await
            .unwrap());

        let first = repo
            .create_idempotent_job(
                "backup",
                "schedule:daily:2026-06-17",
                serde_json::json!({ "kind": "daily" }),
            )
            .await
            .unwrap();
        assert!(first.created);
        let duplicate = repo
            .create_idempotent_job(
                "backup",
                "schedule:daily:2026-06-17",
                serde_json::json!({ "kind": "ignored" }),
            )
            .await
            .unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.job.id, first.job.id);

        let claimed = repo
            .claim_next_idempotent_job("backup", "worker-a", 60)
            .await
            .unwrap()
            .expect("pending job can be claimed");
        assert_eq!(claimed.status, "running");
        let completed = repo
            .complete_idempotent_job(claimed.id, "worker-a", serde_json::json!({ "ok": true }))
            .await
            .unwrap();
        assert_eq!(completed.status, "completed");

        let slot = repo
            .acquire_account_concurrency_slot(7, "req-1", 1, 60, serde_json::json!({}))
            .await
            .unwrap()
            .expect("first slot can be acquired");
        assert_eq!(slot.in_flight, 1);
        assert!(repo
            .acquire_account_concurrency_slot(7, "req-2", 1, 60, serde_json::json!({}))
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .release_account_concurrency_slot(7, "req-1")
            .await
            .unwrap());
        assert!(repo
            .acquire_account_concurrency_slot(7, "req-2", 1, 60, serde_json::json!({}))
            .await
            .unwrap()
            .is_some());

        let first_hit = repo
            .hit_rate_limit_fixed_window("api-key:1", 2, 1_800, 60)
            .await
            .unwrap();
        assert!(first_hit.allowed);
        assert_eq!(first_hit.remaining, 1);
        let second_hit = repo
            .hit_rate_limit_fixed_window("api-key:1", 2, 1_800, 60)
            .await
            .unwrap();
        assert!(second_hit.allowed);
        let third_hit = repo
            .hit_rate_limit_fixed_window("api-key:1", 2, 1_800, 60)
            .await
            .unwrap();
        assert!(!third_hit.allowed);
        assert_eq!(third_hit.count, 3);

        let empty_usage = repo
            .get_rate_limit_usage_fixed_window("api-key:1:5h", 1.0, 3_600, 300)
            .await
            .unwrap();
        assert_eq!(empty_usage.usage, 0.0);
        assert_eq!(empty_usage.remaining, 1.0);
        let usage = repo
            .add_rate_limit_usage_fixed_window("api-key:1:5h", 0.4, 1.0, 3_600, 300)
            .await
            .unwrap();
        assert_eq!(usage.usage, 0.4);
        assert_eq!(usage.remaining, 0.6);
        let usage = repo
            .add_rate_limit_usage_fixed_window("api-key:1:5h", 0.7, 1.0, 3_600, 300)
            .await
            .unwrap();
        assert_eq!(usage.usage, 1.1);
        assert_eq!(usage.remaining, 0.0);
    }

    #[tokio::test]
    async fn external_postgres_consistency_repository_when_enabled() {
        if std::env::var("BACKEND_NEXT_EXTERNAL_DEPS").ok().as_deref() != Some("1") {
            return;
        }
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required when BACKEND_NEXT_EXTERNAL_DEPS=1");
        let repo = PostgresRepository::connect(&database_url).await.unwrap();
        let namespace = format!(
            "test:{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let account_id = 900_000 + Utc::now().timestamp_nanos_opt().unwrap_or_default() % 100_000;
        let lock_name = format!("{namespace}:lock");
        let owner_a = format!("{namespace}:owner-a");
        let owner_b = format!("{namespace}:owner-b");

        let lock = repo
            .try_acquire_lock(&lock_name, &owner_a, 60, serde_json::json!({}))
            .await
            .unwrap()
            .expect("postgres lock can be acquired");
        assert!(repo
            .try_acquire_lock(&lock_name, &owner_b, 60, serde_json::json!({}))
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .release_lock(&lock_name, &owner_a, lock.fencing_token)
            .await
            .unwrap());

        let job_type = format!("{namespace}:backup");
        let create = repo
            .create_idempotent_job(&job_type, "daily-window", serde_json::json!({ "a": 1 }))
            .await
            .unwrap();
        assert!(create.created);
        let duplicate = repo
            .create_idempotent_job(&job_type, "daily-window", serde_json::json!({ "a": 2 }))
            .await
            .unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.job.id, create.job.id);
        let claimed = repo
            .claim_next_idempotent_job(&job_type, &owner_a, 60)
            .await
            .unwrap()
            .expect("postgres job can be claimed");
        assert_eq!(claimed.attempts, 1);
        assert!(repo
            .claim_next_idempotent_job(&job_type, &owner_b, 60)
            .await
            .unwrap()
            .is_none());
        let completed = repo
            .complete_idempotent_job(claimed.id, &owner_a, serde_json::json!({ "done": true }))
            .await
            .unwrap();
        assert_eq!(completed.status, "completed");

        let slot_scope = Utc::now().timestamp();
        let slot_a = format!("{namespace}:req-a:{slot_scope}");
        let slot_b = format!("{namespace}:req-b:{slot_scope}");
        sqlx::query("DELETE FROM account_concurrency_slots WHERE account_id = $1")
            .bind(account_id)
            .execute(repo.pool())
            .await
            .unwrap();
        let slot = repo
            .acquire_account_concurrency_slot(account_id, &slot_a, 1, 60, serde_json::json!({}))
            .await
            .unwrap()
            .expect("postgres concurrency slot can be acquired");
        assert_eq!(slot.in_flight, 1);
        assert!(repo
            .acquire_account_concurrency_slot(account_id, &slot_b, 1, 60, serde_json::json!({}))
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .release_account_concurrency_slot(account_id, &slot_a)
            .await
            .unwrap());

        let window_start = Utc::now().timestamp();
        assert!(
            repo.hit_rate_limit_fixed_window(&format!("{namespace}:rate"), 1, window_start, 60)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !repo
                .hit_rate_limit_fixed_window(&format!("{namespace}:rate"), 1, window_start, 60)
                .await
                .unwrap()
                .allowed
        );
        let usage_scope = format!("{namespace}:rate-usage");
        let empty_usage = repo
            .get_rate_limit_usage_fixed_window(&usage_scope, 1.0, window_start, 60)
            .await
            .unwrap();
        assert_eq!(empty_usage.usage, 0.0);
        let usage = repo
            .add_rate_limit_usage_fixed_window(&usage_scope, 0.4, 1.0, window_start, 60)
            .await
            .unwrap();
        assert_eq!(usage.usage, 0.4);
        let usage = repo
            .add_rate_limit_usage_fixed_window(&usage_scope, 0.7, 1.0, window_start, 60)
            .await
            .unwrap();
        assert!((usage.usage - 1.1).abs() < 0.000_000_1);

        sqlx::query("DELETE FROM idempotent_jobs WHERE job_type = $1")
            .bind(&job_type)
            .execute(repo.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM rate_limit_counters WHERE scope = $1")
            .bind(format!("{namespace}:rate"))
            .execute(repo.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM rate_limit_counters WHERE scope = $1")
            .bind(usage_scope)
            .execute(repo.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM account_concurrency_slots WHERE account_id = $1")
            .bind(account_id)
            .execute(repo.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stores_and_filters_payment_orders() {
        let repo = InMemoryRepository::new();
        repo.upsert_payment_order(payment_order(7, "PENDING"))
            .await
            .unwrap();
        repo.upsert_payment_order(PaymentOrderRecord {
            id: 8,
            payment_type: "stripe".to_owned(),
            provider_instance_id: Some("stripe-default".to_owned()),
            ..payment_order(8, "COMPLETED")
        })
        .await
        .unwrap();

        let by_trade_no = repo
            .get_payment_order_by_trade_no("sub2_2026060600000007")
            .await
            .unwrap();
        assert_eq!(by_trade_no.status, "PENDING");

        let completed = repo
            .list_payment_orders(PaymentOrderFilter {
                status: Some("completed".to_owned()),
                provider_instance_id: Some("stripe-default".to_owned()),
                ..PaymentOrderFilter::all()
            })
            .await
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].id, 8);
    }

    #[tokio::test]
    async fn records_payment_audit_logs_by_order() {
        let repo = InMemoryRepository::new();
        repo.insert_payment_audit(payment_audit("sub2_1", "ORDER_PAID"))
            .await
            .unwrap();
        repo.insert_payment_audit(payment_audit("sub2_1", "ORDER_COMPLETED"))
            .await
            .unwrap();
        repo.insert_payment_audit(payment_audit("sub2_2", "ORDER_PAID"))
            .await
            .unwrap();

        let records = repo.list_payment_audits("sub2_1").await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, 1);
        assert_eq!(records[1].action, "ORDER_COMPLETED");
    }

    #[tokio::test]
    async fn applies_balance_transactions_idempotently() {
        let repo = InMemoryRepository::new();
        let first = repo
            .apply_balance_transaction(balance_transaction("sub2_1", 12.5))
            .await
            .unwrap();
        let duplicate = repo
            .apply_balance_transaction(balance_transaction("sub2_1", 12.5))
            .await
            .unwrap();

        assert_eq!(first.id, duplicate.id);
        assert_eq!(repo.get_user_balance(1).await.unwrap().balance, 12.5);
        assert_eq!(repo.list_balance_transactions(1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn applies_balance_refund_transactions_idempotently() {
        let repo = InMemoryRepository::new();
        repo.apply_balance_transaction(balance_transaction("sub2_1", 12.5))
            .await
            .unwrap();
        let refund = BalanceTransactionRecord {
            transaction_type: "payment_refund".to_owned(),
            amount: -5.0,
            ..balance_transaction("sub2_1", -5.0)
        };
        let first = repo
            .apply_balance_transaction(refund.clone())
            .await
            .unwrap();
        let duplicate = repo.apply_balance_transaction(refund).await.unwrap();

        assert_eq!(first.id, duplicate.id);
        assert_eq!(repo.get_user_balance(1).await.unwrap().balance, 7.5);
        assert_eq!(repo.list_balance_transactions(1).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn stores_subscriptions_by_source_order_idempotently() {
        let repo = InMemoryRepository::new();
        let first = repo
            .upsert_user_subscription(subscription("sub2_1"))
            .await
            .unwrap();
        let duplicate = repo
            .upsert_user_subscription(UserSubscriptionRecord {
                group_id: 9,
                ..subscription("sub2_1")
            })
            .await
            .unwrap();

        assert_eq!(first.id, duplicate.id);
        assert_eq!(
            repo.get_subscription_by_source_order("sub2_1")
                .await
                .unwrap()
                .group_id,
            7
        );
        assert_eq!(
            repo.get_subscription_by_source_order("sub2_1")
                .await
                .unwrap()
                .monthly_usage_usd,
            0.3
        );
        let mut updated = repo.get_user_subscription(first.id).await.unwrap();
        updated.status = "expired".to_owned();
        updated.daily_usage_usd = 0.0;
        repo.update_user_subscription(updated).await.unwrap();
        let all = repo.list_subscriptions().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, "expired");
        assert_eq!(all[0].daily_usage_usd, 0.0);
        assert_eq!(repo.list_user_subscriptions(1).await.unwrap().len(), 1);
        repo.delete_user_subscription(first.id).await.unwrap();
        assert!(repo.get_user_subscription(first.id).await.is_err());
        assert!(repo.list_subscriptions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stores_user_platform_quotas_and_accumulates_usage() {
        let repo = InMemoryRepository::new();
        let quotas = repo
            .replace_user_platform_quotas(
                1,
                vec![
                    platform_quota("openai", Some(1.5)),
                    platform_quota("anthropic", None),
                ],
            )
            .await
            .unwrap();
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[1].platform, "openai");
        assert_eq!(quotas[1].daily_limit_usd, Some(1.5));

        repo.increment_user_platform_quota_usage(
            1,
            "openai",
            0.25,
            "2026-06-06T00:00:00Z".to_owned(),
            "2026-06-01T00:00:00Z".to_owned(),
            "2026-06-06T00:00:00Z".to_owned(),
        )
        .await
        .unwrap();
        let accumulated = repo
            .increment_user_platform_quota_usage(
                1,
                "openai",
                0.5,
                "2026-06-06T00:00:00Z".to_owned(),
                "2026-06-01T00:00:00Z".to_owned(),
                "2026-06-07T00:00:00Z".to_owned(),
            )
            .await
            .unwrap();

        assert_eq!(accumulated.daily_usage_usd, 0.75);
        assert_eq!(accumulated.weekly_usage_usd, 0.75);
        assert_eq!(accumulated.monthly_usage_usd, 0.75);
        assert_eq!(
            accumulated.monthly_window_start.as_deref(),
            Some("2026-06-06T00:00:00Z")
        );

        let reset_month = repo
            .increment_user_platform_quota_usage(
                1,
                "openai",
                0.2,
                "2026-07-06T00:00:00Z".to_owned(),
                "2026-07-06T00:00:00Z".to_owned(),
                "2026-07-06T00:00:00Z".to_owned(),
            )
            .await
            .unwrap();
        assert_eq!(reset_month.monthly_usage_usd, 0.2);
        assert_eq!(
            reset_month.monthly_window_start.as_deref(),
            Some("2026-07-06T00:00:00Z")
        );
        assert!(repo
            .replace_user_platform_quotas(1, vec![platform_quota("bad", Some(1.0))])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_user_group_rate_and_rpm_overrides_independently() {
        let repo = InMemoryRepository::new();
        repo.replace_group_rate_multipliers(
            10,
            vec![
                UserGroupRateRecord {
                    user_id: 1,
                    group_id: 10,
                    rate_multiplier: Some(0.5),
                    rpm_override: None,
                    updated_at: String::new(),
                },
                UserGroupRateRecord {
                    user_id: 2,
                    group_id: 10,
                    rate_multiplier: Some(0.8),
                    rpm_override: None,
                    updated_at: String::new(),
                },
            ],
        )
        .await
        .unwrap();
        repo.replace_group_rpm_overrides(
            10,
            vec![UserGroupRateRecord {
                user_id: 1,
                group_id: 10,
                rate_multiplier: None,
                rpm_override: Some(120),
                updated_at: String::new(),
            }],
        )
        .await
        .unwrap();

        let user_rates = repo.list_user_group_rates(1).await.unwrap();
        assert_eq!(user_rates[0].rate_multiplier, Some(0.5));
        let group_overrides = repo.list_group_rate_overrides(10).await.unwrap();
        assert_eq!(group_overrides.len(), 2);
        assert_eq!(group_overrides[0].rate_multiplier, Some(0.5));
        assert_eq!(group_overrides[0].rpm_override, Some(120));
        assert_eq!(group_overrides[1].rate_multiplier, Some(0.8));

        repo.clear_group_rate_multipliers(10).await.unwrap();
        let group_overrides = repo.list_group_rate_overrides(10).await.unwrap();
        assert_eq!(group_overrides.len(), 1);
        assert_eq!(group_overrides[0].rate_multiplier, None);
        assert_eq!(group_overrides[0].rpm_override, Some(120));
        assert!(repo.list_user_group_rates(1).await.unwrap().is_empty());

        repo.clear_group_rpm_overrides(10).await.unwrap();
        assert!(repo.list_group_rate_overrides(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stores_user_attribute_values_by_user_and_replaces_atomically() {
        let repo = InMemoryRepository::new();
        let initial = repo
            .replace_user_attribute_values(
                3,
                vec![
                    UserAttributeValueRecord {
                        user_id: 0,
                        attribute_id: 8,
                        value: " cn ".to_owned(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                    UserAttributeValueRecord {
                        user_id: 0,
                        attribute_id: 7,
                        value: "enterprise".to_owned(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(initial.len(), 2);
        assert_eq!(initial[0].attribute_id, 7);
        assert_eq!(initial[1].value, "cn");

        let replaced = repo
            .replace_user_attribute_values(
                3,
                vec![
                    UserAttributeValueRecord {
                        user_id: 99,
                        attribute_id: 7,
                        value: "".to_owned(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                    UserAttributeValueRecord {
                        user_id: 99,
                        attribute_id: 9,
                        value: "beta".to_owned(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].user_id, 3);
        assert_eq!(replaced[0].attribute_id, 9);
        assert_eq!(replaced[0].value, "beta");
        assert!(repo.list_user_attribute_values(4).await.unwrap().is_empty());

        assert!(repo
            .replace_user_attribute_values(
                3,
                vec![UserAttributeValueRecord {
                    user_id: 3,
                    attribute_id: 0,
                    value: "bad".to_owned(),
                    created_at: String::new(),
                    updated_at: String::new(),
                }],
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stores_content_moderation_logs_with_filters_and_pagination() {
        let repo = InMemoryRepository::new();
        repo.insert_content_moderation_log(moderation_log(1, "req-allow", "allow", false))
            .await
            .unwrap();
        repo.insert_content_moderation_log(moderation_log(2, "req-block", "keyword_block", true))
            .await
            .unwrap();
        repo.insert_content_moderation_log(ContentModerationLogRecord {
            id: 3,
            request_id: "req-error".to_owned(),
            error: "upstream failed".to_owned(),
            group_id: Some(3),
            created_at: "2026-06-03T00:00:00Z".to_owned(),
            ..moderation_log(3, "req-error", "allow", false)
        })
        .await
        .unwrap();

        let blocked = repo
            .list_content_moderation_logs(
                ContentModerationLogFilter {
                    result: Some("blocked".to_owned()),
                    ..ContentModerationLogFilter::all()
                },
                Pagination::new(1, 20),
            )
            .await
            .unwrap();
        assert_eq!(blocked.total, 1);
        assert_eq!(blocked.items[0].request_id, "req-block");

        let searched = repo
            .list_content_moderation_logs(
                ContentModerationLogFilter {
                    search: Some("error".to_owned()),
                    group_id: Some(3),
                    ..ContentModerationLogFilter::all()
                },
                Pagination::new(1, 20),
            )
            .await
            .unwrap();
        assert_eq!(searched.total, 1);
        assert_eq!(searched.items[0].error, "upstream failed");

        let first_page = repo
            .list_content_moderation_logs(ContentModerationLogFilter::all(), Pagination::new(1, 2))
            .await
            .unwrap();
        assert_eq!(first_page.total, 3);
        assert_eq!(
            first_page
                .items
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            vec![3, 2]
        );
    }

    #[tokio::test]
    async fn updates_subscription_status_by_source_order() {
        let repo = InMemoryRepository::new();
        repo.upsert_user_subscription(subscription("sub2_1"))
            .await
            .unwrap();
        let refunded = repo
            .update_subscription_status_by_source_order(
                "sub2_1",
                "refunded",
                serde_json::json!({ "refund_amount": 9.9 }),
            )
            .await
            .unwrap();

        assert_eq!(refunded.status, "refunded");
        assert_eq!(refunded.metadata["refund_amount"], 9.9);
        assert_eq!(
            repo.get_subscription_by_source_order("sub2_1")
                .await
                .unwrap()
                .status,
            "refunded"
        );
    }

    #[tokio::test]
    async fn stores_payment_provider_instances_ordered_and_normalized() {
        let repo = InMemoryRepository::new();
        repo.upsert_payment_provider_instance(payment_provider_instance(
            20,
            "Stripe",
            vec!["Card", "card", "link"],
            20,
        ))
        .await
        .unwrap();
        repo.upsert_payment_provider_instance(payment_provider_instance(
            10,
            "EasyPay",
            vec!["Alipay"],
            10,
        ))
        .await
        .unwrap();

        let records = repo.list_payment_provider_instances().await.unwrap();
        assert_eq!(records[0].id, 10);
        assert_eq!(records[0].provider_key, "easypay");
        assert_eq!(records[0].supported_types, vec!["alipay"]);
        assert_eq!(records[1].supported_types, vec!["card", "link"]);

        let stripe = repo.get_payment_provider_instance(20).await.unwrap();
        assert_eq!(stripe.config["webhookSecret"], "secret");

        repo.delete_payment_provider_instance(10).await.unwrap();
        assert_eq!(
            repo.list_payment_provider_instances().await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn stores_payment_plans_ordered_filtered_and_validated() {
        let repo = InMemoryRepository::new();
        let hidden = repo
            .upsert_payment_plan(payment_plan(20, "Hidden", 20, false))
            .await
            .unwrap();
        let starter = repo
            .upsert_payment_plan(payment_plan(10, "Starter", 10, true))
            .await
            .unwrap();

        assert_eq!(starter.features, serde_json::json!(["fast", "stable"]));
        assert_eq!(repo.get_payment_plan(10).await.unwrap().name, "Starter");
        let all = repo.list_payment_plans().await.unwrap();
        assert_eq!(
            all.iter().map(|plan| plan.id).collect::<Vec<_>>(),
            vec![10, 20]
        );
        let for_sale = repo.list_payment_plans_for_sale().await.unwrap();
        assert_eq!(for_sale.len(), 1);
        assert_eq!(for_sale[0].id, starter.id);

        assert!(repo
            .upsert_payment_plan(PaymentPlanRecord {
                price: 0.0,
                ..payment_plan(30, "Bad", 30, true)
            })
            .await
            .is_err());
        repo.delete_payment_plan(hidden.id).await.unwrap();
        assert!(repo.get_payment_plan(hidden.id).await.is_err());
    }

    #[test]
    fn postgres_enum_mappers_round_trip_domain_values() {
        assert_eq!(
            parse_api_key_status(api_key_status_to_str(ApiKeyStatus::QuotaExhausted)).unwrap(),
            ApiKeyStatus::QuotaExhausted
        );
        assert_eq!(
            parse_group_status(group_status_to_str(GroupStatus::Deleted)).unwrap(),
            GroupStatus::Deleted
        );
        assert_eq!(
            parse_provider(provider_to_str(Provider::Antigravity)).unwrap(),
            Provider::Antigravity
        );
        assert_eq!(
            parse_upstream_protocol(upstream_protocol_to_str(
                UpstreamProtocol::GeminiGenerateContent
            ))
            .unwrap(),
            UpstreamProtocol::GeminiGenerateContent
        );
    }

    #[test]
    fn core_gateway_migration_contains_persistent_protocol_fields() {
        let migration = core_gateway_migration_sql();
        for expected in [
            "CREATE TABLE IF NOT EXISTS users",
            "CREATE TABLE IF NOT EXISTS oauth_identities",
            "CREATE TABLE IF NOT EXISTS auth_sessions",
            "CREATE TABLE IF NOT EXISTS auth_credentials",
            "CREATE TABLE IF NOT EXISTS groups",
            "CREATE TABLE IF NOT EXISTS api_keys",
            "CREATE TABLE IF NOT EXISTS accounts",
            "CREATE TABLE IF NOT EXISTS account_groups",
            "CREATE TABLE IF NOT EXISTS usage_logs",
            "CREATE TABLE IF NOT EXISTS payment_orders",
            "CREATE TABLE IF NOT EXISTS payment_audit_logs",
            "CREATE TABLE IF NOT EXISTS user_balances",
            "CREATE TABLE IF NOT EXISTS balance_transactions",
            "CREATE TABLE IF NOT EXISTS user_subscriptions",
            "CREATE TABLE IF NOT EXISTS payment_provider_instances",
            "CREATE TABLE IF NOT EXISTS system_settings",
            "CREATE TABLE IF NOT EXISTS email_queue_tasks",
            "CREATE TABLE IF NOT EXISTS distributed_locks",
            "CREATE TABLE IF NOT EXISTS idempotent_jobs",
            "CREATE TABLE IF NOT EXISTS account_concurrency_slots",
            "CREATE TABLE IF NOT EXISTS rate_limit_counters",
            "CREATE TABLE IF NOT EXISTS payment_plans",
            "supported_downstream_protocols TEXT[]",
            "upstream_protocol_override TEXT",
            "downstream_protocol TEXT NOT NULL",
            "requested_model TEXT NOT NULL",
            "upstream_model TEXT NOT NULL",
            "out_trade_no TEXT NOT NULL UNIQUE",
            "idx_backend_next_oauth_identities_external",
            "idx_backend_next_oauth_identities_user_id",
            "idx_backend_next_auth_sessions_user_id",
            "idx_backend_next_auth_sessions_access_token",
            "idx_backend_next_auth_sessions_refresh_token",
            "idx_backend_next_auth_credentials_email",
            "idx_backend_next_usage_api_key_id",
            "idx_backend_next_payment_orders_user_id",
            "idx_backend_next_payment_audit_logs_order_id",
            "idx_backend_next_balance_transactions_user_id",
            "idx_backend_next_user_subscriptions_user_id",
            "idx_backend_next_payment_provider_instances_provider_key",
            "idx_backend_next_system_settings_namespace",
            "idx_backend_next_email_queue_tasks_status_id",
            "idx_backend_next_distributed_locks_expires_at",
            "idx_backend_next_idempotent_jobs_claim",
            "idx_backend_next_account_concurrency_slots_expires_at",
            "idx_backend_next_rate_limit_counters_expires_at",
            "idx_backend_next_payment_plans_sort_order",
        ] {
            assert!(
                migration.contains(expected),
                "migration should contain {expected}"
            );
        }
    }

    fn accepts_app_repository_trait_object(_repository: &dyn AppRepository) {}

    #[test]
    fn in_memory_repository_can_be_used_as_app_repository() {
        let repository = InMemoryRepository::new();

        accepts_app_repository_trait_object(&repository);
    }

    #[test]
    fn postgres_repository_exposes_idempotent_migration_statements() {
        let statements = core_gateway_migration_sql()
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
            .collect::<Vec<_>>();

        assert!(statements.len() >= 10);
        assert!(statements
            .iter()
            .all(|statement| statement.contains("IF NOT EXISTS")));
    }
}
