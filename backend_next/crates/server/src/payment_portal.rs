use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc, RwLock,
};
use uuid::Uuid;

use crate::payment_provider::{
    instance_identifier, CreatePaymentRequest, PaymentNotification, PaymentProviderError,
    PaymentProviderRegistry,
};
use crate::response::ApiError;
use repository::PaymentProviderInstanceRecord;

const NOW: &str = "2026-06-06T00:00:00Z";
const EXPIRES_AT: &str = "2026-06-06T01:00:00Z";

#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrderRequest {
    pub amount: f64,
    pub payment_type: String,
    pub order_type: Option<String>,
    pub plan_id: Option<i64>,
    pub return_url: Option<String>,
    pub payment_source: Option<String>,
    pub openid: Option<String>,
    pub wechat_resume_token: Option<String>,
    pub is_mobile: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyOrderRequest {
    pub out_trade_no: String,
}

#[derive(Debug, Deserialize)]
pub struct ResolveOrderRequest {
    pub resume_token: String,
}

#[derive(Debug, Deserialize)]
pub struct RefundRequest {
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PaymentWebhookResult {
    pub provider: String,
    pub record: Value,
    pub audits: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaymentOrderStatus {
    Pending,
    Paid,
    Recharging,
    Completed,
    Cancelled,
    Failed,
    Expired,
    RefundRequested,
    Refunding,
    PartiallyRefunded,
    Refunded,
    RefundFailed,
}

impl PaymentOrderStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Paid => "PAID",
            Self::Recharging => "RECHARGING",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
            Self::Expired => "EXPIRED",
            Self::RefundRequested => "REFUND_REQUESTED",
            Self::Refunding => "REFUNDING",
            Self::PartiallyRefunded => "PARTIALLY_REFUNDED",
            Self::Refunded => "REFUNDED",
            Self::RefundFailed => "REFUND_FAILED",
        }
    }

    const fn is_final(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Refunded | Self::Failed | Self::Expired
        )
    }
}

const ALL_PAYMENT_ORDER_STATUSES: [PaymentOrderStatus; 12] = [
    PaymentOrderStatus::Pending,
    PaymentOrderStatus::Paid,
    PaymentOrderStatus::Recharging,
    PaymentOrderStatus::Completed,
    PaymentOrderStatus::Expired,
    PaymentOrderStatus::Cancelled,
    PaymentOrderStatus::Failed,
    PaymentOrderStatus::RefundRequested,
    PaymentOrderStatus::Refunding,
    PaymentOrderStatus::PartiallyRefunded,
    PaymentOrderStatus::Refunded,
    PaymentOrderStatus::RefundFailed,
];

#[derive(Debug, Clone)]
struct PaymentOrderRecord {
    id: i64,
    user_id: i64,
    amount: f64,
    pay_amount: f64,
    currency: String,
    fee_rate: f64,
    payment_type: String,
    out_trade_no: String,
    status: PaymentOrderStatus,
    order_type: String,
    refund_amount: f64,
    refund_reason: Option<String>,
    refund_request_reason: Option<String>,
    plan_id: Option<i64>,
    provider_instance_id: Option<String>,
    paid_at: Option<String>,
    completed_at: Option<String>,
    cancelled_at: Option<String>,
    refund_requested_at: Option<String>,
    refunded_at: Option<String>,
    webhook_count: i64,
}

impl PaymentOrderRecord {
    fn public_json(&self) -> Value {
        json!({
            "id": self.id,
            "user_id": self.user_id,
            "amount": self.amount,
            "pay_amount": self.pay_amount,
            "currency": self.currency,
            "fee_rate": self.fee_rate,
            "payment_type": self.payment_type,
            "out_trade_no": self.out_trade_no,
            "status": self.status.as_str(),
            "order_type": self.order_type,
            "created_at": NOW,
            "expires_at": EXPIRES_AT,
            "paid_at": self.paid_at,
            "completed_at": self.completed_at,
            "cancelled_at": self.cancelled_at,
            "refund_amount": self.refund_amount,
            "refund_reason": self.refund_reason,
            "refund_requested_at": self.refund_requested_at,
            "refund_requested_by": null,
            "refund_request_reason": self.refund_request_reason,
            "refunded_at": self.refunded_at,
            "plan_id": self.plan_id,
            "provider_instance_id": self.provider_instance_id,
            "webhook_count": self.webhook_count
        })
    }

