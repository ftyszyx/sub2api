use crate::quota_window;
use chrono::{DateTime, Utc};
use repository::{
    BalanceTransactionRecord, UserBalanceRecord, UserPlatformQuotaRecord, UserSubscriptionRecord,
};
use serde_json::{json, Value};

pub fn redeem_history(transactions: &[BalanceTransactionRecord]) -> Value {
    json!(transactions
        .iter()
        .map(|transaction| {
            json!({
                "id": transaction.id,
                "code": transaction.order_id,
                "type": transaction.transaction_type,
                "value": transaction.amount,
                "new_balance": transaction.balance_after,
                "status": "used",
                "used_at": transaction.created_at,
                "created_at": transaction.created_at,
                "metadata": transaction.metadata
            })
        })
        .collect::<Vec<_>>())
}

pub fn admin_balance_history(
    transactions: &[BalanceTransactionRecord],
    page: i64,
    page_size: i64,
    type_filter: Option<&str>,
) -> Value {
    let requested_type = type_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut items = transactions
        .iter()
        .filter_map(|transaction| {
            let item_type = admin_balance_history_type(transaction);
            if requested_type
                .as_ref()
                .is_some_and(|requested| requested != item_type)
            {
                return None;
            }
            let notes = transaction
                .metadata
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(json!({
                "id": transaction.id,
                "code": transaction.order_id,
                "type": item_type,
                "value": transaction.amount,
                "status": "used",
                "used_by": transaction.user_id,
                "used_at": transaction.created_at,
                "created_at": transaction.created_at,
                "group_id": null,
                "validity_days": 0,
                "notes": notes,
                "new_balance": transaction.balance_after,
                "metadata": transaction.metadata,
                "user": null,
                "group": null
            }))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .get("id")
            .and_then(Value::as_i64)
            .cmp(&left.get("id").and_then(Value::as_i64))
    });

    let total = items.len() as i64;
    let total_recharged = transactions
        .iter()
        .filter(|transaction| {
            matches!(
                admin_balance_history_type(transaction),
                "balance" | "affiliate_balance" | "admin_balance"
            ) && transaction.amount > 0.0
        })
        .map(|transaction| transaction.amount)
        .sum::<f64>();
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let start = ((page - 1) * page_size) as usize;
    let page_items = items
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .collect::<Vec<_>>();
    let pages = if total == 0 {
        1
    } else {
        ((total as f64) / (page_size as f64)).ceil() as i64
    };
    json!({
        "items": page_items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": pages,
        "total_pages": pages,
        "total_recharged": total_recharged
    })
}

fn admin_balance_history_type(transaction: &BalanceTransactionRecord) -> &str {
    match transaction.transaction_type.as_str() {
        "payment_recharge" | "payment_refund" => "balance",
        "affiliate_transfer" | "affiliate_balance" => "affiliate_balance",
        "admin_initial_balance" | "admin_balance" => "admin_balance",
        other => other,
    }
}

pub fn subscriptions(records: &[UserSubscriptionRecord]) -> Value {
    json!(records.iter().map(subscription_json).collect::<Vec<_>>())
}

pub fn active_subscriptions(records: &[UserSubscriptionRecord]) -> Value {
    json!(records
        .iter()
        .filter(|subscription| subscription.status.eq_ignore_ascii_case("active"))
        .map(subscription_json)
        .collect::<Vec<_>>())
}

pub fn subscription_progress(records: &[UserSubscriptionRecord]) -> Value {
    json!(records
        .iter()
        .filter(|subscription| subscription.status.eq_ignore_ascii_case("active"))
        .map(single_subscription_progress)
        .collect::<Vec<_>>())
}

pub fn single_subscription_progress(subscription: &UserSubscriptionRecord) -> Value {
    json!({
        "subscription": subscription_json(subscription),
        "progress": subscription_progress_json(subscription)
    })
}

