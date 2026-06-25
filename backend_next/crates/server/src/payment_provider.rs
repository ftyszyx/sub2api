use async_trait::async_trait;
use repository::PaymentProviderInstanceRecord;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PaymentProviderError {
    #[error("unsupported payment provider: {0}")]
    UnsupportedProvider(String),

    #[error("verify failed")]
    VerifyFailed,

    #[error("provider error: {0}")]
    Provider(String),
}

#[derive(Debug, Clone)]
pub struct CreatePaymentRequest {
    pub order_id: i64,
    pub out_trade_no: String,
    pub amount: f64,
    pub payment_type: String,
    pub return_url: Option<String>,
    pub is_mobile: bool,
}

#[derive(Debug, Clone)]
pub struct CreatePaymentResponse {
    pub provider_key: String,
    pub provider_instance_id: String,
    pub trade_no: String,
    pub pay_url: Option<String>,
    pub qr_code: Option<String>,
    pub client_secret: Option<String>,
    pub intent_id: Option<String>,
    pub currency: String,
    pub country_code: String,
    pub payment_env: String,
    pub result_type: String,
    pub payment_mode: String,
}

#[derive(Debug, Clone)]
pub struct PaymentNotification {
    pub trade_no: String,
    pub order_id: String,
    pub amount: f64,
    pub status: String,
    pub metadata: HashMap<String, String>,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn provider_key(&self) -> &'static str;
    fn supported_types(&self) -> &'static [&'static str];

    async fn create_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse, PaymentProviderError>;

    async fn verify_notification(
        &self,
        payload: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<Option<PaymentNotification>, PaymentProviderError>;
}

#[derive(Clone)]
pub struct PaymentProviderRegistry {
    providers: HashMap<String, Arc<dyn PaymentProvider>>,
    configured_instances: Vec<PaymentProviderInstanceRecord>,
}

impl PaymentProviderRegistry {
    pub fn mock_default() -> Self {
        let mut providers = HashMap::new();
        for provider in [
            Arc::new(MockPaymentProvider::new("alipay", &["alipay"])) as Arc<dyn PaymentProvider>,
            Arc::new(MockPaymentProvider::new("wxpay", &["wxpay"])),
            Arc::new(MockPaymentProvider::new(
                "stripe",
                &["stripe", "card", "link"],
            )),
            Arc::new(MockPaymentProvider::new("easypay", &["easypay"])),
            Arc::new(MockPaymentProvider::new("airwallex", &["airwallex"])),
        ] {
            providers.insert(provider.provider_key().to_owned(), provider);
        }
        Self {
            providers,
            configured_instances: Vec::new(),
        }
    }

    pub fn with_configured_instances(
        mut self,
        configured_instances: Vec<PaymentProviderInstanceRecord>,
    ) -> Self {
        self.configured_instances = configured_instances
            .into_iter()
            .map(normalize_instance)
            .collect::<Vec<_>>();
        self.configured_instances
            .sort_by_key(|instance| (instance.sort_order, instance.id));
        self
    }

    pub fn provider_for_key(
        &self,
        provider_key: &str,
    ) -> Result<Arc<dyn PaymentProvider>, PaymentProviderError> {
        self.providers
            .get(provider_key)
            .cloned()
            .ok_or_else(|| PaymentProviderError::UnsupportedProvider(provider_key.to_owned()))
    }

    pub fn configured_instances(&self) -> &[PaymentProviderInstanceRecord] {
        &self.configured_instances
    }

    pub fn select_instance_for_payment_type(
        &self,
        payment_type: &str,
    ) -> Result<PaymentProviderInstanceRecord, PaymentProviderError> {
        let payment_type = normalize_payment_key(payment_type);
        self.configured_instances
            .iter()
            .find(|instance| instance.enabled && instance_supports_type(instance, &payment_type))
            .cloned()
            .ok_or_else(|| PaymentProviderError::UnsupportedProvider(payment_type))
    }

    pub fn instance_for_webhook(
        &self,
        provider_key: &str,
        provider_instance_id: Option<&str>,
    ) -> Result<Option<PaymentProviderInstanceRecord>, PaymentProviderError> {
        let provider_key = normalize_payment_key(provider_key);
        if let Some(provider_instance_id) = provider_instance_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let Some(instance) = self.configured_instances.iter().find(|instance| {
                instance.id.to_string() == provider_instance_id
                    || instance_identifier(instance).eq_ignore_ascii_case(provider_instance_id)
            }) else {
                if !provider_instance_id.chars().all(|ch| ch.is_ascii_digit()) {
                    return Ok(None);
                }
                return Err(PaymentProviderError::Provider(format!(
                    "provider instance {provider_instance_id} not found"
                )));
            };
            if instance.provider_key != provider_key {
                return Err(PaymentProviderError::VerifyFailed);
            }
            return Ok(Some(instance.clone()));
        }
        Ok(self
            .configured_instances
            .iter()
            .find(|instance| instance.provider_key == provider_key && instance.enabled)
            .cloned())
    }

    pub fn configured_provider_for_instance(
        &self,
        instance: PaymentProviderInstanceRecord,
    ) -> Arc<dyn PaymentProvider> {
        Arc::new(ConfiguredPaymentProvider::new(instance))
    }

    pub fn provider_for_payment_type(
        &self,
        payment_type: &str,
    ) -> Result<Arc<dyn PaymentProvider>, PaymentProviderError> {
        let payment_type = payment_type.trim().to_ascii_lowercase();
        self.providers
            .values()
            .find(|provider| {
                provider
                    .supported_types()
                    .iter()
                    .any(|supported| *supported == payment_type)
            })
            .cloned()
            .ok_or_else(|| PaymentProviderError::UnsupportedProvider(payment_type))
    }
}

