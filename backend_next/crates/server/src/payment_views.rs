use repository::PaymentOrderRecord;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn list_user_orders(
    user_id: i64,
    records: &[PaymentOrderRecord],
    query: &HashMap<String, String>,
) -> Value {
    let page = query_i64(query, "page", 1).max(1);
    let page_size = query_page_size(query);
    let status = normalized_query(query, "status");
    let order_type = normalized_query(query, "order_type");
    let payment_type = normalized_query(query, "payment_type");
    let mut items = records
        .iter()
        .filter(|order| order.user_id == user_id)
        .filter(|order| matches_optional(&status, &order.status))
        .filter(|order| matches_optional(&order_type, &order.order_type))
        .filter(|order| matches_optional(&payment_type, &order.payment_type))
        .map(public_json)
        .collect::<Vec<_>>();
    sort_orders_desc(&mut items);
    paginate_items(items, page, page_size)
}

pub fn list_admin_orders(records: &[PaymentOrderRecord], query: &HashMap<String, String>) -> Value {
    let page = query_i64(query, "page", 1).max(1);
    let page_size = query_page_size(query);
    let status = normalized_query(query, "status");
    let order_type = normalized_query(query, "order_type");
    let payment_type = normalized_query(query, "payment_type");
    let user_id = query
        .get("user_id")
        .and_then(|value| value.parse::<i64>().ok());
    let keyword = normalized_query(query, "keyword");
    let out_trade_no = normalized_query(query, "out_trade_no");
    let mut items = records
        .iter()
        .filter(|order| matches_optional(&status, &order.status))
        .filter(|order| matches_optional(&order_type, &order.order_type))
        .filter(|order| matches_optional(&payment_type, &order.payment_type))
        .filter(|order| {
            user_id
                .map(|expected| order.user_id == expected)
                .unwrap_or(true)
        })
        .filter(|order| {
            out_trade_no
                .as_ref()
                .map(|needle| order.out_trade_no.to_lowercase().contains(needle))
                .unwrap_or(true)
        })
        .filter(|order| {
            keyword
                .as_ref()
                .map(|needle| {
                    order.out_trade_no.to_lowercase().contains(needle)
                        || order.user_id.to_string().contains(needle)
                        || order.payment_type.to_lowercase().contains(needle)
                        || order.order_type.to_lowercase().contains(needle)
                })
                .unwrap_or(true)
        })
        .map(public_json)
        .collect::<Vec<_>>();
    sort_orders_desc(&mut items);
    paginate_items(items, page, page_size)
}

pub fn admin_dashboard(records: &[PaymentOrderRecord]) -> Value {
    let completed = records
        .iter()
        .filter(|order| order.status == "COMPLETED")
        .collect::<Vec<_>>();
    json!({
        "total_orders": records.len() as i64,
        "paid_orders": completed.len() as i64,
        "pending_orders": records.iter().filter(|order| order.status == "PENDING").count() as i64,
        "cancelled_orders": records.iter().filter(|order| order.status == "CANCELLED").count() as i64,
        "total_amount": records.iter().map(|order| order.amount).sum::<f64>(),
        "paid_amount": completed.iter().map(|order| order.amount).sum::<f64>(),
        "refund_amount": records.iter().map(|order| order.refund_amount).sum::<f64>(),
        "orders_by_day": [],
        "amount_by_day": []
    })
}

pub fn public_json(order: &PaymentOrderRecord) -> Value {
    json!({
        "id": order.id,
        "user_id": order.user_id,
        "amount": order.amount,
        "pay_amount": order.pay_amount,
        "currency": order.currency,
        "fee_rate": order.fee_rate,
        "payment_type": order.payment_type,
        "out_trade_no": order.out_trade_no,
        "status": order.status,
        "order_type": order.order_type,
        "created_at": order.created_at,
        "expires_at": order.expires_at,
        "paid_at": order.paid_at,
        "completed_at": order.completed_at,
        "cancelled_at": order.cancelled_at,
        "refund_amount": order.refund_amount,
        "refund_reason": order.refund_reason,
        "refund_requested_at": order.refund_requested_at,
        "refund_requested_by": null,
        "refund_request_reason": order.refund_request_reason,
        "refunded_at": order.refunded_at,
        "plan_id": order.plan_id,
        "provider_instance_id": order.provider_instance_id,
        "webhook_count": order.webhook_count
    })
}

pub fn public_limited_json(order: &PaymentOrderRecord) -> Value {
    let mut value = public_json(order);
    if let Value::Object(object) = &mut value {
        object.remove("user_id");
        object.remove("provider_instance_id");
        object.remove("webhook_count");
    }
    value
}

pub fn pending_provider_instance_ids(records: &[PaymentOrderRecord]) -> Vec<String> {
    records
        .iter()
        .filter(|order| order.status == "PENDING")
        .filter_map(|order| order.provider_instance_id.clone())
        .collect()
}

fn paginate_items(items: Vec<Value>, page: i64, page_size: i64) -> Value {
    let total = items.len() as i64;
    let start = ((page - 1) * page_size) as usize;
    json!({
        "items": items.into_iter().skip(start).take(page_size as usize).collect::<Vec<_>>(),
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": if total == 0 { 1 } else { (total + page_size - 1) / page_size }
    })
}

fn query_i64(query: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    query
        .get(key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn query_page_size(query: &HashMap<String, String>) -> i64 {
    query_i64(query, "page_size", query_i64(query, "limit", 20)).clamp(1, 100)
}

fn normalized_query(query: &HashMap<String, String>, key: &str) -> Option<String> {
    query
        .get(key)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn matches_optional(expected: &Option<String>, actual: &str) -> bool {
    expected
        .as_ref()
        .map(|expected| actual.eq_ignore_ascii_case(expected))
        .unwrap_or(true)
}

fn sort_orders_desc(items: &mut [Value]) {
    items.sort_by(|left, right| {
        let left_created = left["created_at"].as_str().unwrap_or_default();
        let right_created = right["created_at"].as_str().unwrap_or_default();
        right_created.cmp(left_created).then_with(|| {
            right["id"]
                .as_i64()
                .unwrap_or_default()
                .cmp(&left["id"].as_i64().unwrap_or_default())
        })
    });
}
