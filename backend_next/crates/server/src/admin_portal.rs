use domain::{
    Account, AccountGroupBinding, AccountId, Group, GroupId, GroupStatus, ModelMappingRule,
    Provider, UpstreamProtocol,
};
use protocol::DownstreamProtocol;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    RwLock,
};
use uuid::Uuid;

use crate::channel_monitor_probe::ChannelMonitorConfig;
use crate::response::ApiError;
use crate::smtp_probe::SmtpConfig;

const NOW: &str = "2026-06-06T00:00:00Z";

#[derive(Default)]
pub struct AdminPortalService {
    next_id: AtomicI64,
    collections: RwLock<HashMap<String, Vec<Value>>>,
    settings: RwLock<Value>,
    payment_config: RwLock<Value>,
    backup_s3_config: RwLock<Value>,
    backup_schedule: RwLock<Value>,
    backup_records: RwLock<Vec<Value>>,
    data_management_config: RwLock<Value>,
    data_management_source_profiles: RwLock<HashMap<String, Vec<Value>>>,
    data_management_s3_profiles: RwLock<Vec<Value>>,
    data_management_jobs: RwLock<Vec<Value>>,
    risk_control_config: RwLock<Value>,
    flagged_hashes: RwLock<Vec<String>>,
    admin_api_key: RwLock<Option<String>>,
    email_templates: RwLock<HashMap<(String, String), Value>>,
    smtp_attempts: RwLock<Vec<Value>>,
    oauth_sessions: RwLock<HashMap<String, Value>>,
    system_restart_requests: RwLock<Vec<Value>>,
    user_attribute_order: RwLock<Vec<i64>>,
    user_attributes: RwLock<HashMap<i64, HashMap<i64, String>>>,
    group_rate_multipliers: RwLock<HashMap<i64, Vec<Value>>>,
    group_rpm_overrides: RwLock<HashMap<i64, Vec<Value>>>,
    web_search_usage_resets: RwLock<Vec<Value>>,
    dashboard_backfills: RwLock<Vec<Value>>,
    usage_logs: RwLock<Vec<Value>>,
    usage_cleanup_tasks: RwLock<Vec<Value>>,
    promo_code_usages: RwLock<Vec<Value>>,
    affiliate_profiles: RwLock<HashMap<i64, Value>>,
    affiliate_invites: RwLock<Vec<Value>>,
    affiliate_rebates: RwLock<Vec<Value>>,
    affiliate_transfers: RwLock<Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelModelMapping {
    pub channel_id: i64,
    pub requested_model: String,
    pub mapped_model: String,
    pub matched: bool,
    pub matched_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayUsageRecord {
    pub user_id: i64,
    pub api_key_id: i64,
    pub api_key_name: String,
    pub account_id: i64,
    pub group_id: Option<i64>,
    pub provider: String,
    pub downstream_protocol: String,
    pub upstream_protocol: String,
    pub endpoint: String,
    pub request_type: String,
    pub requested_model: String,
    pub upstream_model: String,
    pub model_mapping_chain: Vec<String>,
    pub stream: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub status: String,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GatewayUsageCost {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_creation_cost: f64,
    pub cache_read_cost: f64,
    pub total_cost: f64,
    pub actual_cost: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ModelPricing {
    input_price: f64,
    output_price: f64,
    cache_write_price: f64,
    cache_read_price: f64,
}

impl ChannelModelMapping {
    fn unmapped(requested_model: &str) -> Self {
        Self {
            channel_id: 0,
            requested_model: requested_model.to_owned(),
            mapped_model: requested_model.to_owned(),
            matched: false,
            matched_source: None,
        }
    }
}

impl AdminPortalService {
    pub fn new() -> Self {
        let service = Self {
            next_id: AtomicI64::new(100),
            collections: RwLock::new(seed_collections()),
            settings: RwLock::new(default_settings()),
            payment_config: RwLock::new(default_payment_config()),
            backup_s3_config: RwLock::new(default_backup_s3_config()),
            backup_schedule: RwLock::new(default_backup_schedule()),
            backup_records: RwLock::new(Vec::new()),
            data_management_config: RwLock::new(default_data_management_config()),
            data_management_source_profiles: RwLock::new(HashMap::new()),
            data_management_s3_profiles: RwLock::new(Vec::new()),
            data_management_jobs: RwLock::new(Vec::new()),
            risk_control_config: RwLock::new(default_risk_control_config()),
            flagged_hashes: RwLock::new(Vec::new()),
            admin_api_key: RwLock::new(None),
            email_templates: RwLock::new(default_email_templates()),
            smtp_attempts: RwLock::new(Vec::new()),
            oauth_sessions: RwLock::new(HashMap::new()),
            system_restart_requests: RwLock::new(Vec::new()),
            user_attribute_order: RwLock::new(Vec::new()),
            user_attributes: RwLock::new(HashMap::new()),
            group_rate_multipliers: RwLock::new(HashMap::new()),
            group_rpm_overrides: RwLock::new(HashMap::new()),
            web_search_usage_resets: RwLock::new(Vec::new()),
            dashboard_backfills: RwLock::new(Vec::new()),
            usage_logs: RwLock::new(seed_usage_logs()),
            usage_cleanup_tasks: RwLock::new(Vec::new()),
            promo_code_usages: RwLock::new(seed_promo_code_usages()),
            affiliate_profiles: RwLock::new(seed_affiliate_profiles()),
            affiliate_invites: RwLock::new(seed_affiliate_invites()),
            affiliate_rebates: RwLock::new(seed_affiliate_rebates()),
            affiliate_transfers: RwLock::new(seed_affiliate_transfers()),
        };
        service
    }

    pub fn list_collection(&self, name: &str, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let status_filter = query.get("status").map(|value| value.to_lowercase());
        let platform_filter = query.get("platform").map(|value| value.to_lowercase());
        let search = query.get("search").map(|value| value.to_lowercase());

        let collection_name = normalize_collection_name(name);
        let collections = self.collections.read().expect("admin collection lock");
        let items = collections
            .get(&collection_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                if let Some(status) = &status_filter {
                    if item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(|value| value.to_lowercase() != *status)
                        .unwrap_or(false)
                    {
                        return false;
                    }
                }
                if let Some(platform) = &platform_filter {
                    if item
                        .get("platform")
                        .and_then(Value::as_str)
                        .map(|value| value.to_lowercase() != *platform)
                        .unwrap_or(false)
                    {
                        return false;
                    }
                }
                if let Some(search) = &search {
                    let text = item.to_string().to_lowercase();
                    if !text.contains(search) {
                        return false;
                    }
                }
                true
            })
            .collect::<Vec<_>>();

        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        let page_items = items
            .into_iter()
            .skip(start)
            .take(page_size as usize)
            .collect::<Vec<_>>();
        paginated(page_items, total, page, page_size)
    }

    pub fn list_all(&self, name: &str, query: &HashMap<String, String>) -> Value {
        let listed = self.list_collection(name, query);
        listed.get("items").cloned().unwrap_or_else(|| json!([]))
    }

    pub fn get_collection_item(&self, name: &str, id: i64) -> Result<Value, ApiError> {
        self.find_item(name, id)
            .ok_or_else(|| ApiError::not_found(format!("{name} item not found")))
    }

    pub fn create_collection_item(&self, name: &str, payload: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let collection_name = normalize_collection_name(name);
        let mut item = default_item_for(&collection_name, id);
        merge_json(&mut item, payload);
        set_id(&mut item, id);

        self.collections
            .write()
            .expect("admin collection lock")
            .entry(collection_name)
            .or_default()
            .push(item.clone());
        item
    }

    pub fn update_collection_item(
        &self,
        name: &str,
        id: i64,
        payload: Value,
    ) -> Result<Value, ApiError> {
        let collection_name = normalize_collection_name(name);
        let mut collections = self.collections.write().expect("admin collection lock");
        let items = collections.entry(collection_name.clone()).or_default();
        if let Some(item) = items.iter_mut().find(|item| item_id(item) == Some(id)) {
            merge_json(item, payload);
            set_id(item, id);
            ensure_timestamp(item);
            return Ok(item.clone());
        }

        let mut item = default_item_for(&collection_name, id);
        merge_json(&mut item, payload);
        set_id(&mut item, id);
        items.push(item.clone());
        Ok(item)
    }

    pub fn delete_collection_item(&self, name: &str, id: i64) -> Value {
        let collection_name = normalize_collection_name(name);
        if let Some(items) = self
            .collections
            .write()
            .expect("admin collection lock")
            .get_mut(&collection_name)
        {
            items.retain(|item| item_id(item) != Some(id));
        }
        json!({ "message": "deleted" })
    }

    pub fn batch_delete_collection_items(
        &self,
        name: &str,
        payload: Value,
        message: &str,
    ) -> Result<Value, ApiError> {
        let ids = payload_ids(&payload, "ids")?;
        let collection_name = normalize_collection_name(name);
        let mut collections = self.collections.write().expect("admin collection lock");
        let items = collections.entry(collection_name).or_default();
        let before = items.len();
        items.retain(|item| item_id(item).map(|id| !ids.contains(&id)).unwrap_or(true));
        let deleted = (before - items.len()) as i64;
        Ok(json!({
            "deleted": deleted,
            "message": message
        }))
    }

    pub fn batch_update_collection_items(
        &self,
        name: &str,
        payload: Value,
        message: &str,
    ) -> Result<Value, ApiError> {
        let ids = payload_ids(&payload, "ids")?;
        let fields = payload
            .get("fields")
            .cloned()
            .unwrap_or_else(|| payload_without_ids(&payload));
        let collection_name = normalize_collection_name(name);
        let mut collections = self.collections.write().expect("admin collection lock");
        let items = collections.entry(collection_name).or_default();
        let mut updated = 0;
        for item in items
            .iter_mut()
            .filter(|item| item_id(item).map(|id| ids.contains(&id)).unwrap_or(false))
        {
            merge_json(item, fields.clone());
            ensure_timestamp(item);
            updated += 1;
        }
        Ok(json!({
            "updated": updated,
            "message": message
        }))
    }

    pub fn batch_update_user_concurrency(&self, payload: Value) -> Result<Value, ApiError> {
        let all = payload.get("all").and_then(Value::as_bool).unwrap_or(false);
        let mode = payload
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("set")
            .trim();
        if mode != "set" && mode != "add" {
            return Err(ApiError::bad_request("mode must be set or add"));
        }
        let concurrency = payload
            .get("concurrency")
            .and_then(Value::as_i64)
            .ok_or_else(|| ApiError::bad_request("concurrency is required"))?;
        let ids = if all {
            self.collections
                .read()
                .expect("admin collection lock")
                .get("users")
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(item_id)
                .collect::<Vec<_>>()
        } else {
            payload_ids(&payload, "user_ids")?
        };
        let mut collections = self.collections.write().expect("admin collection lock");
        let users = collections.entry("users".to_owned()).or_default();
        let mut affected = 0;
        for user in users
            .iter_mut()
            .filter(|user| item_id(user).map(|id| ids.contains(&id)).unwrap_or(false))
        {
            let current = user
                .get("concurrency")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            user["concurrency"] = json!(if mode == "add" {
                (current + concurrency).max(0)
            } else {
                concurrency.max(0)
            });
            ensure_timestamp(user);
            affected += 1;
        }
        Ok(json!({ "affected": affected }))
    }

    pub fn dashboard_stats(&self) -> Value {
        let collections = self.collections.read().expect("admin collection lock");
        json!({
            "total_users": collection_len(&collections, "users"),
            "active_users": collection_len(&collections, "users"),
            "total_api_keys": 0,
            "active_api_keys": 0,
            "total_accounts": collection_len(&collections, "accounts"),
            "active_accounts": collection_len(&collections, "accounts"),
            "total_groups": collection_len(&collections, "groups"),
            "total_requests": 0,
            "total_tokens": 0,
            "total_cost": 0.0,
            "today_requests": 0,
            "today_tokens": 0,
            "today_cost": 0.0,
            "uptime": 0
        })
    }

    pub fn dashboard_realtime(&self) -> Value {
        json!({
            "active_requests": 0,
            "requests_per_minute": 0,
            "average_response_time": 0,
            "error_rate": 0
        })
    }

    pub fn dashboard_trend(&self) -> Value {
        json!({
            "trend": [],
            "start_date": "2026-06-06",
            "end_date": "2026-06-06",
            "granularity": "day"
        })
    }

    pub fn dashboard_models(&self) -> Value {
        json!({
            "models": [],
            "start_date": "2026-06-06",
            "end_date": "2026-06-06"
        })
    }

    pub fn dashboard_groups(&self) -> Value {
        json!({
            "groups": [],
            "start_date": "2026-06-06",
            "end_date": "2026-06-06"
        })
    }

    pub fn dashboard_snapshot_v2(&self) -> Value {
        json!({
            "generated_at": NOW,
            "start_date": "2026-06-06",
            "end_date": "2026-06-06",
            "granularity": "day",
            "stats": self.dashboard_stats(),
            "trend": [],
            "models": [],
            "groups": [],
            "users_trend": []
        })
    }

    pub fn dashboard_empty_list(&self, key: &str) -> Value {
        json!({
            key: [],
            "start_date": "2026-06-06",
            "end_date": "2026-06-06",
            "granularity": "day"
        })
    }

    pub fn dashboard_backfill(&self, payload: Value) -> Result<Value, ApiError> {
        let start = payload
            .get("start")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("start is required"))?;
        let end = payload
            .get("end")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("end is required"))?;
        let job = json!({
            "status": "accepted",
            "start": start,
            "end": end,
            "created_at": NOW
        });
        self.dashboard_backfills
            .write()
            .expect("dashboard backfills lock")
            .push(job.clone());
        Ok(job)
    }

    pub fn group_stats(&self, id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("groups", id)?;
        Ok(json!({
            "total_api_keys": 0,
            "active_api_keys": 0,
            "total_requests": 0,
            "total_cost": 0.0
        }))
    }

    pub fn group_models_candidates(&self) -> Value {
        json!({
            "models": ["gpt-5.4", "gpt-5.5", "claude-sonnet-4-5", "deepseek-chat"]
        })
    }

    pub fn group_usage_summary(&self) -> Value {
        Value::Array(
            self.collections
                .read()
                .expect("admin collection lock")
                .get("groups")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|group| {
                    json!({
                        "group_id": group["id"].clone(),
                        "today_cost": 0.0,
                        "total_cost": 0.0
                    })
                })
                .collect::<Vec<_>>(),
        )
    }