    fn public_limited_json(&self) -> Value {
        json!({
            "id": self.id,
            "out_trade_no": self.out_trade_no,
            "amount": self.amount,
            "pay_amount": self.pay_amount,
            "fee_rate": self.fee_rate,
            "currency": self.currency,
            "payment_type": self.payment_type,
            "order_type": self.order_type,
            "status": self.status.as_str(),
            "created_at": NOW,
            "expires_at": EXPIRES_AT,
            "paid_at": self.paid_at,
            "completed_at": self.completed_at,
            "cancelled_at": self.cancelled_at,
            "refund_amount": self.refund_amount,
            "refund_reason": self.refund_reason,
            "refund_requested_at": self.refund_requested_at,
            "refund_requested_by": null,
            "refund_request_reason": self.refund_request_reason,
            "refunded_at": self.refunded_at,
            "plan_id": self.plan_id
        })
    }
}

pub struct PaymentPortalService {
    next_order_id: AtomicI64,
    next_webhook_id: AtomicI64,
    next_event_id: AtomicI64,
    provider_registry: Arc<PaymentProviderRegistry>,
    orders_by_id: RwLock<HashMap<i64, PaymentOrderRecord>>,
    order_id_by_trade_no: RwLock<HashMap<String, i64>>,
    order_id_by_resume_token: RwLock<HashMap<String, i64>>,
    webhook_records: RwLock<Vec<Value>>,
    event_logs: RwLock<Vec<Value>>,
}

impl PaymentPortalService {
    pub fn new() -> Self {
        Self::with_provider_registry(Arc::new(PaymentProviderRegistry::mock_default()))
    }

    pub fn with_provider_registry(provider_registry: Arc<PaymentProviderRegistry>) -> Self {
        Self {
            next_order_id: AtomicI64::new(1),
            next_webhook_id: AtomicI64::new(1),
            next_event_id: AtomicI64::new(1),
            provider_registry,
            orders_by_id: RwLock::new(HashMap::new()),
            order_id_by_trade_no: RwLock::new(HashMap::new()),
            order_id_by_resume_token: RwLock::new(HashMap::new()),
            webhook_records: RwLock::new(Vec::new()),
            event_logs: RwLock::new(Vec::new()),
        }
    }

    pub fn config(&self) -> Value {
        json!({
            "payment_enabled": true,
            "min_amount": 1.0,
            "max_amount": 10000.0,
            "daily_limit": 100000.0,
            "max_pending_orders": 5,
            "order_timeout_minutes": 60,
            "balance_disabled": false,
            "balance_recharge_multiplier": 1.0,
            "enabled_payment_types": ["alipay", "wxpay", "stripe"],
            "help_image_url": "",
            "help_text": "",
            "stripe_publishable_key": "",
            "order_statuses": ALL_PAYMENT_ORDER_STATUSES
                .iter()
                .map(|status| status.as_str())
                .collect::<Vec<_>>()
        })
    }

    pub fn plans(&self) -> Value {
        json!([
            {
                "id": 1,
                "group_id": 1,
                "group_platform": "openai",
                "group_name": "Default OpenAI",
                "rate_multiplier": 1.0,
                "daily_limit_usd": null,
                "weekly_limit_usd": null,
                "monthly_limit_usd": null,
                "supported_model_scopes": [],
                "name": "Starter",
                "description": "Development starter plan",
                "price": 9.9,
                "original_price": null,
                "validity_days": 30,
                "validity_unit": "day",
                "features": ["Default OpenAI group"],
                "for_sale": true,
                "sort_order": 1
            }
        ])
    }

    pub fn channels(&self) -> Value {
        json!([
            {
                "id": 1,
                "group_id": 1,
                "name": "Default OpenAI",
                "platform": "openai",
                "rate_multiplier": 1.0,
                "description": "Default development channel",
                "models": ["gpt-5.4"],
                "features": [],
                "enabled": true
            }
        ])
    }

