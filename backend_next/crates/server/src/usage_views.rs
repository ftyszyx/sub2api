use chrono::{Duration, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use repository::{UsageFilter, UsageRecord};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

const NOW: &str = "2026-06-06T00:00:00Z";

trait JsonItem {
    fn value(&self) -> &Value;
}

impl JsonItem for Value {
    fn value(&self) -> &Value {
        self
    }
}

impl JsonItem for &Value {
    fn value(&self) -> &Value {
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageReferenceData {
    pub users: HashMap<i64, UsageUserReference>,
    pub api_keys: HashMap<i64, UsageApiKeyReference>,
    pub groups: HashMap<i64, UsageGroupReference>,
    pub accounts: HashMap<i64, UsageAccountReference>,
    pub subscriptions: HashMap<(i64, i64), UsageSubscriptionReference>,
}

#[derive(Debug, Clone)]
pub struct UsageUserReference {
    pub id: i64,
    pub email: String,
    pub username: String,
    pub role: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct UsageApiKeyReference {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct UsageGroupReference {
    pub id: i64,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct UsageAccountReference {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct UsageSubscriptionReference {
    pub id: i64,
    pub user_id: i64,
    pub group_id: i64,
    pub status: String,
    pub starts_at: String,
    pub expires_at: String,
}

pub fn filter_from_query(
    user_id: Option<i64>,
    api_key_id: Option<i64>,
    query: &HashMap<String, String>,
) -> UsageFilter {
    let mut filter = UsageFilter::all();
    filter.user_id = user_id;
    filter.api_key_id = api_key_id.map(domain::ApiKeyId);
    filter.group_id = query
        .get("group_id")
        .and_then(|value| value.parse::<i64>().ok())
        .map(domain::GroupId);
    filter.account_id = query
        .get("account_id")
        .and_then(|value| value.parse::<i64>().ok())
        .map(domain::AccountId);
    filter.downstream_protocol = query
        .get("downstream_protocol")
        .and_then(|value| protocol::DownstreamProtocol::from_str(value).ok());
    filter.model_contains = query
        .get("model")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    filter.request_type = query
        .get("request_type")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    filter.stream = query
        .get("stream")
        .and_then(|value| value.parse::<bool>().ok());
    filter.status = query
        .get("status")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    filter.billing_mode = query
        .get("billing_mode")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    filter.billing_type = query
        .get("billing_type")
        .and_then(|value| value.parse::<i8>().ok());
    if let Some((start, end)) = usage_time_range(query) {
        filter.created_at_unix_gte = Some(start);
        filter.created_at_unix_lt = Some(end);
    }
    filter
}

pub fn has_usage_records(records: &[UsageRecord]) -> bool {
    !records.is_empty()
}

pub fn usage_list(records: &[UsageRecord], query: &HashMap<String, String>) -> Value {
    usage_list_with_references(records, query, None)
}

pub fn usage_list_with_references(
    records: &[UsageRecord],
    query: &HashMap<String, String>,
    references: Option<&UsageReferenceData>,
) -> Value {
    let page = query_i64(query, "page", 1).max(1);
    let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
    let mut items = filtered_records(records, query)
        .into_iter()
        .map(|record| usage_record_json_with_references(record, references))
        .collect::<Vec<_>>();
    sort_usage_items(&mut items, query);
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

pub fn admin_usage_stats(records: &[UsageRecord], query: &HashMap<String, String>) -> Value {
    let items = filtered_usage_items(records, query);
    aggregate_usage_stats(&items)
}

pub fn admin_user_usage_stats(records: &[UsageRecord]) -> Value {
    let input_tokens = records
        .iter()
        .map(|record| record.input_tokens)
        .sum::<i64>();
    let output_tokens = records
        .iter()
        .map(|record| record.output_tokens)
        .sum::<i64>();
    let cache_creation_tokens = records
        .iter()
        .map(|record| record.cache_creation_tokens)
        .sum::<i64>();
    let cache_read_tokens = records
        .iter()
        .map(|record| record.cache_read_tokens)
        .sum::<i64>();
    let actual_cost = records.iter().map(|record| record.actual_cost).sum::<f64>();
    json!({
        "total_requests": records.len() as i64,
        "total_cost": actual_cost,
        "total_tokens": input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_creation_tokens": cache_creation_tokens,
        "cache_read_tokens": cache_read_tokens,
        "actual_cost": actual_cost
    })
}

pub fn current_user_usage_stats(records: &[UsageRecord]) -> Value {
    let items = records.iter().map(usage_record_json).collect::<Vec<_>>();
    let refs = items.iter().collect::<Vec<_>>();
    json!({
        "period": "today",
        "total_requests": records.len() as i64,
        "total_input_tokens": sum_i64(&refs, "input_tokens"),
        "total_output_tokens": sum_i64(&refs, "output_tokens"),
        "total_cache_tokens": sum_i64(&refs, "cache_creation_tokens") + sum_i64(&refs, "cache_read_tokens"),
        "total_tokens": sum_i64(&refs, "total_tokens"),
        "total_cost": sum_f64(&refs, "cost"),
        "total_actual_cost": sum_f64(&refs, "actual_cost"),
        "average_duration_ms": average_i64(&refs, "duration_ms"),
        "models": usage_model_stats_from_items(&refs)
    })
}

pub fn user_usage_dashboard_stats(records: &[UsageRecord], api_key_count: i64) -> Value {
    let items = records.iter().map(usage_record_json).collect::<Vec<_>>();
    let refs = items.iter().collect::<Vec<_>>();
    let (today_start, today_end) = today_time_window();
    let today_items = records
        .iter()
        .filter(|record| {
            record.created_at_unix >= today_start && record.created_at_unix < today_end
        })
        .map(usage_record_json)
        .collect::<Vec<_>>();
    let today_refs = today_items.iter().collect::<Vec<_>>();
    let api_key_ids = records
        .iter()
        .map(|record| record.api_key_id)
        .collect::<HashSet<_>>();
    let active_api_keys = if api_key_count > 0 {
        api_key_count
    } else {
        api_key_ids.len() as i64
    };
    json!({
        "total_api_keys": api_key_count.max(api_key_ids.len() as i64),
        "active_api_keys": active_api_keys,
        "total_requests": records.len() as i64,
        "total_input_tokens": sum_i64(&refs, "input_tokens"),
        "total_output_tokens": sum_i64(&refs, "output_tokens"),
        "total_cache_creation_tokens": sum_i64(&refs, "cache_creation_tokens"),
        "total_cache_read_tokens": sum_i64(&refs, "cache_read_tokens"),
        "total_tokens": sum_i64(&refs, "total_tokens"),
        "total_cost": sum_f64(&refs, "cost"),
        "total_actual_cost": sum_f64(&refs, "actual_cost"),
        "today_requests": today_refs.len() as i64,
        "today_input_tokens": sum_i64(&today_refs, "input_tokens"),
        "today_output_tokens": sum_i64(&today_refs, "output_tokens"),
        "today_cache_creation_tokens": sum_i64(&today_refs, "cache_creation_tokens"),
        "today_cache_read_tokens": sum_i64(&today_refs, "cache_read_tokens"),
        "today_tokens": sum_i64(&today_refs, "total_tokens"),
        "today_cost": sum_f64(&today_refs, "cost"),
        "today_actual_cost": sum_f64(&today_refs, "actual_cost"),
        "average_duration_ms": average_i64(&refs, "duration_ms"),
        "rpm": 0,
        "tpm": 0,
        "by_platform": usage_user_platform_stats_from_items(&refs, &today_refs)
    })
}

pub fn user_usage_dashboard_trend(
    records: &[UsageRecord],
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let range = dashboard_time_range(query);
    let timezone = query_timezone(query);
    let granularity = query
        .get("granularity")
        .cloned()
        .unwrap_or_else(|| "day".to_owned());
    json!({
        "trend": usage_trend_from_items(&refs, trend_bucket_granularity(&granularity), timezone),
        "start_date": range.start_date,
        "end_date": range.end_date,
        "granularity": granularity
    })
}

pub fn user_usage_dashboard_models(
    records: &[UsageRecord],
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let range = dashboard_time_range(query);
    json!({
        "models": usage_model_stats_from_items(&refs),
        "start_date": range.start_date,
        "end_date": range.end_date
    })
}

pub fn admin_dashboard_stats(
    records: &[UsageRecord],
    today_records: &[UsageRecord],
    total_users: i64,
    today_new_users: i64,
    total_api_keys: i64,
    active_api_keys: i64,
    total_accounts: i64,
    normal_accounts: i64,
    error_accounts: i64,
    ratelimit_accounts: i64,
    overload_accounts: i64,
    total_groups: i64,
) -> Value {
    let items = usage_items(records);
    let refs = items.iter().collect::<Vec<_>>();
    let today_items = usage_items(today_records);
    let today_refs = today_items.iter().collect::<Vec<_>>();
    let active_users = today_records
        .iter()
        .map(|record| record.user_id)
        .collect::<HashSet<_>>()
        .len() as i64;
    let hourly_active_users = today_records
        .iter()
        .filter(|record| record.created_at_unix >= current_hour_start_unix())
        .map(|record| record.user_id)
        .collect::<HashSet<_>>()
        .len() as i64;
    json!({
        "total_users": total_users,
        "today_new_users": today_new_users,
        "active_users": active_users,
        "hourly_active_users": hourly_active_users,
        "stats_updated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "stats_stale": false,
        "total_api_keys": total_api_keys,
        "active_api_keys": active_api_keys,
        "total_accounts": total_accounts,
        "active_accounts": normal_accounts,
        "normal_accounts": normal_accounts,
        "error_accounts": error_accounts,
        "ratelimit_accounts": ratelimit_accounts,
        "overload_accounts": overload_accounts,
        "total_groups": total_groups,
        "total_requests": refs.len() as i64,
        "total_input_tokens": sum_i64(&refs, "input_tokens"),
        "total_output_tokens": sum_i64(&refs, "output_tokens"),
        "total_cache_creation_tokens": sum_i64(&refs, "cache_creation_tokens"),
        "total_cache_read_tokens": sum_i64(&refs, "cache_read_tokens"),
        "total_cache_tokens": sum_i64(&refs, "cache_creation_tokens") + sum_i64(&refs, "cache_read_tokens"),
        "total_tokens": sum_i64(&refs, "total_tokens"),
        "total_cost": sum_f64(&refs, "cost"),
        "total_actual_cost": sum_f64(&refs, "actual_cost"),
        "total_account_cost": sum_f64(&refs, "account_cost"),
        "today_requests": today_refs.len() as i64,
        "today_input_tokens": sum_i64(&today_refs, "input_tokens"),
        "today_output_tokens": sum_i64(&today_refs, "output_tokens"),
        "today_cache_creation_tokens": sum_i64(&today_refs, "cache_creation_tokens"),
        "today_cache_read_tokens": sum_i64(&today_refs, "cache_read_tokens"),
        "today_tokens": sum_i64(&today_refs, "total_tokens"),
        "today_cost": sum_f64(&today_refs, "cost"),
        "today_actual_cost": sum_f64(&today_refs, "actual_cost"),
        "today_account_cost": sum_f64(&today_refs, "account_cost"),
        "average_duration_ms": average_i64(&refs, "duration_ms"),
        "uptime": 0,
        "rpm": 0,
        "tpm": 0
    })
}

pub fn admin_dashboard_trend(records: &[UsageRecord], query: &HashMap<String, String>) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let range = dashboard_time_range(query);
    let timezone = query_timezone(query);
    let granularity = query
        .get("granularity")
        .cloned()
        .unwrap_or_else(|| "day".to_owned());
    json!({
        "trend": usage_trend_from_items(&refs, trend_bucket_granularity(&granularity), timezone),
        "start_date": range.start_date,
        "end_date": range.end_date,
        "granularity": granularity
    })
}

pub fn admin_dashboard_models(records: &[UsageRecord], query: &HashMap<String, String>) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let range = dashboard_time_range(query);
    json!({
        "models": usage_model_stats_from_items_by_source(&refs, dashboard_model_source(query)),
        "start_date": range.start_date,
        "end_date": range.end_date
    })
}

pub fn admin_dashboard_groups(
    records: &[UsageRecord],
    group_names: &HashMap<i64, String>,
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let range = dashboard_time_range(query);
    json!({
        "groups": usage_group_stats_from_items(&refs, group_names),
        "start_date": range.start_date,
        "end_date": range.end_date
    })
}

pub fn admin_dashboard_snapshot_v2(
    records: &[UsageRecord],
    group_names: &HashMap<i64, String>,
    stats: Value,
    query: &HashMap<String, String>,
) -> Value {
    let include_stats = query_bool(query, "include_stats", true);
    let include_trend = query_bool(query, "include_trend", true);
    let include_model_stats = query_bool(query, "include_model_stats", true);
    let include_group_stats = query_bool(query, "include_group_stats", false);
    let include_users_trend = query_bool(query, "include_users_trend", false);
    let range = dashboard_time_range(query);
    let timezone = query_timezone(query);
    let granularity = snapshot_granularity(query);
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let mut payload = Map::new();
    payload.insert("generated_at".to_owned(), json!(Utc::now().to_rfc3339()));
    payload.insert("start_date".to_owned(), json!(range.start_date));
    payload.insert("end_date".to_owned(), json!(range.end_date));
    payload.insert("granularity".to_owned(), json!(granularity.clone()));
    if include_stats {
        payload.insert("stats".to_owned(), stats);
    }
    if include_trend {
        payload.insert(
            "trend".to_owned(),
            json!(usage_trend_from_items(&refs, &granularity, timezone)),
        );
    }
    if include_model_stats {
        payload.insert(
            "models".to_owned(),
            json!(usage_model_stats_from_items_by_source(
                &refs,
                dashboard_model_source(query)
            )),
        );
    }
    if include_group_stats {
        payload.insert(
            "groups".to_owned(),
            json!(usage_group_stats_from_items(&refs, group_names)),
        );
    }
    if include_users_trend {
        payload.insert(
            "users_trend".to_owned(),
            json!(usage_user_trend_from_items(
                &refs,
                &HashMap::new(),
                users_trend_limit(query)
            )),
        );
    }
    Value::Object(payload)
}

pub fn admin_dashboard_api_keys_trend(
    records: &[UsageRecord],
    api_key_names: &HashMap<i64, String>,
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    json!({
        "trend": usage_api_key_trend_from_items(&refs, api_key_names, query_limit(query, 10)),
        "start_date": query_date(query, "start_date"),
        "end_date": query_date(query, "end_date"),
        "granularity": query.get("granularity").cloned().unwrap_or_else(|| "day".to_owned())
    })
}

pub fn admin_dashboard_users_trend(
    records: &[UsageRecord],
    users: &HashMap<i64, (String, String)>,
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    json!({
        "trend": usage_user_trend_from_items(&refs, users, query_limit(query, 10)),
        "start_date": query_date(query, "start_date"),
        "end_date": query_date(query, "end_date"),
        "granularity": query.get("granularity").cloned().unwrap_or_else(|| "day".to_owned())
    })
}

pub fn admin_dashboard_users_ranking(
    records: &[UsageRecord],
    users: &HashMap<i64, (String, String)>,
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    let ranking = usage_user_ranking_from_items(&refs, users, query_limit(query, 10));
    json!({
        "ranking": ranking,
        "total_actual_cost": sum_f64(&refs, "actual_cost"),
        "total_requests": refs.len() as i64,
        "total_tokens": sum_i64(&refs, "total_tokens"),
        "start_date": query_date(query, "start_date"),
        "end_date": query_date(query, "end_date")
    })
}

pub fn admin_dashboard_users_usage(records: &[UsageRecord], user_ids: &[i64]) -> Value {
    let items = usage_items(records);
    let stats = user_ids
        .iter()
        .map(|id| {
            let user_items = items
                .iter()
                .filter(|item| item.get("user_id").and_then(Value::as_i64) == Some(*id))
                .collect::<Vec<_>>();
            (
                id.to_string(),
                json!({
                    "user_id": id,
                    "today_actual_cost": sum_f64(&user_items, "actual_cost"),
                    "total_actual_cost": sum_f64(&user_items, "actual_cost"),
                    "today_requests": user_items.len() as i64,
                    "total_requests": user_items.len() as i64,
                    "today_tokens": sum_i64(&user_items, "total_tokens"),
                    "total_tokens": sum_i64(&user_items, "total_tokens"),
                    "by_platform": usage_platform_stats_from_items(&user_items)
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({ "stats": stats })
}

pub fn admin_dashboard_api_keys_usage(records: &[UsageRecord], api_key_ids: &[i64]) -> Value {
    let items = usage_items(records);
    let stats = api_key_ids
        .iter()
        .map(|id| {
            let key_items = items
                .iter()
                .filter(|item| item.get("api_key_id").and_then(Value::as_i64) == Some(*id))
                .collect::<Vec<_>>();
            (
                id.to_string(),
                json!({
                    "api_key_id": id,
                    "today_actual_cost": sum_f64(&key_items, "actual_cost"),
                    "total_actual_cost": sum_f64(&key_items, "actual_cost"),
                    "today_requests": key_items.len() as i64,
                    "total_requests": key_items.len() as i64,
                    "today_tokens": sum_i64(&key_items, "total_tokens"),
                    "total_tokens": sum_i64(&key_items, "total_tokens")
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({ "stats": stats })
}

pub fn admin_dashboard_user_breakdown(
    records: &[UsageRecord],
    users: &HashMap<i64, (String, String)>,
    query: &HashMap<String, String>,
) -> Value {
    let items = filtered_usage_items(records, query);
    let refs = items.iter().collect::<Vec<_>>();
    json!({
        "users": usage_user_breakdown_from_items(&refs, users, query_limit(query, 30)),
        "start_date": query_date(query, "start_date"),
        "end_date": query_date(query, "end_date")
    })
}

pub fn admin_account_stats(records: &[UsageRecord], days: i64) -> Value {
    let items = usage_items(records);
    let refs = items.iter().collect::<Vec<_>>();
    let history = account_daily_history_from_items(&refs);
    let actual_days_used = history.len() as i64;
    let total_requests = refs.len() as i64;
    let total_tokens = sum_i64(&refs, "total_tokens");
    let total_cost = sum_f64(&refs, "account_cost");
    let total_user_cost = sum_f64(&refs, "actual_cost");
    let total_standard_cost = sum_f64(&refs, "cost");
    let highest_cost_day = history.iter().max_by(|left, right| {
        left.get("cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(&right.get("cost").and_then(Value::as_f64).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let highest_request_day = history.iter().max_by_key(|item| {
        item.get("requests")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    });
    let day_divisor = actual_days_used.max(1) as f64;
    let request_divisor = actual_days_used.max(1);
    json!({
        "total_requests": total_requests,
        "total_cost": total_cost,
        "total_tokens": total_tokens,
        "history": history,
        "summary": {
            "days": days,
            "actual_days_used": actual_days_used,
            "total_cost": total_cost,
            "total_user_cost": total_user_cost,
            "total_standard_cost": total_standard_cost,
            "total_requests": total_requests,
            "total_tokens": total_tokens,
            "avg_daily_cost": total_cost / day_divisor,
            "avg_daily_user_cost": total_user_cost / day_divisor,
            "avg_daily_requests": total_requests / request_divisor,
            "avg_daily_tokens": total_tokens / request_divisor,
            "avg_duration_ms": average_i64(&refs, "duration_ms"),
            "highest_cost_day": highest_cost_day.cloned(),
            "highest_request_day": highest_request_day.cloned()
        },
        "today": admin_account_today_stats(records),
        "models": usage_model_stats_from_items(&refs),
        "endpoints": usage_endpoint_stats_from_items(&refs, "endpoint"),
        "upstream_endpoints": usage_endpoint_stats_from_items(&refs, "upstream_endpoint")
    })
}

pub fn admin_account_usage(records: &[UsageRecord], query: &HashMap<String, String>) -> Value {
    usage_list(records, query)
}

pub fn admin_account_usage_info(records: &[UsageRecord], source: Option<&str>) -> Value {
    let window_stats = admin_account_today_stats(records);
    let requests = window_stats
        .get("requests")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let window_progress = json!({
        "utilization": 0,
        "resets_at": null,
        "remaining_seconds": 0,
        "window_stats": window_stats,
        "used_requests": requests,
        "limit_requests": null
    });
    json!({
        "source": source.unwrap_or("passive"),
        "updated_at": NOW,
        "five_hour": window_progress,
        "seven_day": window_progress,
        "seven_day_sonnet": null,
        "gemini_shared_daily": null,
        "gemini_pro_daily": null,
        "gemini_flash_daily": null,
        "gemini_shared_minute": null,
        "gemini_pro_minute": null,
        "gemini_flash_minute": null,
        "antigravity_quota": null,
        "ai_credits": null,
        "is_forbidden": false,
        "needs_verify": false,
        "is_banned": false,
        "needs_reauth": false,
        "error_code": null,
        "error": null
    })
}

pub fn admin_account_today_stats(records: &[UsageRecord]) -> Value {
    let items = usage_items(records);
    let refs = items.iter().collect::<Vec<_>>();
    json!({
        "requests": refs.len() as i64,
        "tokens": sum_i64(&refs, "total_tokens"),
        "cost": sum_f64(&refs, "account_cost"),
        "standard_cost": sum_f64(&refs, "cost"),
        "user_cost": sum_f64(&refs, "actual_cost")
    })
}

pub fn admin_batch_account_today_stats(records: &[UsageRecord], account_ids: &[i64]) -> Value {
    let stats = account_ids
        .iter()
        .map(|id| {
            let account_records = records
                .iter()
                .filter(|record| record.account_id.map(|account_id| account_id.0) == Some(*id))
                .cloned()
                .collect::<Vec<_>>();
            (id.to_string(), admin_account_today_stats(&account_records))
        })
        .collect::<serde_json::Map<_, _>>();
    json!({ "stats": stats })
}

pub fn user_api_keys_usage_stats(
    user_id: i64,
    api_key_ids: &[i64],
    records: &[UsageRecord],
) -> Value {
    let (today_start, today_end) = today_time_window();
    let stats = api_key_ids
        .iter()
        .map(|id| {
            let items = records
                .iter()
                .filter(|record| record.user_id == user_id && record.api_key_id == *id)
                .map(usage_record_json)
                .collect::<Vec<_>>();
            let refs = items.iter().collect::<Vec<_>>();
            let today_items = items
                .iter()
                .filter(|item| {
                    item.get("created_at_unix")
                        .and_then(Value::as_i64)
                        .map(|created_at| created_at >= today_start && created_at < today_end)
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            (
                id.to_string(),
                json!({
                    "api_key_id": id,
                    "today_actual_cost": sum_f64(&today_items, "actual_cost"),
                    "total_actual_cost": sum_f64(&refs, "actual_cost"),
                    "today_requests": today_items.len() as i64,
                    "total_requests": items.len() as i64,
                    "today_tokens": sum_i64(&today_items, "total_tokens"),
                    "total_tokens": sum_i64(&refs, "total_tokens")
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    json!({ "stats": stats })
}

pub fn user_api_key_daily_usage(records: &[UsageRecord], days: i64) -> Value {
    let items = records.iter().map(usage_record_json).collect::<Vec<_>>();
    let refs = items.iter().collect::<Vec<_>>();
    let today = Utc::now().date_naive();
    let clamped_days = days.clamp(1, 90);
    let start_date = today - Duration::days(clamped_days - 1);
    json!({
        "items": usage_api_key_daily_from_items(&refs),
        "days": clamped_days,
        "start_date": start_date.format("%Y-%m-%d").to_string(),
        "end_date": today.format("%Y-%m-%d").to_string()
    })
}

pub fn user_usage_by_id(user_id: i64, id: i64, records: &[UsageRecord]) -> Option<Value> {
    user_usage_by_id_with_references(user_id, id, records, None)
}

pub fn user_usage_by_id_with_references(
    user_id: i64,
    id: i64,
    records: &[UsageRecord],
    references: Option<&UsageReferenceData>,
) -> Option<Value> {
    records
        .iter()
        .find(|record| record.id == id && record.user_id == user_id)
        .map(|record| usage_record_json_with_references(record, references))
}

fn filtered_records<'a>(
    records: &'a [UsageRecord],
    query: &HashMap<String, String>,
) -> Vec<&'a UsageRecord> {
    let model = query.get("model").map(|value| value.to_lowercase());
    let request_type = query.get("request_type").map(|value| value.to_lowercase());
    let stream = query
        .get("stream")
        .and_then(|value| value.parse::<bool>().ok());
    records
        .iter()
        .filter(|record| {
            model
                .as_ref()
                .map(|needle| {
                    record.requested_model.to_lowercase().contains(needle)
                        || record.upstream_model.to_lowercase().contains(needle)
                        || record
                            .metadata
                            .get("model_mapping_chain")
                            .map(|value| value.to_string().to_lowercase().contains(needle))
                            .unwrap_or(false)
                })
                .unwrap_or(true)
        })
        .filter(|record| {
            request_type
                .as_ref()
                .map(|needle| {
                    metadata_str(&record.metadata, "request_type") == Some(needle.as_str())
                })
                .unwrap_or(true)
        })
        .filter(|record| {
            stream
                .map(|value| metadata_bool(&record.metadata, "stream") == Some(value))
                .unwrap_or(true)
        })
        .collect()
}

fn usage_time_range(query: &HashMap<String, String>) -> Option<(i64, i64)> {
    let start = query
        .get("start_date")
        .and_then(|value| parse_date_start(value));
    let end = query
        .get("end_date")
        .and_then(|value| parse_date_start(value))
        .map(|value| value + 24 * 60 * 60);
    match (start, end) {
        (Some(start), Some(end)) => Some((start, end.max(start))),
        (Some(start), None) => Some((start, i64::MAX)),
        (None, Some(end)) => Some((0, end)),
        (None, None) => period_time_range(query.get("period").map(String::as_str))
            .or_else(|| dashboard_default_time_range(query)),
    }
}

fn dashboard_default_time_range(query: &HashMap<String, String>) -> Option<(i64, i64)> {
    if !query_contains_dashboard_param(query) {
        return None;
    }
    let today = Utc::now().date_naive();
    Some((
        date_start_unix(today - Duration::days(7))?,
        date_start_unix(today + Duration::days(1))?,
    ))
}

fn query_contains_dashboard_param(query: &HashMap<String, String>) -> bool {
    [
        "granularity",
        "model_source",
        "include_stats",
        "include_trend",
        "include_model_stats",
        "include_group_stats",
        "include_users_trend",
        "users_trend_limit",
    ]
    .iter()
    .any(|key| query.contains_key(*key))
}

fn period_time_range(period: Option<&str>) -> Option<(i64, i64)> {
    let today = NaiveDate::from_ymd_opt(2026, 6, 6)?;
    let start = match period.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "today" => today,
        "week" => today - Duration::days(6),
        "month" => today - Duration::days(29),
        _ => return None,
    };
    Some((
        date_start_unix(start)?,
        date_start_unix(today + Duration::days(1))?,
    ))
}

fn parse_date_start(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    date_start_unix(date)
}

fn date_start_unix(date: NaiveDate) -> Option<i64> {
    let dt = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&dt).timestamp())
}

fn sort_usage_items(items: &mut [Value], query: &HashMap<String, String>) {
    let sort_by = query
        .get("sort_by")
        .map(String::as_str)
        .unwrap_or("created_at");
    let desc = !matches!(
        query.get("sort_order").map(|value| value.to_ascii_lowercase()),
        Some(value) if value == "asc"
    );
    items.sort_by(|left, right| {
        let ordering = match sort_by {
            "id" => value_i64(left, "id").cmp(&value_i64(right, "id")),
            "input_tokens" | "prompt_tokens" => {
                value_i64(left, "input_tokens").cmp(&value_i64(right, "input_tokens"))
            }
            "output_tokens" | "completion_tokens" => {
                value_i64(left, "output_tokens").cmp(&value_i64(right, "output_tokens"))
            }
            "total_tokens" => {
                value_i64(left, "total_tokens").cmp(&value_i64(right, "total_tokens"))
            }
            "cost" | "total_cost" => value_f64(left, "cost").total_cmp(&value_f64(right, "cost")),
            "actual_cost" => {
                value_f64(left, "actual_cost").total_cmp(&value_f64(right, "actual_cost"))
            }
            "duration_ms" => value_i64(left, "duration_ms").cmp(&value_i64(right, "duration_ms")),
            "model" | "requested_model" => value_str(left, "model").cmp(value_str(right, "model")),
            _ => value_i64(left, "created_at_unix").cmp(&value_i64(right, "created_at_unix")),
        };
        if desc {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn value_i64(item: &Value, field: &str) -> i64 {
    item.get(field).and_then(Value::as_i64).unwrap_or_default()
}

fn value_f64(item: &Value, field: &str) -> f64 {
    item.get(field).and_then(Value::as_f64).unwrap_or_default()
}

fn value_str<'a>(item: &'a Value, field: &str) -> &'a str {
    item.get(field).and_then(Value::as_str).unwrap_or_default()
}

fn usage_record_json(record: &UsageRecord) -> Value {
    usage_record_json_with_references(record, None)
}

fn usage_record_json_with_references(
    record: &UsageRecord,
    references: Option<&UsageReferenceData>,
) -> Value {
    let total_tokens = record.input_tokens
        + record.output_tokens
        + record.cache_creation_tokens
        + record.cache_read_tokens;
    let user_ref = references.and_then(|refs| refs.users.get(&record.user_id));
    let api_key_ref = references.and_then(|refs| refs.api_keys.get(&record.api_key_id));
    let group_ref =
        references.and_then(|refs| record.group_id.and_then(|id| refs.groups.get(&id.0)));
    let account_ref =
        references.and_then(|refs| record.account_id.and_then(|id| refs.accounts.get(&id.0)));
    let subscription_ref = references.and_then(|refs| {
        record
            .group_id
            .and_then(|group_id| refs.subscriptions.get(&(record.user_id, group_id.0)))
    });
    let api_key_name = api_key_ref
        .map(|key| json!(key.name.clone()))
        .or_else(|| record.metadata.get("api_key_name").cloned())
        .unwrap_or(Value::Null);
    let request_type = record
        .metadata
        .get("request_type")
        .cloned()
        .unwrap_or_else(|| json!("unknown"));
    let stream = record
        .metadata
        .get("stream")
        .cloned()
        .unwrap_or_else(|| json!(false));
    let model_mapping_chain = record
        .metadata
        .get("model_mapping_chain")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let cost = record
        .metadata
        .get("cost")
        .and_then(Value::as_f64)
        .unwrap_or(record.actual_cost);
    let total_cost = record
        .metadata
        .get("total_cost")
        .and_then(Value::as_f64)
        .unwrap_or(record.actual_cost);
    let account_cost = record
        .metadata
        .get("account_cost")
        .and_then(Value::as_f64)
        .unwrap_or(record.actual_cost);
    let upstream_endpoint = record
        .metadata
        .get("upstream_endpoint")
        .and_then(Value::as_str)
        .unwrap_or(record.endpoint.as_str());
    let input_cost = metadata_f64(&record.metadata, "input_cost");
    let output_cost = metadata_f64(&record.metadata, "output_cost");
    let cache_creation_cost = metadata_f64(&record.metadata, "cache_creation_cost");
    let cache_read_cost = metadata_f64(&record.metadata, "cache_read_cost");
    let billing_mode = record
        .metadata
        .get("billing_mode")
        .and_then(Value::as_str)
        .unwrap_or("token");
    let billing_type = record
        .metadata
        .get("billing_type")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let rate_multiplier = record
        .metadata
        .get("rate_multiplier")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let account_rate_multiplier = record
        .metadata
        .get("account_rate_multiplier")
        .and_then(Value::as_f64);
    let account_stats_cost = record
        .metadata
        .get("account_stats_cost")
        .and_then(Value::as_f64);
    let channel_id = record.metadata.get("channel_id").and_then(Value::as_i64);
    let billing_tier = record
        .metadata
        .get("billing_tier")
        .cloned()
        .unwrap_or(Value::Null);
    let ip_address = record
        .metadata
        .get("ip_address")
        .cloned()
        .unwrap_or(Value::Null);
    let user_agent = record
        .metadata
        .get("user_agent")
        .cloned()
        .unwrap_or(Value::Null);
    let first_token_ms = record
        .metadata
        .get("first_token_ms")
        .cloned()
        .unwrap_or(Value::Null);
    let account = usage_account_json(record, account_ref);
    let mut out = Map::new();
    out.insert("id".to_owned(), json!(record.id));
    out.insert(
        "request_id".to_owned(),
        json!(format!("repository-usage-{}", record.id)),
    );
    out.insert("user_id".to_owned(), json!(record.user_id));
    out.insert(
        "user_email".to_owned(),
        user_ref
            .map(|user| json!(user.email.clone()))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "user".to_owned(),
        user_ref.map(usage_user_json).unwrap_or(Value::Null),
    );
    out.insert("api_key_id".to_owned(), json!(record.api_key_id));
    out.insert("api_key_name".to_owned(), api_key_name);
    out.insert(
        "api_key".to_owned(),
        api_key_ref.map(usage_api_key_json).unwrap_or(Value::Null),
    );
    out.insert(
        "account_id".to_owned(),
        record
            .account_id
            .map(|id| json!(id.0))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "group_id".to_owned(),
        record.group_id.map(|id| json!(id.0)).unwrap_or(Value::Null),
    );
    out.insert(
        "group".to_owned(),
        group_ref.map(usage_group_json).unwrap_or(Value::Null),
    );
    out.insert("provider".to_owned(), json!(record.provider));
    out.insert("platform".to_owned(), json!(record.provider));
    out.insert(
        "downstream_protocol".to_owned(),
        json!(record.downstream_protocol.as_str()),
    );
    out.insert(
        "upstream_protocol".to_owned(),
        json!(record.upstream_protocol),
    );
    out.insert("endpoint".to_owned(), json!(record.endpoint));
    out.insert("inbound_endpoint".to_owned(), json!(record.endpoint));
    out.insert("request_type".to_owned(), request_type);
    out.insert("model".to_owned(), json!(record.requested_model));
    out.insert("requested_model".to_owned(), json!(record.requested_model));
    out.insert("upstream_model".to_owned(), json!(record.upstream_model));
    out.insert("model_mapping_chain".to_owned(), model_mapping_chain);
    out.insert("stream".to_owned(), stream);
    out.insert("input_tokens".to_owned(), json!(record.input_tokens));
    out.insert("prompt_tokens".to_owned(), json!(record.input_tokens));
    out.insert("output_tokens".to_owned(), json!(record.output_tokens));
    out.insert("completion_tokens".to_owned(), json!(record.output_tokens));
    out.insert(
        "cache_creation_tokens".to_owned(),
        json!(record.cache_creation_tokens),
    );
    out.insert(
        "cache_read_tokens".to_owned(),
        json!(record.cache_read_tokens),
    );
    out.insert("total_tokens".to_owned(), json!(total_tokens));
    out.insert("input_cost".to_owned(), json!(input_cost));
    out.insert("output_cost".to_owned(), json!(output_cost));
    out.insert("cache_creation_cost".to_owned(), json!(cache_creation_cost));
    out.insert("cache_read_cost".to_owned(), json!(cache_read_cost));
    out.insert("total_cost".to_owned(), json!(total_cost));
    out.insert("cost".to_owned(), json!(cost));
    out.insert("actual_cost".to_owned(), json!(record.actual_cost));
    out.insert("rate_multiplier".to_owned(), json!(rate_multiplier));
    out.insert("account_cost".to_owned(), json!(account_cost));
    out.insert(
        "account_rate_multiplier".to_owned(),
        account_rate_multiplier
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "account_stats_cost".to_owned(),
        account_stats_cost
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
    );
    out.insert("status".to_owned(), json!(record.status));
    out.insert("upstream_endpoint".to_owned(), json!(upstream_endpoint));
    out.insert("billing_mode".to_owned(), json!(billing_mode));
    out.insert("billing_type".to_owned(), json!(billing_type));
    out.insert("billing_tier".to_owned(), billing_tier);
    out.insert(
        "channel_id".to_owned(),
        channel_id.map(|value| json!(value)).unwrap_or(Value::Null),
    );
    out.insert("ip_address".to_owned(), ip_address);
    out.insert("account".to_owned(), account);
    out.insert(
        "subscription".to_owned(),
        subscription_ref
            .map(usage_subscription_json)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "duration_ms".to_owned(),
        json!(record
            .metadata
            .get("duration_ms")
            .and_then(Value::as_i64)
            .unwrap_or(0)),
    );
    out.insert("first_token_ms".to_owned(), first_token_ms);
    out.insert(
        "openai_ws_mode".to_owned(),
        json!(record
            .metadata
            .get("openai_ws_mode")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
    );
    out.insert(
        "cache_ttl_overridden".to_owned(),
        json!(record
            .metadata
            .get("cache_ttl_overridden")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
    );
    out.insert(
        "image_count".to_owned(),
        json!(record
            .metadata
            .get("image_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)),
    );
    out.insert(
        "image_size".to_owned(),
        record
            .metadata
            .get("image_size")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "image_input_size".to_owned(),
        record
            .metadata
            .get("image_input_size")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "image_output_size".to_owned(),
        record
            .metadata
            .get("image_output_size")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "image_size_source".to_owned(),
        record
            .metadata
            .get("image_size_source")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "image_size_breakdown".to_owned(),
        record
            .metadata
            .get("image_size_breakdown")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "media_type".to_owned(),
        record
            .metadata
            .get("media_type")
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert("user_agent".to_owned(), user_agent);
    out.insert("created_at".to_owned(), json!(NOW));
    out.insert("created_at_unix".to_owned(), json!(record.created_at_unix));
    Value::Object(out)
}

fn usage_user_json(user: &UsageUserReference) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
        "status": user.status
    })
}

fn usage_api_key_json(api_key: &UsageApiKeyReference) -> Value {
    json!({
        "id": api_key.id,
        "user_id": api_key.user_id,
        "name": api_key.name,
        "group_id": api_key.group_id,
        "status": api_key.status
    })
}

fn usage_group_json(group: &UsageGroupReference) -> Value {
    json!({
        "id": group.id,
        "name": group.name,
        "status": group.status
    })
}

fn usage_account_json(record: &UsageRecord, account: Option<&UsageAccountReference>) -> Value {
    if let Some(account) = account {
        return json!({
            "id": account.id,
            "name": account.name,
            "provider": account.provider,
            "platform": account.provider,
            "status": account.status
        });
    }
    record
        .account_id
        .map(|id| {
            json!({
                "id": id.0,
                "name": record
                    .metadata
                    .get("account_name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown Account")
            })
        })
        .unwrap_or(Value::Null)
}

fn usage_subscription_json(subscription: &UsageSubscriptionReference) -> Value {
    json!({
        "id": subscription.id,
        "user_id": subscription.user_id,
        "group_id": subscription.group_id,
        "status": subscription.status,
        "starts_at": subscription.starts_at,
        "expires_at": subscription.expires_at
    })
}

fn usage_items(records: &[UsageRecord]) -> Vec<Value> {
    records.iter().map(usage_record_json).collect()
}

fn filtered_usage_items(records: &[UsageRecord], query: &HashMap<String, String>) -> Vec<Value> {
    filtered_records(records, query)
        .into_iter()
        .map(usage_record_json)
        .collect()
}

fn aggregate_usage_stats(items: &[Value]) -> Value {
    let refs = items.iter().collect::<Vec<_>>();
    let success_requests = refs
        .iter()
        .filter(|item| {
            !matches!(
                item.get("status").and_then(Value::as_str),
                Some("failed" | "error")
            )
        })
        .count() as i64;
    let failed_requests = refs.len() as i64 - success_requests;
    json!({
        "total_requests": refs.len() as i64,
        "total_input_tokens": sum_i64(&refs, "input_tokens"),
        "total_output_tokens": sum_i64(&refs, "output_tokens"),
        "total_cache_tokens": sum_i64(&refs, "cache_creation_tokens") + sum_i64(&refs, "cache_read_tokens"),
        "total_cache_creation_tokens": sum_i64(&refs, "cache_creation_tokens"),
        "total_cache_read_tokens": sum_i64(&refs, "cache_read_tokens"),
        "total_tokens": sum_i64(&refs, "total_tokens"),
        "total_cost": sum_f64(&refs, "cost"),
        "total_actual_cost": sum_f64(&refs, "actual_cost"),
        "total_account_cost": sum_f64(&refs, "account_cost"),
        "success_requests": success_requests,
        "failed_requests": failed_requests,
        "average_duration_ms": average_i64(&refs, "duration_ms"),
        "average_latency_ms": average_i64(&refs, "duration_ms"),
        "models": usage_model_stats_from_items(&refs),
        "endpoints": usage_endpoint_stats_from_items(&refs, "endpoint"),
        "upstream_endpoints": usage_endpoint_stats_from_items(&refs, "upstream_endpoint"),
        "endpoint_paths": usage_endpoint_stats_from_items(&refs, "endpoint")
    })
}

fn usage_trend_from_items(items: &[&Value], granularity: &str, timezone: Tz) -> Vec<Value> {
    let mut by_bucket: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let bucket = item
            .get("created_at_unix")
            .and_then(Value::as_i64)
            .and_then(|value| usage_bucket_from_unix(value, granularity, timezone))
            .unwrap_or_else(|| {
                if granularity == "hour" {
                    format!("{} 00:00", today())
                } else {
                    today().to_owned()
                }
            });
        by_bucket.entry(bucket).or_default().push(*item);
    }
    let mut trend = by_bucket
        .into_iter()
        .map(|(date, bucket_items)| {
            json!({
                "date": date,
                "requests": bucket_items.len() as i64,
                "input_tokens": sum_i64(&bucket_items, "input_tokens"),
                "output_tokens": sum_i64(&bucket_items, "output_tokens"),
                "cache_creation_tokens": sum_i64(&bucket_items, "cache_creation_tokens"),
                "cache_read_tokens": sum_i64(&bucket_items, "cache_read_tokens"),
                "total_tokens": sum_i64(&bucket_items, "total_tokens"),
                "cost": sum_f64(&bucket_items, "cost"),
                "actual_cost": sum_f64(&bucket_items, "actual_cost"),
                "account_cost": sum_f64(&bucket_items, "account_cost")
            })
        })
        .collect::<Vec<_>>();
    trend.sort_by(|left, right| value_str(left, "date").cmp(value_str(right, "date")));
    trend
}

fn usage_api_key_daily_from_items(items: &[&Value]) -> Vec<Value> {
    let mut by_date: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let date = item
            .get("created_at_unix")
            .and_then(Value::as_i64)
            .and_then(utc_date_from_unix)
            .unwrap_or_else(|| today().to_owned());
        by_date.entry(date).or_default().push(*item);
    }
    let mut daily = by_date
        .into_iter()
        .map(|(date, day_items)| {
            json!({
                "date": date,
                "requests": day_items.len() as i64,
                "input_tokens": sum_i64(&day_items, "input_tokens"),
                "output_tokens": sum_i64(&day_items, "output_tokens"),
                "cache_read_tokens": sum_i64(&day_items, "cache_read_tokens"),
                "cache_write_tokens": sum_i64(&day_items, "cache_creation_tokens"),
                "total_tokens": sum_i64(&day_items, "total_tokens"),
                "cost": sum_f64(&day_items, "cost"),
                "actual_cost": sum_f64(&day_items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    daily.sort_by(|left, right| value_str(left, "date").cmp(value_str(right, "date")));
    daily
}

fn usage_bucket_from_unix(value: i64, granularity: &str, timezone: Tz) -> Option<String> {
    let dt = Utc
        .timestamp_opt(value, 0)
        .single()?
        .with_timezone(&timezone);
    Some(match granularity {
        "hour" => dt.format("%Y-%m-%d %H:00").to_string(),
        "week" => dt.format("%G-%V").to_string(),
        "month" => dt.format("%Y-%m").to_string(),
        _ => dt.format("%Y-%m-%d").to_string(),
    })
}

fn account_daily_history_from_items(items: &[&Value]) -> Vec<Value> {
    let mut by_date: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let date = item
            .get("created_at_unix")
            .and_then(Value::as_i64)
            .and_then(utc_date_from_unix)
            .unwrap_or_else(|| today().to_owned());
        by_date.entry(date).or_default().push(*item);
    }
    let mut history = by_date
        .into_iter()
        .map(|(date, day_items)| {
            json!({
                "date": date,
                "label": date,
                "requests": day_items.len() as i64,
                "tokens": sum_i64(&day_items, "total_tokens"),
                "cost": sum_f64(&day_items, "account_cost"),
                "actual_cost": sum_f64(&day_items, "account_cost"),
                "standard_cost": sum_f64(&day_items, "cost"),
                "user_cost": sum_f64(&day_items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    history.sort_by(|left, right| value_str(left, "date").cmp(value_str(right, "date")));
    history
}

fn utc_date_from_unix(value: i64) -> Option<String> {
    Utc.timestamp_opt(value, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

fn usage_model_stats_from_items(items: &[&Value]) -> Vec<Value> {
    usage_model_stats_from_items_by_source(items, "requested")
}

fn usage_model_stats_from_items_by_source(items: &[&Value], source: &str) -> Vec<Value> {
    let mut by_model: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let model = model_value_by_source(item, source);
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
                "actual_cost": sum_f64(&items, "actual_cost"),
                "account_cost": sum_f64(&items, "account_cost")
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

fn model_value_by_source(item: &Value, source: &str) -> String {
    match source {
        "upstream" => item
            .get("upstream_model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_owned(),
        "mapping" => {
            let requested = item
                .get("model")
                .or_else(|| item.get("requested_model"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("unknown");
            let upstream = item
                .get("upstream_model")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| mapping_model_value(item))
                .unwrap_or_else(|| requested.to_owned());
            format!("{requested} -> {upstream}")
        }
        _ => item
            .get("model")
            .or_else(|| item.get("requested_model"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_owned(),
    }
}

fn mapping_model_value(item: &Value) -> Option<String> {
    let chain = item.get("model_mapping_chain")?.as_array()?;
    chain.iter().rev().find_map(|step| {
        if let Some(target) = step
            .get("target")
            .or_else(|| step.get("to"))
            .or_else(|| step.get("mapped_to"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(target.to_owned());
        }
        if let Some(model) = step
            .get("model")
            .or_else(|| step.get("upstream_model"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(model.to_owned());
        }
        None
    })
}

fn usage_group_stats_from_items(
    items: &[&Value],
    group_names: &HashMap<i64, String>,
) -> Vec<Value> {
    let mut by_group: HashMap<i64, Vec<&Value>> = HashMap::new();
    for item in items {
        let group_id = item.get("group_id").and_then(Value::as_i64).unwrap_or(0);
        by_group.entry(group_id).or_default().push(*item);
    }
    let mut stats = by_group
        .into_iter()
        .map(|(group_id, items)| {
            json!({
                "group_id": group_id,
                "group_name": group_names
                    .get(&group_id)
                    .cloned()
                    .unwrap_or_else(|| format!("Group {group_id}")),
                "requests": items.len() as i64,
                "total_tokens": sum_i64(&items, "total_tokens"),
                "cost": sum_f64(&items, "cost"),
                "actual_cost": sum_f64(&items, "actual_cost"),
                "account_cost": sum_f64(&items, "account_cost")
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by_key(|item| {
        item.get("group_id")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    });
    stats
}

fn usage_endpoint_stats_from_items(items: &[&Value], field: &str) -> Vec<Value> {
    let mut by_endpoint: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let endpoint = item
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        by_endpoint.entry(endpoint).or_default().push(*item);
    }
    let mut stats = by_endpoint
        .into_iter()
        .map(|(endpoint, items)| {
            json!({
                "endpoint": endpoint,
                "requests": items.len() as i64,
                "total_tokens": sum_i64(&items, "total_tokens"),
                "cost": sum_f64(&items, "cost"),
                "actual_cost": sum_f64(&items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        left.get("endpoint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    stats
}

fn usage_api_key_trend_from_items(
    items: &[&Value],
    api_key_names: &HashMap<i64, String>,
    limit: usize,
) -> Vec<Value> {
    let mut by_key: HashMap<i64, Vec<&Value>> = HashMap::new();
    for item in items {
        let api_key_id = item
            .get("api_key_id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        by_key.entry(api_key_id).or_default().push(*item);
    }
    let mut trend = by_key
        .into_iter()
        .map(|(api_key_id, items)| {
            json!({
                "date": today(),
                "api_key_id": api_key_id,
                "key_name": api_key_names
                    .get(&api_key_id)
                    .cloned()
                    .or_else(|| items.first().and_then(|item| {
                        item.get("api_key_name")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    }))
                    .unwrap_or_else(|| format!("API Key {api_key_id}")),
                "requests": items.len() as i64,
                "tokens": sum_i64(&items, "total_tokens")
            })
        })
        .collect::<Vec<_>>();
    trend.sort_by_key(|item| {
        std::cmp::Reverse(
            item.get("tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
    });
    trend.truncate(limit);
    trend
}

fn usage_user_trend_from_items(
    items: &[&Value],
    users: &HashMap<i64, (String, String)>,
    limit: usize,
) -> Vec<Value> {
    let mut by_user: HashMap<i64, Vec<&Value>> = HashMap::new();
    for item in items {
        let user_id = item
            .get("user_id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        by_user.entry(user_id).or_default().push(*item);
    }
    let mut trend = by_user
        .into_iter()
        .map(|(user_id, items)| {
            let (email, username) = users
                .get(&user_id)
                .cloned()
                .unwrap_or_else(|| (format!("user-{user_id}@local"), format!("user-{user_id}")));
            json!({
                "date": today(),
                "user_id": user_id,
                "email": email,
                "username": username,
                "requests": items.len() as i64,
                "tokens": sum_i64(&items, "total_tokens"),
                "cost": sum_f64(&items, "cost"),
                "actual_cost": sum_f64(&items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    trend.sort_by_key(|item| {
        std::cmp::Reverse(
            item.get("tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
    });
    trend.truncate(limit);
    trend
}

fn usage_user_ranking_from_items(
    items: &[&Value],
    users: &HashMap<i64, (String, String)>,
    limit: usize,
) -> Vec<Value> {
    let mut ranking = usage_user_trend_from_items(items, users, usize::MAX)
        .into_iter()
        .map(|item| {
            json!({
                "user_id": item.get("user_id").cloned().unwrap_or(Value::Null),
                "email": item.get("email").cloned().unwrap_or(Value::Null),
                "actual_cost": item.get("actual_cost").cloned().unwrap_or_else(|| json!(0.0)),
                "requests": item.get("requests").cloned().unwrap_or_else(|| json!(0)),
                "tokens": item.get("tokens").cloned().unwrap_or_else(|| json!(0))
            })
        })
        .collect::<Vec<_>>();
    ranking.sort_by(|left, right| {
        right
            .get("actual_cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(
                &left
                    .get("actual_cost")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            )
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranking.truncate(limit);
    ranking
}

fn usage_user_breakdown_from_items(
    items: &[&Value],
    users: &HashMap<i64, (String, String)>,
    limit: usize,
) -> Vec<Value> {
    let mut by_user: HashMap<i64, Vec<&Value>> = HashMap::new();
    for item in items {
        let user_id = item
            .get("user_id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        by_user.entry(user_id).or_default().push(*item);
    }
    let mut users_payload = by_user
        .into_iter()
        .map(|(user_id, items)| {
            let (email, _) = users
                .get(&user_id)
                .cloned()
                .unwrap_or_else(|| (format!("user-{user_id}@local"), format!("user-{user_id}")));
            json!({
                "user_id": user_id,
                "email": email,
                "requests": items.len() as i64,
                "total_tokens": sum_i64(&items, "total_tokens"),
                "cost": sum_f64(&items, "cost"),
                "actual_cost": sum_f64(&items, "actual_cost"),
                "account_cost": sum_f64(&items, "account_cost")
            })
        })
        .collect::<Vec<_>>();
    users_payload.sort_by_key(|item| {
        std::cmp::Reverse(
            item.get("total_tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
    });
    users_payload.truncate(limit);
    users_payload
}

fn usage_platform_stats_from_items(items: &[&Value]) -> Vec<Value> {
    let mut by_platform: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in items {
        let platform = item
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        by_platform.entry(platform).or_default().push(*item);
    }
    let mut stats = by_platform
        .into_iter()
        .map(|(platform, items)| {
            json!({
                "platform": platform,
                "today_actual_cost": sum_f64(&items, "actual_cost"),
                "total_actual_cost": sum_f64(&items, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        left.get("platform")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("platform")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    stats
}

fn usage_user_platform_stats_from_items(
    total_items: &[&Value],
    today_items: &[&Value],
) -> Vec<Value> {
    let mut by_platform: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in total_items {
        let platform = item
            .get("platform")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_owned();
        if matches!(platform.as_str(), "unknown" | "") {
            continue;
        }
        by_platform.entry(platform).or_default().push(*item);
    }
    let mut today_by_platform: HashMap<String, Vec<&Value>> = HashMap::new();
    for item in today_items {
        let platform = item
            .get("platform")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_owned();
        if matches!(platform.as_str(), "unknown" | "") {
            continue;
        }
        today_by_platform.entry(platform).or_default().push(*item);
    }
    let mut stats = by_platform
        .into_iter()
        .map(|(platform, items)| {
            let today_refs = today_by_platform.remove(&platform).unwrap_or_default();
            json!({
                "platform": platform,
                "total_requests": items.len() as i64,
                "total_tokens": sum_i64(&items, "total_tokens"),
                "total_actual_cost": sum_f64(&items, "actual_cost"),
                "today_requests": today_refs.len() as i64,
                "today_tokens": sum_i64(&today_refs, "total_tokens"),
                "today_actual_cost": sum_f64(&today_refs, "actual_cost")
            })
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        right
            .get("total_actual_cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .total_cmp(
                &left
                    .get("total_actual_cost")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            )
    });
    stats
}

#[derive(Debug, Clone)]
struct DashboardTimeRange {
    start_date: String,
    end_date: String,
}

fn dashboard_time_range(query: &HashMap<String, String>) -> DashboardTimeRange {
    let timezone = query_timezone(query);
    let today = Utc::now().with_timezone(&timezone).date_naive();
    let start = query
        .get("start_date")
        .and_then(|value| NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok())
        .unwrap_or_else(|| today - Duration::days(7));
    let end = query
        .get("end_date")
        .and_then(|value| NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok())
        .unwrap_or(today);
    DashboardTimeRange {
        start_date: start.format("%Y-%m-%d").to_string(),
        end_date: end.format("%Y-%m-%d").to_string(),
    }
}

fn snapshot_granularity(query: &HashMap<String, String>) -> String {
    if matches!(
        query.get("granularity").map(|value| value.trim()),
        Some("hour")
    ) {
        "hour".to_owned()
    } else {
        "day".to_owned()
    }
}

fn trend_bucket_granularity(granularity: &str) -> &str {
    match granularity {
        "hour" | "week" | "month" => granularity,
        _ => "day",
    }
}

fn dashboard_model_source(query: &HashMap<String, String>) -> &str {
    match query.get("model_source").map(|value| value.trim()) {
        Some("upstream") => "upstream",
        Some("mapping") => "mapping",
        _ => "requested",
    }
}

fn query_bool(query: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match query
        .get(key)
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

fn users_trend_limit(query: &HashMap<String, String>) -> usize {
    query
        .get("users_trend_limit")
        .or_else(|| query.get("limit"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(12)
        .clamp(1, 50)
}

fn query_date(query: &HashMap<String, String>, key: &str) -> String {
    query
        .get(key)
        .cloned()
        .unwrap_or_else(|| today().to_owned())
}

fn today() -> &'static str {
    NOW.split('T').next().unwrap_or(NOW)
}

fn today_time_window() -> (i64, i64) {
    let timezone = default_timezone();
    let today = Utc::now().with_timezone(&timezone).date_naive();
    let start = date_start_unix_in_timezone(today, timezone).expect("valid current date");
    let end =
        date_start_unix_in_timezone(today + Duration::days(1), timezone).expect("valid next date");
    (start, end)
}

fn current_hour_start_unix() -> i64 {
    let timezone = default_timezone();
    let local = Utc::now().with_timezone(&timezone);
    let Some(local_hour) = local.date_naive().and_hms_opt(local.hour(), 0, 0) else {
        return Utc::now().timestamp();
    };
    match timezone.from_local_datetime(&local_hour) {
        LocalResult::Single(value) => value.with_timezone(&Utc).timestamp(),
        LocalResult::Ambiguous(value, _) => value.with_timezone(&Utc).timestamp(),
        LocalResult::None => {
            Utc::now().timestamp() - i64::from(local.minute()) * 60 - i64::from(local.second())
        }
    }
}

fn query_timezone(query: &HashMap<String, String>) -> Tz {
    query
        .get("timezone")
        .and_then(|value| value.trim().parse::<Tz>().ok())
        .unwrap_or_else(default_timezone)
}

fn default_timezone() -> Tz {
    chrono_tz::Asia::Shanghai
}

fn date_start_unix_in_timezone(date: NaiveDate, timezone: Tz) -> Option<i64> {
    let local = date.and_hms_opt(0, 0, 0)?;
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc).timestamp()),
        LocalResult::Ambiguous(value, _) => Some(value.with_timezone(&Utc).timestamp()),
        LocalResult::None => None,
    }
}

fn query_limit(query: &HashMap<String, String>, default: usize) -> usize {
    query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, 100)
}

fn query_i64(query: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    query
        .get(key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn paginated(items: Vec<Value>, total: i64, page: i64, page_size: i64) -> Value {
    let total_pages = if page_size <= 0 {
        0
    } else {
        (total + page_size - 1) / page_size
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

fn sum_i64<T: JsonItem>(items: &[T], field: &str) -> i64 {
    items
        .iter()
        .map(|item| item.value().get(field).and_then(Value::as_i64).unwrap_or(0))
        .sum()
}

fn sum_f64<T: JsonItem>(items: &[T], field: &str) -> f64 {
    items
        .iter()
        .map(|item| {
            item.value()
                .get(field)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        })
        .sum()
}

fn average_i64<T: JsonItem>(items: &[T], field: &str) -> i64 {
    if items.is_empty() {
        return 0;
    }
    sum_i64(items, field) / items.len() as i64
}

fn metadata_str<'a>(metadata: &'a Value, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(Value::as_str)
}

fn metadata_bool(metadata: &Value, key: &str) -> Option<bool> {
    metadata.get(key).and_then(Value::as_bool)
}

fn metadata_f64(metadata: &Value, key: &str) -> f64 {
    metadata.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DownstreamProtocol;

    #[test]
    fn renders_repository_usage_list_with_existing_contract_fields() {
        let record = UsageRecord {
            id: 9,
            user_id: 1,
            api_key_id: 7,
            group_id: Some(domain::GroupId(2)),
            account_id: Some(domain::AccountId(3)),
            downstream_protocol: DownstreamProtocol::OpenAiChatCompletions,
            upstream_protocol: "chat_completions".to_owned(),
            provider: "deepseek".to_owned(),
            endpoint: "/v1/chat/completions".to_owned(),
            requested_model: "gpt-5.4".to_owned(),
            upstream_model: "deepseek-chat".to_owned(),
            input_tokens: 11,
            output_tokens: 5,
            cache_creation_tokens: 2,
            cache_read_tokens: 1,
            actual_cost: 0.19,
            status: "success".to_owned(),
            created_at_unix: 1_000,
            metadata: json!({
                "api_key_name": "repo-key",
                "account_name": "deepseek-account",
                "request_type": "sync",
                "stream": false,
                "cost": 0.19,
                "billing_type": 1,
                "billing_mode": "token",
                "rate_multiplier": 1.25,
                "account_rate_multiplier": 0.8,
                "account_stats_cost": 0.152,
                "channel_id": 88,
                "billing_tier": "standard",
                "ip_address": "127.0.0.1",
                "user_agent": "usage-test",
                "first_token_ms": 123,
                "openai_ws_mode": true,
                "cache_ttl_overridden": true,
                "image_count": 2,
                "image_size": "1024x1024",
                "image_size_breakdown": { "1024x1024": 2 }
            }),
        };
        let query = HashMap::from([
            ("model".to_owned(), "gpt-5".to_owned()),
            ("billing_type".to_owned(), "1".to_owned()),
        ]);
        let references = UsageReferenceData {
            users: HashMap::from([(
                1,
                UsageUserReference {
                    id: 1,
                    email: "repo-user@example.com".to_owned(),
                    username: "repo-user".to_owned(),
                    role: "user".to_owned(),
                    status: "active".to_owned(),
                },
            )]),
            api_keys: HashMap::from([(
                7,
                UsageApiKeyReference {
                    id: 7,
                    user_id: 1,
                    name: "repository-key-name".to_owned(),
                    group_id: Some(2),
                    status: "active".to_owned(),
                },
            )]),
            groups: HashMap::from([(
                2,
                UsageGroupReference {
                    id: 2,
                    name: "repo-group".to_owned(),
                    status: "active".to_owned(),
                },
            )]),
            accounts: HashMap::from([(
                3,
                UsageAccountReference {
                    id: 3,
                    name: "repository-account-name".to_owned(),
                    provider: "openai".to_owned(),
                    status: "active".to_owned(),
                },
            )]),
            subscriptions: HashMap::from([(
                (1, 2),
                UsageSubscriptionReference {
                    id: 11,
                    user_id: 1,
                    group_id: 2,
                    status: "active".to_owned(),
                    starts_at: "2026-06-01T00:00:00Z".to_owned(),
                    expires_at: "2026-07-01T00:00:00Z".to_owned(),
                },
            )]),
        };

        let payload = usage_list_with_references(&[record], &query, Some(&references));

        assert_eq!(payload["total"], 1);
        assert_eq!(payload["items"][0]["api_key_name"], "repository-key-name");
        assert_eq!(payload["items"][0]["total_tokens"], 19);
        assert_eq!(payload["items"][0]["platform"], "deepseek");
        assert_eq!(payload["items"][0]["billing_type"], 1);
        assert_eq!(payload["items"][0]["billing_mode"], "token");
        assert_eq!(payload["items"][0]["user_email"], "repo-user@example.com");
        assert_eq!(payload["items"][0]["user"]["username"], "repo-user");
        assert_eq!(
            payload["items"][0]["api_key"]["name"],
            "repository-key-name"
        );
        assert_eq!(payload["items"][0]["group"]["name"], "repo-group");
        assert_eq!(payload["items"][0]["account"]["id"], 3);
        assert_eq!(
            payload["items"][0]["account"]["name"],
            "repository-account-name"
        );
        assert_eq!(payload["items"][0]["subscription"]["id"], 11);
        assert_eq!(payload["items"][0]["channel_id"], 88);
        assert_eq!(payload["items"][0]["billing_tier"], "standard");
        assert_eq!(payload["items"][0]["ip_address"], "127.0.0.1");
        assert_eq!(payload["items"][0]["user_agent"], "usage-test");
        assert_eq!(payload["items"][0]["first_token_ms"], 123);
        assert_eq!(payload["items"][0]["openai_ws_mode"], true);
        assert_eq!(payload["items"][0]["cache_ttl_overridden"], true);
        assert_eq!(payload["items"][0]["image_count"], 2);
        assert_eq!(payload["items"][0]["image_size"], "1024x1024");
        assert_eq!(payload["items"][0]["account_rate_multiplier"], 0.8);
        assert_eq!(payload["items"][0]["account_stats_cost"], 0.152);
    }

    #[test]
    fn dashboard_time_range_uses_query_timezone_for_defaults() {
        let query = HashMap::from([("timezone".to_owned(), "Asia/Shanghai".to_owned())]);
        let range = dashboard_time_range(&query);
        let local_today = Utc::now()
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive();

        assert_eq!(
            range.start_date,
            (local_today - Duration::days(7))
                .format("%Y-%m-%d")
                .to_string()
        );
        assert_eq!(range.end_date, local_today.format("%Y-%m-%d").to_string());
    }

    #[test]
    fn timezone_midnight_converts_to_utc_boundary() {
        let shanghai_midnight = date_start_unix_in_timezone(
            NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(),
            chrono_tz::Asia::Shanghai,
        )
        .unwrap();

        assert_eq!(shanghai_midnight, 1_780_934_400);
    }
}