    pub fn group_capacity_summary(&self) -> Value {
        Value::Array(
            self.collections
                .read()
                .expect("admin collection lock")
                .get("groups")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|group| {
                    json!({
                        "group_id": group["id"].clone(),
                        "concurrency_used": 0,
                        "concurrency_max": 0,
                        "sessions_used": 0,
                        "sessions_max": 0,
                        "rpm_used": 0,
                        "rpm_max": 0
                    })
                })
                .collect::<Vec<_>>(),
        )
    }

    pub fn group_user_overrides(&self) -> Value {
        json!([])
    }

    pub fn group_rate_multipliers(&self, group_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("groups", group_id)?;
        Ok(Value::Array(
            self.group_rate_multipliers
                .read()
                .expect("group rate multipliers lock")
                .get(&group_id)
                .cloned()
                .unwrap_or_default(),
        ))
    }

    pub fn set_group_rate_multipliers(
        &self,
        group_id: i64,
        payload: Value,
    ) -> Result<Value, ApiError> {
        self.get_collection_item("groups", group_id)?;
        let entries = payload
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ApiError::bad_request("entries is required"))?;
        self.group_rate_multipliers
            .write()
            .expect("group rate multipliers lock")
            .insert(group_id, entries);
        Ok(json!({ "message": "Rate multipliers updated successfully" }))
    }

    pub fn clear_group_rate_multipliers(&self, group_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("groups", group_id)?;
        self.group_rate_multipliers
            .write()
            .expect("group rate multipliers lock")
            .remove(&group_id);
        Ok(json!({ "message": "Rate multipliers cleared successfully" }))
    }

    pub fn set_group_rpm_overrides(
        &self,
        group_id: i64,
        payload: Value,
    ) -> Result<Value, ApiError> {
        self.get_collection_item("groups", group_id)?;
        let entries = payload
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ApiError::bad_request("entries is required"))?;
        self.group_rpm_overrides
            .write()
            .expect("group rpm overrides lock")
            .insert(group_id, entries);
        Ok(json!({ "message": "RPM overrides updated successfully" }))
    }

    pub fn clear_group_rpm_overrides(&self, group_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("groups", group_id)?;
        self.group_rpm_overrides
            .write()
            .expect("group rpm overrides lock")
            .remove(&group_id);
        Ok(json!({ "message": "RPM overrides cleared successfully" }))
    }

    pub fn update_group_sort_order(&self, payload: Value) -> Result<Value, ApiError> {
        let updates = payload
            .get("updates")
            .and_then(Value::as_array)
            .ok_or_else(|| ApiError::bad_request("updates is required"))?;
        if updates.is_empty() {
            return Err(ApiError::bad_request("updates must not be empty"));
        }
        let mut changed = 0;
        let mut collections = self.collections.write().expect("admin collection lock");
        let groups = collections.entry("groups".to_owned()).or_default();
        for update in updates {
            let id = update
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ApiError::bad_request("group id is required"))?;
            let sort_order = update
                .get("sort_order")
                .and_then(Value::as_i64)
                .unwrap_or(id);
            let group = groups
                .iter_mut()
                .find(|item| item_id(item) == Some(id))
                .ok_or_else(|| ApiError::not_found(format!("group {id} not found")))?;
            group["sort_order"] = json!(sort_order);
            ensure_timestamp(group);
            changed += 1;
        }
        groups.sort_by_key(|item| {
            item.get("sort_order")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX)
        });
        Ok(json!({
            "message": "Sort order updated successfully",
            "updated": changed
        }))
    }

    pub fn user_usage_stats(&self, id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("users", id)?;
        Ok(json!({
            "total_requests": 0,
            "total_cost": 0.0,
            "total_tokens": 0
        }))
    }

    pub fn user_rpm_status(&self, id: i64) -> Result<Value, ApiError> {
        let user = self.get_collection_item("users", id)?;
        let rpm_limit = user
            .get("rpm_limit")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        Ok(json!({
            "user_id": id,
            "rpm_limit": rpm_limit,
            "rpm_used": 0,
            "remaining": if rpm_limit > 0 { rpm_limit } else { 0 },
            "reset_at": null
        }))
    }

    pub fn user_balance_history(&self, id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("users", id)?;
        let mut response = paginated(Vec::new(), 0, 1, 20);
        response["total_recharged"] = json!(0.0);
        Ok(response)
    }

    pub fn user_platform_quotas(&self, id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("users", id)?;
        Ok(json!({
            "platform_quotas": []
        }))
    }

    pub fn user_attributes(&self, id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("users", id)?;
        let values = self
            .user_attributes
            .read()
            .expect("user attributes lock")
            .get(&id)
            .cloned()
            .unwrap_or_default();
        Ok(json!({
            "items": values
                .into_iter()
                .map(|(attribute_id, value)| {
                    json!({
                        "user_id": id,
                        "attribute_id": attribute_id,
                        "value": value,
                        "updated_at": NOW
                    })
                })
                .collect::<Vec<_>>(),
            "total": self.user_attributes.read().expect("user attributes lock").get(&id).map(HashMap::len).unwrap_or(0)
        }))
    }

    pub fn user_attribute_values(&self, id: i64) -> Result<HashMap<i64, String>, ApiError> {
        self.get_collection_item("users", id)?;
        Ok(self
            .user_attributes
            .read()
            .expect("user attributes lock")
            .get(&id)
            .cloned()
            .unwrap_or_default())
    }

    pub fn update_user_attributes(&self, id: i64, payload: Value) -> Result<Value, ApiError> {
        self.get_collection_item("users", id)?;
        let values = payload
            .get("values")
            .and_then(Value::as_object)
            .ok_or_else(|| ApiError::bad_request("values is required"))?;
        let mut attrs = self.user_attributes.write().expect("user attributes lock");
        let user_values = attrs.entry(id).or_default();
        for (attribute_id, value) in values {
            let attribute_id = attribute_id
                .parse::<i64>()
                .map_err(|_| ApiError::bad_request("attribute id must be an integer"))?;
            user_values.insert(
                attribute_id,
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
        Ok(json!({ "message": "User attributes updated successfully" }))
    }

    pub fn bind_user_auth_identity(&self, user_id: i64, payload: Value) -> Result<Value, ApiError> {
        self.get_collection_item("users", user_id)?;
        Ok(json!({
            "user_id": user_id,
            "provider_type": payload.get("provider_type").cloned().unwrap_or_else(|| json!("email")),
            "provider_key": payload.get("provider_key").cloned().unwrap_or_else(|| json!("default")),
            "provider_subject": payload.get("provider_subject").cloned().unwrap_or_else(|| json!("subject")),
            "verified_at": NOW,
            "issuer": payload.get("issuer").cloned().unwrap_or(Value::Null),
            "metadata": payload.get("metadata").cloned().unwrap_or(Value::Null),
            "created_at": NOW,
            "updated_at": NOW,
            "channel": null
        }))
    }

    pub fn update_user_balance(&self, user_id: i64, payload: Value) -> Result<Value, ApiError> {
        let mut user = self.get_collection_item("users", user_id)?;
        let balance = payload
            .get("balance")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| user.get("balance").and_then(Value::as_f64).unwrap_or(0.0));
        let operation = payload
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("set");
        let current = user.get("balance").and_then(Value::as_f64).unwrap_or(0.0);
        let next = match operation {
            "add" => current + balance,
            "subtract" => (current - balance).max(0.0),
            _ => balance,
        };
        user["balance"] = json!(next);
        self.update_collection_item("users", user_id, user)
    }

    pub fn account_action(&self, id: i64, action: &str) -> Result<Value, ApiError> {
        let mut account = self.get_collection_item("accounts", id)?;
        match action {
            "test" => Ok(json!({
                "success": true,
                "message": "account test accepted",
                "latency_ms": 0
            })),
            "stats" => Ok(json!({
                "total_requests": 0,
                "total_cost": 0.0,
                "total_tokens": 0
            })),
            "usage" => Ok(paginated(Vec::new(), 0, 1, 20)),
            "today-stats" => Ok(json!({
                "requests": 0,
                "tokens": 0,
                "cost": 0.0
            })),
            "temp-unschedulable" => Ok(account_temp_unschedulable_status(id, &account)),
            "models" => Ok(json!([
                { "id": "gpt-5.4", "name": "gpt-5.4" },
                { "id": "deepseek-chat", "name": "deepseek-chat" }
            ])),
            _ => {
                account["last_action"] = json!(action);
                account["updated_at"] = json!(NOW);
                self.update_collection_item("accounts", id, account)
            }
        }
    }

    pub fn check_mixed_channel(&self) -> Value {
        json!({
            "mixed": false,
            "has_risk": false,
            "message": "",
            "conflicts": []
        })
    }

    pub fn batch_create_accounts(&self, payload: Value) -> Result<Value, ApiError> {
        let accounts = payload
            .get("accounts")
            .and_then(Value::as_array)
            .ok_or_else(|| ApiError::bad_request("accounts is required"))?;
        if accounts.is_empty() {
            return Err(ApiError::bad_request("accounts cannot be empty"));
        }
        let mut results = Vec::new();
        let mut success = 0;
        let mut failed = 0;
        for account in accounts {
            if !account.is_object() {
                failed += 1;
                results.push(json!({
                    "success": false,
                    "error": "account must be an object"
                }));
                continue;
            }
            let created = self.create_collection_item("accounts", account.clone());
            success += 1;
            results.push(json!({
                "success": true,
                "id": created.get("id").cloned().unwrap_or(Value::Null),
                "account": created
            }));
        }
        Ok(json!({
            "success": success,
            "failed": failed,
            "results": results
        }))
    }

    pub fn reset_web_search_usage(&self, payload: Value) -> Result<Value, ApiError> {
        let provider = payload
            .get("provider_type")
            .or_else(|| payload.get("provider"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("provider_type is required"))?;
        let reset = json!({
            "provider_type": provider,
            "reset_at": NOW,
            "message": "Web search usage reset successfully"
        });
        self.web_search_usage_resets
            .write()
            .expect("web search usage reset lock")
            .push(reset.clone());
        Ok(reset)
    }

    pub fn usage_list(&self, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let user_id = query
            .get("user_id")
            .and_then(|value| value.parse::<i64>().ok());
        let api_key_id = query
            .get("api_key_id")
            .and_then(|value| value.parse::<i64>().ok());
        let model = query.get("model").map(|value| value.to_lowercase());
        let items = self
            .usage_logs
            .read()
            .expect("usage logs lock")
            .iter()
            .filter(|item| {
                user_id
                    .map(|id| item.get("user_id").and_then(Value::as_i64) == Some(id))
                    .unwrap_or(true)
            })
            .filter(|item| {
                api_key_id
                    .map(|id| item.get("api_key_id").and_then(Value::as_i64) == Some(id))
                    .unwrap_or(true)
            })
            .filter(|item| {
                model
                    .as_ref()
                    .map(|needle| {
                        item.get("model")
                            .and_then(Value::as_str)
                            .map(|value| value.to_lowercase().contains(needle))
                            .unwrap_or(false)
                    })
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        paginated(
            items
                .into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect(),
            total,
            page,
            page_size,
        )
    }

    pub fn record_gateway_usage(&self, record: GatewayUsageRecord) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let total_tokens = record.input_tokens
            + record.output_tokens
            + record.cache_creation_tokens
            + record.cache_read_tokens;
        let cost = self.calculate_gateway_usage_cost(&record);
        let entry = json!({
            "id": id,
            "request_id": format!("gateway-usage-{id}"),
            "user_id": record.user_id,
            "user_email": null,
            "api_key_id": record.api_key_id,
            "api_key_name": record.api_key_name,
            "account_id": record.account_id,
            "group_id": record.group_id,
            "provider": record.provider,
            "platform": record.provider,
            "downstream_protocol": record.downstream_protocol,
            "upstream_protocol": record.upstream_protocol,
            "endpoint": record.endpoint,
            "inbound_endpoint": record.endpoint,
            "request_type": record.request_type,
            "model": record.requested_model,
            "requested_model": record.requested_model,
            "upstream_model": record.upstream_model,
            "model_mapping_chain": record.model_mapping_chain,
            "stream": record.stream,
            "input_tokens": record.input_tokens,
            "prompt_tokens": record.input_tokens,
            "output_tokens": record.output_tokens,
            "completion_tokens": record.output_tokens,
            "cache_creation_tokens": record.cache_creation_tokens,
            "cache_read_tokens": record.cache_read_tokens,
            "total_tokens": total_tokens,
            "input_cost": cost.input_cost,
            "output_cost": cost.output_cost,
            "cache_creation_cost": cost.cache_creation_cost,
            "cache_read_cost": cost.cache_read_cost,
            "total_cost": cost.total_cost,
            "cost": cost.total_cost,
            "actual_cost": cost.actual_cost,
            "status": record.status,
            "billing_mode": "token",
            "duration_ms": record.duration_ms,
            "created_at": NOW
        });
        self.usage_logs
            .write()
            .expect("usage logs lock")
            .push(entry.clone());
        entry
    }

    pub fn calculate_gateway_usage_cost(&self, record: &GatewayUsageRecord) -> GatewayUsageCost {
        let pricing = self
            .model_pricing(&record.upstream_model)
            .or_else(|| self.model_pricing(&record.requested_model))
            .unwrap_or_default();
        let input_cost = record.input_tokens as f64 * pricing.input_price;
        let output_cost = record.output_tokens as f64 * pricing.output_price;
        let cache_creation_cost = record.cache_creation_tokens as f64 * pricing.cache_write_price;
        let cache_read_cost = record.cache_read_tokens as f64 * pricing.cache_read_price;
        let total_cost = input_cost + output_cost + cache_creation_cost + cache_read_cost;
        GatewayUsageCost {
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            total_cost,
            actual_cost: total_cost,
        }
    }

    pub fn usage_stats(&self) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let total_requests = logs.len() as i64;
        let total_tokens = logs
            .iter()
            .map(|item| {
                item.get("total_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            })
            .sum::<i64>();
        let total_cost = logs
            .iter()
            .map(|item| item.get("cost").and_then(Value::as_f64).unwrap_or(0.0))
            .sum::<f64>();
        let success_requests = logs
            .iter()
            .filter(|item| item.get("status").and_then(Value::as_str) == Some("success"))
            .count() as i64;
        json!({
            "total_requests": total_requests,
            "total_tokens": total_tokens,
            "total_cost": total_cost,
            "success_requests": success_requests,
            "failed_requests": total_requests - success_requests,
            "average_latency_ms": 0
        })
    }

    pub fn api_key_usage_summary(&self, api_key_id: i64) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let items = logs
            .iter()
            .filter(|item| item.get("api_key_id").and_then(Value::as_i64) == Some(api_key_id))
            .collect::<Vec<_>>();
        let summary = usage_summary_from_items(&items);
        let daily_usage = usage_daily_from_items(&items);
        let model_stats = usage_model_stats_from_items(&items);
        json!({
            "usage": summary,
            "daily_usage": daily_usage,
            "model_stats": model_stats
        })
    }

    pub fn user_usage_list(
        &self,
        user_id: i64,
        api_key_id: Option<i64>,
        query: &HashMap<String, String>,
    ) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let model = query.get("model").map(|value| value.to_lowercase());
        let request_type = query.get("request_type").map(|value| value.to_lowercase());
        let stream = query
            .get("stream")
            .and_then(|value| value.parse::<bool>().ok());
        let items = self
            .usage_logs
            .read()
            .expect("usage logs lock")
            .iter()
            .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(user_id))
            .filter(|item| {
                api_key_id
                    .map(|id| item.get("api_key_id").and_then(Value::as_i64) == Some(id))
                    .unwrap_or(true)
            })
            .filter(|item| {
                model
                    .as_ref()
                    .map(|needle| {
                        item.get("model")
                            .and_then(Value::as_str)
                            .map(|value| value.to_lowercase().contains(needle))
                            .unwrap_or(false)
                    })
                    .unwrap_or(true)
            })
            .filter(|item| {
                request_type
                    .as_ref()
                    .map(|needle| {
                        item.get("request_type")
                            .and_then(Value::as_str)
                            .map(|value| value.to_lowercase() == *needle)
                            .unwrap_or(false)
                    })
                    .unwrap_or(true)
            })
            .filter(|item| {
                stream
                    .map(|value| item.get("stream").and_then(Value::as_bool) == Some(value))
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        paginated(
            items
                .into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect(),
            total,
            page,
            page_size,
        )
    }

    pub fn current_user_usage_stats(&self, user_id: i64, api_key_id: Option<i64>) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let items = logs
            .iter()
            .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(user_id))
            .filter(|item| {
                api_key_id
                    .map(|id| item.get("api_key_id").and_then(Value::as_i64) == Some(id))
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        let model_stats = usage_model_stats_from_items(&items);
        json!({
            "period": "today",
            "total_requests": items.len() as i64,
            "total_input_tokens": sum_i64(&items, "input_tokens"),
            "total_output_tokens": sum_i64(&items, "output_tokens"),
            "total_cache_tokens": sum_i64(&items, "cache_creation_tokens") + sum_i64(&items, "cache_read_tokens"),
            "total_tokens": sum_i64(&items, "total_tokens"),
            "total_cost": sum_f64(&items, "cost"),
            "total_actual_cost": sum_f64(&items, "actual_cost"),
            "average_duration_ms": average_i64(&items, "duration_ms"),
            "models": model_stats
        })
    }

    pub fn user_usage_dashboard_stats(&self, user_id: i64) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let items = logs
            .iter()
            .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(user_id))
            .collect::<Vec<_>>();
        let api_key_ids = items
            .iter()
            .filter_map(|item| item.get("api_key_id").and_then(Value::as_i64))
            .collect::<std::collections::HashSet<_>>();
        json!({
            "total_api_keys": api_key_ids.len() as i64,
            "active_api_keys": api_key_ids.len() as i64,
            "total_requests": items.len() as i64,
            "total_input_tokens": sum_i64(&items, "input_tokens"),
            "total_output_tokens": sum_i64(&items, "output_tokens"),
            "total_cache_creation_tokens": sum_i64(&items, "cache_creation_tokens"),
            "total_cache_read_tokens": sum_i64(&items, "cache_read_tokens"),
            "total_tokens": sum_i64(&items, "total_tokens"),
            "total_cost": sum_f64(&items, "cost"),
            "total_actual_cost": sum_f64(&items, "actual_cost"),
            "today_requests": items.len() as i64,
            "today_input_tokens": sum_i64(&items, "input_tokens"),
            "today_output_tokens": sum_i64(&items, "output_tokens"),
            "today_cache_creation_tokens": sum_i64(&items, "cache_creation_tokens"),
            "today_cache_read_tokens": sum_i64(&items, "cache_read_tokens"),
            "today_tokens": sum_i64(&items, "total_tokens"),
            "today_cost": sum_f64(&items, "cost"),
            "today_actual_cost": sum_f64(&items, "actual_cost"),
            "average_duration_ms": average_i64(&items, "duration_ms"),
            "rpm": 0,
            "tpm": 0,
            "by_platform": []
        })
    }

    pub fn user_api_keys_usage_stats(&self, user_id: i64, api_key_ids: &[i64]) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let stats = api_key_ids
            .iter()
            .map(|id| {
                let items = logs
                    .iter()
                    .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(user_id))
                    .filter(|item| item.get("api_key_id").and_then(Value::as_i64) == Some(*id))
                    .collect::<Vec<_>>();
                (
                    id.to_string(),
                    json!({
                        "api_key_id": id,
                        "today_actual_cost": sum_f64(&items, "actual_cost"),
                        "total_actual_cost": sum_f64(&items, "actual_cost"),
                        "today_requests": items.len() as i64,
                        "total_requests": items.len() as i64,
                        "today_tokens": sum_i64(&items, "total_tokens"),
                        "total_tokens": sum_i64(&items, "total_tokens")
                    }),
                )
            })
            .collect::<serde_json::Map<String, Value>>();
        json!({ "stats": stats })
    }

    pub fn user_api_key_daily_usage(&self, user_id: i64, api_key_id: i64, days: i64) -> Value {
        let logs = self.usage_logs.read().expect("usage logs lock");
        let items = logs
            .iter()
            .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(user_id))
            .filter(|item| item.get("api_key_id").and_then(Value::as_i64) == Some(api_key_id))
            .collect::<Vec<_>>();
        json!({
            "items": usage_daily_from_items(&items),
            "days": days,
            "start_date": NOW.split('T').next().unwrap_or(NOW),
            "end_date": NOW.split('T').next().unwrap_or(NOW)
        })
    }

    pub fn user_usage_by_id(&self, user_id: i64, id: i64) -> Result<Value, ApiError> {
        self.usage_logs
            .read()
            .expect("usage logs lock")
            .iter()
            .find(|item| {
                item.get("id").and_then(Value::as_i64) == Some(id)
                    && item.get("user_id").and_then(Value::as_i64) == Some(user_id)
            })
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("usage log {id} not found")))
    }

    pub fn usage_search_api_keys(&self, auth_keys: Value) -> Value {
        let items = auth_keys
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                json!({
                    "id": item.get("id").cloned().unwrap_or(Value::Null),
                    "name": item.get("name").cloned().unwrap_or(Value::Null),
                    "key_preview": item.get("key_preview").cloned().unwrap_or(Value::Null),
                    "group_id": item.get("group_id").cloned().unwrap_or(Value::Null)
                })
            })
            .collect::<Vec<_>>();
        json!(items)
    }

    pub fn usage_cleanup_tasks(&self, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 100);
        let items = self
            .usage_cleanup_tasks
            .read()
            .expect("usage cleanup tasks lock")
            .clone();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        paginated(
            items
                .into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect(),
            total,
            page,
            page_size,
        )
    }

    pub fn create_usage_cleanup_task(&self, payload: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let task = json!({
            "id": id,
            "status": "pending",
            "filters": payload,
            "created_at": NOW,
            "updated_at": NOW
        });
        self.usage_cleanup_tasks
            .write()
            .expect("usage cleanup tasks lock")
            .push(task.clone());
        task
    }

    pub fn cancel_usage_cleanup_task(&self, id: i64) -> Result<Value, ApiError> {
        let mut tasks = self
            .usage_cleanup_tasks
            .write()
            .expect("usage cleanup tasks lock");
        let task = tasks
            .iter_mut()
            .find(|task| task.get("id").and_then(Value::as_i64) == Some(id))
            .ok_or_else(|| ApiError::not_found("usage cleanup task not found"))?;
        task["status"] = json!("cancelled");
        task["updated_at"] = json!(NOW);
        Ok(task.clone())
    }

    pub fn admin_data_export(&self, name: &str) -> Value {
        let include_accounts = normalize_collection_name(name) == "accounts";
        let collections = self.collections.read().expect("admin collection lock");
        let proxies = collections
            .get("proxies")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(proxy_to_data_proxy)
            .collect::<Vec<_>>();
        let accounts = if include_accounts {
            collections
                .get("accounts")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(account_to_data_account)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        json!({
            "type": "sub2api-data",
            "version": 1,
            "exported_at": NOW,
            "proxies": proxies,
            "accounts": accounts
        })
    }

    pub fn admin_data_import(&self, payload: Value, include_accounts: bool) -> Value {
        let data = payload.get("data").cloned().unwrap_or(payload);
        let proxies = data
            .get("proxies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let accounts = if include_accounts {
            data.get("accounts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut proxy_created = 0;
        let mut proxy_reused = 0;
        let mut proxy_failed = 0;
        let mut account_created = 0;
        let mut account_failed = 0;
        let mut errors = Vec::new();
        let mut imported_proxy_ids = HashMap::new();

        for proxy in proxies {
            let proxy_key = data_proxy_key(&proxy);
            if proxy_key.trim().is_empty() {
                proxy_failed += 1;
                errors.push(json!({
                    "kind": "proxy",
                    "name": proxy.get("name").and_then(Value::as_str).unwrap_or(""),
                    "proxy_key": proxy_key,
                    "message": "proxy_key is required"
                }));
                continue;
            }

            let mut collections = self.collections.write().expect("admin collection lock");
            let proxies = collections.entry("proxies".to_owned()).or_default();
            if let Some(existing) = proxies.iter_mut().find(|item| {
                data_proxy_key(item) == proxy_key
                    || item.get("proxy_key").and_then(Value::as_str) == Some(proxy_key.as_str())
            }) {
                merge_json(existing, proxy.clone());
                existing["proxy_key"] = json!(proxy_key);
                if let Some(id) = item_id(existing) {
                    imported_proxy_ids.insert(proxy_key, id);
                }
                proxy_reused += 1;
                continue;
            }

            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let mut item = default_item_for("proxies", id);
            merge_json(&mut item, proxy.clone());
            item["proxy_key"] = json!(proxy_key.clone());
            set_id(&mut item, id);
            proxies.push(item);
            imported_proxy_ids.insert(proxy_key, id);
            proxy_created += 1;
        }

        for account in accounts {
            let name = account
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let Some(name) = name else {
                account_failed += 1;
                errors.push(json!({
                    "kind": "account",
                    "name": "",
                    "message": "name is required"
                }));
                continue;
            };

            let mut collections = self.collections.write().expect("admin collection lock");
            let accounts = collections.entry("accounts".to_owned()).or_default();
            if accounts
                .iter()
                .any(|item| item.get("name").and_then(Value::as_str) == Some(name.as_str()))
            {
                account_failed += 1;
                errors.push(json!({
                    "kind": "account",
                    "name": name,
                    "message": "account already exists"
                }));
                continue;
            }

            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let mut item = default_item_for("accounts", id);
            merge_json(&mut item, account.clone());
            if let Some(proxy_key) = account.get("proxy_key").and_then(Value::as_str) {
                if let Some(proxy_id) = imported_proxy_ids.get(proxy_key) {
                    item["proxy_id"] = json!(proxy_id);
                }
            }
            set_id(&mut item, id);
            accounts.push(item);
            account_created += 1;
        }

        json!({
            "proxy_created": proxy_created,
            "proxy_reused": proxy_reused,
            "proxy_failed": proxy_failed,
            "account_created": account_created,
            "account_failed": account_failed,
            "errors": errors
        })
    }

    pub fn redeem_stats(&self) -> Value {
        json!({
            "total": collection_len(&self.collections.read().expect("admin collection lock"), "redeem-codes"),
            "used": 0,
            "unused": 0,
            "expired": 0
        })
    }

    pub fn generate_redeem_codes(&self, payload: Value) -> Value {
        let count = payload
            .get("count")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .clamp(1, 100);
        (0..count)
            .map(|_| self.create_collection_item("redeem-codes", payload.clone()))
            .collect::<Value>()
    }

    pub fn clear_temp_unschedulable(&self, id: i64) -> Result<Value, ApiError> {
        let account = self.update_collection_item(
            "accounts",
            id,
            json!({
                "temp_unschedulable_until": null,
                "temp_unschedulable_reason": ""
            }),
        )?;
        Ok(json!({
            "account_id": id,
            "unschedulable": false,
            "reason": null,
            "expires_at": null,
            "account": account
        }))
    }

    pub fn proxy_test(&self, id: i64) -> Result<Value, ApiError> {
        let mut proxy = self.get_collection_item("proxies", id)?;
        proxy["last_tested_at"] = json!(NOW);
        proxy["last_test_success"] = json!(true);
        self.update_collection_item("proxies", id, proxy)?;
        Ok(json!({
            "id": id,
            "success": true,
            "message": "proxy test accepted",
            "latency_ms": 0,
            "tested_at": NOW
        }))
    }

    pub fn proxy_accounts(&self, proxy_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("proxies", proxy_id)?;
        let items = self
            .collections
            .read()
            .expect("admin collection lock")
            .get("accounts")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|account| {
                account
                    .get("proxy_id")
                    .and_then(Value::as_i64)
                    .map(|id| id == proxy_id)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        let total = items.len() as i64;
        Ok(paginated(items, total, 1, 20))
    }

    pub fn announcement_read_status(
        &self,
        announcement_id: i64,
        query: &HashMap<String, String>,
    ) -> Result<Value, ApiError> {
        self.get_collection_item("announcements", announcement_id)?;
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let search = query.get("search").map(|value| value.to_lowercase());
        let users = self
            .collections
            .read()
            .expect("admin collection lock")
            .get("users")
            .cloned()
            .unwrap_or_default();
        let items = users
            .into_iter()
            .filter(|user| {
                search
                    .as_ref()
                    .map(|needle| user.to_string().to_lowercase().contains(needle))
                    .unwrap_or(true)
            })
            .map(|user| {
                json!({
                    "user_id": item_id(&user).unwrap_or_default(),
                    "email": user.get("email").cloned().unwrap_or(Value::Null),
                    "username": user.get("username").cloned().unwrap_or(Value::Null),
                    "balance": user.get("balance").cloned().unwrap_or_else(|| json!(0.0)),
                    "eligible": true,
                    "read_at": null
                })
            })
            .collect::<Vec<_>>();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        Ok(paginated(
            items
                .into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect(),
            total,
            page,
            page_size,
        ))
    }

    pub fn settings(&self) -> Value {
        self.settings.read().expect("admin settings lock").clone()
    }

    pub fn update_settings(&self, payload: Value) -> Value {
        let mut settings = self.settings.write().expect("admin settings lock");
        merge_json(&mut settings, payload);
        settings.clone()
    }

    pub fn smtp_config_from_payload(&self, payload: &Value) -> Result<SmtpConfig, ApiError> {
        let host = first_non_empty_str(payload, &["smtp_host", "host"])
            .ok_or_else(|| ApiError::bad_request("SMTP host is required"))?;
        let port = payload
            .get("smtp_port")
            .or_else(|| payload.get("port"))
            .and_then(Value::as_i64)
            .unwrap_or(587);
        if !(1..=65535).contains(&port) {
            return Err(ApiError::bad_request(
                "SMTP port must be between 1 and 65535",
            ));
        }
        Ok(SmtpConfig {
            host,
            port: port as u16,
            username: first_non_empty_str(payload, &["smtp_username", "username"])
                .unwrap_or_default(),
            password: first_non_empty_str(payload, &["smtp_password", "password"])
                .unwrap_or_default(),
            from: first_non_empty_str(payload, &["smtp_from_email", "smtp_from", "from"])
                .unwrap_or_default(),
            from_name: first_non_empty_str(payload, &["smtp_from_name", "from_name"])
                .unwrap_or_default(),
            use_tls: payload
                .get("smtp_use_tls")
                .or_else(|| payload.get("use_tls"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
        })
    }

    pub fn record_smtp_test(&self, config: &SmtpConfig) -> Value {
        let attempt = json!({
            "host": config.host.clone(),
            "port": config.port,
            "username": config.username.clone(),
            "from": config.from.clone(),
            "use_tls": config.use_tls,
            "tested_at": NOW
        });
        self.smtp_attempts
            .write()
            .expect("smtp attempts lock")
            .push(attempt.clone());
        json!({
            "success": true,
            "message": "SMTP connection successful",
            "attempt": attempt
        })
    }

    pub fn test_email_request(&self, payload: &Value) -> Result<(SmtpConfig, String), ApiError> {
        let email = first_non_empty_str(&payload, &["email", "to"])
            .ok_or_else(|| ApiError::bad_request("email is required"))?;
        if !email.contains('@') {
            return Err(ApiError::bad_request("email must be valid"));
        }
        let config = self.smtp_config_from_payload(payload)?;
        Ok((config, email))
    }

    pub fn record_test_email(&self, config: &SmtpConfig, email: &str, subject: &str) -> Value {
        let attempt = json!({
            "email": email,
            "host": config.host.clone(),
            "port": config.port,
            "from": config.from.clone(),
            "subject": subject,
            "sent_at": NOW
        });
        self.smtp_attempts
            .write()
            .expect("smtp attempts lock")
            .push(attempt.clone());
        json!({
            "success": true,
            "message": "Test email sent successfully",
            "attempt": attempt
        })
    }

    pub fn admin_api_key_status(&self) -> Value {
        let key = self.admin_api_key.read().expect("admin api key lock");
        match key.as_deref() {
            Some(value) => json!({
                "exists": true,
                "masked_key": mask_admin_api_key(value)
            }),
            None => json!({
                "exists": false,
                "masked_key": null
            }),
        }
    }

    pub fn regenerate_admin_api_key(&self) -> Value {
        let key = format!("admin-{}", Uuid::new_v4().simple());
        *self.admin_api_key.write().expect("admin api key lock") = Some(key.clone());
        json!({ "key": key })
    }

    pub fn validate_admin_api_key(&self, candidate: &str) -> bool {
        let Some(stored_key) = self
            .admin_api_key
            .read()
            .expect("admin api key lock")
            .clone()
        else {
            return false;
        };
        constant_time_eq(candidate.as_bytes(), stored_key.as_bytes())
    }

    pub fn delete_admin_api_key(&self) -> Value {
        *self.admin_api_key.write().expect("admin api key lock") = None;
        json!({ "message": "Admin API key deleted" })
    }

    pub fn email_templates(&self) -> Value {
        let templates = self.email_templates.read().expect("email templates lock");
        let mut summaries = templates
            .values()
            .map(|template| {
                json!({
                    "event": template["event"].clone(),
                    "locale": template["locale"].clone(),
                    "subject": template["subject"].clone(),
                    "is_custom": template.get("is_custom").and_then(Value::as_bool).unwrap_or(false),
                    "updated_at": template.get("updated_at").cloned().unwrap_or_else(|| json!(""))
                })
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| {
            let left = format!("{}:{}", a["event"], a["locale"]);
            let right = format!("{}:{}", b["event"], b["locale"]);
            left.cmp(&right)
        });
        json!({
            "events": email_template_events(),
            "locales": ["zh-CN", "en-US"],
            "templates": summaries,
            "placeholders": email_template_placeholder_union()
        })
    }

    pub fn email_template_detail(&self, event: &str, locale: &str) -> Result<Value, ApiError> {
        let key = normalize_template_key(event, locale)?;
        let mut templates = self.email_templates.write().expect("email templates lock");
        let template = templates
            .entry(key)
            .or_insert_with(|| official_email_template(event, locale));
        Ok(template.clone())
    }

    pub fn update_email_template(
        &self,
        event: &str,
        locale: &str,
        payload: Value,
    ) -> Result<Value, ApiError> {
        let key = normalize_template_key(event, locale)?;
        let subject = payload
            .get("subject")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("subject is required"))?;
        let html = payload
            .get("html")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("html is required"))?;
        let mut template = official_email_template(event, locale);
        template["subject"] = json!(subject);
        template["html"] = json!(html);
        template["is_custom"] = json!(true);
        template["updated_at"] = json!(NOW);
        self.email_templates
            .write()
            .expect("email templates lock")
            .insert(key, template.clone());
        Ok(template)
    }

    pub fn restore_email_template(&self, event: &str, locale: &str) -> Result<Value, ApiError> {
        let key = normalize_template_key(event, locale)?;
        let template = official_email_template(event, locale);
        self.email_templates
            .write()
            .expect("email templates lock")
            .insert(key, template.clone());
        Ok(template)
    }

    pub fn preview_email_template(&self, payload: Value) -> Value {
        let variables = payload
            .get("variables")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let subject = payload
            .get("subject")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                let event = payload
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or("verify_email");
                let locale = payload
                    .get("locale")
                    .and_then(Value::as_str)
                    .unwrap_or("zh-CN");
                self.email_template_detail(event, locale)
                    .ok()
                    .and_then(|template| {
                        template
                            .get("subject")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            })
            .unwrap_or_default();
        let html = payload
            .get("html")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                let event = payload
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or("verify_email");
                let locale = payload
                    .get("locale")
                    .and_then(Value::as_str)
                    .unwrap_or("zh-CN");
                self.email_template_detail(event, locale)
                    .ok()
                    .and_then(|template| {
                        template
                            .get("html")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            })
            .unwrap_or_default();
        json!({
            "subject": render_template_text(&subject, &variables),
            "html": render_template_text(&html, &variables)
        })
    }

    pub fn payment_config(&self) -> Value {
        self.payment_config
            .read()
            .expect("admin payment config lock")
            .clone()
    }

    pub fn update_payment_config(&self, payload: Value) -> Value {
        let mut config = self
            .payment_config
            .write()
            .expect("admin payment config lock");
        merge_json(&mut config, payload);
        config.clone()
    }

    pub fn payment_dashboard(&self) -> Value {
        json!({
            "total_orders": 0,
            "paid_orders": 0,
            "pending_orders": 0,
            "cancelled_orders": 0,
            "total_amount": 0.0,
            "paid_amount": 0.0,
            "refund_amount": 0.0,
            "orders_by_day": [],
            "amount_by_day": []
        })
    }

    pub fn list_payment_providers(&self, query: &HashMap<String, String>) -> Value {
        let mut listed = self
            .list_all("payment/providers", query)
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in &mut listed {
            mask_payment_provider_config(item);
        }
        json!(listed)
    }

    pub fn get_payment_provider_internal(&self, id: i64) -> Result<Value, ApiError> {
        let mut item = self.get_collection_item("payment/providers", id)?;
        ensure_payment_provider_shape(&mut item);
        Ok(item)
    }

    pub fn create_payment_provider(&self, payload: Value) -> Result<Value, ApiError> {
        validate_payment_provider_payload(&payload)?;
        let mut item = self.create_collection_item("payment/providers", payload);
        ensure_payment_provider_shape(&mut item);
        self.update_collection_item(
            "payment/providers",
            item_id(&item).unwrap_or_default(),
            item.clone(),
        )?;
        mask_payment_provider_config(&mut item);
        Ok(item)
    }

    pub fn update_payment_provider(&self, id: i64, payload: Value) -> Result<Value, ApiError> {
        self.update_payment_provider_with_pending(id, payload, &[])
    }

    pub fn update_payment_provider_with_pending(
        &self,
        id: i64,
        payload: Value,
        pending_provider_instance_ids: &[String],
    ) -> Result<Value, ApiError> {
        let current = self.get_collection_item("payment/providers", id)?;
        let mut next = current.clone();
        merge_payment_provider_update(&mut next, payload);
        validate_payment_provider_payload(&next)?;
        if has_pending_provider_orders(&self.collections, id, pending_provider_instance_ids)
            && payment_provider_has_protected_change(&current, &next)
        {
            return Err(ApiError::conflict("instance has pending orders"));
        }
        let mut saved = self.update_collection_item("payment/providers", id, next)?;
        ensure_payment_provider_shape(&mut saved);
        self.update_collection_item("payment/providers", id, saved.clone())?;
        mask_payment_provider_config(&mut saved);
        Ok(saved)
    }

    pub fn delete_payment_provider(&self, id: i64) -> Result<Value, ApiError> {
        self.delete_payment_provider_with_pending(id, &[])
    }

    pub fn delete_payment_provider_with_pending(
        &self,
        id: i64,
        pending_provider_instance_ids: &[String],
    ) -> Result<Value, ApiError> {
        if has_pending_provider_orders(&self.collections, id, pending_provider_instance_ids) {
            return Err(ApiError::conflict("instance has pending orders"));
        }
        Ok(self.delete_collection_item("payment/providers", id))
    }

    pub fn payment_order_action(&self, id: i64, status: &str, payload: Option<Value>) -> Value {
        let mut order = self
            .get_collection_item("payment-orders", id)
            .unwrap_or_else(|_| default_item_for("payment-orders", id));
        order["status"] = json!(status);
        if let Some(payload) = payload {
            merge_json(&mut order, payload);
        }
        self.update_collection_item("payment-orders", id, order)
            .unwrap_or_else(|_| json!({ "id": id, "status": status }))
    }

    pub fn promo_usages(&self, code_id: i64, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let mut items = self
            .promo_code_usages
            .read()
            .expect("promo code usages lock")
            .iter()
            .filter(|item| item.get("promo_code_id").and_then(Value::as_i64) == Some(code_id))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.get("id").and_then(Value::as_i64).unwrap_or_default());
        items.reverse();
        let total = items.len() as i64;
        let start = ((page - 1) * page_size) as usize;
        paginated(
            items
                .into_iter()
                .skip(start)
                .take(page_size as usize)
                .collect(),
            total,
            page,
            page_size,
        )
    }

    pub fn system_version(&self) -> Value {
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "backend": "backend_next",
            "runtime": "rust"
        })
    }

    pub fn system_restart(&self) -> Value {
        let request = json!({
            "accepted": true,
            "message": "restart request recorded",
            "requested_at": NOW
        });
        self.system_restart_requests
            .write()
            .expect("system restart lock")
            .push(request.clone());
        request
    }

    pub fn generic_ack(&self, path: &str) -> Value {
        json!({
            "message": "ok",
            "path": path
        })
    }

    pub fn delete_affiliate_user(&self, user_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("users", user_id)?;
        let mut profiles = self
            .affiliate_profiles
            .write()
            .expect("affiliate profiles lock");
        let profile = profiles
            .entry(user_id)
            .or_insert_with(|| default_affiliate_profile(user_id));
        profile["aff_code"] = json!(format!("AFF{user_id:04}"));
        profile["aff_code_custom"] = json!(false);
        profile["aff_rebate_rate_percent"] = Value::Null;
        profile["updated_at"] = json!(NOW);
        Ok(json!({ "user_id": user_id }))
    }

    pub fn affiliate_records(&self, kind: &str, query: &HashMap<String, String>) -> Value {
        let records = match kind {
            "invites" => self
                .affiliate_invites
                .read()
                .expect("affiliate invites lock")
                .clone(),
            "rebates" => self
                .affiliate_rebates
                .read()
                .expect("affiliate rebates lock")
                .clone(),
            "transfers" => self
                .affiliate_transfers
                .read()
                .expect("affiliate transfers lock")
                .clone(),
            _ => Vec::new(),
        };
        paginate_values(records, query, 20, 200)
    }

    pub fn affiliate_users(&self, query: &HashMap<String, String>) -> Value {
        let profiles = self
            .affiliate_profiles
            .read()
            .expect("affiliate profiles lock");
        let users = self
            .collections
            .read()
            .expect("admin collection lock")
            .get("users")
            .cloned()
            .unwrap_or_default();
        let entries = profiles
            .values()
            .filter_map(|profile| {
                let user_id = profile.get("user_id").and_then(Value::as_i64)?;
                let user = users.iter().find(|item| item_id(item) == Some(user_id));
                Some(json!({
                    "user_id": user_id,
                    "email": user.and_then(|item| item.get("email")).cloned().unwrap_or(Value::Null),
                    "username": user.and_then(|item| item.get("username")).cloned().unwrap_or(Value::Null),
                    "aff_code": profile.get("aff_code").cloned().unwrap_or(Value::Null),
                    "aff_code_custom": profile.get("aff_code_custom").cloned().unwrap_or_else(|| json!(false)),
                    "aff_rebate_rate_percent": profile.get("aff_rebate_rate_percent").cloned().unwrap_or(Value::Null),
                    "aff_count": profile.get("aff_count").cloned().unwrap_or_else(|| json!(0))
                }))
            })
            .collect::<Vec<_>>();
        paginate_values(entries, query, 20, 200)
    }

    pub fn affiliate_user_lookup(&self, query: &HashMap<String, String>) -> Value {
        let keyword = query.get("q").map(|value| value.trim()).unwrap_or_default();
        if keyword.is_empty() {
            return json!([]);
        }
        let mut scoped_query = HashMap::new();
        scoped_query.insert("search".to_owned(), keyword.to_owned());
        self.list_all("users", &scoped_query)
    }

    pub fn affiliate_user_overview(&self, user_id: i64) -> Result<Value, ApiError> {
        let user = self.get_collection_item("users", user_id)?;
        let profile = self
            .affiliate_profiles
            .read()
            .expect("affiliate profiles lock")
            .get(&user_id)
            .cloned()
            .unwrap_or_else(|| default_affiliate_profile(user_id));
        Ok(json!({
            "user_id": user_id,
            "email": user.get("email").cloned().unwrap_or(Value::Null),
            "username": user.get("username").cloned().unwrap_or(Value::Null),
            "aff_code": profile.get("aff_code").cloned().unwrap_or(Value::Null),
            "rebate_rate_percent": profile.get("aff_rebate_rate_percent").cloned().unwrap_or_else(|| json!(5.0)),
            "invited_count": profile.get("aff_count").cloned().unwrap_or_else(|| json!(0)),
            "rebated_invitee_count": profile.get("rebated_invitee_count").cloned().unwrap_or_else(|| json!(0)),
            "available_quota": profile.get("available_quota").cloned().unwrap_or_else(|| json!(0.0)),
            "history_quota": profile.get("history_quota").cloned().unwrap_or_else(|| json!(0.0))
        }))
    }

    pub fn update_affiliate_user(&self, user_id: i64, payload: Value) -> Result<Value, ApiError> {
        self.get_collection_item("users", user_id)?;
        let mut profiles = self
            .affiliate_profiles
            .write()
            .expect("affiliate profiles lock");
        let profile = profiles
            .entry(user_id)
            .or_insert_with(|| default_affiliate_profile(user_id));
        if let Some(code) = payload.get("aff_code").and_then(Value::as_str) {
            let code = normalize_aff_code(code)?;
            profile["aff_code"] = json!(code);
            profile["aff_code_custom"] = json!(true);
        }
        if payload
            .get("clear_rebate_rate")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            profile["aff_rebate_rate_percent"] = Value::Null;
        } else if let Some(rate) = payload
            .get("aff_rebate_rate_percent")
            .and_then(Value::as_f64)
        {
            profile["aff_rebate_rate_percent"] = json!(rate.clamp(0.0, 100.0));
        }
        profile["updated_at"] = json!(NOW);
        Ok(json!({ "user_id": user_id }))
    }

    pub fn batch_affiliate_rate(&self, payload: Value) -> Result<Value, ApiError> {
        let user_ids = payload
            .get("user_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| ApiError::bad_request("user_ids is required"))?
            .iter()
            .filter_map(Value::as_i64)
            .collect::<Vec<_>>();
        let clear = payload
            .get("clear")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let rate = payload
            .get("aff_rebate_rate_percent")
            .and_then(Value::as_f64)
            .map(|value| value.clamp(0.0, 100.0));
        if user_ids.is_empty() {
            return Err(ApiError::bad_request("user_ids cannot be empty"));
        }
        if !clear && rate.is_none() {
            return Err(ApiError::bad_request(
                "aff_rebate_rate_percent is required unless clear=true",
            ));
        }
        let mut profiles = self
            .affiliate_profiles
            .write()
            .expect("affiliate profiles lock");
        let mut affected = 0;
        for user_id in user_ids {
            if self.get_collection_item("users", user_id).is_err() {
                continue;
            }
            let profile = profiles
                .entry(user_id)
                .or_insert_with(|| default_affiliate_profile(user_id));
            if clear {
                profile["aff_rebate_rate_percent"] = Value::Null;
            } else if let Some(rate) = rate {
                profile["aff_rebate_rate_percent"] = json!(rate);
            }
            profile["updated_at"] = json!(NOW);
            affected += 1;
        }
        Ok(json!({ "affected": affected }))
    }

    pub fn gateway_account_candidates(&self, group_id: i64) -> Vec<AccountGroupBinding> {
        let collections = self.collections.read().expect("admin collection lock");
        collections
            .get("accounts")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.get("group_id").and_then(Value::as_i64) == Some(group_id))
            .filter(|item| {
                item.get("status")
                    .and_then(Value::as_str)
                    .map(|status| status == "active")
                    .unwrap_or(true)
            })
            .filter_map(|item| account_binding_from_json(group_id, &item))
            .collect()
    }

    pub fn resolve_channel_model_mapping(
        &self,
        group_id: i64,
        provider: Provider,
        requested_model: &str,
    ) -> ChannelModelMapping {
        let requested_model = requested_model.trim();
        if requested_model.is_empty() {
            return ChannelModelMapping::unmapped(requested_model);
        }

        let collections = self.collections.read().expect("admin collection lock");
        let mapping_platform = provider.as_str();

        let channel = collections.get("channels").and_then(|channels| {
            channels.iter().find(|channel| {
                channel
                    .get("status")
                    .and_then(Value::as_str)
                    .map(|status| status == "active")
                    .unwrap_or(true)
                    && channel_group_ids(channel).contains(&group_id)
            })
        });

        let Some(channel) = channel else {
            return ChannelModelMapping::unmapped(requested_model);
        };
        let channel_id = item_id(channel).unwrap_or_default();
        let mapping_value = channel.get("model_mapping");
        let rules = parse_channel_model_mapping(mapping_value, mapping_platform);
        let resolved = domain::resolve_model_mapping(&rules, requested_model);
        ChannelModelMapping {
            channel_id,
            requested_model: requested_model.to_owned(),
            mapped_model: resolved.upstream_model,
            matched: resolved.matched,
            matched_source: resolved.matched_source,
        }
    }

    pub fn model_default_pricing(&self, model: Option<&str>) -> Result<Value, ApiError> {
        let model = model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("model parameter is required"))?;
        if let Some(pricing) = builtin_model_pricing(model) {
            return Ok(pricing);
        }
        let collections = self.collections.read().expect("admin collection lock");
        let pricing = collections
            .get("channels")
            .into_iter()
            .flatten()
            .flat_map(|channel| {
                channel
                    .get("model_pricing")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .find(|item| item.get("model").and_then(Value::as_str) == Some(model));
        Ok(pricing
            .map(|item| {
                json!({
                    "found": true,
                    "input_price": item.get("input_price").or_else(|| item.get("input_price_per_token")).cloned().unwrap_or(Value::Null),
                    "output_price": item.get("output_price").or_else(|| item.get("output_price_per_token")).cloned().unwrap_or(Value::Null),
                    "cache_write_price": item.get("cache_write_price").cloned().unwrap_or(Value::Null),
                    "cache_read_price": item.get("cache_read_price").cloned().unwrap_or(Value::Null),
                    "image_output_price": item.get("image_output_price").cloned().unwrap_or(Value::Null)
                })
            })
            .unwrap_or_else(|| json!({ "found": false })))
    }

    fn model_pricing(&self, model: &str) -> Option<ModelPricing> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let collections = self.collections.read().expect("admin collection lock");
        let channel_pricing = collections
            .get("channels")
            .into_iter()
            .flatten()
            .flat_map(|channel| {
                channel
                    .get("model_pricing")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .find(|item| item.get("model").and_then(Value::as_str) == Some(model))
            .and_then(pricing_from_value);
        channel_pricing.or_else(|| {
            builtin_model_pricing(model)
                .as_ref()
                .and_then(pricing_from_value)
        })
    }

    pub fn sync_pricing_models(&self, platform: Option<&str>) -> Result<Value, ApiError> {
        let platform = platform
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("platform parameter is required"))?
            .to_lowercase();
        let models = match platform.as_str() {
            "openai" => vec!["gpt-5.4", "gpt-5.5", "gpt-4.1"],
            "anthropic" | "antigravity" => vec!["claude-sonnet-4-5", "claude-opus-4-1"],
            "gemini" | "google" => vec!["gemini-2.5-pro", "gemini-2.5-flash"],
            "deepseek" => vec!["deepseek-chat", "deepseek-reasoner"],
            _ => {
                return Err(ApiError::bad_request(format!(
                    "unsupported platform: {platform}"
                )))
            }
        };
        Ok(json!({ "models": models }))
    }

    pub fn channel_monitor_config(&self, id: i64) -> Result<ChannelMonitorConfig, ApiError> {
        let monitor = self.get_collection_item("channel-monitors", id)?;
        Ok(ChannelMonitorConfig {
            id,
            provider: monitor
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("openai")
                .to_owned(),
            api_mode: monitor
                .get("api_mode")
                .and_then(Value::as_str)
                .unwrap_or("chat_completions")
                .to_owned(),
            endpoint: monitor
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            api_key: monitor
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            primary_model: monitor
                .get("primary_model")
                .and_then(Value::as_str)
                .unwrap_or("gpt-5.4")
                .to_owned(),
            extra_models: monitor
                .get("extra_models")
                .and_then(Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            extra_headers: monitor
                .get("extra_headers")
                .cloned()
                .unwrap_or_else(|| json!({})),
            body_override_mode: monitor
                .get("body_override_mode")
                .and_then(Value::as_str)
                .unwrap_or("off")
                .to_owned(),
            body_override: monitor
                .get("body_override")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }

    pub fn apply_channel_monitor_template(
        &self,
        template_id: i64,
        payload: Value,
    ) -> Result<Value, ApiError> {
        let template = self.get_collection_item("channel-monitor-templates", template_id)?;
        let monitor_ids = payload
            .get("monitor_ids")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut affected = 0;
        for monitor_id in monitor_ids.into_iter().filter_map(|value| value.as_i64()) {
            let Ok(monitor) = self.get_collection_item("channel-monitors", monitor_id) else {
                continue;
            };
            if monitor.get("template_id").and_then(Value::as_i64) != Some(template_id) {
                continue;
            }
            if monitor.get("provider").and_then(Value::as_str)
                != template.get("provider").and_then(Value::as_str)
            {
                continue;
            }
            if monitor.get("api_mode").and_then(Value::as_str)
                != template.get("api_mode").and_then(Value::as_str)
            {
                continue;
            }
            self.update_collection_item(
                "channel-monitors",
                monitor_id,
                json!({
                    "extra_headers": template.get("extra_headers").cloned().unwrap_or_else(|| json!({})),
                    "body_override_mode": template.get("body_override_mode").cloned().unwrap_or_else(|| json!("off")),
                    "body_override": template.get("body_override").cloned().unwrap_or_else(|| json!({}))
                }),
            )?;
            affected += 1;
        }
        Ok(json!({ "affected": affected }))
    }

    pub fn associated_channel_monitors(&self, template_id: i64) -> Value {
        let collections = self.collections.read().expect("admin collection lock");
        let items = collections
            .get("channel-monitors")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.get("template_id").and_then(Value::as_i64) == Some(template_id))
            .map(|item| {
                json!({
                    "id": item["id"].clone(),
                    "name": item["name"].clone(),
                    "provider": item.get("provider").cloned().unwrap_or_else(|| json!("openai")),
                    "api_mode": item.get("api_mode").cloned().unwrap_or_else(|| json!("responses")),
                    "enabled": item.get("enabled").and_then(Value::as_bool).unwrap_or(true)
                })
            })
            .collect::<Vec<_>>();
        json!({ "items": items })
    }

    pub fn scheduled_test_results(&self, plan_id: i64, query: &HashMap<String, String>) -> Value {
        let limit = query_i64(query, "limit", 50).clamp(1, 500) as usize;
        let results = self
            .collections
            .read()
            .expect("admin collection lock")
            .get("scheduled-test-results")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.get("plan_id").and_then(Value::as_i64) == Some(plan_id))
            .take(limit)
            .collect::<Vec<_>>();
        Value::Array(results)
    }

    pub fn scheduled_test_plans_by_account(&self, account_id: i64) -> Result<Value, ApiError> {
        self.get_collection_item("accounts", account_id)?;
        let plans = self
            .collections
            .read()
            .expect("admin collection lock")
            .get("scheduled-test-plans")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.get("account_id").and_then(Value::as_i64) == Some(account_id))
            .collect::<Vec<_>>();
        Ok(Value::Array(plans))
    }

    pub fn batch_account_action(&self, payload: Value, action: &str) -> Result<Value, ApiError> {
        let ids = payload_ids(&payload, "account_ids")?;
        let mut collections = self.collections.write().expect("admin collection lock");
        let accounts = collections.entry("accounts".to_owned()).or_default();
        let mut success = 0;
        let mut failed = 0;
        let mut errors = Vec::new();
        for id in ids {
            if let Some(account) = accounts.iter_mut().find(|item| item_id(item) == Some(id)) {
                account["last_action"] = json!(action);
                account["last_refreshed_at"] = json!(NOW);
                ensure_timestamp(account);
                success += 1;
            } else {
                failed += 1;
                errors.push(json!({
                    "account_id": id,
                    "error": "account not found"
                }));
            }
        }
        Ok(json!({
            "total": success + failed,
            "success": success,
            "failed": failed,
            "errors": errors,
            "warnings": []
        }))
    }

    pub fn backup_s3_config(&self) -> Value {
        self.backup_s3_config
            .read()
            .expect("backup s3 config lock")
            .clone()
    }

    pub fn update_backup_s3_config(&self, payload: Value) -> Value {
        let mut config = self
            .backup_s3_config
            .write()
            .expect("backup s3 config lock");
        merge_json(&mut config, payload);
        if config
            .get("secret_access_key")
            .and_then(Value::as_str)
            .map(str::is_empty)
            .unwrap_or(true)
        {
            config["secret_access_key_configured"] = json!(false);
        } else {
            config["secret_access_key_configured"] = json!(true);
        }
        config.clone()
    }

    pub fn backup_schedule(&self) -> Value {
        self.backup_schedule
            .read()
            .expect("backup schedule lock")
            .clone()
    }

    pub fn update_backup_schedule(&self, payload: Value) -> Value {
        let mut schedule = self.backup_schedule.write().expect("backup schedule lock");
        merge_json(&mut schedule, payload);
        schedule.clone()
    }

    pub fn list_backups(&self) -> Value {
        json!({
            "items": self.backup_records.read().expect("backup records lock").clone()
        })
    }

    pub fn create_backup(&self, payload: Value) -> Value {
        let id = format!("backup-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let expire_days = payload
            .get("expire_days")
            .and_then(Value::as_i64)
            .unwrap_or(7)
            .max(0);
        let triggered_by = payload
            .get("triggered_by")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("manual");
        let record = json!({
            "id": id,
            "status": "completed",
            "backup_type": "full",
            "file_name": format!("sub2api-backup-{NOW}.tar.gz"),
            "s3_key": format!("backups/sub2api-backup-{NOW}.tar.gz"),
            "size_bytes": 0,
            "triggered_by": triggered_by,
            "error_message": null,
            "started_at": NOW,
            "finished_at": NOW,
            "expires_at": if expire_days == 0 { Value::Null } else { json!(NOW) },
            "progress": "completed",
            "restore_status": null,
            "restore_error": null,
            "restored_at": null
        });
        self.backup_records
            .write()
            .expect("backup records lock")
            .push(record.clone());
        record
    }

    pub fn get_backup(&self, id: &str) -> Result<Value, ApiError> {
        self.backup_records
            .read()
            .expect("backup records lock")
            .iter()
            .find(|record| record.get("id").and_then(Value::as_str) == Some(id))
            .cloned()
            .ok_or_else(|| ApiError::not_found("backup record not found"))
    }

    pub fn delete_backup(&self, id: &str) -> Value {
        self.backup_records
            .write()
            .expect("backup records lock")
            .retain(|record| record.get("id").and_then(Value::as_str) != Some(id));
        json!({ "deleted": true })
    }

    pub fn backup_download_url(&self, id: &str) -> Result<Value, ApiError> {
        self.get_backup(id)?;
        Ok(json!({ "url": format!("/api/v1/admin/backups/{id}/download") }))
    }

    pub fn restore_backup(&self, id: &str) -> Result<Value, ApiError> {
        let mut records = self.backup_records.write().expect("backup records lock");
        let record = records
            .iter_mut()
            .find(|record| record.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| ApiError::not_found("backup record not found"))?;
        record["restore_status"] = json!("completed");
        record["restored_at"] = json!(NOW);
        Ok(record.clone())
    }

    pub fn data_management_agent_health(&self) -> Value {
        json!({
            "enabled": false,
            "reason": "agent disabled by data management config",
            "socket_path": "",
            "agent": null
        })
    }

    pub fn data_management_config(&self) -> Value {
        self.data_management_config
            .read()
            .expect("data management config lock")
            .clone()
    }

    pub fn update_data_management_config(&self, payload: Value) -> Value {
        let mut config = self
            .data_management_config
            .write()
            .expect("data management config lock");
        merge_json(&mut config, payload);
        config.clone()
    }

    pub fn list_source_profiles(&self, source_type: &str) -> Value {
        let profiles = self
            .data_management_source_profiles
            .read()
            .expect("source profiles lock")
            .get(source_type)
            .cloned()
            .unwrap_or_default();
        json!({ "items": profiles })
    }

    pub fn create_source_profile(&self, source_type: &str, payload: Value) -> Value {
        let profile_id = payload
            .get("profile_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{source_type}-{}",
                    self.next_id.fetch_add(1, Ordering::SeqCst)
                )
            });
        let set_active = payload
            .get("set_active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let profile = json!({
            "source_type": source_type,
            "profile_id": profile_id,
            "name": payload.get("name").and_then(Value::as_str).unwrap_or("Default"),
            "is_active": set_active,
            "password_configured": payload.pointer("/config/password").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false),
            "config": payload.get("config").cloned().unwrap_or_else(|| default_source_config(source_type)),
            "created_at": NOW,
            "updated_at": NOW
        });
        let mut profiles = self
            .data_management_source_profiles
            .write()
            .expect("source profiles lock");
        let items = profiles.entry(source_type.to_owned()).or_default();
        if set_active {
            set_active_profile(items, &profile_id);
        }
        items.push(profile.clone());
        profile
    }

    pub fn update_source_profile(
        &self,
        source_type: &str,
        profile_id: &str,
        payload: Value,
    ) -> Result<Value, ApiError> {
        let mut profiles = self
            .data_management_source_profiles
            .write()
            .expect("source profiles lock");
        let items = profiles.entry(source_type.to_owned()).or_default();
        let profile = items
            .iter_mut()
            .find(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
            .ok_or_else(|| ApiError::not_found("source profile not found"))?;
        merge_json(profile, payload);
        Ok(profile.clone())
    }

    pub fn delete_source_profile(&self, source_type: &str, profile_id: &str) -> Value {
        if let Some(items) = self
            .data_management_source_profiles
            .write()
            .expect("source profiles lock")
            .get_mut(source_type)
        {
            items.retain(|item| item.get("profile_id").and_then(Value::as_str) != Some(profile_id));
        }
        json!({ "deleted": true })
    }

    pub fn activate_source_profile(
        &self,
        source_type: &str,
        profile_id: &str,
    ) -> Result<Value, ApiError> {
        let mut profiles = self
            .data_management_source_profiles
            .write()
            .expect("source profiles lock");
        let items = profiles.entry(source_type.to_owned()).or_default();
        if !items
            .iter()
            .any(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
        {
            return Err(ApiError::not_found("source profile not found"));
        }
        set_active_profile(items, profile_id);
        Ok(items
            .iter()
            .find(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
            .cloned()
            .unwrap())
    }

    pub fn list_s3_profiles(&self) -> Value {
        json!({
            "items": self.data_management_s3_profiles.read().expect("s3 profiles lock").clone()
        })
    }

    pub fn create_s3_profile(&self, payload: Value) -> Value {
        let profile_id = payload
            .get("profile_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("s3-{}", self.next_id.fetch_add(1, Ordering::SeqCst)));
        let set_active = payload
            .get("set_active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let profile = json!({
            "profile_id": profile_id,
            "name": payload.get("name").and_then(Value::as_str).unwrap_or("Default S3"),
            "is_active": set_active,
            "s3": s3_profile_config_from_payload(&payload),
            "secret_access_key_configured": payload.get("secret_access_key").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false),
            "created_at": NOW,
            "updated_at": NOW
        });
        let mut profiles = self
            .data_management_s3_profiles
            .write()
            .expect("s3 profiles lock");
        if set_active {
            set_active_profile(&mut profiles, &profile_id);
        }
        profiles.push(profile.clone());
        profile
    }

    pub fn update_s3_profile(&self, profile_id: &str, payload: Value) -> Result<Value, ApiError> {
        let mut profiles = self
            .data_management_s3_profiles
            .write()
            .expect("s3 profiles lock");
        let profile = profiles
            .iter_mut()
            .find(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
            .ok_or_else(|| ApiError::not_found("s3 profile not found"))?;
        merge_json(
            profile,
            json!({
                "name": payload.get("name").cloned().unwrap_or_else(|| profile["name"].clone()),
                "s3": s3_profile_config_from_payload(&payload),
                "secret_access_key_configured": payload.get("secret_access_key").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or_else(|| profile.get("secret_access_key_configured").and_then(Value::as_bool).unwrap_or(false))
            }),
        );
        Ok(profile.clone())
    }

    pub fn delete_s3_profile(&self, profile_id: &str) -> Value {
        self.data_management_s3_profiles
            .write()
            .expect("s3 profiles lock")
            .retain(|item| item.get("profile_id").and_then(Value::as_str) != Some(profile_id));
        json!({ "deleted": true })
    }

    pub fn activate_s3_profile(&self, profile_id: &str) -> Result<Value, ApiError> {
        let mut profiles = self
            .data_management_s3_profiles
            .write()
            .expect("s3 profiles lock");
        if !profiles
            .iter()
            .any(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
        {
            return Err(ApiError::not_found("s3 profile not found"));
        }
        set_active_profile(&mut profiles, profile_id);
        Ok(profiles
            .iter()
            .find(|item| item.get("profile_id").and_then(Value::as_str) == Some(profile_id))
            .cloned()
            .unwrap())
    }

    pub fn create_data_management_job(&self, payload: Value) -> Value {
        let job_id = format!("job-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let job = json!({
            "job_id": job_id,
            "backup_type": payload.get("backup_type").and_then(Value::as_str).unwrap_or("full"),
            "status": "succeeded",
            "triggered_by": "manual",
            "s3_profile_id": payload.get("s3_profile_id").cloned().unwrap_or(Value::Null),
            "postgres_profile_id": payload.get("postgres_profile_id").cloned().unwrap_or(Value::Null),
            "redis_profile_id": payload.get("redis_profile_id").cloned().unwrap_or(Value::Null),
            "started_at": NOW,
            "finished_at": NOW,
            "error_message": null,
            "artifact": {
                "local_path": "",
                "size_bytes": 0,
                "sha256": ""
            },
            "s3": null
        });
        self.data_management_jobs
            .write()
            .expect("data management jobs lock")
            .push(job.clone());
        json!({
            "job_id": job["job_id"].clone(),
            "status": job["status"].clone()
        })
    }

    pub fn list_data_management_jobs(&self) -> Value {
        json!({
            "items": self.data_management_jobs.read().expect("data management jobs lock").clone(),
            "next_page_token": null
        })
    }

    pub fn get_data_management_job(&self, job_id: &str) -> Result<Value, ApiError> {
        self.data_management_jobs
            .read()
            .expect("data management jobs lock")
            .iter()
            .find(|job| job.get("job_id").and_then(Value::as_str) == Some(job_id))
            .cloned()
            .ok_or_else(|| ApiError::not_found("backup job not found"))
    }

    pub fn risk_control_config(&self) -> Value {
        self.risk_control_config
            .read()
            .expect("risk control config lock")
            .clone()
    }

    pub fn update_risk_control_config(&self, payload: Value) -> Value {
        let mut config = self
            .risk_control_config
            .write()
            .expect("risk control config lock");
        merge_json(&mut config, payload);
        config["api_key_configured"] = json!(config
            .get("api_key")
            .and_then(Value::as_str)
            .map(|value| !value.is_empty())
            .unwrap_or(false));
        config.clone()
    }

    pub fn risk_control_status(&self) -> Value {
        let config = self.risk_control_config();
        let enabled = config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        json!({
            "enabled": enabled,
            "risk_control_enabled": enabled,
            "mode": config.get("mode").cloned().unwrap_or_else(|| json!("off")),
            "worker_count": config.get("worker_count").cloned().unwrap_or_else(|| json!(1)),
            "max_workers": 16,
            "active_workers": 0,
            "idle_workers": config.get("worker_count").cloned().unwrap_or_else(|| json!(1)),
            "queue_size": config.get("queue_size").cloned().unwrap_or_else(|| json!(1000)),
            "queue_length": 0,
            "queue_usage_percent": 0,
            "enqueued": 0,
            "dropped": 0,
            "processed": 0,
            "errors": 0,
            "pre_block_active": 0,
            "pre_block_checked": 0,
            "pre_block_allowed": 0,
            "pre_block_blocked": 0,
            "pre_block_errors": 0,
            "pre_block_avg_latency_ms": 0,
            "pre_block_api_key_active": 0,
            "pre_block_api_key_available_count": config.get("api_key_count").cloned().unwrap_or_else(|| json!(0)),
            "pre_block_api_key_total_calls": 0,
            "pre_block_api_key_loads": [],
            "api_key_statuses": config.get("api_key_statuses").cloned().unwrap_or_else(|| json!([])),
            "flagged_hash_count": self.flagged_hashes.read().expect("flagged hashes lock").len(),
            "last_cleanup_at": null,
            "last_cleanup_deleted_hit": 0,
            "last_cleanup_deleted_non_hit": 0
        })
    }

    pub fn risk_control_test_api_keys(&self, payload: Value) -> Value {
        let keys = payload
            .get("api_keys")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let items = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let configured = key.as_str().map(|value| !value.is_empty()).unwrap_or(false);
                moderation_api_key_status(index, configured)
            })
            .collect::<Vec<_>>();
        json!({
            "items": items,
            "audit_result": {
                "flagged": false,
                "highest_category": "",
                "highest_score": 0,
                "composite_score": 0,
                "category_scores": {},
                "thresholds": self.risk_control_config().get("thresholds").cloned().unwrap_or_else(|| json!({}))
            },
            "image_count": payload.get("images").and_then(Value::as_array).map(Vec::len).unwrap_or(0)
        })
    }

    pub fn risk_control_logs(&self, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        json!({
            "items": [],
            "total": 0,
            "page": page,
            "page_size": page_size,
            "pages": 0
        })
    }

    pub fn unban_user(&self, user_id: i64) -> Value {
        json!({
            "user_id": user_id,
            "status": "active"
        })
    }

    pub fn delete_flagged_hash(&self, payload: Value) -> Value {
        let input_hash = payload
            .get("input_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        self.flagged_hashes
            .write()
            .expect("flagged hashes lock")
            .retain(|hash| hash != &input_hash);
        json!({
            "input_hash": input_hash,
            "deleted": true
        })
    }

    pub fn clear_flagged_hashes(&self) -> Value {
        let mut hashes = self.flagged_hashes.write().expect("flagged hashes lock");
        let deleted = hashes.len();
        hashes.clear();
        json!({ "deleted": deleted })
    }

    pub fn oauth_auth_url(&self, path: &str, payload: Value) -> Value {
        let provider = admin_oauth_provider_from_path(path);
        let session_id = format!("oauth-{}-{}", provider, Uuid::new_v4().simple());
        let redirect_uri = payload
            .get("redirect_uri")
            .or_else(|| payload.get("redirect_url"))
            .and_then(Value::as_str)
            .unwrap_or("http://localhost/auth/callback");
        let state = payload
            .get("state")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("state-{}", Uuid::new_v4().simple()));
        let auth_url =
            format!("http://localhost/oauth/{provider}?state={state}&redirect_uri={redirect_uri}");
        let session = json!({
            "session_id": session_id,
            "provider": provider,
            "state": state,
            "redirect_uri": redirect_uri,
            "created_at": NOW
        });
        self.oauth_sessions
            .write()
            .expect("oauth session lock")
            .insert(
                session["session_id"].as_str().unwrap().to_owned(),
                session.clone(),
            );
        json!({
            "auth_url": auth_url,
            "session_id": session["session_id"].clone(),
            "state": session["state"].clone(),
            "provider": session["provider"].clone()
        })
    }

    pub fn oauth_token_info(&self, path: &str, payload: Value) -> Result<Value, ApiError> {
        let provider = admin_oauth_provider_from_path(path);
        if path.contains("exchange-code")
            && payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(ApiError::bad_request("code is required"));
        }
        Ok(json!({
            "provider": provider,
            "access_token": format!("{}-access-{}", provider, Uuid::new_v4().simple()),
            "refresh_token": format!("{}-refresh-{}", provider, Uuid::new_v4().simple()),
            "expires_in": 3600,
            "token_type": "Bearer",
            "scope": payload.get("scope").and_then(Value::as_str).unwrap_or("default"),
            "created_at": NOW
        }))
    }

    pub fn reorder_user_attributes(&self, payload: Value) -> Result<Value, ApiError> {
        let ids = payload
            .get("ids")
            .or_else(|| payload.get("order"))
            .and_then(Value::as_array)
            .ok_or_else(|| ApiError::bad_request("ids is required"))?
            .iter()
            .map(|value| {
                value
                    .as_i64()
                    .ok_or_else(|| ApiError::bad_request("ids must contain integers"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        *self
            .user_attribute_order
            .write()
            .expect("user attribute order lock") = ids.clone();
        Ok(json!({
            "message": "User attributes reordered successfully",
            "ids": ids
        }))
    }

    fn find_item(&self, name: &str, id: i64) -> Option<Value> {
        let collection_name = normalize_collection_name(name);
        self.collections
            .read()
            .expect("admin collection lock")
            .get(&collection_name)
            .and_then(|items| items.iter().find(|item| item_id(item) == Some(id)).cloned())
    }
}

fn seed_collections() -> HashMap<String, Vec<Value>> {
    let mut collections = HashMap::new();
    collections.insert(
        "users".to_owned(),
        vec![json!({
            "id": 1,
            "username": "admin",
            "email": "admin@example.com",
            "role": "admin",
            "status": "active",
            "balance": 0.0,
            "affiliate_balance": 0.0,
            "concurrency": 5,
            "rpm_limit": 0,
            "allowed_groups": null,
            "created_at": NOW,
            "updated_at": NOW,
            "last_used_at": null,
            "notes": ""
        })],
    );
    collections.insert(
        "groups".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Default OpenAI",
            "platform": "openai",
            "description": "Default development group",
            "status": "active",
            "rate_multiplier": 1.0,
            "is_exclusive": false,
            "sort_order": 1,
            "models": ["gpt-5.4"],
            "supported_protocols": ["openai_chat_completions", "openai_responses", "openai_embeddings", "openai_images", "gemini_generate_content", "anthropic_messages"],
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "accounts".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Default upstream account",
            "platform": "openai",
            "type": "api-key",
            "status": "active",
            "group_id": 1,
            "group_name": "Default OpenAI",
            "base_url": "https://api.openai.com",
            "api_key": "",
            "proxy_id": null,
            "error_message": "",
            "supported_protocols": ["openai_chat_completions", "openai_responses", "openai_embeddings", "openai_images", "gemini_generate_content", "anthropic_messages"],
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "proxies".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Direct",
            "type": "direct",
            "url": "",
            "status": "active",
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "announcements".to_owned(),
        vec![json!({
            "id": 1,
            "title": "Welcome",
            "content": "backend_next demo announcement",
            "type": "info",
            "status": "published",
            "priority": 0,
            "published_at": NOW,
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert("redeem-codes".to_owned(), Vec::new());
    collections.insert(
        "promo-codes".to_owned(),
        vec![json!({
            "id": 1,
            "code": "WELCOME",
            "bonus_amount": 10.0,
            "max_uses": 100,
            "used_count": 1,
            "status": "active",
            "expires_at": null,
            "notes": "Default development promo code",
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert("subscriptions".to_owned(), Vec::new());
    collections.insert(
        "scheduled-test-plans".to_owned(),
        vec![json!({
            "id": 1,
            "account_id": 1,
            "model_id": "gpt-5.4",
            "cron_expression": "*/5 * * * *",
            "enabled": true,
            "max_results": 10,
            "auto_recover": false,
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "scheduled-test-results".to_owned(),
        vec![json!({
            "id": 1,
            "plan_id": 1,
            "status": "success",
            "response_text": "ok",
            "error_message": "",
            "latency_ms": 42,
            "started_at": NOW,
            "finished_at": NOW,
            "created_at": NOW
        })],
    );
    collections.insert(
        "channels".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Default Channel",
            "description": "Default development channel",
            "status": "active",
            "billing_model_source": "default",
            "restrict_models": false,
            "features_config": {},
            "group_ids": [1],
            "model_pricing": [],
            "model_mapping": {},
            "apply_pricing_to_account_stats": false,
            "account_stats_pricing_rules": [],
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "channel-monitors".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Default OpenAI monitor",
            "provider": "openai",
            "api_mode": "responses",
            "endpoint": "https://api.openai.com/v1/responses",
            "api_key_masked": "sk-***",
            "primary_model": "gpt-5.4",
            "extra_models": [],
            "group_name": "Default OpenAI",
            "enabled": true,
            "interval_seconds": 60,
            "template_id": null,
            "extra_headers": {},
            "body_override_mode": "off",
            "body_override": {},
            "primary_status": "unknown",
            "primary_latency_ms": null,
            "availability_7d": 0.0,
            "extra_models_status": [],
            "last_checked_at": null,
            "created_by": 1,
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "channel-monitor-templates".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Default monitor template",
            "provider": "openai",
            "api_mode": "responses",
            "extra_headers": {},
            "body_override_mode": "off",
            "body_override": {},
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert("payment-orders".to_owned(), Vec::new());
    collections.insert(
        "payment-channels".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Alipay",
            "payment_type": "alipay",
            "enabled": true,
            "fee_rate": 0.0,
            "sort_order": 1,
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "payment-plans".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Starter",
            "description": "Development starter plan",
            "price": 9.9,
            "original_price": null,
            "validity_days": 30,
            "group_id": 1,
            "features": ["Default OpenAI group"],
            "for_sale": true,
            "sort_order": 1,
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert(
        "payment-providers".to_owned(),
        vec![json!({
            "id": 1,
            "name": "Alipay Default",
            "payment_type": "alipay",
            "enabled": true,
            "priority": 1,
            "config": {},
            "created_at": NOW,
            "updated_at": NOW
        })],
    );
    collections.insert("tls-fingerprint-profiles".to_owned(), Vec::new());
    collections.insert("error-passthrough-rules".to_owned(), Vec::new());
    collections.insert("user-attributes".to_owned(), Vec::new());
    collections
}

fn default_item_for(name: &str, id: i64) -> Value {
    match name {
        "users" => json!({
            "id": id,
            "username": format!("user{id}"),
            "email": format!("user{id}@example.com"),
            "role": "user",
            "status": "active",
            "balance": 0.0,
            "affiliate_balance": 0.0,
            "concurrency": 1,
            "rpm_limit": 0,
            "allowed_groups": null,
            "created_at": NOW,
            "updated_at": NOW,
            "last_used_at": null,
            "notes": ""
        }),
        "groups" => json!({
            "id": id,
            "name": format!("Group {id}"),
            "platform": "openai",
            "description": "",
            "status": "active",
            "rate_multiplier": 1.0,
            "is_exclusive": false,
            "sort_order": id,
            "models": [],
            "supported_protocols": [],
            "created_at": NOW,
            "updated_at": NOW
        }),
        "accounts" => json!({
            "id": id,
            "name": format!("Account {id}"),
            "platform": "openai",
            "type": "api-key",
            "status": "active",
            "group_id": 1,
            "group_name": "Default OpenAI",
            "base_url": "",
            "api_key": "",
            "proxy_id": null,
            "error_message": "",
            "supported_protocols": [],
            "created_at": NOW,
            "updated_at": NOW
        }),
        "channels" => json!({
            "id": id,
            "name": format!("Channel {id}"),
            "description": "",
            "status": "active",
            "billing_model_source": "default",
            "restrict_models": false,
            "features_config": {},
            "group_ids": [],
            "model_pricing": [],
            "model_mapping": {},
            "apply_pricing_to_account_stats": false,
            "account_stats_pricing_rules": [],
            "created_at": NOW,
            "updated_at": NOW
        }),
        "channel-monitors" => json!({
            "id": id,
            "name": format!("Monitor {id}"),
            "provider": "openai",
            "api_mode": "responses",
            "endpoint": "",
            "api_key_masked": "***",
            "primary_model": "gpt-5.4",
            "extra_models": [],
            "group_name": "",
            "enabled": true,
            "interval_seconds": 60,
            "template_id": null,
            "extra_headers": {},
            "body_override_mode": "off",
            "body_override": {},
            "primary_status": "unknown",
            "primary_latency_ms": null,
            "availability_7d": 0.0,
            "extra_models_status": [],
            "last_checked_at": null,
            "created_by": 1,
            "created_at": NOW,
            "updated_at": NOW
        }),
        "channel-monitor-templates" => json!({
            "id": id,
            "name": format!("Monitor Template {id}"),
            "provider": "openai",
            "api_mode": "responses",
            "extra_headers": {},
            "body_override_mode": "off",
            "body_override": {},
            "created_at": NOW,
            "updated_at": NOW
        }),
        "payment-orders" => json!({
            "id": id,
            "user_id": 1,
            "amount": 0.0,
            "pay_amount": 0.0,
            "currency": "CNY",
            "fee_rate": 0.0,
            "payment_type": "alipay",
            "out_trade_no": format!("ADMIN{id}"),
            "status": "PENDING",
            "order_type": "balance",
            "created_at": NOW,
            "expires_at": NOW,
            "paid_at": null,
            "completed_at": null,
            "refund_amount": 0.0,
            "refund_reason": null
        }),
        "tls-fingerprint-profiles" => json!({
            "id": id,
            "name": format!("TLS Profile {id}"),
            "description": "",
            "fingerprint_type": "chrome",
            "ja3": "",
            "akamai": "",
            "enabled": true,
            "created_at": NOW,
            "updated_at": NOW
        }),
        "error-passthrough-rules" => json!({
            "id": id,
            "name": format!("Error Passthrough Rule {id}"),
            "provider": "openai",
            "status_code": 429,
            "match_type": "status_code",
            "match_value": "429",
            "enabled": true,
            "created_at": NOW,
            "updated_at": NOW
        }),
        "user-attributes" => json!({
            "id": id,
            "name": format!("attribute_{id}"),
            "label": format!("Attribute {id}"),
            "type": "text",
            "required": false,
            "enabled": true,
            "sort_order": id,
            "created_at": NOW,
            "updated_at": NOW
        }),
        _ => json!({
            "id": id,
            "name": format!("{name}-{id}"),
            "status": "active",
            "created_at": NOW,
            "updated_at": NOW
        }),
    }
}

fn default_settings() -> Value {
    json!({
        "site_name": "Sub2API",
        "registration_enabled": true,
        "turnstile_enabled": false,
        "email_enabled": false,
        "payment_enabled": true,
        "backend_mode": "standard",
        "updated_at": NOW
    })
}

fn seed_usage_logs() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "request_id": "usage-seed-1",
        "user_id": 1,
        "user_email": "admin@example.com",
        "api_key_id": 0,
        "api_key_name": "backend_next development key",
        "account_id": 1,
        "group_id": 1,
        "model": "gpt-5.4",
        "requested_model": "gpt-5.4",
        "upstream_model": "gpt-5.4",
        "endpoint": "/v1/responses",
        "request_type": "responses",
        "stream": false,
        "prompt_tokens": 12,
        "completion_tokens": 8,
        "total_tokens": 20,
        "cost": 0.0001,
        "status": "success",
        "billing_mode": "token",
        "created_at": NOW
    })]
}

fn usage_summary_from_items(items: &[&Value]) -> Value {
    let total = usage_totals(items);
    json!({
        "today": total,
        "total": total,
        "average_duration_ms": average_i64(items, "duration_ms"),
        "rpm": 0,
        "tpm": 0
    })
}

fn usage_totals(items: &[&Value]) -> Value {
    let requests = items.len() as i64;
    let input_tokens = sum_i64(items, "input_tokens");
    let output_tokens = sum_i64(items, "output_tokens");
    let cache_creation_tokens = sum_i64(items, "cache_creation_tokens");
    let cache_read_tokens = sum_i64(items, "cache_read_tokens");
    json!({
        "requests": requests,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_creation_tokens": cache_creation_tokens,
        "cache_read_tokens": cache_read_tokens,
        "total_tokens": input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens,
        "cost": sum_f64(items, "cost"),
        "actual_cost": sum_f64(items, "actual_cost")
    })
}

fn usage_daily_from_items(items: &[&Value]) -> Vec<Value> {
    if items.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "date": NOW.split('T').next().unwrap_or(NOW),
        "requests": items.len() as i64,
        "input_tokens": sum_i64(items, "input_tokens"),
        "output_tokens": sum_i64(items, "output_tokens"),
        "cache_creation_tokens": sum_i64(items, "cache_creation_tokens"),
        "cache_read_tokens": sum_i64(items, "cache_read_tokens"),
        "total_tokens": sum_i64(items, "total_tokens"),
        "cost": sum_f64(items, "cost"),
        "actual_cost": sum_f64(items, "actual_cost")
    })]
}

fn usage_model_stats_from_items(items: &[&Value]) -> Vec<Value> {
    let mut by_model: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let model = item
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        by_model.entry(model).or_default().push(*item);
    }
    let mut stats = by_model
        .into_iter()
        .map(|(model, items)| {
            json!({
                "model": model,
                "requests": items.len() as i64,
                "input_tokens": sum_i64(&items, "input_tokens"),
                "output_tokens": sum_i64(&items, "output_tokens"),
                "cache_creation_tokens": sum_i64(&items, "cache_creation_tokens"),
                "cache_read_tokens": sum_i64(&items, "cache_read_tokens"),
                "total_tokens": sum_i64(&items, "total_tokens"),
                "cost": sum_f64(&items, "cost"),
                "actual_cost": sum_f64(&items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        left.get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    stats
}

fn sum_i64(items: &[&Value], field: &str) -> i64 {
    items
        .iter()
        .map(|item| item.get(field).and_then(Value::as_i64).unwrap_or(0))
        .sum()
}

fn sum_f64(items: &[&Value], field: &str) -> f64 {
    items
        .iter()
        .map(|item| item.get(field).and_then(Value::as_f64).unwrap_or(0.0))
        .sum()
}

fn average_i64(items: &[&Value], field: &str) -> i64 {
    if items.is_empty() {
        return 0;
    }
    sum_i64(items, field) / items.len() as i64
}

fn seed_promo_code_usages() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "promo_code_id": 1,
        "user_id": 1,
        "bonus_amount": 10.0,
        "used_at": NOW,
        "user": {
            "id": 1,
            "email": "admin@example.com",
            "username": "admin"
        }
    })]
}

fn seed_affiliate_profiles() -> HashMap<i64, Value> {
    HashMap::from([(
        1,
        json!({
            "user_id": 1,
            "aff_code": "AFF0001",
            "aff_code_custom": false,
            "aff_rebate_rate_percent": null,
            "aff_count": 0,
            "rebated_invitee_count": 0,
            "available_quota": 0.0,
            "history_quota": 0.0,
            "created_at": NOW,
            "updated_at": NOW
        }),
    )])
}

fn seed_affiliate_invites() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "inviter_user_id": 1,
        "inviter_email": "admin@example.com",
        "inviter_username": "admin",
        "invitee_user_id": 2,
        "invitee_email": "user2@example.com",
        "invitee_username": "user2",
        "aff_code": "AFF0001",
        "created_at": NOW
    })]
}

fn seed_affiliate_rebates() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "inviter_user_id": 1,
        "inviter_email": "admin@example.com",
        "invitee_user_id": 2,
        "invitee_email": "user2@example.com",
        "source_order_id": 1,
        "base_amount": 10.0,
        "rebate_rate_percent": 5.0,
        "rebate_quota": 0.5,
        "status": "available",
        "available_at": NOW,
        "created_at": NOW
    })]
}

fn seed_affiliate_transfers() -> Vec<Value> {
    vec![json!({
        "id": 1,
        "user_id": 1,
        "email": "admin@example.com",
        "username": "admin",
        "quota_amount": 0.5,
        "balance_amount": 0.5,
        "created_at": NOW
    })]
}

fn default_affiliate_profile(user_id: i64) -> Value {
    json!({
        "user_id": user_id,
        "aff_code": format!("AFF{user_id:04}"),
        "aff_code_custom": false,
        "aff_rebate_rate_percent": null,
        "aff_count": 0,
        "rebated_invitee_count": 0,
        "available_quota": 0.0,
        "history_quota": 0.0,
        "created_at": NOW,
        "updated_at": NOW
    })
}

fn normalize_aff_code(raw: &str) -> Result<String, ApiError> {
    let code = raw.trim().to_uppercase();
    let valid_len = (4..=32).contains(&code.len());
    let valid_chars = code
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
    if !valid_len || !valid_chars {
        return Err(ApiError::bad_request("invalid affiliate code"));
    }
    Ok(code)
}

fn paginate_values(
    items: Vec<Value>,
    query: &HashMap<String, String>,
    default_page_size: i64,
    max_page_size: i64,
) -> Value {
    let page = query_i64(query, "page", 1).max(1);
    let page_size = query_i64(query, "page_size", default_page_size).clamp(1, max_page_size);
    let search = query
        .get("search")
        .or_else(|| query.get("q"))
        .map(|value| value.to_lowercase());
    let mut filtered = items
        .into_iter()
        .filter(|item| {
            search
                .as_ref()
                .map(|keyword| item.to_string().to_lowercase().contains(keyword))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    if query
        .get("sort_order")
        .map(|value| value.eq_ignore_ascii_case("asc"))
        .unwrap_or(false)
    {
        filtered.reverse();
    }

    let total = filtered.len() as i64;
    let start = ((page - 1) * page_size) as usize;
    let page_items = filtered
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .collect();
    paginated(page_items, total, page, page_size)
}

fn default_email_templates() -> HashMap<(String, String), Value> {
    let mut templates = HashMap::new();
    for event in [
        "verify_email",
        "reset_password",
        "notification_email.verify_code",
        "balance_low",
    ] {
        for locale in ["zh-CN", "en-US"] {
            templates.insert(
                (event.to_owned(), locale.to_owned()),
                official_email_template(event, locale),
            );
        }
    }
    templates
}

fn email_template_events() -> Value {
    json!([
        {
            "value": "verify_email",
            "label": "Verify Email",
            "description": "Verification code email",
            "category": "auth",
            "optional": false
        },
        {
            "value": "reset_password",
            "label": "Reset Password",
            "description": "Password reset email",
            "category": "auth",
            "optional": false
        },
        {
            "value": "notification_email.verify_code",
            "label": "Notification Email Verification",
            "description": "Extra notification email verification code",
            "category": "auth",
            "optional": false
        },
        {
            "value": "balance_low",
            "label": "Balance Low",
            "description": "Low balance notification",
            "category": "notification",
            "optional": true
        }
    ])
}

fn email_template_placeholder_union() -> Value {
    json!([
        "site_name",
        "code",
        "username",
        "reset_url",
        "balance",
        "recharge_url"
    ])
}

fn normalize_template_key(event: &str, locale: &str) -> Result<(String, String), ApiError> {
    let event = event.trim();
    let locale = locale.trim();
    if event.is_empty() {
        return Err(ApiError::bad_request("event is required"));
    }
    if locale.is_empty() {
        return Err(ApiError::bad_request("locale is required"));
    }
    Ok((event.to_owned(), locale.to_owned()))
}

fn official_email_template(event: &str, locale: &str) -> Value {
    let (subject, html, placeholders) = match event {
        "reset_password" => (
            "[{{site_name}}] Reset your password",
            "<p>Hello {{username}}, reset your password: {{reset_url}}</p>",
            vec!["site_name", "username", "reset_url"],
        ),
        "balance_low" => (
            "[{{site_name}}] Balance reminder",
            "<p>Your balance is {{balance}}. Recharge at {{recharge_url}}</p>",
            vec!["site_name", "balance", "recharge_url"],
        ),
        "notification_email.verify_code" => (
            "[{{site_name}}] Notification Email Verification",
            "<p>Your notification email verification code is {{code}}</p>",
            vec!["site_name", "code"],
        ),
        _ => (
            "[{{site_name}}] Verification code",
            "<p>Your verification code is {{code}}</p>",
            vec!["site_name", "code"],
        ),
    };
    json!({
        "event": event,
        "locale": locale,
        "subject": subject,
        "html": html,
        "is_custom": false,
        "updated_at": "",
        "placeholders": placeholders
    })
}

fn render_template_text(input: &str, variables: &serde_json::Map<String, Value>) -> String {
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

fn mask_admin_api_key(key: &str) -> String {
    let chars = key.chars().collect::<Vec<_>>();
    if chars.len() <= 12 {
        return "****".to_owned();
    }
    let prefix = chars.iter().take(8).collect::<String>();
    let suffix = chars
        .iter()
        .skip(chars.len().saturating_sub(4))
        .collect::<String>();
    format!("{prefix}...{suffix}")
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

fn first_non_empty_str(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn default_payment_config() -> Value {
    json!({
        "enabled": true,
        "min_amount": 1.0,
        "max_amount": 10000.0,
        "daily_limit": 100000.0,
        "order_timeout_minutes": 60,
        "max_pending_orders": 5,
        "enabled_payment_types": ["alipay", "wxpay", "stripe"],
        "balance_disabled": false,
        "balance_recharge_multiplier": 1.0,
        "load_balance_strategy": "priority",
        "product_name_prefix": "",
        "product_name_suffix": "",
        "help_image_url": "",
        "help_text": ""
    })
}

fn account_binding_from_json(group_id: i64, item: &Value) -> Option<AccountGroupBinding> {
    account_binding_from_admin_json(group_id, item)
}

pub fn group_from_admin_json(item: &Value) -> Option<Group> {
    let id = item.get("id").and_then(Value::as_i64)?;
    Some(Group {
        id: GroupId(id),
        name: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Group")
            .to_owned(),
        status: parse_group_status(
            item.get("status")
                .and_then(Value::as_str)
                .unwrap_or("active"),
        ),
    })
}

pub fn account_binding_from_admin_json(group_id: i64, item: &Value) -> Option<AccountGroupBinding> {
    let id = item.get("id").and_then(Value::as_i64)?;
    let provider = parse_provider(
        item.get("provider")
            .or_else(|| item.get("platform"))
            .and_then(Value::as_str)
            .unwrap_or("openai"),
    );
    let upstream_protocol = item
        .get("upstream_protocol")
        .and_then(Value::as_str)
        .map(parse_upstream_protocol)
        .unwrap_or_else(|| default_upstream_protocol(provider));
    let supported_downstream_protocols = item
        .get("supported_downstream_protocols")
        .or_else(|| item.get("supported_protocols"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(parse_downstream_protocol)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(AccountGroupBinding {
        account: Account {
            id: AccountId(id),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("upstream account")
                .to_owned(),
            provider,
            default_upstream_protocol: upstream_protocol,
            base_url: item
                .get("base_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            api_key: item
                .get("api_key")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            model_mapping: parse_model_mapping(item.get("model_mapping")),
            extra: item.get("extra").cloned().unwrap_or(Value::Null),
            enabled: item
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    !matches!(
                        item.get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("active")
                            .trim()
                            .to_ascii_lowercase()
                            .as_str(),
                        "disabled" | "inactive" | "deleted"
                    )
                }),
        },
        group_id: GroupId(group_id),
        supported_downstream_protocols,
        upstream_protocol_override: item
            .get("upstream_protocol_override")
            .and_then(Value::as_str)
            .map(parse_upstream_protocol),
        priority: item
            .get("priority")
            .or_else(|| item.get("sort_order"))
            .and_then(Value::as_i64)
            .unwrap_or(50) as i32,
    })
}

fn parse_group_status(value: &str) -> GroupStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "disabled" | "inactive" => GroupStatus::Disabled,
        "deleted" => GroupStatus::Deleted,
        _ => GroupStatus::Active,
    }
}

fn parse_provider(value: &str) -> Provider {
    match value.trim().to_ascii_lowercase().as_str() {
        "deepseek" => Provider::DeepSeek,
        "anthropic" | "claude" => Provider::Anthropic,
        "gemini" | "google" => Provider::Gemini,
        "vertex" => Provider::Vertex,
        "antigravity" => Provider::Antigravity,
        _ => Provider::OpenAi,
    }
}

fn default_upstream_protocol(provider: Provider) -> UpstreamProtocol {
    match provider {
        Provider::Anthropic | Provider::Antigravity => UpstreamProtocol::AnthropicMessages,
        Provider::Gemini | Provider::Vertex => UpstreamProtocol::GeminiGenerateContent,
        Provider::DeepSeek => UpstreamProtocol::OpenAiChatCompletions,
        Provider::OpenAi => UpstreamProtocol::OpenAiResponses,
    }
}

fn parse_upstream_protocol(value: &str) -> UpstreamProtocol {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai_chat_completions" | "chat_completions" | "chat" => {
            UpstreamProtocol::OpenAiChatCompletions
        }
        "anthropic_messages" | "messages" | "claude" => UpstreamProtocol::AnthropicMessages,
        "gemini_generate_content" | "gemini" => UpstreamProtocol::GeminiGenerateContent,
        _ => UpstreamProtocol::OpenAiResponses,
    }
}

fn parse_downstream_protocol(value: &str) -> Option<DownstreamProtocol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai_responses" | "responses" => Some(DownstreamProtocol::OpenAiResponses),
        "openai_chat_completions" | "chat_completions" | "chat" => {
            Some(DownstreamProtocol::OpenAiChatCompletions)
        }
        "anthropic_messages" | "messages" => Some(DownstreamProtocol::AnthropicMessages),
        "gemini_generate_content" | "gemini" => Some(DownstreamProtocol::GeminiGenerateContent),
        "openai_embeddings" | "embeddings" => Some(DownstreamProtocol::OpenAiEmbeddings),
        "openai_images" | "images" => Some(DownstreamProtocol::OpenAiImages),
        _ => None,
    }
}

fn parse_model_mapping(value: Option<&Value>) -> Vec<ModelMappingRule> {
    value
        .and_then(Value::as_object)
        .map(|mapping| {
            let mut rules = mapping
                .iter()
                .filter_map(|(source, target)| {
                    let source = source.trim();
                    let target = target.as_str()?.trim();
                    (!source.is_empty() && !target.is_empty()).then(|| ModelMappingRule {
                        source: source.to_owned(),
                        target: target.to_owned(),
                    })
                })
                .collect::<Vec<_>>();
            rules.sort_by(|left, right| left.source.cmp(&right.source));
            rules
        })
        .unwrap_or_default()
}

fn parse_channel_model_mapping(value: Option<&Value>, platform: &str) -> Vec<ModelMappingRule> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(object) = value.as_object() {
        if object.values().any(Value::is_object) {
            return parse_model_mapping(object.get(platform));
        }
    }
    parse_model_mapping(Some(value))
}