pub fn subscription_summary(records: &[UserSubscriptionRecord]) -> Value {
    let active = records
        .iter()
        .filter(|subscription| subscription.status.eq_ignore_ascii_case("active"))
        .collect::<Vec<_>>();
    json!({
        "active_count": active.len(),
        "total_used_usd": active
            .iter()
            .map(|subscription| subscription.monthly_usage_usd)
            .sum::<f64>(),
        "subscriptions": active
            .iter()
            .map(|subscription| {
                json!({
                    "id": subscription.id,
                    "group_id": subscription.group_id,
                    "group_name": format!("Group {}", subscription.group_id),
                    "status": subscription.status,
                    "daily_progress": null,
                    "weekly_progress": null,
                    "monthly_progress": null,
                    "daily_used_usd": subscription.daily_usage_usd,
                    "weekly_used_usd": subscription.weekly_usage_usd,
                    "monthly_used_usd": subscription.monthly_usage_usd,
                    "expires_at": subscription.expires_at,
                    "days_remaining": null
                })
            })
            .collect::<Vec<_>>()
    })
}

pub fn platform_quotas(
    balance: &UserBalanceRecord,
    subscriptions: &[UserSubscriptionRecord],
) -> Value {
    json!({
        "balance": balance.balance,
        "platform_quotas": subscriptions
            .iter()
            .filter(|subscription| subscription.status.eq_ignore_ascii_case("active"))
            .map(|subscription| {
                json!({
                    "platform": "group",
                    "group_id": subscription.group_id,
                    "status": subscription.status,
                    "expires_at": subscription.expires_at,
                    "daily_limit_usd": null,
                    "weekly_limit_usd": null,
                    "monthly_limit_usd": null,
                    "daily_used_usd": 0.0,
                    "weekly_used_usd": 0.0,
                    "monthly_used_usd": 0.0
                })
            })
            .collect::<Vec<_>>()
    })
}

pub fn platform_quota_response(
    records: &[UserPlatformQuotaRecord],
    include_window_start: bool,
) -> Value {
    platform_quota_response_at(records, include_window_start, Utc::now())
}