    pub fn limits(&self) -> Value {
        json!({
            "methods": {
                "alipay": method_limit("CNY"),
                "wxpay": method_limit("CNY"),
                "stripe": method_limit("USD")
            },
            "global_min": 1.0,
            "global_max": 10000.0
        })
    }

    pub fn checkout_info(&self) -> Value {
        let limits = self.limits();
        json!({
            "methods": limits["methods"].clone(),
            "global_min": limits["global_min"].clone(),
            "global_max": limits["global_max"].clone(),
            "plans": self.plans(),
            "balance_disabled": false,
            "balance_recharge_multiplier": 1.0,
            "recharge_fee_rate": 0.0,
            "help_text": "",
            "help_image_url": "",
            "stripe_publishable_key": "",
            "alipay_force_qrcode": false
        })
    }

    pub async fn create_order(
        &self,
        user_id: i64,
        request: CreateOrderRequest,
        provider_instances: &[PaymentProviderInstanceRecord],
    ) -> Result<Value, ApiError> {
        if request.amount <= 0.0 {
            return Err(ApiError::bad_request("amount must be greater than zero"));
        }
        if request.amount < 1.0 || request.amount > 10_000.0 {
            return Err(ApiError::bad_request("amount out of range"));
        }
        let payment_type = normalize_payment_type(&request.payment_type);
        if payment_type.is_empty() {
            return Err(ApiError::bad_request("payment_type is required"));
        }
        let order_type = request
            .order_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("balance")
            .to_owned();
        if order_type == "subscription" && request.plan_id.unwrap_or_default() <= 0 {
            return Err(ApiError::bad_request("subscription order requires a plan"));
        }
        let pending_count = self
            .orders_by_id
            .read()
            .expect("payment order lock")
            .values()
            .filter(|order| order.user_id == user_id && order.status == PaymentOrderStatus::Pending)
            .count();
        if pending_count >= 5 {
            return Err(ApiError::conflict("too many pending payment orders"));
        }
        let id = self.next_order_id.fetch_add(1, Ordering::SeqCst);
        let out_trade_no = format!(
            "sub2_20260606{}",
            Uuid::new_v4().simple().to_string()[..8].to_owned()
        );
        let resume_token = format!("resume-{}", Uuid::new_v4());
        let request_registry = self
            .provider_registry
            .as_ref()
            .clone()
            .with_configured_instances(provider_instances.to_vec());
        let selected_instance = if provider_instances.is_empty() {
            None
        } else {
            Some(
                request_registry
                    .select_instance_for_payment_type(&payment_type)
                    .map_err(payment_provider_api_error)?,
            )
        };
        let provider = match &selected_instance {
            Some(instance) => request_registry.configured_provider_for_instance(instance.clone()),
            None => request_registry
                .provider_for_payment_type(&payment_type)
                .map_err(payment_provider_api_error)?,
        };
        let provider_response = provider
            .create_payment(CreatePaymentRequest {
                order_id: id,
                out_trade_no: out_trade_no.clone(),
                amount: request.amount,
                payment_type: payment_type.clone(),
                return_url: request.return_url.clone(),
                is_mobile: request.is_mobile.unwrap_or(false),
            })
            .await
            .map_err(payment_provider_api_error)?;
        let provider_key = provider_response.provider_key.clone();
        let provider_instance_id = provider_response.provider_instance_id.clone();
        let plan_id = request.plan_id.filter(|id| *id > 0);
        let response_order_type = order_type.clone();
        let order = PaymentOrderRecord {
            id,
            user_id,
            amount: request.amount,
            pay_amount: request.amount,
            currency: provider_response.currency.clone(),
            fee_rate: 0.0,
            payment_type: payment_type.clone(),
            out_trade_no: out_trade_no.clone(),
            status: PaymentOrderStatus::Pending,
            order_type,
            refund_amount: 0.0,
            refund_reason: None,
            refund_request_reason: None,
            plan_id,
            provider_instance_id: Some(provider_instance_id.clone()),
            paid_at: None,
            completed_at: None,
            cancelled_at: None,
            refund_requested_at: None,
            refunded_at: None,
            webhook_count: 0,
        };
        self.orders_by_id
            .write()
            .expect("payment order lock")
            .insert(id, order);
        self.order_id_by_trade_no
            .write()
            .expect("payment trade lock")
            .insert(out_trade_no.clone(), id);
        self.order_id_by_resume_token
            .write()
            .expect("payment resume lock")
            .insert(resume_token.clone(), id);
        Ok(json!({
            "order_id": id,
            "amount": request.amount,
            "pay_url": provider_response.pay_url,
            "qr_code": provider_response.qr_code,
            "client_secret": provider_response.client_secret,
            "intent_id": provider_response.intent_id,
            "currency": provider_response.currency,
            "country_code": provider_response.country_code,
            "payment_env": provider_response.payment_env,
            "pay_amount": request.amount,
            "fee_rate": 0.0,
            "expires_at": EXPIRES_AT,
            "result_type": provider_response.result_type,
            "payment_type": payment_type,
            "out_trade_no": out_trade_no,
            "status": "PENDING",
            "order_type": response_order_type,
            "plan_id": plan_id,
            "plan_group_id": Value::Null,
            "plan_validity_days": Value::Null,
            "provider_key": provider_key,
            "provider_instance_id": provider_instance_id,
            "provider_snapshot": provider_snapshot_json(&provider_response, selected_instance.as_ref()),
            "payment_trade_no": provider_response.trade_no,
            "recharge_code": format!("PAY-{id}-{}", Uuid::new_v4().simple().to_string()[..6].to_owned()),
            "payment_mode": provider_response.payment_mode,
            "resume_token": resume_token,
            "oauth": null,
            "jsapi": null,
            "jsapi_payload": null
        }))
    }