fn builtin_model_pricing(model: &str) -> Option<Value> {
    let (input, output) = match model {
        "gpt-5.4" | "gpt-5.5" => (0.00000125, 0.00001),
        "gpt-4.1" => (0.000002, 0.000008),
        "claude-sonnet-4-5" => (0.000003, 0.000015),
        "deepseek-chat" => (0.00000014, 0.00000028),
        _ => return None,
    };
    Some(json!({
        "found": true,
        "input_price": input,
        "output_price": output,
        "cache_write_price": null,
        "cache_read_price": null,
        "image_output_price": null
    }))
}

fn pricing_from_value(value: &Value) -> Option<ModelPricing> {
    let input_price = numeric_field(value, &["input_price", "input_price_per_token"])?;
    let output_price = numeric_field(value, &["output_price", "output_price_per_token"])?;
    Some(ModelPricing {
        input_price,
        output_price,
        cache_write_price: numeric_field(value, &["cache_write_price"])
            .or_else(|| numeric_field(value, &["cache_creation_price"]))
            .unwrap_or(input_price),
        cache_read_price: numeric_field(value, &["cache_read_price"]).unwrap_or(input_price),
    })
}

fn numeric_field(value: &Value, fields: &[&str]) -> Option<f64> {
    fields.iter().find_map(|field| {
        value.get(*field).and_then(|raw| {
            raw.as_f64()
                .or_else(|| raw.as_str()?.trim().parse::<f64>().ok())
        })
    })
}