struct MockPaymentProvider {
    provider_key: &'static str,
    supported_types: &'static [&'static str],
}

impl MockPaymentProvider {
    const fn new(provider_key: &'static str, supported_types: &'static [&'static str]) -> Self {
        Self {
            provider_key,
            supported_types,
        }
    }
}

#[async_trait]
impl PaymentProvider for MockPaymentProvider {
    fn provider_key(&self) -> &'static str {
        self.provider_key
    }

    fn supported_types(&self) -> &'static [&'static str] {
        self.supported_types
    }

    async fn create_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse, PaymentProviderError> {
        let currency = if self.provider_key == "stripe" {
            "USD"
        } else {
            "CNY"
        };
        let provider_instance_id = format!("{}-default", self.provider_key);
        Ok(CreatePaymentResponse {
            provider_key: self.provider_key.to_owned(),
            provider_instance_id,
            trade_no: format!("mock_{}_{}", self.provider_key, request.out_trade_no),
            pay_url: Some(format!(
                "https://pay.local/{}/orders/{}",
                self.provider_key, request.out_trade_no
            )),
            qr_code: Some(format!(
                "https://pay.local/{}/qrcode/{}",
                self.provider_key, request.out_trade_no
            )),
            client_secret: (self.provider_key == "stripe")
                .then(|| format!("pi_{}_secret_mock", request.order_id)),
            intent_id: (self.provider_key == "stripe").then(|| format!("pi_{}", request.order_id)),
            currency: currency.to_owned(),
            country_code: "CN".to_owned(),
            payment_env: "development".to_owned(),
            result_type: "order_created".to_owned(),
            payment_mode: if request.is_mobile {
                "redirect"
            } else if self.provider_key == "stripe" {
                "popup"
            } else {
                "redirect"
            }
            .to_owned(),
        })
    }

    async fn verify_notification(
        &self,
        payload: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<Option<PaymentNotification>, PaymentProviderError> {
        if payload
            .get("event")
            .and_then(Value::as_str)
            .is_some_and(|event| event == "ignored")
        {
            return Ok(None);
        }
        let signature = headers
            .get("x-payment-mock-signature")
            .or_else(|| headers.get("x-mock-signature"))
            .cloned()
            .or_else(|| {
                payload
                    .get("mock_signature")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        if signature != "valid" {
            return Err(PaymentProviderError::VerifyFailed);
        }
        let order_id = string_payload_field(payload, &["out_trade_no", "order_id"])
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let amount = f64_payload_field(payload, &["amount", "pay_amount"])
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let trade_no = string_payload_field(payload, &["trade_no", "transaction_id"])
            .unwrap_or_else(|| format!("mock_{}_{}", self.provider_key, order_id));
        let status = string_payload_field(payload, &["status"])
            .unwrap_or_else(|| "success".to_owned())
            .to_ascii_lowercase();
        Ok(Some(PaymentNotification {
            trade_no,
            order_id,
            amount,
            status,
            metadata: HashMap::new(),
        }))
    }
}

struct ConfiguredPaymentProvider {
    instance: PaymentProviderInstanceRecord,
}

impl ConfiguredPaymentProvider {
    fn new(instance: PaymentProviderInstanceRecord) -> Self {
        Self { instance }
    }

    fn currency(&self) -> String {
        config_string(&self.instance.config, &["currency"]).unwrap_or_else(|| {
            if self.instance.provider_key == "stripe" || self.instance.provider_key == "airwallex" {
                "USD".to_owned()
            } else {
                "CNY".to_owned()
            }
        })
    }

    fn payment_mode(&self, is_mobile: bool) -> String {
        if !self.instance.payment_mode.trim().is_empty() {
            return self.instance.payment_mode.trim().to_owned();
        }
        if is_mobile {
            "redirect".to_owned()
        } else if self.instance.provider_key == "stripe" {
            "popup".to_owned()
        } else {
            "redirect".to_owned()
        }
    }

    fn create_easypay_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse, PaymentProviderError> {
        let pid = required_config_string(&self.instance.config, &["pid"])?;
        let pkey = required_config_string(&self.instance.config, &["pkey"])?;
        let api_base = normalize_easypay_api_base(&required_config_string(
            &self.instance.config,
            &["apiBase", "api_base"],
        )?);
        let notify_url = config_string(&self.instance.config, &["notifyUrl", "notify_url"])
            .unwrap_or_else(|| {
                format!("{}/payment/webhook/easypay", api_base.trim_end_matches('/'))
            });
        let return_url = request
            .return_url
            .clone()
            .or_else(|| config_string(&self.instance.config, &["returnUrl", "return_url"]))
            .unwrap_or_default();
        let mut params = HashMap::from([
            ("pid".to_owned(), pid),
            (
                "type".to_owned(),
                normalize_payment_key(&request.payment_type),
            ),
            ("out_trade_no".to_owned(), request.out_trade_no.clone()),
            ("notify_url".to_owned(), notify_url),
            ("return_url".to_owned(), return_url),
            ("name".to_owned(), format!("Order {}", request.out_trade_no)),
            ("money".to_owned(), format_amount(request.amount)),
        ]);
        if let Some(cid) = resolve_easypay_cid(&self.instance.config, &request.payment_type) {
            params.insert("cid".to_owned(), cid);
        }
        if request.is_mobile {
            params.insert("device".to_owned(), "mobile".to_owned());
        }
        let sign = easypay_sign(&params, &pkey);
        params.insert("sign".to_owned(), sign);
        params.insert("sign_type".to_owned(), "MD5".to_owned());
        let pay_url = format!(
            "{}/submit.php?{}",
            api_base.trim_end_matches('/'),
            form_urlencode(&params)
        );
        Ok(CreatePaymentResponse {
            provider_key: self.instance.provider_key.clone(),
            provider_instance_id: instance_identifier(&self.instance),
            trade_no: request.out_trade_no,
            pay_url: Some(pay_url),
            qr_code: None,
            client_secret: None,
            intent_id: None,
            currency: self.currency(),
            country_code: config_string(&self.instance.config, &["countryCode", "country_code"])
                .unwrap_or_else(|| "CN".to_owned()),
            payment_env: config_string(&self.instance.config, &["paymentEnv", "payment_env"])
                .unwrap_or_else(|| "production".to_owned()),
            result_type: "order_created".to_owned(),
            payment_mode: self.payment_mode(request.is_mobile),
        })
    }

    async fn create_stripe_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse, PaymentProviderError> {
        let secret_key =
            required_config_string(&self.instance.config, &["secretKey", "secret_key"])?;
        let api_base = normalize_stripe_api_base(
            &config_string(&self.instance.config, &["apiBase", "api_base"])
                .unwrap_or_else(|| "https://api.stripe.com".to_owned()),
        );
        let currency = normalize_currency(&self.currency());
        validate_payment_currency(&currency)?;
        let amount = amount_to_minor_unit(request.amount, &currency)?;
        let methods = stripe_method_types(&self.instance.supported_types);
        let mut params = vec![
            ("amount".to_owned(), amount.to_string()),
            ("currency".to_owned(), currency.to_ascii_lowercase()),
            (
                "description".to_owned(),
                format!("Order {}", request.out_trade_no),
            ),
            ("metadata[orderId]".to_owned(), request.out_trade_no.clone()),
        ];
        for method in methods {
            params.push(("payment_method_types[]".to_owned(), method));
        }
        if params
            .iter()
            .any(|(key, value)| key == "payment_method_types[]" && value == "wechat_pay")
        {
            params.push((
                "payment_method_options[wechat_pay][client]".to_owned(),
                "web".to_owned(),
            ));
        }

        let body = form_urlencode_pairs(&params);
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1/payment_intents",
                api_base.trim_end_matches('/')
            ))
            .bearer_auth(secret_key)
            .header("idempotency-key", format!("pi-{}", request.out_trade_no))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|error| {
                PaymentProviderError::Provider(format!("stripe create payment: {error}"))
            })?;
        let status = response.status();
        let payload = response.json::<Value>().await.map_err(|error| {
            PaymentProviderError::Provider(format!("stripe create payment response: {error}"))
        })?;
        if !status.is_success() {
            return Err(PaymentProviderError::Provider(format!(
                "stripe create payment failed: {}",
                stripe_error_message(&payload)
            )));
        }
        let intent_id = payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| PaymentProviderError::Provider("stripe response missing id".to_owned()))?
            .to_owned();
        let client_secret = payload
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                PaymentProviderError::Provider("stripe response missing client_secret".to_owned())
            })?
            .to_owned();
        Ok(CreatePaymentResponse {
            provider_key: self.instance.provider_key.clone(),
            provider_instance_id: instance_identifier(&self.instance),
            trade_no: intent_id.clone(),
            pay_url: None,
            qr_code: None,
            client_secret: Some(client_secret),
            intent_id: Some(intent_id),
            currency,
            country_code: config_string(&self.instance.config, &["countryCode", "country_code"])
                .unwrap_or_else(|| "US".to_owned()),
            payment_env: config_string(&self.instance.config, &["paymentEnv", "payment_env"])
                .unwrap_or_else(|| "production".to_owned()),
            result_type: "payment_intent".to_owned(),
            payment_mode: self.payment_mode(request.is_mobile),
        })
    }

    fn verify_easypay_notification(
        &self,
        payload: &Value,
    ) -> Result<PaymentNotification, PaymentProviderError> {
        let pkey = required_config_string(&self.instance.config, &["pkey"])?;
        let params = easypay_params_from_payload(payload)?;
        let sign = params
            .get("sign")
            .filter(|value| !value.trim().is_empty())
            .ok_or(PaymentProviderError::VerifyFailed)?;
        if !easypay_verify_sign(&params, &pkey, sign) {
            return Err(PaymentProviderError::VerifyFailed);
        }
        let order_id = params
            .get("out_trade_no")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let amount = params
            .get("money")
            .or_else(|| params.get("amount"))
            .and_then(|value| value.trim().parse::<f64>().ok())
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let trade_no = params
            .get("trade_no")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| order_id.clone());
        let status = match params.get("trade_status").map(String::as_str) {
            Some("TRADE_SUCCESS") => "success",
            _ => "failed",
        }
        .to_owned();
        let mut metadata = HashMap::from([
            (
                "provider_instance_id".to_owned(),
                instance_identifier(&self.instance),
            ),
            (
                "provider_key".to_owned(),
                self.instance.provider_key.clone(),
            ),
        ]);
        if let Some(pid) = params.get("pid").filter(|value| !value.trim().is_empty()) {
            metadata.insert("pid".to_owned(), pid.clone());
        }
        Ok(PaymentNotification {
            trade_no,
            order_id,
            amount,
            status,
            metadata,
        })
    }

    fn verify_stripe_notification(
        &self,
        payload: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<Option<PaymentNotification>, PaymentProviderError> {
        if let Some(signature_header) = headers.get("stripe-signature") {
            let webhook_secret = required_config_string(
                &self.instance.config,
                &["webhookSecret", "webhook_secret"],
            )?;
            let raw_body = payload
                .get("_raw")
                .or_else(|| payload.get("raw_body"))
                .and_then(Value::as_str)
                .ok_or(PaymentProviderError::VerifyFailed)?;
            verify_stripe_signature(raw_body, signature_header, &webhook_secret)?;
            return parse_stripe_event(payload);
        }

        self.verify_legacy_configured_notification(payload, headers)
    }

    fn verify_legacy_configured_notification(
        &self,
        payload: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<Option<PaymentNotification>, PaymentProviderError> {
        if payload
            .get("event")
            .and_then(Value::as_str)
            .is_some_and(|event| event == "ignored")
        {
            return Ok(None);
        }
        if let Some(expected) = config_string(
            &self.instance.config,
            &[
                "webhookSecret",
                "webhook_secret",
                "webhookToken",
                "webhook_token",
            ],
        ) {
            let actual = headers
                .get("x-payment-webhook-secret")
                .or_else(|| headers.get("x-webhook-secret"))
                .or_else(|| headers.get("x-payment-mock-signature"))
                .or_else(|| headers.get("x-mock-signature"))
                .cloned()
                .or_else(|| {
                    payload
                        .get("webhook_secret")
                        .or_else(|| payload.get("mock_signature"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_default();
            if actual != expected {
                return Err(PaymentProviderError::VerifyFailed);
            }
        } else {
            let signature = headers
                .get("x-payment-mock-signature")
                .or_else(|| headers.get("x-mock-signature"))
                .cloned()
                .or_else(|| {
                    payload
                        .get("mock_signature")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_default();
            if signature != "valid" {
                return Err(PaymentProviderError::VerifyFailed);
            }
        }

        let order_id = string_payload_field(payload, &["out_trade_no", "order_id"])
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let amount = f64_payload_field(payload, &["amount", "pay_amount"])
            .ok_or(PaymentProviderError::VerifyFailed)?;
        let trade_no = string_payload_field(payload, &["trade_no", "transaction_id"])
            .unwrap_or_else(|| format!("mock_{}_{}", self.instance.provider_key, order_id));
        let status = string_payload_field(payload, &["status"])
            .unwrap_or_else(|| "success".to_owned())
            .to_ascii_lowercase();
        Ok(Some(PaymentNotification {
            trade_no,
            order_id,
            amount,
            status,
            metadata: HashMap::from([
                (
                    "provider_instance_id".to_owned(),
                    instance_identifier(&self.instance),
                ),
                (
                    "provider_key".to_owned(),
                    self.instance.provider_key.clone(),
                ),
            ]),
        }))
    }
}

#[async_trait]
impl PaymentProvider for ConfiguredPaymentProvider {
    fn provider_key(&self) -> &'static str {
        "configured"
    }

    fn supported_types(&self) -> &'static [&'static str] {
        &[]
    }

    async fn create_payment(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatePaymentResponse, PaymentProviderError> {
        if self.instance.provider_key == "easypay" {
            return self.create_easypay_payment(request);
        }
        if self.instance.provider_key == "stripe" {
            return self.create_stripe_payment(request).await;
        }
        let currency = self.currency();
        let provider_key = self.instance.provider_key.clone();
        let provider_instance_id = instance_identifier(&self.instance);
        Ok(CreatePaymentResponse {
            provider_key: provider_key.clone(),
            provider_instance_id,
            trade_no: format!("mock_{}_{}", provider_key, request.out_trade_no),
            pay_url: Some(format!(
                "https://pay.local/{}/orders/{}",
                provider_key, request.out_trade_no
            )),
            qr_code: Some(format!(
                "https://pay.local/{}/qrcode/{}",
                provider_key, request.out_trade_no
            )),
            client_secret: (provider_key == "stripe")
                .then(|| format!("pi_{}_secret_configured", request.order_id)),
            intent_id: (provider_key == "stripe").then(|| format!("pi_{}", request.order_id)),
            currency,
            country_code: config_string(&self.instance.config, &["countryCode", "country_code"])
                .unwrap_or_else(|| "CN".to_owned()),
            payment_env: config_string(&self.instance.config, &["paymentEnv", "payment_env"])
                .unwrap_or_else(|| "development".to_owned()),
            result_type: "order_created".to_owned(),
            payment_mode: self.payment_mode(request.is_mobile),
        })
    }

    async fn verify_notification(
        &self,
        payload: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<Option<PaymentNotification>, PaymentProviderError> {
        if self.instance.provider_key == "easypay" {
            return self.verify_easypay_notification(payload).map(Some);
        }
        if self.instance.provider_key == "stripe" {
            return self.verify_stripe_notification(payload, headers);
        }
        self.verify_legacy_configured_notification(payload, headers)
    }
}

fn parse_stripe_event(
    payload: &Value,
) -> Result<Option<PaymentNotification>, PaymentProviderError> {
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| payload.get("event").and_then(Value::as_str))
        .ok_or(PaymentProviderError::VerifyFailed)?;
    let status = match event_type {
        "payment_intent.succeeded" => "success",
        "payment_intent.payment_failed" => "failed",
        _ => return Ok(None),
    };
    let object = payload
        .get("data")
        .and_then(|data| data.get("object"))
        .unwrap_or(payload);
    let trade_no = string_payload_field(object, &["id", "trade_no", "transaction_id"])
        .ok_or(PaymentProviderError::VerifyFailed)?;
    let order_id = stripe_metadata_field(object, &["orderId", "order_id", "out_trade_no"])
        .or_else(|| string_payload_field(object, &["out_trade_no", "order_id"]))
        .ok_or(PaymentProviderError::VerifyFailed)?;
    let currency = string_payload_field(object, &["currency"]).unwrap_or_else(|| "USD".to_owned());
    let amount = stripe_amount_field(object, &["amount_received", "amount", "amount_total"])
        .map(|amount| stripe_minor_to_amount(amount, &currency))
        .or_else(|| f64_payload_field(object, &["pay_amount"]))
        .ok_or(PaymentProviderError::VerifyFailed)?;
    let mut metadata = HashMap::from([
        ("provider_key".to_owned(), "stripe".to_owned()),
        ("currency".to_owned(), normalize_currency(&currency)),
        ("event_type".to_owned(), event_type.to_owned()),
    ]);
    if let Some(event_id) = payload.get("id").and_then(Value::as_str) {
        metadata.insert("event_id".to_owned(), event_id.to_owned());
    }
    Ok(Some(PaymentNotification {
        trade_no,
        order_id,
        amount,
        status: status.to_owned(),
        metadata,
    }))
}

fn normalize_stripe_api_base(value: &str) -> String {
    let mut base = value.trim().trim_end_matches('/').to_owned();
    if base.to_ascii_lowercase().ends_with("/v1/payment_intents") {
        base.truncate(base.len() - "/v1/payment_intents".len());
    } else if base.to_ascii_lowercase().ends_with("/v1") {
        base.truncate(base.len() - "/v1".len());
    }
    base.trim_end_matches('/').to_owned()
}

fn stripe_method_types(supported_types: &[String]) -> Vec<String> {
    let mut methods = Vec::new();
    for supported_type in supported_types {
        let mapped = match normalize_payment_key(supported_type).as_str() {
            "card" | "stripe" => Some("card"),
            "alipay" => Some("alipay"),
            "wxpay" => Some("wechat_pay"),
            "link" => Some("link"),
            _ => None,
        };
        if let Some(mapped) = mapped {
            if !methods.iter().any(|method| method == mapped) {
                methods.push(mapped.to_owned());
            }
        }
    }
    if methods.is_empty() {
        methods.push("card".to_owned());
    }
    methods
}

fn validate_payment_currency(currency: &str) -> Result<(), PaymentProviderError> {
    if currency.len() != 3 || !currency.chars().all(|ch| ch.is_ascii_uppercase()) {
        return Err(PaymentProviderError::Provider(
            "payment currency must be a 3-letter ISO currency code".to_owned(),
        ));
    }
    Ok(())
}

fn amount_to_minor_unit(amount: f64, currency: &str) -> Result<i64, PaymentProviderError> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err(PaymentProviderError::Provider(
            "payment amount must be greater than zero".to_owned(),
        ));
    }
    let max_fraction_digits = currency_max_fraction_digits(currency);
    let display_factor = 10_f64.powi(max_fraction_digits as i32);
    if (amount * display_factor).fract().abs() > 0.000_000_1 {
        return Err(PaymentProviderError::Provider(format!(
            "payment amount for {} must not have more than {} decimal places",
            normalize_currency(currency),
            max_fraction_digits
        )));
    }
    let api_factor = 10_f64.powi(currency_api_minor_unit(currency) as i32);
    Ok((amount * api_factor).round() as i64)
}

fn currency_api_minor_unit(currency: &str) -> i32 {
    match normalize_currency(currency).as_str() {
        "BIF" | "CLP" | "DJF" | "GNF" | "JPY" | "KMF" | "KRW" | "MGA" | "PYG" | "RWF" | "VND"
        | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn currency_max_fraction_digits(currency: &str) -> i32 {
    match normalize_currency(currency).as_str() {
        "ISK" | "UGX" => 0,
        "BIF" | "CLP" | "DJF" | "GNF" | "JPY" | "KMF" | "KRW" | "MGA" | "PYG" | "RWF" | "VND"
        | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn stripe_error_message(payload: &Value) -> String {
    payload
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .unwrap_or("unknown error")
        .to_owned()
}

fn stripe_metadata_field(payload: &Value, keys: &[&str]) -> Option<String> {
    let metadata = payload.get("metadata")?.as_object()?;
    for key in keys {
        if let Some(value) = metadata.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

fn stripe_amount_field(payload: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(number) = value.as_i64() {
                return Some(number);
            }
            if let Some(number) = value.as_str().and_then(|value| value.trim().parse().ok()) {
                return Some(number);
            }
        }
    }
    None
}

fn stripe_minor_to_amount(amount: i64, currency: &str) -> f64 {
    let factor = 10_f64.powi(currency_api_minor_unit(currency));
    amount as f64 / factor
}

fn normalize_currency(currency: &str) -> String {
    currency.trim().to_ascii_uppercase()
}

fn verify_stripe_signature(
    raw_body: &str,
    signature_header: &str,
    webhook_secret: &str,
) -> Result<(), PaymentProviderError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in signature_header.split(',') {
        let mut split = part.trim().splitn(2, '=');
        let key = split.next().unwrap_or_default();
        let value = split.next().unwrap_or_default().trim();
        match key {
            "t" if !value.is_empty() => timestamp = Some(value.to_owned()),
            "v1" if !value.is_empty() => signatures.push(value.to_owned()),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(PaymentProviderError::VerifyFailed)?;
    if signatures.is_empty() {
        return Err(PaymentProviderError::VerifyFailed);
    }
    let expected = stripe_webhook_signature(raw_body, webhook_secret, &timestamp);
    if signatures
        .iter()
        .any(|signature| constant_time_ascii_eq(signature, &expected))
    {
        Ok(())
    } else {
        Err(PaymentProviderError::VerifyFailed)
    }
}

pub(crate) fn stripe_webhook_signature(
    raw_body: &str,
    webhook_secret: &str,
    timestamp: &str,
) -> String {
    let signed_payload = format!("{timestamp}.{raw_body}");
    hmac_sha256_hex(webhook_secret.as_bytes(), signed_payload.as_bytes())
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let digest = outer.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn constant_time_ascii_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left.to_ascii_lowercase() ^ right.to_ascii_lowercase();
    }
    diff == 0
}

fn string_payload_field(payload: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = payload.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    payload
        .get("data")
        .and_then(|value| string_payload_field(value, keys))
        .or_else(|| {
            payload
                .get("object")
                .and_then(|value| string_payload_field(value, keys))
        })
}

fn normalize_instance(
    mut instance: PaymentProviderInstanceRecord,
) -> PaymentProviderInstanceRecord {
    instance.provider_key = normalize_payment_key(&instance.provider_key);
    instance.supported_types = normalize_payment_keys(&instance.supported_types);
    instance
}

fn normalize_payment_key(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "wechat" | "weixin" | "wx" | "wxpay_direct" => "wxpay".to_owned(),
        "alipay_direct" => "alipay".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_payment_keys(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let value = normalize_payment_key(value);
        if !value.is_empty() && !normalized.iter().any(|existing| existing == &value) {
            normalized.push(value);
        }
    }
    normalized
}

fn instance_supports_type(instance: &PaymentProviderInstanceRecord, payment_type: &str) -> bool {
    if payment_type == "stripe" && instance.provider_key == "stripe" {
        return true;
    }
    let payment_type = normalize_payment_key(payment_type);
    if instance.supported_types.is_empty() {
        return instance.provider_key == payment_type;
    }
    instance
        .supported_types
        .iter()
        .any(|supported| normalize_payment_key(supported) == payment_type)
}

pub fn instance_identifier(instance: &PaymentProviderInstanceRecord) -> String {
    if instance.id > 0 {
        instance.id.to_string()
    } else {
        format!("{}-default", instance.provider_key)
    }
}

fn config_string(config: &Value, keys: &[&str]) -> Option<String> {
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

fn required_config_string(config: &Value, keys: &[&str]) -> Result<String, PaymentProviderError> {
    config_string(config, keys).ok_or_else(|| {
        PaymentProviderError::Provider(format!("missing config key {}", keys.join("/")))
    })
}

fn normalize_easypay_api_base(value: &str) -> String {
    let mut base = value.trim().trim_end_matches('/').to_owned();
    for endpoint in ["/submit.php", "/mapi.php", "/api.php"] {
        if base.to_ascii_lowercase().ends_with(endpoint) {
            let keep = base.len() - endpoint.len();
            base.truncate(keep);
            base = base.trim_end_matches('/').to_owned();
            break;
        }
    }
    base
}

fn resolve_easypay_cid(config: &Value, payment_type: &str) -> Option<String> {
    if normalize_payment_key(payment_type).starts_with("alipay") {
        return config_string(config, &["cidAlipay", "cid_alipay"])
            .or_else(|| config_string(config, &["cid"]));
    }
    config_string(config, &["cidWxpay", "cid_wxpay"]).or_else(|| config_string(config, &["cid"]))
}

fn format_amount(amount: f64) -> String {
    let mut text = format!("{amount:.2}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn easypay_params_from_payload(
    payload: &Value,
) -> Result<HashMap<String, String>, PaymentProviderError> {
    if let Some(raw) = payload.get("_raw").and_then(Value::as_str) {
        return Ok(parse_form_encoded(raw));
    }
    if let Some(raw) = payload.get("raw_body").and_then(Value::as_str) {
        return Ok(parse_form_encoded(raw));
    }
    let object = payload
        .as_object()
        .ok_or(PaymentProviderError::VerifyFailed)?;
    let mut params = HashMap::new();
    for (key, value) in object {
        if key.starts_with('_') {
            continue;
        }
        if let Some(text) = value.as_str() {
            params.insert(key.clone(), text.to_owned());
        } else if value.is_number() || value.is_boolean() {
            params.insert(key.clone(), value.to_string());
        }
    }
    Ok(params)
}

pub(crate) fn easypay_sign(params: &HashMap<String, String>, pkey: &str) -> String {
    let mut keys = params
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "sign" && key.as_str() != "sign_type" && !value.is_empty()
        })
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    let mut sign_source = String::new();
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            sign_source.push('&');
        }
        sign_source.push_str(key);
        sign_source.push('=');
        sign_source.push_str(params.get(*key).map(String::as_str).unwrap_or_default());
    }
    sign_source.push_str(pkey);
    md5_hex(sign_source.as_bytes())
}

fn easypay_verify_sign(params: &HashMap<String, String>, pkey: &str, sign: &str) -> bool {
    let expected = easypay_sign(params, pkey);
    expected.eq_ignore_ascii_case(sign.trim())
}

fn md5_hex(input: &[u8]) -> String {
    let digest = md5_bytes(input);
    let mut output = String::with_capacity(32);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn md5_bytes(input: &[u8]) -> [u8; 16] {
    let mut message = input.to_vec();
    let bit_len = (message.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    for chunk in message.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (index, word) in m.iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut output = [0u8; 16];
    output[0..4].copy_from_slice(&a0.to_le_bytes());
    output[4..8].copy_from_slice(&b0.to_le_bytes());
    output[8..12].copy_from_slice(&c0.to_le_bytes());
    output[12..16].copy_from_slice(&d0.to_le_bytes());
    output
}

pub(crate) fn form_urlencode(params: &HashMap<String, String>) -> String {
    let mut keys = params.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    keys.into_iter()
        .map(|key| {
            format!(
                "{}={}",
                percent_encode(key.as_bytes()),
                percent_encode(params.get(key).map(String::as_bytes).unwrap_or_default())
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn form_urlencode_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                percent_encode(key.as_bytes()),
                percent_encode(value.as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub(crate) fn parse_form_encoded(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let mut split = part.splitn(2, '=');
            let key = percent_decode(split.next().unwrap_or_default())?;
            let value = percent_decode(split.next().unwrap_or_default())?;
            Some((key, value))
        })
        .collect()
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut output = String::new();
    for &byte in bytes {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            b' ' => output.push('+'),
            _ => {
                let _ = write!(output, "%{byte:02X}");
            }
        }
    }
    output
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1])?;
                let low = hex_value(bytes[index + 2])?;
                output.push((high << 4) | low);
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn f64_payload_field(payload: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            if let Some(number) = value.as_f64() {
                return Some(number);
            }
            if let Some(text) = value.as_str().and_then(|value| value.trim().parse().ok()) {
                return Some(text);
            }
        }
    }
    payload
        .get("data")
        .and_then(|value| f64_payload_field(value, keys))
        .or_else(|| {
            payload
                .get("object")
                .and_then(|value| f64_payload_field(value, keys))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn easypay_instance() -> PaymentProviderInstanceRecord {
        PaymentProviderInstanceRecord {
            id: 7,
            provider_key: "easypay".to_owned(),
            name: "EasyPay".to_owned(),
            config: json!({
                "pid": "1001",
                "pkey": "secret",
                "apiBase": "https://pay.example.com/mapi.php",
                "notifyUrl": "https://app.example.com/api/v1/payment/webhook/easypay",
                "returnUrl": "https://app.example.com/payment/result"
            }),
            supported_types: vec!["alipay".to_owned(), "wxpay".to_owned()],
            enabled: true,
            payment_mode: "popup".to_owned(),
            sort_order: 1,
            limits: json!({}),
            refund_enabled: true,
            allow_user_refund: true,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    fn stripe_instance() -> PaymentProviderInstanceRecord {
        PaymentProviderInstanceRecord {
            id: 9,
            provider_key: "stripe".to_owned(),
            name: "Stripe".to_owned(),
            config: json!({
                "secretKey": "sk_test",
                "webhookSecret": "whsec_test",
                "publishableKey": "pk_test",
                "currency": "USD"
            }),
            supported_types: vec!["stripe".to_owned(), "card".to_owned()],
            enabled: true,
            payment_mode: "popup".to_owned(),
            sort_order: 1,
            limits: json!({}),
            refund_enabled: true,
            allow_user_refund: true,
            created_at: "2026-06-06T00:00:00Z".to_owned(),
            updated_at: "2026-06-06T00:00:00Z".to_owned(),
        }
    }

    async fn spawn_stripe_payment_intent_server() -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap();
            let request_text = String::from_utf8_lossy(&buffer[..read]).to_string();
            let response_body = json!({
                "id": "pi_real_test",
                "client_secret": "pi_real_test_secret_123",
                "amount": 1250,
                "currency": "usd",
                "metadata": {
                    "orderId": "ORDER123"
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request_text
        });
        (base_url, handle)
    }

    #[test]
    fn easypay_sign_is_deterministic_and_excludes_signature_fields() {
        let mut params = HashMap::from([
            ("pid".to_owned(), "1001".to_owned()),
            ("type".to_owned(), "alipay".to_owned()),
            ("out_trade_no".to_owned(), "ORDER123".to_owned()),
            ("money".to_owned(), "10.00".to_owned()),
        ]);
        let sign = easypay_sign(&params, "secret");
        params.insert("sign".to_owned(), "ignored".to_owned());
        params.insert("sign_type".to_owned(), "MD5".to_owned());
        params.insert("empty".to_owned(), String::new());
        assert_eq!(sign, easypay_sign(&params, "secret"));
        assert_eq!(sign.len(), 32);
        assert!(easypay_verify_sign(&params, "secret", &sign));
        params.insert("money".to_owned(), "99.99".to_owned());
        assert!(!easypay_verify_sign(&params, "secret", &sign));
    }

    #[tokio::test]
    async fn configured_easypay_builds_submit_url_and_verifies_callback() {
        let provider = ConfiguredPaymentProvider::new(easypay_instance());
        let response = provider
            .create_payment(CreatePaymentRequest {
                order_id: 11,
                out_trade_no: "ORDER123".to_owned(),
                amount: 10.0,
                payment_type: "alipay".to_owned(),
                return_url: None,
                is_mobile: true,
            })
            .await
            .unwrap();
        let pay_url = response.pay_url.unwrap();
        assert!(pay_url.starts_with("https://pay.example.com/submit.php?"));
        assert!(pay_url.contains("out_trade_no=ORDER123"));
        assert!(pay_url.contains("device=mobile"));
        assert_eq!(response.trade_no, "ORDER123");
        assert_eq!(response.provider_key, "easypay");

        let mut notify = HashMap::from([
            ("pid".to_owned(), "1001".to_owned()),
            ("trade_no".to_owned(), "TRADE123".to_owned()),
            ("out_trade_no".to_owned(), "ORDER123".to_owned()),
            ("type".to_owned(), "alipay".to_owned()),
            ("money".to_owned(), "10".to_owned()),
            ("trade_status".to_owned(), "TRADE_SUCCESS".to_owned()),
        ]);
        let sign = easypay_sign(&notify, "secret");
        notify.insert("sign".to_owned(), sign);
        notify.insert("sign_type".to_owned(), "MD5".to_owned());
        let raw = form_urlencode(&notify);
        let notification = provider
            .verify_notification(&json!({ "_raw": raw }), &HashMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification.order_id, "ORDER123");
        assert_eq!(notification.trade_no, "TRADE123");
        assert_eq!(notification.amount, 10.0);
        assert_eq!(notification.status, "success");
        assert_eq!(notification.metadata["pid"], "1001");
    }

    #[tokio::test]
    async fn configured_stripe_creates_payment_intent_against_api() {
        let (api_base, request_handle) = spawn_stripe_payment_intent_server().await;
        let mut instance = stripe_instance();
        instance.config["apiBase"] = json!(api_base);
        instance.supported_types = vec![
            "card".to_owned(),
            "alipay".to_owned(),
            "wxpay".to_owned(),
            "link".to_owned(),
        ];
        let provider = ConfiguredPaymentProvider::new(instance);

        let response = provider
            .create_payment(CreatePaymentRequest {
                order_id: 11,
                out_trade_no: "ORDER123".to_owned(),
                amount: 12.5,
                payment_type: "stripe".to_owned(),
                return_url: None,
                is_mobile: false,
            })
            .await
            .unwrap();
        let request_text = request_handle.await.unwrap();

        assert!(request_text.starts_with("POST /v1/payment_intents HTTP/1.1"));
        assert!(request_text.contains("authorization: Bearer sk_test"));
        assert!(request_text.contains("idempotency-key: pi-ORDER123"));
        assert!(request_text.contains("amount=1250"));
        assert!(request_text.contains("currency=usd"));
        assert!(request_text.contains("metadata%5BorderId%5D=ORDER123"));
        assert!(request_text.contains("payment_method_types%5B%5D=card"));
        assert!(request_text.contains("payment_method_types%5B%5D=alipay"));
        assert!(request_text.contains("payment_method_types%5B%5D=wechat_pay"));
        assert!(request_text.contains("payment_method_types%5B%5D=link"));
        assert!(request_text.contains("payment_method_options%5Bwechat_pay%5D%5Bclient%5D=web"));
        assert_eq!(response.provider_key, "stripe");
        assert_eq!(response.provider_instance_id, "9");
        assert_eq!(response.trade_no, "pi_real_test");
        assert_eq!(response.intent_id.as_deref(), Some("pi_real_test"));
        assert_eq!(
            response.client_secret.as_deref(),
            Some("pi_real_test_secret_123")
        );
        assert!(response.pay_url.is_none());
        assert!(response.qr_code.is_none());
        assert_eq!(response.currency, "USD");
        assert_eq!(response.payment_mode, "popup");
        assert_eq!(response.result_type, "payment_intent");
    }

    #[tokio::test]
    async fn configured_stripe_verifies_signed_payment_intent_webhook() {
        let provider = ConfiguredPaymentProvider::new(stripe_instance());
        let event = json!({
            "id": "evt_test",
            "type": "payment_intent.succeeded",
            "data": {
                "object": {
                    "id": "pi_test",
                    "amount_received": 1250,
                    "currency": "usd",
                    "metadata": {
                        "orderId": "ORDER123"
                    }
                }
            }
        });
        let raw_body = event.to_string();
        let timestamp = "1780451824";
        let signature = stripe_webhook_signature(&raw_body, "whsec_test", timestamp);
        let headers = HashMap::from([(
            "stripe-signature".to_owned(),
            format!("t={timestamp},v1={signature}"),
        )]);
        let mut payload = event;
        payload["_raw"] = json!(raw_body);

        let notification = provider
            .verify_notification(&payload, &headers)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(notification.order_id, "ORDER123");
        assert_eq!(notification.trade_no, "pi_test");
        assert_eq!(notification.amount, 12.5);
        assert_eq!(notification.status, "success");
        assert_eq!(notification.metadata["currency"], "USD");

        let bad_headers = HashMap::from([(
            "stripe-signature".to_owned(),
            format!("t={timestamp},v1=bad{signature}"),
        )]);
        assert_eq!(
            provider
                .verify_notification(&payload, &bad_headers)
                .await
                .unwrap_err(),
            PaymentProviderError::VerifyFailed
        );
    }
}