    pub fn list_user_orders(&self, user_id: i64) -> Value {
        let mut items: Vec<Value> = self
            .orders_by_id
            .read()
            .expect("payment order lock")
            .values()
            .filter(|order| order.user_id == user_id)
            .map(PaymentOrderRecord::public_json)
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

    pub fn list_admin_orders(&self, query: &HashMap<String, String>) -> Value {
        let page = query_i64(query, "page", 1).max(1);
        let page_size = query_i64(query, "page_size", 20).clamp(1, 200);
        let status = query.get("status").map(|value| value.to_ascii_uppercase());
        let payment_type = query.get("payment_type").map(|value| value.to_lowercase());
        let user_id = query
            .get("user_id")
            .and_then(|value| value.parse::<i64>().ok());
        let out_trade_no = query.get("out_trade_no").map(|value| value.to_lowercase());
        let mut items = self
            .orders_by_id
            .read()
            .expect("payment order lock")
            .values()
            .filter(|order| {
                status
                    .as_ref()
                    .map(|expected| order.status.as_str() == expected)
                    .unwrap_or(true)
            })
            .filter(|order| {
                payment_type
                    .as_ref()
                    .map(|expected| order.payment_type.to_lowercase() == *expected)
                    .unwrap_or(true)
            })
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
            .map(PaymentOrderRecord::public_json)
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item["id"].as_i64().unwrap_or_default());
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

    pub fn pending_provider_instance_ids(&self) -> Vec<String> {
        self.orders_by_id
            .read()
            .expect("payment order lock")
            .values()
            .filter(|order| order.status == PaymentOrderStatus::Pending)
            .filter_map(|order| order.provider_instance_id.clone())
            .collect()
    }

    pub fn admin_dashboard(&self) -> Value {
        let orders = self.orders_by_id.read().expect("payment order lock");
        let total_orders = orders.len() as i64;
        let completed = orders
            .values()
            .filter(|order| order.status == PaymentOrderStatus::Completed)
            .collect::<Vec<_>>();
        let pending_orders = orders
            .values()
            .filter(|order| order.status == PaymentOrderStatus::Pending)
            .count() as i64;
        let cancelled_orders = orders
            .values()
            .filter(|order| order.status == PaymentOrderStatus::Cancelled)
            .count() as i64;
        let refund_amount = orders
            .values()
            .map(|order| order.refund_amount)
            .sum::<f64>();
        json!({
            "total_orders": total_orders,
            "paid_orders": completed.len() as i64,
            "pending_orders": pending_orders,
            "cancelled_orders": cancelled_orders,
            "total_amount": orders.values().map(|order| order.amount).sum::<f64>(),
            "paid_amount": completed.iter().map(|order| order.amount).sum::<f64>(),
            "refund_amount": refund_amount,
            "orders_by_day": [],
            "amount_by_day": []
        })
    }

    pub fn get_order(&self, user_id: i64, id: i64) -> Result<Value, ApiError> {
        Ok(self.find_order_for_user(user_id, id)?.public_json())
    }

    pub fn get_order_admin(&self, id: i64) -> Result<Value, ApiError> {
        Ok(self.find_order(id)?.public_json())
    }

    pub fn cancel_order(&self, user_id: i64, id: i64) -> Result<Value, ApiError> {
        self.cancel_order_internal(Some(user_id), id)
    }

    pub fn cancel_order_admin(&self, id: i64) -> Result<Value, ApiError> {
        self.cancel_order_internal(None, id)
    }

    fn cancel_order_internal(&self, user_id: Option<i64>, id: i64) -> Result<Value, ApiError> {
        let mut orders = self.orders_by_id.write().expect("payment order lock");
        let order = orders
            .get_mut(&id)
            .filter(|order| {
                user_id
                    .map(|expected| order.user_id == expected)
                    .unwrap_or(true)
            })
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        if order.status != PaymentOrderStatus::Pending {
            return Err(ApiError::conflict(format!(
                "cannot cancel order in {} status",
                order.status.as_str()
            )));
        }
        order.status = PaymentOrderStatus::Cancelled;
        order.cancelled_at = Some(NOW.to_owned());
        Ok(order.public_json())
    }

    pub fn request_refund(
        &self,
        user_id: i64,
        id: i64,
        request: RefundRequest,
    ) -> Result<Value, ApiError> {
        let mut orders = self.orders_by_id.write().expect("payment order lock");
        let order = orders
            .get_mut(&id)
            .filter(|order| order.user_id == user_id)
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        if order.status != PaymentOrderStatus::Completed {
            return Err(ApiError::conflict(format!(
                "cannot request refund for order in {} status",
                order.status.as_str()
            )));
        }
        if order.order_type != "balance" {
            return Err(ApiError::bad_request(
                "only balance orders can request refund",
            ));
        }
        let reason = request.reason.trim();
        if reason.is_empty() {
            return Err(ApiError::bad_request("refund reason is required"));
        }
        order.status = PaymentOrderStatus::RefundRequested;
        order.refund_requested_at = Some(NOW.to_owned());
        order.refund_request_reason = Some(request.reason);
        Ok(order.public_json())
    }

    pub fn refund_order_admin(&self, id: i64, payload: Value) -> Result<Value, ApiError> {
        let mut orders = self.orders_by_id.write().expect("payment order lock");
        let order = orders
            .get_mut(&id)
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        if !matches!(
            order.status,
            PaymentOrderStatus::Completed | PaymentOrderStatus::RefundRequested
        ) {
            return Err(ApiError::conflict(format!(
                "cannot refund order in {} status",
                order.status.as_str()
            )));
        }
        let refund_amount = payload
            .get("refund_amount")
            .or_else(|| payload.get("amount"))
            .and_then(Value::as_f64)
            .unwrap_or(order.pay_amount);
        if refund_amount <= 0.0 || refund_amount > order.pay_amount {
            return Err(ApiError::bad_request("invalid refund amount"));
        }
        order.status = PaymentOrderStatus::Refunded;
        order.refund_amount = refund_amount;
        order.refunded_at = Some(NOW.to_owned());
        order.refund_reason = payload
            .get("reason")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(order.public_json())
    }

    pub fn retry_order_admin(&self, id: i64) -> Result<Value, ApiError> {
        let mut orders = self.orders_by_id.write().expect("payment order lock");
        let order = orders
            .get_mut(&id)
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        if order.status.is_final() && order.status != PaymentOrderStatus::Cancelled {
            return Err(ApiError::conflict(format!(
                "cannot retry order in {} status",
                order.status.as_str()
            )));
        }
        order.status = PaymentOrderStatus::Pending;
        order.cancelled_at = None;
        Ok(order.public_json())
    }

    pub fn verify_order(&self, user_id: i64, out_trade_no: String) -> Result<Value, ApiError> {
        let out_trade_no = out_trade_no.trim().to_owned();
        validate_out_trade_no(&out_trade_no)?;
        let id = self.order_id_by_trade_no(&out_trade_no)?;
        self.get_order(user_id, id)
    }

    pub fn verify_order_public(&self, out_trade_no: String) -> Result<Value, ApiError> {
        let out_trade_no = out_trade_no.trim().to_owned();
        validate_out_trade_no(&out_trade_no)?;
        let id = self.order_id_by_trade_no(&out_trade_no)?;
        let orders = self.orders_by_id.read().expect("payment order lock");
        let order = orders
            .get(&id)
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        Ok(order.public_limited_json())
    }

    pub fn resolve_order_public(&self, resume_token: String) -> Result<Value, ApiError> {
        let id = self
            .order_id_by_resume_token
            .read()
            .expect("payment resume lock")
            .get(&resume_token)
            .copied()
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        let orders = self.orders_by_id.read().expect("payment order lock");
        let order = orders
            .get(&id)
            .ok_or_else(|| ApiError::not_found("payment order not found"))?;
        Ok(order.public_limited_json())
    }

    pub fn refund_eligible_providers(&self) -> Value {
        json!({ "provider_instance_ids": ["alipay-default", "wxpay-default", "stripe-default"] })
    }

    pub fn record_event_batch(&self, payload: Value) -> Value {
        let id = self.next_event_id.fetch_add(1, Ordering::SeqCst);
        let count = payload
            .get("events")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_else(|| {
                if payload.is_null() || payload == json!({}) {
                    0
                } else {
                    1
                }
            });
        let record = json!({
            "id": id,
            "count": count,
            "payload": payload,
            "received_at": NOW
        });
        self.event_logs
            .write()
            .expect("payment event log lock")
            .push(record.clone());
        json!({
            "accepted": true,
            "id": id,
            "count": count,
            "received_at": NOW
        })
    }

    pub fn event_log_count(&self) -> usize {
        self.event_logs
            .read()
            .expect("payment event log lock")
            .len()
    }

    pub async fn handle_webhook(
        &self,
        provider: &str,
        method: &str,
        payload: Value,
        headers: &HashMap<String, String>,
        provider_instances: &[PaymentProviderInstanceRecord],
    ) -> Result<PaymentWebhookResult, ApiError> {
        let id = self.next_webhook_id.fetch_add(1, Ordering::SeqCst);
        let provider = normalize_payment_type(provider);
        let request_registry = self
            .provider_registry
            .as_ref()
            .clone()
            .with_configured_instances(provider_instances.to_vec());
        let preflight_out_trade_no = extract_trade_no(&payload);
        let pinned_provider_instance_id = preflight_out_trade_no
            .as_deref()
            .and_then(|out_trade_no| self.order_id_by_trade_no(out_trade_no).ok())
            .and_then(|order_id| {
                self.orders_by_id
                    .read()
                    .expect("payment order lock")
                    .get(&order_id)
                    .and_then(|order| order.provider_instance_id.clone())
            });
        let payment_provider = match request_registry
            .instance_for_webhook(&provider, pinned_provider_instance_id.as_deref())
            .map_err(payment_provider_api_error)?
        {
            Some(instance) => request_registry.configured_provider_for_instance(instance),
            None => request_registry
                .provider_for_key(&provider)
                .map_err(payment_provider_api_error)?,
        };
        let notification = payment_provider
            .verify_notification(&payload, headers)
            .await
            .map_err(payment_provider_api_error)?;
        let out_trade_no = notification
            .as_ref()
            .map(|notification| notification.order_id.clone())
            .or_else(|| extract_trade_no(&payload));
        let mut matched_order = false;
        let mut order_status = Value::Null;
        let mut order_snapshot = Value::Null;
        let mut idempotent = false;
        let mut audits = Vec::new();

        if let (Some(notification), Some(out_trade_no)) =
            (notification.as_ref(), out_trade_no.as_deref())
        {
            if let Ok(order_id) = self.order_id_by_trade_no(out_trade_no) {
                let mut orders = self.orders_by_id.write().expect("payment order lock");
                if let Some(order) = orders.get_mut(&order_id) {
                    validate_payment_notification(&provider, order, notification)?;
                    if order.status == PaymentOrderStatus::Completed {
                        idempotent = true;
                    }
                    if order.status == PaymentOrderStatus::Pending
                        && notification.status == "success"
                    {
                        order.status = PaymentOrderStatus::Paid;
                        order.pay_amount = notification.amount;
                        order.paid_at = Some(NOW.to_owned());
                        audits.push(payment_audit_json(
                            order,
                            "ORDER_PAID",
                            &provider,
                            json!({
                                "trade_no": notification.trade_no,
                                "paid_amount": notification.amount,
                                "provider": provider
                            }),
                        ));
                        order.status = PaymentOrderStatus::Recharging;
                        audits.push(payment_audit_json(
                            order,
                            "ORDER_RECHARGING",
                            "system",
                            json!({ "order_type": order.order_type }),
                        ));
                        execute_local_fulfillment(order, &mut audits);
                    }
                    order.webhook_count += 1;
                    matched_order = true;
                    order_status = json!(order.status.as_str());
                    order_snapshot = order.public_json();
                }
            }
        }

        let record = json!({
            "id": id,
            "provider": provider,
            "method": method,
            "out_trade_no": out_trade_no,
            "matched_order": matched_order,
            "order_status": order_status,
            "idempotent": idempotent,
            "payload": payload,
            "received_at": NOW
        });
        self.webhook_records
            .write()
            .expect("payment webhook lock")
            .push(record.clone());

        let public = json!({
            "success": true,
            "provider": provider,
            "webhook_id": id,
            "matched_order": matched_order,
            "order_status": order_status,
            "order": order_snapshot,
            "idempotent": idempotent,
            "notification": notification.as_ref().map(|notification| json!({
                "trade_no": notification.trade_no,
                "order_id": notification.order_id,
                "amount": notification.amount,
                "status": notification.status,
                "metadata": notification.metadata
            }))
        });
        Ok(PaymentWebhookResult {
            provider,
            record: public,
            audits,
        })
    }

    pub fn webhook_records(&self) -> Value {
        let records = self
            .webhook_records
            .read()
            .expect("payment webhook lock")
            .clone();
        json!({
            "items": records,
            "total": records.len(),
            "page": 1,
            "page_size": records.len().max(1)
        })
    }

    fn order_id_by_trade_no(&self, out_trade_no: &str) -> Result<i64, ApiError> {
        self.order_id_by_trade_no
            .read()
            .expect("payment trade lock")
            .get(out_trade_no)
            .copied()
            .ok_or_else(|| ApiError::not_found("payment order not found"))
    }

    fn find_order_for_user(&self, user_id: i64, id: i64) -> Result<PaymentOrderRecord, ApiError> {
        self.orders_by_id
            .read()
            .expect("payment order lock")
            .get(&id)
            .filter(|order| order.user_id == user_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("payment order not found"))
    }

    fn find_order(&self, id: i64) -> Result<PaymentOrderRecord, ApiError> {
        self.orders_by_id
            .read()
            .expect("payment order lock")
            .get(&id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("payment order not found"))
    }
}

fn method_limit(currency: &str) -> Value {
    json!({
        "currency": currency,
        "daily_limit": 100000.0,
        "daily_used": 0.0,
        "daily_remaining": 100000.0,
        "single_min": 1.0,
        "single_max": 10000.0,
        "fee_rate": 0.0,
        "available": true
    })
}

fn extract_trade_no(payload: &Value) -> Option<String> {
    if let Some(raw) = payload
        .get("_raw")
        .or_else(|| payload.get("raw_body"))
        .and_then(Value::as_str)
    {
        let params = crate::payment_provider::parse_form_encoded(raw);
        for key in ["out_trade_no", "trade_no", "order_id"] {
            if let Some(value) = params.get(key).filter(|value| !value.trim().is_empty()) {
                return Some(value.clone());
            }
        }
    }
    for key in [
        "out_trade_no",
        "outTradeNo",
        "trade_no",
        "order_id",
        "merchant_order_id",
        "merchantOrderId",
    ] {
        if let Some(value) = payload.get(key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    payload
        .get("data")
        .and_then(extract_trade_no)
        .or_else(|| payload.get("object").and_then(extract_trade_no))
}

fn normalize_payment_type(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "wechat" | "weixin" | "wx" | "wxpay_direct" => "wxpay".to_owned(),
        "alipay_direct" => "alipay".to_owned(),
        other => other.to_owned(),
    }
}

fn validate_payment_notification(
    provider: &str,
    order: &PaymentOrderRecord,
    notification: &PaymentNotification,
) -> Result<(), ApiError> {
    if !notification.status.eq_ignore_ascii_case("success")
        && !notification.status.eq_ignore_ascii_case("paid")
    {
        return Err(ApiError::bad_request(
            "payment notification is not successful",
        ));
    }
    let expected_provider = order
        .provider_instance_id
        .as_deref()
        .and_then(|value| value.split('-').next())
        .unwrap_or(order.payment_type.as_str());
    let pinned_configured_instance = order
        .provider_instance_id
        .as_deref()
        .is_some_and(|value| value.chars().all(|ch| ch.is_ascii_digit()));
    if !pinned_configured_instance && !expected_provider.eq_ignore_ascii_case(provider) {
        return Err(ApiError::bad_request(format!(
            "provider mismatch: expected {expected_provider}, got {provider}"
        )));
    }
    if notification.amount <= 0.0 || (notification.amount - order.pay_amount).abs() > 0.000_001 {
        return Err(ApiError::bad_request(format!(
            "amount mismatch: expected {}, got {}",
            order.pay_amount, notification.amount
        )));
    }
    Ok(())
}

fn provider_snapshot_json(
    response: &crate::payment_provider::CreatePaymentResponse,
    instance: Option<&PaymentProviderInstanceRecord>,
) -> Value {
    match instance {
        Some(instance) => json!({
            "schema_version": 2,
            "provider_instance_id": instance_identifier(instance),
            "provider_key": instance.provider_key,
            "payment_mode": response.payment_mode,
            "currency": response.currency,
            "merchant_app_id": provider_config_string(&instance.config, &["appId", "appid", "mpAppId", "mp_app_id"]),
            "merchant_id": provider_config_string(&instance.config, &["merchantId", "merchant_id", "mchId", "mchid", "accountId", "account_id"])
        }),
        None => json!({
            "schema_version": 2,
            "provider_instance_id": response.provider_instance_id,
            "provider_key": response.provider_key,
            "payment_mode": response.payment_mode,
            "currency": response.currency
        }),
    }
}

fn provider_config_string(config: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = config.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

fn execute_local_fulfillment(order: &mut PaymentOrderRecord, audits: &mut Vec<Value>) {
    order.status = PaymentOrderStatus::Completed;
    order.completed_at = Some(NOW.to_owned());
    audits.push(payment_audit_json(
        order,
        "ORDER_COMPLETED",
        "system",
        json!({
            "mode": "local",
            "order_type": order.order_type,
            "amount": order.amount
        }),
    ));
}

fn payment_audit_json(
    order: &PaymentOrderRecord,
    action: &str,
    operator: &str,
    detail: Value,
) -> Value {
    json!({
        "order_id": order.out_trade_no,
        "action": action,
        "detail": detail,
        "operator": operator,
        "created_at": NOW
    })
}

fn payment_provider_api_error(error: PaymentProviderError) -> ApiError {
    match error {
        PaymentProviderError::UnsupportedProvider(message) => ApiError::bad_request(message),
        PaymentProviderError::VerifyFailed => ApiError::bad_request("verify failed"),
        PaymentProviderError::Provider(message) => ApiError::internal_server_error(message),
    }
}

fn validate_out_trade_no(out_trade_no: &str) -> Result<(), ApiError> {
    let trimmed = out_trade_no.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("out_trade_no is required"));
    }
    if trimmed.len() > 64 {
        return Err(ApiError::bad_request("out_trade_no is too long"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ApiError::bad_request(
            "out_trade_no contains invalid characters",
        ));
    }
    Ok(())
}

fn query_i64(query: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    query
        .get(key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn paginated(items: Vec<Value>, total: i64, page: i64, page_size: i64) -> Value {
    json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": if page_size <= 0 { 0 } else { (total + page_size - 1) / page_size }
    })
}