fn account_temp_unschedulable_status(id: i64, account: &Value) -> Value {
    let until = account
        .get("temp_unschedulable_until")
        .or_else(|| account.get("temp_unsched_until"))
        .cloned()
        .unwrap_or(Value::Null);
    let reason = account
        .get("temp_unschedulable_reason")
        .or_else(|| account.get("temp_unsched_reason"))
        .cloned()
        .unwrap_or(Value::Null);
    let active = !until.is_null()
        && until
            .as_str()
            .map(|value| !value.is_empty())
            .unwrap_or_else(|| until.as_i64().is_some());
    if !active {
        return json!({
            "active": false,
            "account_id": id,
            "state": null
        });
    }
    json!({
        "active": true,
        "account_id": id,
        "state": {
            "until": until,
            "until_unix": account.get("temp_unschedulable_until_unix").cloned().unwrap_or(Value::Null),
            "triggered_at_unix": account.get("temp_unschedulable_triggered_at_unix").cloned().unwrap_or(Value::Null),
            "status_code": account.get("temp_unschedulable_status_code").cloned().unwrap_or(Value::Null),
            "matched_keyword": account.get("temp_unschedulable_matched_keyword").cloned().unwrap_or(Value::Null),
            "rule_index": account.get("temp_unschedulable_rule_index").cloned().unwrap_or(Value::Null),
            "error_message": reason
        }
    })
}