pub fn channel_monitors(monitors: &[Value]) -> Value {
    let mut items = monitors
        .iter()
        .filter(|monitor| {
            monitor
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
        .map(channel_monitor_summary)
        .collect::<Vec<_>>();
    sort_json_items_by_id(&mut items);
    let total = items.len() as i64;
    json!({
        "items": items,
        "total": total,
        "page": 1,
        "page_size": total.max(20),
        "pages": if total == 0 { 0 } else { 1 },
        "total_pages": if total == 0 { 0 } else { 1 }
    })
}

pub fn channel_monitor_status(monitors: &[Value], id: i64) -> Option<Value> {
    monitors
        .iter()
        .find(|monitor| monitor.get("id").and_then(Value::as_i64) == Some(id))
        .filter(|monitor| {
            monitor
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
        .map(channel_monitor_status_json)
}

pub fn platform_quota_response_at(
    records: &[UserPlatformQuotaRecord],
    include_window_start: bool,
    now: DateTime<Utc>,
) -> Value {
    json!({
        "platform_quotas": records
            .iter()
            .map(|record| quota_window::platform_quota_json(record, include_window_start, now))
            .collect::<Vec<_>>()
    })
}

fn subscription_json(subscription: &UserSubscriptionRecord) -> Value {
    json!({
        "id": subscription.id,
        "user_id": subscription.user_id,
        "group_id": subscription.group_id,
        "group_name": format!("Group {}", subscription.group_id),
        "plan_id": subscription.plan_id,
        "status": subscription.status,
        "daily_window_start": subscription.daily_window_start,
        "weekly_window_start": subscription.weekly_window_start,
        "monthly_window_start": subscription.monthly_window_start,
        "daily_usage_usd": subscription.daily_usage_usd,
        "weekly_usage_usd": subscription.weekly_usage_usd,
        "monthly_usage_usd": subscription.monthly_usage_usd,
        "starts_at": subscription.starts_at,
        "expires_at": subscription.expires_at,
        "source_order_id": subscription.source_order_id,
        "created_at": subscription.created_at,
        "updated_at": subscription.created_at,
        "metadata": subscription.metadata
    })
}

fn subscription_progress_json(subscription: &UserSubscriptionRecord) -> Value {
    json!({
        "subscription_id": subscription.id,
        "group_id": subscription.group_id,
        "status": subscription.status,
        "daily": null,
        "weekly": null,
        "monthly": null,
        "expires_in_days": null
    })
}

fn channel_monitor_summary(monitor: &Value) -> Value {
    json!({
        "id": monitor.get("id").cloned().unwrap_or(Value::Null),
        "name": monitor.get("name").cloned().unwrap_or_else(|| json!("")),
        "provider": monitor.get("provider").cloned().unwrap_or_else(|| json!("openai")),
        "api_mode": monitor.get("api_mode").cloned().unwrap_or_else(|| json!("responses")),
        "group_name": monitor.get("group_name").cloned().unwrap_or(Value::Null),
        "primary_model": monitor.get("primary_model").cloned().unwrap_or(Value::Null),
        "primary_status": monitor.get("primary_status").cloned().unwrap_or_else(|| json!("unknown")),
        "primary_latency_ms": monitor.get("primary_latency_ms").cloned().unwrap_or(Value::Null),
        "availability_7d": monitor.get("availability_7d").cloned().unwrap_or_else(|| json!(0.0)),
        "extra_models_status": monitor.get("extra_models_status").cloned().unwrap_or_else(|| json!([])),
        "last_checked_at": monitor.get("last_checked_at").cloned().unwrap_or(Value::Null),
        "enabled": monitor.get("enabled").cloned().unwrap_or_else(|| json!(true))
    })
}

fn channel_monitor_status_json(monitor: &Value) -> Value {
    let status = monitor
        .get("primary_status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    json!({
        "id": monitor.get("id").cloned().unwrap_or(Value::Null),
        "name": monitor.get("name").cloned().unwrap_or_else(|| json!("")),
        "status": status,
        "last_checked_at": monitor.get("last_checked_at").cloned().unwrap_or(Value::Null),
        "latency_ms": monitor.get("primary_latency_ms").cloned().unwrap_or(Value::Null),
        "available": status.eq_ignore_ascii_case("success"),
        "primary_model": monitor.get("primary_model").cloned().unwrap_or(Value::Null),
        "provider": monitor.get("provider").cloned().unwrap_or_else(|| json!("openai")),
        "api_mode": monitor.get("api_mode").cloned().unwrap_or_else(|| json!("responses"))
    })
}

fn sort_json_items_by_id(items: &mut [Value]) {
    items.sort_by_key(|item| item.get("id").and_then(Value::as_i64).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn platform_quota_response_uses_lazy_zero_and_real_reset_times() {
        let payload = platform_quota_response_at(
            &[UserPlatformQuotaRecord {
                id: 1,
                user_id: 1,
                platform: "openai".to_owned(),
                daily_limit_usd: Some(1.0),
                weekly_limit_usd: Some(2.0),
                monthly_limit_usd: Some(3.0),
                daily_usage_usd: 0.5,
                weekly_usage_usd: 1.5,
                monthly_usage_usd: 2.5,
                daily_window_start: Some("2026-06-06T16:00:00Z".to_owned()),
                weekly_window_start: Some("2026-05-24T16:00:00Z".to_owned()),
                monthly_window_start: Some("2026-06-01T00:00:00Z".to_owned()),
            }],
            true,
            dt("2026-06-07T10:30:00Z"),
        );

        let item = &payload["platform_quotas"][0];
        assert_eq!(item["daily_usage_usd"], 0.5);
        assert_eq!(item["daily_window_resets_at"], "2026-06-07T16:00:00Z");
        assert_eq!(item["weekly_usage_usd"], 0.0);
        assert_eq!(item["weekly_window_resets_at"], Value::Null);
        assert_eq!(item["monthly_window_resets_at"], "2026-07-01T00:00:00Z");
        assert_eq!(item["weekly_window_start"], "2026-05-24T16:00:00Z");
    }

    #[test]
    fn user_channel_monitor_views_project_admin_collection_safely() {
        let monitors = vec![
            json!({
                "id": 2,
                "name": "Disabled",
                "enabled": false
            }),
            json!({
                "id": 1,
                "name": "DeepSeek",
                "provider": "deepseek",
                "api_mode": "chat_completions",
                "primary_model": "deepseek-chat",
                "primary_status": "success",
                "primary_latency_ms": 123,
                "last_checked_at": "2026-06-10T00:00:00Z"
            }),
        ];

        let list = channel_monitors(&monitors);
        assert_eq!(list["total"], 1);
        assert_eq!(list["items"][0]["id"], 1);
        assert_eq!(list["items"][0]["provider"], "deepseek");

        let status = channel_monitor_status(&monitors, 1).unwrap();
        assert_eq!(status["status"], "success");
        assert_eq!(status["available"], true);
        assert!(channel_monitor_status(&monitors, 2).is_none());
    }
}