fn admin_oauth_provider_from_path(path: &str) -> String {
    let parts = path.split('/').collect::<Vec<_>>();
    if let Some(index) = parts.iter().position(|part| *part == "admin") {
        return parts
            .get(index + 1)
            .copied()
            .unwrap_or("unknown")
            .to_owned();
    }
    "unknown".to_owned()
}

fn channel_group_ids(channel: &Value) -> Vec<i64> {
    channel
        .get("group_ids")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

fn default_backup_s3_config() -> Value {
    json!({
        "endpoint": "",
        "region": "auto",
        "bucket": "",
        "access_key_id": "",
        "secret_access_key": "",
        "secret_access_key_configured": false,
        "prefix": "backups/",
        "force_path_style": false
    })
}

fn default_backup_schedule() -> Value {
    json!({
        "enabled": false,
        "cron_expr": "0 3 * * *",
        "retain_days": 7,
        "retain_count": 10
    })
}

fn default_data_management_config() -> Value {
    json!({
        "source_mode": "direct",
        "backup_root": "data/backups",
        "sqlite_path": null,
        "retention_days": 7,
        "keep_last": 10,
        "active_postgres_profile_id": null,
        "active_redis_profile_id": null,
        "active_s3_profile_id": null,
        "postgres": {
            "host": "127.0.0.1",
            "port": 5432,
            "user": "sub2api",
            "password": "",
            "password_configured": false,
            "database": "sub2api",
            "ssl_mode": "disable",
            "container_name": "postgres"
        },
        "redis": {
            "addr": "127.0.0.1:6379",
            "username": "",
            "password": "",
            "password_configured": false,
            "db": 0,
            "container_name": "redis"
        },
        "s3": {
            "enabled": false,
            "endpoint": "",
            "region": "auto",
            "bucket": "",
            "access_key_id": "",
            "secret_access_key": "",
            "secret_access_key_configured": false,
            "prefix": "backups/",
            "force_path_style": false,
            "use_ssl": true
        }
    })
}

fn default_source_config(source_type: &str) -> Value {
    match source_type {
        "redis" => json!({
            "host": "",
            "port": 0,
            "user": "",
            "password": "",
            "database": "",
            "ssl_mode": "",
            "addr": "127.0.0.1:6379",
            "username": "",
            "db": 0,
            "container_name": "redis"
        }),
        _ => json!({
            "host": "127.0.0.1",
            "port": 5432,
            "user": "sub2api",
            "password": "",
            "database": "sub2api",
            "ssl_mode": "disable",
            "addr": "",
            "username": "",
            "db": 0,
            "container_name": "postgres"
        }),
    }
}

fn s3_profile_config_from_payload(payload: &Value) -> Value {
    json!({
        "enabled": payload.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        "endpoint": payload.get("endpoint").and_then(Value::as_str).unwrap_or(""),
        "region": payload.get("region").and_then(Value::as_str).unwrap_or("auto"),
        "bucket": payload.get("bucket").and_then(Value::as_str).unwrap_or(""),
        "access_key_id": payload.get("access_key_id").and_then(Value::as_str).unwrap_or(""),
        "secret_access_key": payload.get("secret_access_key").and_then(Value::as_str).unwrap_or(""),
        "secret_access_key_configured": payload.get("secret_access_key").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false),
        "prefix": payload.get("prefix").and_then(Value::as_str).unwrap_or("backups/"),
        "force_path_style": payload.get("force_path_style").and_then(Value::as_bool).unwrap_or(false),
        "use_ssl": payload.get("use_ssl").and_then(Value::as_bool).unwrap_or(true)
    })
}

fn set_active_profile(items: &mut [Value], active_profile_id: &str) {
    for item in items {
        item["is_active"] =
            json!(item.get("profile_id").and_then(Value::as_str) == Some(active_profile_id));
    }
}

fn default_risk_control_config() -> Value {
    json!({
        "enabled": false,
        "mode": "off",
        "base_url": "https://api.openai.com",
        "model": "omni-moderation-latest",
        "api_key_configured": false,
        "api_key_masked": "",
        "api_key_count": 0,
        "api_key_masks": [],
        "api_key_statuses": [],
        "timeout_ms": 5000,
        "sample_rate": 1.0,
        "all_groups": true,
        "group_ids": [],
        "record_non_hits": false,
        "thresholds": {},
        "worker_count": 1,
        "queue_size": 1000,
        "block_status": 400,
        "block_message": "content moderation blocked this request",
        "email_on_hit": false,
        "auto_ban_enabled": false,
        "ban_threshold": 3,
        "violation_window_hours": 24,
        "retry_count": 0,
        "hit_retention_days": 30,
        "non_hit_retention_days": 7,
        "pre_hash_check_enabled": false,
        "blocked_keywords": [],
        "keyword_blocking_mode": "keyword_and_api",
        "model_filter": {
            "type": "all",
            "models": []
        }
    })
}

fn moderation_api_key_status(index: usize, configured: bool) -> Value {
    json!({
        "index": index,
        "key_hash": if configured { format!("key-{index}") } else { String::new() },
        "masked": if configured { format!("sk-***{index}") } else { String::new() },
        "status": if configured { "ok" } else { "unknown" },
        "failure_count": 0,
        "success_count": if configured { 1 } else { 0 },
        "last_error": "",
        "last_checked_at": if configured { json!(NOW) } else { Value::Null },
        "frozen_until": null,
        "last_latency_ms": 0,
        "last_http_status": if configured { 200 } else { 0 },
        "last_tested": configured,
        "configured": configured
    })
}

fn normalize_collection_name(name: &str) -> String {
    match name {
        "payment/channels" => "payment-channels".to_owned(),
        "payment/plans" => "payment-plans".to_owned(),
        "payment/providers" => "payment-providers".to_owned(),
        "payment/orders" => "payment-orders".to_owned(),
        value => value.trim_matches('/').to_owned(),
    }
}

fn validate_payment_provider_payload(item: &Value) -> Result<(), ApiError> {
    let provider = payment_provider_key(item);
    if provider.trim().is_empty() {
        return Err(ApiError::bad_request("provider_key is required"));
    }
    if !matches!(
        provider.as_str(),
        "easypay" | "alipay" | "wxpay" | "stripe" | "airwallex"
    ) {
        return Err(ApiError::bad_request(format!(
            "invalid provider key: {provider}"
        )));
    }
    if item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(ApiError::bad_request("provider name is required"));
    }
    Ok(())
}

fn ensure_payment_provider_shape(item: &mut Value) {
    let provider = payment_provider_key(item);
    item["provider_key"] = json!(provider.clone());
    item["payment_type"] = json!(provider.clone());
    if item.get("provider_instance_id").is_none() {
        item["provider_instance_id"] = json!(format!("{provider}-default"));
    }
    if item.get("supported_types").is_none() {
        item["supported_types"] = json!([provider]);
    }
    if item.get("enabled").is_none() {
        item["enabled"] = json!(true);
    }
    if item.get("refund_enabled").is_none() {
        item["refund_enabled"] = json!(false);
    }
    if item.get("allow_user_refund").is_none() {
        item["allow_user_refund"] = json!(false);
    }
    if item.get("payment_mode").is_none() {
        item["payment_mode"] = json!("redirect");
    }
    if !item.get("config").map(Value::is_object).unwrap_or(false) {
        item["config"] = json!({});
    }
}

fn merge_payment_provider_update(target: &mut Value, update: Value) {
    let provider = payment_provider_key(target);
    if let Some(config_update) = update.get("config").and_then(Value::as_object) {
        if !target.get("config").map(Value::is_object).unwrap_or(false) {
            target["config"] = json!({});
        }
        let current_config = target
            .get_mut("config")
            .and_then(Value::as_object_mut)
            .expect("provider config object");
        for (key, value) in config_update {
            let empty_sensitive = is_sensitive_payment_provider_config(&provider, key)
                && value.as_str().map(str::trim).unwrap_or_default().is_empty();
            if !empty_sensitive {
                current_config.insert(key.clone(), value.clone());
            }
        }
    }
    let mut update_without_config = update;
    if let Some(object) = update_without_config.as_object_mut() {
        object.remove("config");
    }
    merge_json(target, update_without_config);
    ensure_payment_provider_shape(target);
}

fn mask_payment_provider_config(item: &mut Value) {
    ensure_payment_provider_shape(item);
    let provider = payment_provider_key(item);
    if let Some(config) = item.get_mut("config").and_then(Value::as_object_mut) {
        let sensitive = config
            .keys()
            .filter(|key| is_sensitive_payment_provider_config(&provider, key))
            .cloned()
            .collect::<Vec<_>>();
        for key in sensitive {
            config.remove(&key);
        }
    }
}

fn payment_provider_key(item: &Value) -> String {
    item.get("provider_key")
        .or_else(|| item.get("payment_type"))
        .and_then(Value::as_str)
        .map(normalize_payment_provider_key)
        .unwrap_or_default()
}

fn normalize_payment_provider_key(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "wechat" | "weixin" | "wx" | "wxpay_direct" => "wxpay".to_owned(),
        "alipay_direct" => "alipay".to_owned(),
        other => other.to_owned(),
    }
}

fn is_sensitive_payment_provider_config(provider: &str, field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    match provider {
        "easypay" => matches!(field.as_str(), "pkey"),
        "alipay" => matches!(
            field.as_str(),
            "privatekey" | "publickey" | "alipaypublickey"
        ),
        "wxpay" => matches!(field.as_str(), "privatekey" | "apiv3key" | "publickey"),
        "stripe" => matches!(field.as_str(), "secretkey" | "webhooksecret"),
        "airwallex" => matches!(field.as_str(), "apikey" | "webhooksecret"),
        _ => false,
    }
}

fn is_protected_payment_provider_config(provider: &str, field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    match provider {
        "easypay" => matches!(field.as_str(), "pkey" | "pid"),
        "alipay" => matches!(
            field.as_str(),
            "privatekey" | "publickey" | "alipaypublickey" | "appid"
        ),
        "wxpay" => matches!(
            field.as_str(),
            "privatekey"
                | "apiv3key"
                | "publickey"
                | "appid"
                | "mpappid"
                | "mchid"
                | "publickeyid"
                | "certserial"
        ),
        "stripe" => matches!(field.as_str(), "secretkey" | "webhooksecret" | "currency"),
        "airwallex" => matches!(
            field.as_str(),
            "clientid" | "apikey" | "webhooksecret" | "apibase" | "accountid" | "currency"
        ),
        _ => false,
    }
}

fn payment_provider_has_protected_change(current: &Value, next: &Value) -> bool {
    if current.get("enabled").and_then(Value::as_bool) == Some(true)
        && next.get("enabled").and_then(Value::as_bool) == Some(false)
    {
        return true;
    }
    if supported_types_removed(current, next) {
        return true;
    }
    let provider = payment_provider_key(current);
    let current_config = current.get("config").and_then(Value::as_object);
    let Some(next_config) = next.get("config").and_then(Value::as_object) else {
        return false;
    };
    for key in next_config.keys() {
        if is_protected_payment_provider_config(&provider, key) {
            let current_value = current_config
                .and_then(|config| config.get(key))
                .cloned()
                .unwrap_or(Value::Null);
            let next_value = next_config.get(key).cloned().unwrap_or(Value::Null);
            if current_value != next_value {
                return true;
            }
        }
    }
    false
}

fn supported_types_removed(current: &Value, next: &Value) -> bool {
    let current_types = payment_provider_supported_types(current);
    let next_types = payment_provider_supported_types(next);
    current_types
        .iter()
        .any(|value| !next_types.iter().any(|next| next == value))
}

fn payment_provider_supported_types(item: &Value) -> Vec<String> {
    item.get("supported_types")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_payment_provider_key)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec![payment_provider_key(item)])
}

fn has_pending_provider_orders(
    collections: &RwLock<HashMap<String, Vec<Value>>>,
    provider_id: i64,
    pending_provider_instance_ids: &[String],
) -> bool {
    let provider_id_string = provider_id.to_string();
    let provider_item = collections
        .read()
        .expect("admin collection lock")
        .get("payment-providers")
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item_id(item) == Some(provider_id));
    let provider_instance_id = provider_item
        .as_ref()
        .and_then(|item| item.get("provider_instance_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            provider_item
                .as_ref()
                .map(payment_provider_key)
                .filter(|provider| !provider.is_empty())
                .map(|provider| format!("{provider}-default"))
        });
    let provider_id_matches = pending_provider_instance_ids
        .iter()
        .any(|pending| pending == &provider_id_string);
    let has_runtime_pending = provider_instance_id
        .as_ref()
        .map(|provider_instance_id| {
            pending_provider_instance_ids
                .iter()
                .any(|pending| pending == provider_instance_id)
        })
        .unwrap_or(false);
    if has_runtime_pending || provider_id_matches {
        return true;
    }
    collections
        .read()
        .expect("admin collection lock")
        .get("payment-orders")
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .any(|order| {
            matches!(
                order.get("status").and_then(Value::as_str),
                Some("PENDING" | "PAID" | "RECHARGING")
            ) && (order.get("provider_instance_id").and_then(Value::as_i64) == Some(provider_id)
                || order.get("provider_instance_id").and_then(Value::as_str)
                    == Some(provider_id_string.as_str()))
        })
}

fn proxy_to_data_proxy(item: Value) -> Value {
    let protocol = item
        .get("protocol")
        .or_else(|| item.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("direct");
    let host = item
        .get("host")
        .and_then(Value::as_str)
        .or_else(|| item.get("url").and_then(Value::as_str))
        .unwrap_or("");
    let port = item.get("port").and_then(Value::as_i64).unwrap_or(0);
    let username = item.get("username").and_then(Value::as_str).unwrap_or("");
    let password = item.get("password").and_then(Value::as_str).unwrap_or("");
    let proxy_key = item
        .get("proxy_key")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| build_proxy_key(protocol, host, port, username, password));
    json!({
        "proxy_key": proxy_key,
        "name": item.get("name").and_then(Value::as_str).unwrap_or("Proxy"),
        "protocol": protocol,
        "host": host,
        "port": port,
        "username": username,
        "password": password,
        "status": item.get("status").and_then(Value::as_str).unwrap_or("active")
    })
}

fn account_to_data_account(item: Value) -> Value {
    json!({
        "name": item.get("name").and_then(Value::as_str).unwrap_or("Account"),
        "notes": item.get("notes").cloned().unwrap_or(Value::Null),
        "platform": item.get("platform").and_then(Value::as_str).unwrap_or("openai"),
        "type": item.get("type").and_then(Value::as_str).unwrap_or("api-key"),
        "credentials": item.get("credentials").cloned().unwrap_or_else(|| {
            json!({
                "api_key": item.get("api_key").and_then(Value::as_str).unwrap_or(""),
                "base_url": item.get("base_url").and_then(Value::as_str).unwrap_or("")
            })
        }),
        "extra": item.get("extra").cloned().unwrap_or_else(|| json!({})),
        "proxy_key": item.get("proxy_key").cloned().unwrap_or(Value::Null),
        "concurrency": item.get("concurrency").and_then(Value::as_i64).unwrap_or(1),
        "priority": item.get("priority").and_then(Value::as_i64).unwrap_or(1),
        "rate_multiplier": item.get("rate_multiplier").cloned().unwrap_or(Value::Null),
        "expires_at": item.get("expires_at").cloned().unwrap_or(Value::Null),
        "auto_pause_on_expired": item.get("auto_pause_on_expired").cloned().unwrap_or(json!(true))
    })
}

fn data_proxy_key(item: &Value) -> String {
    item.get("proxy_key")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            build_proxy_key(
                item.get("protocol").and_then(Value::as_str).unwrap_or(""),
                item.get("host").and_then(Value::as_str).unwrap_or(""),
                item.get("port").and_then(Value::as_i64).unwrap_or(0),
                item.get("username").and_then(Value::as_str).unwrap_or(""),
                item.get("password").and_then(Value::as_str).unwrap_or(""),
            )
        })
}

fn build_proxy_key(
    protocol: &str,
    host: &str,
    port: i64,
    username: &str,
    password: &str,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        protocol.trim(),
        host.trim(),
        port,
        username.trim(),
        password.trim()
    )
}

fn query_i64(query: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    query
        .get(key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn payload_ids(payload: &Value, key: &str) -> Result<Vec<i64>, ApiError> {
    let ids = payload
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request(format!("{key} is required")))?
        .iter()
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| ApiError::bad_request(format!("{key} must contain integers")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(ApiError::bad_request(format!("{key} cannot be empty")));
    }
    Ok(ids)
}

fn payload_without_ids(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Value::Object(object) = &mut value {
        object.remove("ids");
        object.remove("user_ids");
        object.remove("account_ids");
    }
    value
}

fn paginated(items: Vec<Value>, total: i64, page: i64, page_size: i64) -> Value {
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (page_size as f64)).ceil() as i64
    };
    json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": total_pages,
        "total_pages": total_pages
    })
}

fn collection_len(collections: &HashMap<String, Vec<Value>>, name: &str) -> usize {
    collections.get(name).map(Vec::len).unwrap_or(0)
}

fn item_id(item: &Value) -> Option<i64> {
    item.get("id").and_then(Value::as_i64)
}

fn set_id(item: &mut Value, id: i64) {
    if let Value::Object(object) = item {
        object.insert("id".to_owned(), json!(id));
    }
}

fn ensure_timestamp(item: &mut Value) {
    if let Value::Object(object) = item {
        object.insert("updated_at".to_owned(), json!(NOW));
    }
}

fn merge_json(target: &mut Value, update: Value) {
    match (target, update) {
        (Value::Object(target), Value::Object(update)) => {
            for (key, value) in update {
                if key == "id" {
                    continue;
                }
                match (target.get_mut(&key), value) {
                    (Some(existing @ Value::Object(_)), Value::Object(next)) => {
                        merge_json(existing, Value::Object(next));
                    }
                    (_, value) => {
                        target.insert(key, value);
                    }
                }
            }
            target.insert("updated_at".to_owned(), json!(NOW));
        }
        (target, update) => {
            *target = update;
        }
    }
}
