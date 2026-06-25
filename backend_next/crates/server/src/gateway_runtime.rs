use axum::{
    body::Body,
    extract::ws::{
        rejection::WebSocketUpgradeRejection, Message as AxumWsMessage, WebSocket, WebSocketUpgrade,
    },
    http::{
        header::{ACCEPT_LANGUAGE, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT},
        HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri,
    },
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use domain::{AccountGroupBinding, ApiKeyStatus, ModelMappingResolution};
use futures_util::{stream, SinkExt, StreamExt};
use protocol::{DownstreamProtocol, GatewayEndpoint, GatewayEndpointKind};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use crate::admin_ops::GatewayOpsEvent;
use crate::admin_portal::GatewayUsageRecord;
use crate::auth::{ApiKeyUsageSnapshot, GatewayApiKeyIdentity};
use crate::gateway_scheduler::{
    AccountAttemptGuard, AccountRuntimeMetadata, AccountRuntimeScheduler, AccountRuntimeSnapshot,
    AccountScheduleOptions, AccountScheduleRejection,
};
use crate::responses_chat_bridge::{
    assistant_message_from_chat_response, build_chat_completions_request,
    chat_completions_response_to_responses, responses_stream_events_to_sse,
    responses_to_chat_messages, ChatToResponsesStreamState,
};
use crate::responses_chat_state_store::{
    DynResponsesChatStateStore, MemoryResponsesChatStateStore, RedisResponsesChatStateStore,
    ResponsesChatState,
};
use crate::state::AppState;
use domain::ApiKey;
use repository::UsageRecord;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MODEL: &str = "gpt-5.4";
const OPENAI_WS_BETA_V2: &str = "responses_websockets=2026-02-06";
const OPENAI_WS_TURN_STATE_HEADER: &str = "x-codex-turn-state";
const OPENAI_WS_TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const RISK_CONTROL_CATEGORIES: &[&str] = &[
    "harassment",
    "harassment/threatening",
    "hate",
    "hate/threatening",
    "illicit",
    "illicit/violent",
    "self-harm",
    "self-harm/intent",
    "self-harm/instructions",
    "sexual",
    "sexual/minors",
    "violence",
    "violence/graphic",
];

#[derive(Debug, Clone)]
struct ApiKeyUsageView {
    id: i64,
    user_id: i64,
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

impl From<ApiKeyUsageSnapshot> for ApiKeyUsageView {
    fn from(snapshot: ApiKeyUsageSnapshot) -> Self {
        Self {
            id: snapshot.id,
            user_id: snapshot.user_id,
            name: snapshot.name,
            group_id: snapshot.group_id,
            status: snapshot.status,
            quota: snapshot.quota,
            quota_used: snapshot.quota_used,
            rate_limit_5h: snapshot.rate_limit_5h,
            rate_limit_1d: snapshot.rate_limit_1d,
            rate_limit_7d: snapshot.rate_limit_7d,
            usage_5h: snapshot.usage_5h,
            usage_1d: snapshot.usage_1d,
            usage_7d: snapshot.usage_7d,
            window_5h_start: snapshot.window_5h_start,
            window_1d_start: snapshot.window_1d_start,
            window_7d_start: snapshot.window_7d_start,
        }
    }
}

impl From<ApiKey> for ApiKeyUsageView {
    fn from(api_key: ApiKey) -> Self {
        Self {
            id: api_key.id.0,
            user_id: api_key.user_id,
            name: api_key.name,
            group_id: api_key.group_id.map(|group_id| group_id.0),
            status: match api_key.status {
                domain::ApiKeyStatus::Active => "active",
                domain::ApiKeyStatus::Disabled => "inactive",
                domain::ApiKeyStatus::QuotaExhausted => "quota_exhausted",
                domain::ApiKeyStatus::Expired => "expired",
            }
            .to_owned(),
            quota: api_key.quota,
            quota_used: api_key.quota_used,
            rate_limit_5h: api_key.rate_limit_5h,
            rate_limit_1d: api_key.rate_limit_1d,
            rate_limit_7d: api_key.rate_limit_7d,
            usage_5h: api_key.usage_5h,
            usage_1d: api_key.usage_1d,
            usage_7d: api_key.usage_7d,
            window_5h_start: api_key.window_5h_start,
            window_1d_start: api_key.window_1d_start,
            window_7d_start: api_key.window_7d_start,
        }
    }
}

#[derive(Clone)]
pub struct GatewayRuntimeService {
    http_client: reqwest::Client,
    responses_chat_store: DynResponsesChatStateStore,
    account_scheduler: AccountRuntimeScheduler,
}

impl GatewayRuntimeService {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
            responses_chat_store: MemoryResponsesChatStateStore::shared(),
            account_scheduler: AccountRuntimeScheduler::default(),
        }
    }

    pub fn with_http_client(http_client: reqwest::Client) -> Self {
        Self {
            http_client,
            responses_chat_store: MemoryResponsesChatStateStore::shared(),
            account_scheduler: AccountRuntimeScheduler::default(),
        }
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    pub(crate) async fn account_runtime_snapshots(
        &self,
        state: &AppState,
    ) -> HashMap<i64, AccountRuntimeSnapshot> {
        let mut snapshots = self.account_scheduler.snapshots();
        match state.repository.list_account_concurrency_snapshots().await {
            Ok(repository_snapshots) => {
                for snapshot in repository_snapshots {
                    snapshots
                        .entry(snapshot.account_id)
                        .and_modify(|runtime| runtime.in_flight = snapshot.in_flight as usize)
                        .or_insert_with(|| AccountRuntimeSnapshot {
                            account_id: snapshot.account_id,
                            account_name: None,
                            platform: None,
                            group_id: None,
                            group_name: None,
                            max_concurrent: None,
                            in_flight: snapshot.in_flight as usize,
                            consecutive_failures: 0,
                            cooling_until_unix: None,
                            retry_after_seconds: None,
                            last_error: None,
                            total_successes: 0,
                            total_failures: 0,
                        });
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to load postgres account concurrency snapshots");
            }
        }
        snapshots
    }

    pub async fn from_config(config: &crate::config::AppConfig) -> anyhow::Result<Self> {
        let responses_chat_store =
            if let Some(store) = RedisResponsesChatStateStore::from_app_config(config) {
                store.ping().await?;
                std::sync::Arc::new(store) as DynResponsesChatStateStore
            } else {
                MemoryResponsesChatStateStore::shared()
            };
        Ok(Self {
            http_client: reqwest::Client::new(),
            responses_chat_store,
            account_scheduler: AccountRuntimeScheduler::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn responses_chat_store_for_tests(&self) -> DynResponsesChatStateStore {
        self.responses_chat_store.clone()
    }

    pub async fn handle(
        &self,
        state: &AppState,
        endpoint: GatewayEndpoint,
        uri: &Uri,
        method: &str,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let identity = match authorize_gateway_request(state, endpoint.kind, uri, &headers).await {
            Ok(identity) => identity,
            Err(error) => {
                record_gateway_error_ops_event(
                    state,
                    endpoint.kind,
                    endpoint.normalized_path.as_str(),
                    uri.path(),
                    method,
                    None,
                    None,
                    &error,
                );
                return gateway_error(error).into_response();
            }
        };
        if should_apply_api_key_rate_limit(endpoint.kind) {
            if let Err(runtime_error) =
                api_key_rate_limit_preflight(state, endpoint.kind, &identity).await
            {
                record_gateway_error_ops_event(
                    state,
                    endpoint.kind,
                    endpoint.normalized_path.as_str(),
                    uri.path(),
                    method,
                    Some(&identity),
                    None,
                    &runtime_error,
                );
                return gateway_error(runtime_error).into_response();
            }
        }

        let request = parse_json_body(&body);
        let model = request_model(&request)
            .or_else(|| request_model_from_path(endpoint.kind, endpoint.normalized_path.as_str()))
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let stream = request_bool(&request, "stream")
            || endpoint_requests_stream(endpoint.kind, endpoint.normalized_path.as_str());
        let routes = match resolve_gateway_routes(state, endpoint.kind, &identity).await {
            Ok(routes) => routes,
            Err(error) => {
                record_gateway_error_ops_event(
                    state,
                    endpoint.kind,
                    endpoint.normalized_path.as_str(),
                    uri.path(),
                    method,
                    Some(&identity),
                    None,
                    &error,
                );
                return gateway_error(error).into_response();
            }
        };
        if !stream
            && matches!(
                endpoint.kind,
                GatewayEndpointKind::OpenAiResponses | GatewayEndpointKind::OpenAiChatCompletions
            )
        {
            return self
                .handle_openai_non_stream_routes(
                    state, &identity, &endpoint, uri, body, &request, &model, routes,
                )
                .await;
        }

        let route = routes
            .first()
            .expect("gateway route resolver returns at least one route")
            .clone();
        let schedule_options = route_schedule_options(&route);
        let attempt_guard = match self
            .acquire_account_attempt(state, &route, schedule_options)
            .await
        {
            Ok(guard) => guard,
            Err(rejection) => {
                return gateway_error(GatewayRuntimeError {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    message: rejection.message(),
                    style: error_style_for_kind(endpoint.kind),
                })
                .into_response();
            }
        };
        if should_apply_api_key_rate_limit(endpoint.kind) {
            match platform_quota_preflight(state, endpoint.kind, &identity, &route).await {
                Ok(()) => {}
                Err(response) => return response,
            }
        }
        let model_mapping = resolve_gateway_model_mapping(state, &route, &model);
        if let Err(response) = risk_control_preflight(
            state,
            &identity,
            &route,
            endpoint.kind,
            endpoint.normalized_path.as_str(),
            &request,
            &model_mapping,
            true,
        )
        .await
        {
            return response;
        }

        match endpoint.kind {
            GatewayEndpointKind::OpenAiResponses => {
                if can_forward_openai_responses(&route) {
                    match self
                        .forward_openai_responses(
                            &route,
                            endpoint.normalized_path.as_str(),
                            body.clone(),
                            uri.query(),
                            &model_mapping,
                            stream,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else if can_forward_responses_via_chat_completions(&route) {
                    match self
                        .forward_openai_responses_via_chat_completions(
                            &route,
                            &request,
                            uri.query(),
                            &model_mapping,
                            stream,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        upstream_not_configured(
                            endpoint.kind,
                            "OpenAI Responses upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiChatCompletions => {
                if can_forward_openai_chat_completions(&route) {
                    match self
                        .forward_openai_chat_completions(
                            &route,
                            body,
                            uri.query(),
                            &model_mapping,
                            stream,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        upstream_not_configured(
                            endpoint.kind,
                            "OpenAI Chat Completions upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::AnthropicMessages | GatewayEndpointKind::AntigravityMessages => {
                if can_forward_anthropic_messages(&route) {
                    match self
                        .forward_anthropic_request(
                            &route,
                            "/v1/messages",
                            body,
                            uri.query(),
                            &model_mapping,
                            stream,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        upstream_not_configured(
                            endpoint.kind,
                            "Anthropic Messages upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::AnthropicCountTokens
            | GatewayEndpointKind::AntigravityCountTokens => {
                if can_forward_anthropic_messages(&route) {
                    match self
                        .forward_anthropic_request(
                            &route,
                            "/v1/messages/count_tokens",
                            body,
                            uri.query(),
                            &model_mapping,
                            false,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                false,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            false,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        false,
                        upstream_not_configured(
                            endpoint.kind,
                            "Anthropic token counting upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiEmbeddings => {
                if can_forward_openai_embeddings(&route) {
                    match self
                        .forward_openai_json_request(
                            &route,
                            "/v1/embeddings",
                            body,
                            uri.query(),
                            &model_mapping,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                false,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            false,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        false,
                        upstream_not_configured(
                            endpoint.kind,
                            "OpenAI Embeddings upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiImageGenerations | GatewayEndpointKind::OpenAiImageEdits => {
                if can_forward_openai_images(&route) {
                    let upstream_path = match endpoint.kind {
                        GatewayEndpointKind::OpenAiImageEdits => "/v1/images/edits",
                        _ => "/v1/images/generations",
                    };
                    match self
                        .forward_openai_body_request(
                            &route,
                            upstream_path,
                            body,
                            uri.query(),
                            &model_mapping,
                            headers.get(CONTENT_TYPE).cloned(),
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        upstream_not_configured(
                            endpoint.kind,
                            "OpenAI Images upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiModels | GatewayEndpointKind::AntigravityModels => {
                if endpoint.kind == GatewayEndpointKind::OpenAiModels
                    && can_forward_openai_models(&route)
                {
                    match self
                        .forward_openai_get_request(&route, "/v1/models", uri.query())
                        .await
                    {
                        Ok(response) => attach_attempt_guard_to_response(response, attempt_guard),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    attach_attempt_guard_to_response(
                        json_response(openai_models_response()),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiUsage | GatewayEndpointKind::AntigravityUsage => {
                match openai_usage_response(state, &identity, &route).await {
                    Ok(usage) => {
                        attach_attempt_guard_to_response(json_response(usage), attempt_guard)
                    }
                    Err(error) => self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        error,
                        attempt_guard,
                    ),
                }
            }
            GatewayEndpointKind::GeminiModels | GatewayEndpointKind::AntigravityGeminiModels => {
                if endpoint.kind == GatewayEndpointKind::GeminiModels
                    && can_forward_gemini_generate_content(&route)
                {
                    let upstream_path =
                        gemini_models_upstream_path(endpoint.normalized_path.as_str());
                    match self
                        .forward_gemini_get_request(&route, &upstream_path, uri.query())
                        .await
                    {
                        Ok(response) => attach_attempt_guard_to_response(response, attempt_guard),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    attach_attempt_guard_to_response(
                        json_response(gemini_models_response(endpoint.normalized_path.as_str())),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::GeminiGenerateContent
            | GatewayEndpointKind::AntigravityGeminiGenerateContent => {
                if can_forward_gemini_generate_content(&route) {
                    match self
                        .forward_gemini_generate_content(
                            &route,
                            endpoint.normalized_path.as_str(),
                            body,
                            uri.query(),
                            &model_mapping,
                            stream,
                        )
                        .await
                    {
                        Ok(response) => self
                            .record_scheduled_gateway_response_usage(
                                state,
                                &identity,
                                &route,
                                endpoint.kind,
                                endpoint.normalized_path.as_str(),
                                &model_mapping,
                                stream,
                                response,
                                attempt_guard,
                            )
                            .await
                            .unwrap_or_else(|error| gateway_error(error).into_response()),
                        Err(error) => self.gateway_error_with_scheduled_ops(
                            state,
                            &identity,
                            &route,
                            endpoint.kind,
                            endpoint.normalized_path.as_str(),
                            &model_mapping,
                            stream,
                            error,
                            attempt_guard,
                        ),
                    }
                } else {
                    self.gateway_error_with_scheduled_ops(
                        state,
                        &identity,
                        &route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        stream,
                        upstream_not_configured(
                            endpoint.kind,
                            "Gemini GenerateContent upstream is not configured",
                        ),
                        attempt_guard,
                    )
                }
            }
            GatewayEndpointKind::OpenAiResponsesWebSocket => self.gateway_error_with_scheduled_ops(
                state,
                &identity,
                &route,
                endpoint.kind,
                endpoint.normalized_path.as_str(),
                &model_mapping,
                stream,
                GatewayRuntimeError {
                    status: StatusCode::UPGRADE_REQUIRED,
                    message: "responses websocket requires a WebSocket upgrade request".to_owned(),
                    style: error_style_for_kind(endpoint.kind),
                },
                attempt_guard,
            ),
        }
    }

    pub async fn handle_websocket(
        &self,
        state: AppState,
        endpoint: GatewayEndpoint,
        uri: Uri,
        headers: HeaderMap,
        ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    ) -> Response {
        let ws = match ws {
            Ok(ws) => ws,
            Err(_) => {
                return gateway_error(GatewayRuntimeError {
                    status: StatusCode::UPGRADE_REQUIRED,
                    message: "responses websocket requires a WebSocket upgrade request".to_owned(),
                    style: error_style_for_kind(endpoint.kind),
                })
                .into_response();
            }
        };
        let identity = match authorize_gateway_request(&state, endpoint.kind, &uri, &headers).await
        {
            Ok(identity) => identity,
            Err(error) => return gateway_error(error).into_response(),
        };
        if should_apply_api_key_rate_limit(endpoint.kind) {
            if let Err(error) = api_key_rate_limit_preflight(&state, endpoint.kind, &identity).await
            {
                return gateway_error(error).into_response();
            }
        }

        let routes = match resolve_gateway_routes(&state, endpoint.kind, &identity).await {
            Ok(routes) => routes,
            Err(error) => return gateway_error(error).into_response(),
        };
        let has_chat_bridge = routes
            .iter()
            .any(can_forward_responses_via_chat_completions);
        let Some(route) = routes.into_iter().find(can_forward_openai_responses) else {
            let message = if has_chat_bridge {
                "Responses WebSocket cannot be bridged to Chat Completions upstream"
            } else {
                "OpenAI Responses WebSocket upstream is not configured"
            };
            return gateway_error(upstream_not_configured(endpoint.kind, message)).into_response();
        };
        if let Err(response) =
            platform_quota_preflight(&state, endpoint.kind, &identity, &route).await
        {
            return response;
        }
        let upstream_url = match build_openai_responses_ws_url(
            &route,
            endpoint.normalized_path.as_str(),
            uri.query(),
        ) {
            Ok(value) => value,
            Err(error) => return gateway_error(error).into_response(),
        };
        let upstream_headers = build_openai_ws_request_headers(&route, &headers);
        let runtime = self.clone();
        ws.on_upgrade(move |socket| async move {
            runtime
                .proxy_openai_responses_websocket(socket, upstream_url, upstream_headers)
                .await;
        })
    }

    async fn handle_openai_non_stream_routes(
        &self,
        state: &AppState,
        identity: &GatewayApiKeyIdentity,
        endpoint: &GatewayEndpoint,
        uri: &Uri,
        body: Bytes,
        request: &Value,
        model: &str,
        routes: Vec<GatewayRoute>,
    ) -> Response {
        let mut forwardable_routes = routes
            .into_iter()
            .filter(|route| can_forward_openai_endpoint(endpoint.kind, route))
            .collect::<Vec<_>>();
        if forwardable_routes.is_empty() {
            return gateway_error(openai_upstream_not_configured(endpoint.kind)).into_response();
        }
        if endpoint.kind == GatewayEndpointKind::OpenAiResponses
            && has_previous_response_id(request)
            && forwardable_routes
                .iter()
                .any(can_forward_responses_via_chat_completions)
        {
            match self
                .routes_for_responses_request(forwardable_routes, request)
                .await
            {
                Ok(routes) => forwardable_routes = routes,
                Err(error) => return gateway_error(error).into_response(),
            }
        }

        let allow_failover = endpoint.kind == GatewayEndpointKind::OpenAiChatCompletions
            || !has_previous_response_id(request);
        let route_count = forwardable_routes.len();
        let mut last_schedule_rejection = None;
        let mut risk_control_checked = false;
        for (index, route) in forwardable_routes.iter().enumerate() {
            let schedule_options = route_schedule_options(route);
            let _attempt_guard = match self
                .acquire_account_attempt(state, route, schedule_options)
                .await
            {
                Ok(guard) => guard,
                Err(rejection) => {
                    last_schedule_rejection = Some(rejection);
                    continue;
                }
            };
            if should_apply_api_key_rate_limit(endpoint.kind) {
                match platform_quota_preflight(state, endpoint.kind, identity, route).await {
                    Ok(()) => {}
                    Err(response) => return response,
                }
            }
            let model_mapping = resolve_gateway_model_mapping(state, route, model);
            if !risk_control_checked {
                match risk_control_preflight(
                    state,
                    identity,
                    route,
                    endpoint.kind,
                    endpoint.normalized_path.as_str(),
                    request,
                    &model_mapping,
                    true,
                )
                .await
                {
                    Ok(()) => risk_control_checked = true,
                    Err(response) => return response,
                }
            }
            let result = match endpoint.kind {
                GatewayEndpointKind::OpenAiResponses => {
                    if can_forward_openai_responses(route) {
                        self.forward_openai_responses(
                            route,
                            endpoint.normalized_path.as_str(),
                            body.clone(),
                            uri.query(),
                            &model_mapping,
                            false,
                        )
                        .await
                    } else {
                        self.forward_openai_responses_via_chat_completions(
                            route,
                            request,
                            uri.query(),
                            &model_mapping,
                            false,
                        )
                        .await
                    }
                }
                GatewayEndpointKind::OpenAiChatCompletions => {
                    self.forward_openai_chat_completions(
                        route,
                        body.clone(),
                        uri.query(),
                        &model_mapping,
                        false,
                    )
                    .await
                }
                _ => unreachable!("non-stream OpenAI failover is only used for OpenAI routes"),
            };

            let has_next = allow_failover && index + 1 < route_count;
            match result {
                Ok(response) => {
                    if has_next && should_retry_gateway_status(response.status()) {
                        self.mark_account_failure(
                            route,
                            schedule_options,
                            format!("upstream returned {}", response.status()),
                        );
                        continue;
                    }
                    if response.status().is_success() {
                        self.mark_account_success(route);
                    } else if should_retry_gateway_status(response.status()) {
                        self.mark_account_failure(
                            route,
                            schedule_options,
                            format!("upstream returned {}", response.status()),
                        );
                    }
                    return record_gateway_response_usage(
                        state,
                        identity,
                        route,
                        endpoint.kind,
                        endpoint.normalized_path.as_str(),
                        &model_mapping,
                        false,
                        response,
                    )
                    .await
                    .unwrap_or_else(|error| gateway_error(error).into_response());
                }
                Err(error) => {
                    if has_next && should_retry_gateway_error(&error) {
                        self.mark_account_failure(route, schedule_options, error.message.clone());
                        continue;
                    }
                    if should_retry_gateway_error(&error) {
                        self.mark_account_failure(route, schedule_options, error.message.clone());
                    }
                    return gateway_error(error).into_response();
                }
            }
        }

        if let Some(rejection) = last_schedule_rejection {
            gateway_error(GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: rejection.message(),
                style: error_style_for_kind(endpoint.kind),
            })
            .into_response()
        } else {
            gateway_error(openai_upstream_not_configured(endpoint.kind)).into_response()
        }
    }

    async fn routes_for_responses_request(
        &self,
        routes: Vec<GatewayRoute>,
        request: &Value,
    ) -> Result<Vec<GatewayRoute>, GatewayRuntimeError> {
        let Some(previous_response_id) = previous_response_id(request) else {
            return Ok(routes);
        };
        let previous = self
            .responses_chat_store
            .get(previous_response_id)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("responses chat state lookup failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let Some(previous) = previous else {
            let native_routes = routes
                .into_iter()
                .filter(can_forward_openai_responses)
                .collect::<Vec<_>>();
            if native_routes.is_empty() {
                return Err(GatewayRuntimeError {
                    status: StatusCode::BAD_REQUEST,
                    message: "previous_response_id was not found or has expired".to_owned(),
                    style: GatewayErrorStyle::OpenAi,
                });
            }
            return Ok(native_routes);
        };
        routes
            .into_iter()
            .find(|route| {
                can_forward_responses_via_chat_completions(route)
                    && previous.group_id == route.account.group_id.0
                    && previous.account_id == route.account.account.id.0
            })
            .map(|route| vec![route])
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::BAD_REQUEST,
                message: "previous_response_id does not belong to this route".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })
    }

    async fn acquire_account_attempt(
        &self,
        state: &AppState,
        route: &GatewayRoute,
        options: AccountScheduleOptions,
    ) -> Result<AccountAttemptGuard, AccountScheduleRejection> {
        let guard = self.account_scheduler.try_acquire_with_metadata(
            route.account.account.id.0,
            options,
            AccountRuntimeMetadata {
                account_name: Some(route.account.account.name.clone()),
                platform: Some(route_provider(route).to_owned()),
                group_id: Some(route.account.group_id.0),
                group_name: Some(format!("Group {}", route.account.group_id.0)),
                max_concurrent: options.max_concurrent,
            },
        )?;
        let Some(max_concurrent) = options.max_concurrent.filter(|value| *value > 0) else {
            return Ok(guard);
        };
        if !state.repository.uses_shared_consistency_backend() {
            return Ok(guard);
        }
        let request_id = format!("gateway-{}", uuid::Uuid::new_v4().simple());
        match state
            .repository
            .acquire_account_concurrency_slot(
                route.account.account.id.0,
                &request_id,
                max_concurrent as i64,
                account_concurrency_lease_seconds(route),
                json!({
                    "account_name": route.account.account.name,
                    "platform": route_provider(route),
                    "group_id": route.account.group_id.0
                }),
            )
            .await
        {
            Ok(Some(_slot)) => Ok(guard.with_repository_slot(state.repository.clone(), request_id)),
            Ok(None) => {
                drop(guard);
                Err(AccountScheduleRejection::ConcurrentLimit { max_concurrent })
            }
            Err(error) => {
                drop(guard);
                tracing::warn!(
                    account_id = route.account.account.id.0,
                    error = %error,
                    "postgres account concurrency slot acquisition failed"
                );
                Err(AccountScheduleRejection::ConcurrentLimit { max_concurrent })
            }
        }
    }

    fn mark_account_success(&self, route: &GatewayRoute) {
        self.account_scheduler
            .mark_success(route.account.account.id.0);
    }

    fn mark_account_failure(
        &self,
        route: &GatewayRoute,
        options: AccountScheduleOptions,
        error: impl Into<String>,
    ) {
        self.account_scheduler
            .mark_failure(route.account.account.id.0, error, options);
    }

    async fn record_scheduled_gateway_response_usage(
        &self,
        state: &AppState,
        identity: &GatewayApiKeyIdentity,
        route: &GatewayRoute,
        kind: GatewayEndpointKind,
        endpoint_path: &str,
        model_mapping: &ModelMappingResolution,
        stream: bool,
        response: Response,
        attempt_guard: AccountAttemptGuard,
    ) -> Result<Response, GatewayRuntimeError> {
        let status = response.status();
        let response = record_gateway_response_usage(
            state,
            identity,
            route,
            kind,
            endpoint_path,
            model_mapping,
            stream,
            response,
        )
        .await?;
        if status.is_success() {
            self.mark_account_success(route);
        } else if should_retry_gateway_status(status) {
            self.mark_account_failure(
                route,
                route_schedule_options(route),
                format!("upstream returned {status}"),
            );
        }
        Ok(attach_attempt_guard_to_response(response, attempt_guard))
    }

    fn gateway_error_with_scheduled_ops(
        &self,
        state: &AppState,
        identity: &GatewayApiKeyIdentity,
        route: &GatewayRoute,
        kind: GatewayEndpointKind,
        endpoint_path: &str,
        model_mapping: &ModelMappingResolution,
        stream: bool,
        error: GatewayRuntimeError,
        attempt_guard: AccountAttemptGuard,
    ) -> Response {
        if should_mark_account_failure(&error) {
            self.mark_account_failure(route, route_schedule_options(route), error.message.clone());
        }
        let response = gateway_error_with_ops(
            state,
            identity,
            route,
            kind,
            endpoint_path,
            model_mapping,
            stream,
            error,
        );
        attach_attempt_guard_to_response(response, attempt_guard)
    }
}

#[derive(Debug, Clone)]
pub struct GatewayRuntimeError {
    status: StatusCode,
    message: String,
    style: GatewayErrorStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayErrorStyle {
    OpenAi,
    Anthropic,
    Google,
}

#[derive(Debug, Clone)]
struct GatewayRoute {
    downstream: DownstreamProtocol,
    account: AccountGroupBinding,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct GatewayTokenUsage {
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
}

impl GatewayTokenUsage {
    fn total_tokens(self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

fn upstream_not_configured(kind: GatewayEndpointKind, message: &str) -> GatewayRuntimeError {
    GatewayRuntimeError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: message.to_owned(),
        style: error_style_for_kind(kind),
    }
}

#[derive(Debug, Clone)]
struct ResponsesChatCompactionConfig {
    context_window_tokens: usize,
    max_output_tokens: usize,
    keep_recent_turns: usize,
    compaction_model: String,
}

impl ResponsesChatCompactionConfig {
    fn from_account(route: &GatewayRoute, fallback_model: &str) -> Self {
        let extra = &route.account.account.extra;
        Self {
            context_window_tokens: extra_usize(
                extra,
                "openai_responses_chat_context_window_tokens",
            )
            .unwrap_or(32_000),
            max_output_tokens: extra_usize(extra, "openai_responses_chat_max_output_tokens")
                .unwrap_or(4_096),
            keep_recent_turns: extra_usize(extra, "openai_responses_chat_keep_recent_turns")
                .unwrap_or(8),
            compaction_model: extra_string(extra, "openai_responses_chat_compaction_model")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| fallback_model.to_owned()),
        }
    }

    fn limit(&self) -> usize {
        let limit = self
            .context_window_tokens
            .saturating_sub(self.max_output_tokens);
        if limit < 1024 {
            self.context_window_tokens
        } else {
            limit
        }
    }
}

async fn authorize_gateway_request(
    state: &AppState,
    kind: GatewayEndpointKind,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<GatewayApiKeyIdentity, GatewayRuntimeError> {
    let auth_mode = gateway_auth_mode(kind);
    let token = match auth_mode {
        GatewayAuthMode::OpenAiLike => extract_openai_api_key(uri, headers)?,
        GatewayAuthMode::GoogleLike => extract_google_api_key(uri, headers)?,
    };

    if let Ok(identity) = state.auth.validate_gateway_api_key(&token) {
        return Ok(identity);
    }

    match state.repository.get_api_key_by_key(&token).await {
        Ok(api_key) => {
            if api_key.status != ApiKeyStatus::Active {
                return Err(GatewayRuntimeError {
                    status: StatusCode::UNAUTHORIZED,
                    message: "API key is not active".to_owned(),
                    style: auth_mode.error_style(),
                });
            }
            Ok(GatewayApiKeyIdentity {
                id: api_key.id.0,
                user_id: api_key.user_id,
                name: api_key.name,
                group_id: api_key.group_id.map(|group_id| group_id.0),
            })
        }
        Err(repository::RepositoryError::NotFound { .. }) => state
            .auth
            .validate_gateway_api_key(&token)
            .map_err(|error| GatewayRuntimeError {
                status: error.status(),
                message: normalize_gateway_auth_error_message(error.message()),
                style: auth_mode.error_style(),
            }),
        Err(error) => Err(GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("repository error: {error}"),
            style: auth_mode.error_style(),
        }),
    }
}

async fn resolve_gateway_routes(
    state: &AppState,
    kind: GatewayEndpointKind,
    identity: &GatewayApiKeyIdentity,
) -> Result<Vec<GatewayRoute>, GatewayRuntimeError> {
    let downstream = downstream_protocol_for_kind(kind).ok_or_else(|| GatewayRuntimeError {
        status: StatusCode::BAD_REQUEST,
        message: "unsupported gateway endpoint".to_owned(),
        style: error_style_for_kind(kind),
    })?;
    let group_id = identity.group_id.ok_or_else(|| GatewayRuntimeError {
        status: StatusCode::FORBIDDEN,
        message: "API key is not bound to an active group".to_owned(),
        style: error_style_for_kind(kind),
    })?;
    let mut candidates = state
        .repository
        .list_bindings_by_group(domain::GroupId(group_id))
        .await
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("repository error: {error}"),
            style: error_style_for_kind(kind),
        })?;
    if candidates.is_empty() {
        candidates = state.admin_portal.gateway_account_candidates(group_id);
    }
    let candidates = filter_candidates_for_kind(kind, candidates);
    let mut routes = candidates
        .into_iter()
        .filter(|candidate| candidate.supports(downstream))
        .map(|account| GatewayRoute {
            downstream,
            account,
        })
        .collect::<Vec<_>>();
    routes.sort_by_key(|route| route.account.priority);
    if routes.is_empty() {
        return Err(GatewayRuntimeError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: format!("no account in group supports downstream protocol {downstream}"),
            style: error_style_for_kind(kind),
        });
    }
    Ok(routes)
}

fn downstream_protocol_for_kind(kind: GatewayEndpointKind) -> Option<DownstreamProtocol> {
    match kind {
        GatewayEndpointKind::OpenAiResponses | GatewayEndpointKind::OpenAiResponsesWebSocket => {
            Some(DownstreamProtocol::OpenAiResponses)
        }
        GatewayEndpointKind::OpenAiChatCompletions => {
            Some(DownstreamProtocol::OpenAiChatCompletions)
        }
        GatewayEndpointKind::AnthropicMessages
        | GatewayEndpointKind::AntigravityMessages
        | GatewayEndpointKind::AnthropicCountTokens
        | GatewayEndpointKind::AntigravityCountTokens => {
            Some(DownstreamProtocol::AnthropicMessages)
        }
        GatewayEndpointKind::OpenAiEmbeddings => Some(DownstreamProtocol::OpenAiEmbeddings),
        GatewayEndpointKind::OpenAiImageGenerations | GatewayEndpointKind::OpenAiImageEdits => {
            Some(DownstreamProtocol::OpenAiImages)
        }
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent
        | GatewayEndpointKind::GeminiModels
        | GatewayEndpointKind::AntigravityGeminiModels => {
            Some(DownstreamProtocol::GeminiGenerateContent)
        }
        GatewayEndpointKind::OpenAiModels
        | GatewayEndpointKind::AntigravityModels
        | GatewayEndpointKind::OpenAiUsage
        | GatewayEndpointKind::AntigravityUsage => Some(DownstreamProtocol::OpenAiChatCompletions),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayAuthMode {
    OpenAiLike,
    GoogleLike,
}

impl GatewayAuthMode {
    const fn error_style(self) -> GatewayErrorStyle {
        match self {
            Self::OpenAiLike => GatewayErrorStyle::OpenAi,
            Self::GoogleLike => GatewayErrorStyle::Google,
        }
    }
}

fn gateway_auth_mode(kind: GatewayEndpointKind) -> GatewayAuthMode {
    match kind {
        GatewayEndpointKind::GeminiModels
        | GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiModels
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => GatewayAuthMode::GoogleLike,
        _ => GatewayAuthMode::OpenAiLike,
    }
}

fn error_style_for_kind(kind: GatewayEndpointKind) -> GatewayErrorStyle {
    match kind {
        GatewayEndpointKind::AnthropicMessages
        | GatewayEndpointKind::AnthropicCountTokens
        | GatewayEndpointKind::AntigravityMessages
        | GatewayEndpointKind::AntigravityCountTokens
        | GatewayEndpointKind::AntigravityModels
        | GatewayEndpointKind::AntigravityUsage => GatewayErrorStyle::Anthropic,
        _ => gateway_auth_mode(kind).error_style(),
    }
}

fn filter_candidates_for_kind(
    kind: GatewayEndpointKind,
    candidates: Vec<AccountGroupBinding>,
) -> Vec<AccountGroupBinding> {
    if is_antigravity_kind(kind) {
        candidates
            .into_iter()
            .filter(|candidate| candidate.account.provider == domain::Provider::Antigravity)
            .collect()
    } else {
        candidates
    }
}

fn is_antigravity_kind(kind: GatewayEndpointKind) -> bool {
    matches!(
        kind,
        GatewayEndpointKind::AntigravityMessages
            | GatewayEndpointKind::AntigravityCountTokens
            | GatewayEndpointKind::AntigravityModels
            | GatewayEndpointKind::AntigravityUsage
            | GatewayEndpointKind::AntigravityGeminiModels
            | GatewayEndpointKind::AntigravityGeminiGenerateContent
    )
}

fn extract_openai_api_key(uri: &Uri, headers: &HeaderMap) -> Result<String, GatewayRuntimeError> {
    let query = uri.query().unwrap_or_default();
    if query_param(query, "key").is_some() || query_param(query, "api_key").is_some() {
        return Err(GatewayRuntimeError {
            status: StatusCode::BAD_REQUEST,
            message:
                "API key in query parameter is deprecated. Please use Authorization header instead."
                    .to_owned(),
            style: GatewayErrorStyle::OpenAi,
        });
    }

    bearer_token(headers)
        .or_else(|| header_token(headers, "x-api-key"))
        .or_else(|| header_token(headers, "x-goog-api-key"))
        .ok_or_else(|| GatewayRuntimeError {
            status: StatusCode::UNAUTHORIZED,
            message: "API key is required in Authorization header (Bearer scheme), x-api-key header, or x-goog-api-key header".to_owned(),
            style: GatewayErrorStyle::OpenAi,
        })
}

fn extract_google_api_key(uri: &Uri, headers: &HeaderMap) -> Result<String, GatewayRuntimeError> {
    let query = uri.query().unwrap_or_default();
    if query_param(query, "api_key").is_some() {
        return Err(GatewayRuntimeError {
            status: StatusCode::BAD_REQUEST,
            message:
                "Query parameter api_key is deprecated. Use Authorization header or key instead."
                    .to_owned(),
            style: GatewayErrorStyle::Google,
        });
    }

    header_token(headers, "x-goog-api-key")
        .or_else(|| bearer_token(headers))
        .or_else(|| header_token(headers, "x-api-key"))
        .or_else(|| {
            allow_google_query_key(uri.path())
                .then(|| query_param(query, "key"))
                .flatten()
        })
        .ok_or_else(|| GatewayRuntimeError {
            status: StatusCode::UNAUTHORIZED,
            message: "API key is required".to_owned(),
            style: GatewayErrorStyle::Google,
        })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme.eq_ignore_ascii_case("Bearer").then_some(token)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn header_token(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn query_param(query: &str, target: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == target)
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn allow_google_query_key(path: &str) -> bool {
    path.starts_with("/v1beta") || path.starts_with("/antigravity/v1beta")
}

fn normalize_gateway_auth_error_message(message: &str) -> String {
    match message {
        "invalid API key" => "Invalid API key".to_owned(),
        "API key is not active" => "API key is disabled".to_owned(),
        other => other.to_owned(),
    }
}

fn parse_json_body(body: &Bytes) -> Value {
    if body.is_empty() {
        return json!({});
    }
    serde_json::from_slice(body).unwrap_or_else(|_| json!({}))
}

fn request_model(request: &Value) -> Option<String> {
    request
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn request_bool(request: &Value, key: &str) -> bool {
    request.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn request_model_from_path(kind: GatewayEndpointKind, normalized_path: &str) -> Option<String> {
    match kind {
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => {
            gemini_model_action_from_path(normalized_path).map(|(model, _)| model)
        }
        _ => None,
    }
}

fn endpoint_requests_stream(kind: GatewayEndpointKind, normalized_path: &str) -> bool {
    match kind {
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => {
            gemini_model_action_from_path(normalized_path)
                .is_some_and(|(_, action)| action.eq_ignore_ascii_case("streamGenerateContent"))
        }
        _ => false,
    }
}

fn resolve_gateway_model_mapping(
    state: &AppState,
    route: &GatewayRoute,
    requested_model: &str,
) -> ModelMappingResolution {
    let channel = state.admin_portal.resolve_channel_model_mapping(
        route.account.group_id.0,
        route.account.account.provider,
        requested_model,
    );
    let account_mapping = route
        .account
        .account
        .resolve_mapped_model(&channel.mapped_model);
    let upstream_model = account_mapping.upstream_model.clone();
    let matched = channel.matched || account_mapping.matched;
    let matched_source = match (
        channel.matched_source.as_deref(),
        account_mapping.matched_source.as_deref(),
    ) {
        (Some(channel_source), Some(account_source)) => {
            Some(format!("channel:{channel_source};account:{account_source}"))
        }
        (Some(channel_source), None) => Some(format!("channel:{channel_source}")),
        (None, Some(account_source)) => Some(format!("account:{account_source}")),
        (None, None) => None,
    };
    let intermediate_models = if channel.matched && channel.mapped_model != upstream_model {
        vec![channel.mapped_model]
    } else {
        Vec::new()
    };

    ModelMappingResolution {
        requested_model: requested_model.trim().to_owned(),
        upstream_model,
        intermediate_models,
        matched,
        matched_source,
    }
}

fn can_forward_openai_chat_completions(route: &GatewayRoute) -> bool {
    route.account.upstream_protocol() == domain::UpstreamProtocol::OpenAiChatCompletions
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_openai_responses(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::OpenAiResponses
        && route.account.upstream_protocol() == domain::UpstreamProtocol::OpenAiResponses
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_openai_embeddings(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::OpenAiEmbeddings
        && matches!(
            route.account.upstream_protocol(),
            domain::UpstreamProtocol::OpenAiResponses
                | domain::UpstreamProtocol::OpenAiChatCompletions
        )
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_openai_images(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::OpenAiImages
        && matches!(
            route.account.upstream_protocol(),
            domain::UpstreamProtocol::OpenAiResponses
                | domain::UpstreamProtocol::OpenAiChatCompletions
        )
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_openai_models(route: &GatewayRoute) -> bool {
    matches!(
        route.account.upstream_protocol(),
        domain::UpstreamProtocol::OpenAiResponses | domain::UpstreamProtocol::OpenAiChatCompletions
    ) && route
        .account
        .account
        .base_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_responses_via_chat_completions(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::OpenAiResponses
        && route.account.upstream_protocol() == domain::UpstreamProtocol::OpenAiChatCompletions
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_anthropic_messages(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::AnthropicMessages
        && route.account.upstream_protocol() == domain::UpstreamProtocol::AnthropicMessages
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_gemini_generate_content(route: &GatewayRoute) -> bool {
    route.downstream == DownstreamProtocol::GeminiGenerateContent
        && route.account.upstream_protocol() == domain::UpstreamProtocol::GeminiGenerateContent
        && route
            .account
            .account
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && route
            .account
            .account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn can_forward_openai_endpoint(kind: GatewayEndpointKind, route: &GatewayRoute) -> bool {
    match kind {
        GatewayEndpointKind::OpenAiResponses => {
            can_forward_openai_responses(route) || can_forward_responses_via_chat_completions(route)
        }
        GatewayEndpointKind::OpenAiChatCompletions => can_forward_openai_chat_completions(route),
        _ => false,
    }
}

fn openai_upstream_not_configured(kind: GatewayEndpointKind) -> GatewayRuntimeError {
    match kind {
        GatewayEndpointKind::OpenAiResponses => {
            upstream_not_configured(kind, "OpenAI Responses upstream is not configured")
        }
        GatewayEndpointKind::OpenAiChatCompletions => {
            upstream_not_configured(kind, "OpenAI Chat Completions upstream is not configured")
        }
        _ => upstream_not_configured(kind, "OpenAI upstream is not configured"),
    }
}

fn should_retry_gateway_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn should_retry_gateway_error(error: &GatewayRuntimeError) -> bool {
    error.status == StatusCode::BAD_GATEWAY
        || error.status == StatusCode::SERVICE_UNAVAILABLE
        || error.status == StatusCode::GATEWAY_TIMEOUT
}

fn should_mark_account_failure(error: &GatewayRuntimeError) -> bool {
    should_retry_gateway_error(error)
        && !error.message.contains("upstream is not configured")
        && !error
            .message
            .contains("WebSocket cannot be bridged to Chat Completions upstream")
        && !error
            .message
            .contains("requires a WebSocket upgrade request")
}

fn has_previous_response_id(request: &Value) -> bool {
    previous_response_id(request).is_some()
}

fn previous_response_id(request: &Value) -> Option<&str> {
    request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn route_schedule_options(route: &GatewayRoute) -> AccountScheduleOptions {
    let extra = &route.account.account.extra;
    AccountScheduleOptions {
        max_concurrent: extra_usize(extra, "gateway_max_concurrent")
            .or_else(|| extra_usize(extra, "max_concurrent")),
        failure_cooldown_seconds: extra_u64(extra, "gateway_failure_cooldown_seconds")
            .or_else(|| extra_u64(extra, "failure_cooldown_seconds"))
            .unwrap_or(30),
        max_failure_cooldown_seconds: extra_u64(extra, "gateway_max_failure_cooldown_seconds")
            .or_else(|| extra_u64(extra, "max_failure_cooldown_seconds"))
            .unwrap_or(300),
    }
}

fn account_concurrency_lease_seconds(route: &GatewayRoute) -> i64 {
    let extra = &route.account.account.extra;
    extra_u64(extra, "gateway_concurrency_lease_seconds")
        .or_else(|| extra_u64(extra, "concurrency_lease_seconds"))
        .unwrap_or(300)
        .clamp(5, 3600)
        .try_into()
        .unwrap_or(300)
}

impl GatewayRuntimeService {
    async fn proxy_openai_responses_websocket(
        &self,
        client_socket: WebSocket,
        upstream_url: String,
        upstream_headers: HeaderMap,
    ) {
        let request = match build_tungstenite_ws_request(&upstream_url, &upstream_headers) {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    error = %error.message,
                    "failed to build upstream websocket request"
                );
                return;
            }
        };
        let Ok((upstream_socket, _response)) = tokio_tungstenite::connect_async(request).await
        else {
            tracing::warn!(upstream_url = %upstream_url, "failed to connect upstream websocket");
            return;
        };

        let (mut client_sender, mut client_receiver) = client_socket.split();
        let (mut upstream_sender, mut upstream_receiver) = upstream_socket.split();

        let client_to_upstream = async {
            while let Some(message) = client_receiver.next().await {
                let Ok(message) = message else {
                    break;
                };
                let close = matches!(message, AxumWsMessage::Close(_));
                let Some(message) = axum_ws_to_tungstenite(message) else {
                    continue;
                };
                if upstream_sender.send(message).await.is_err() {
                    break;
                }
                if close {
                    break;
                }
            }
        };

        let upstream_to_client = async {
            while let Some(message) = upstream_receiver.next().await {
                let Ok(message) = message else {
                    break;
                };
                let close = matches!(message, TungsteniteMessage::Close(_));
                let Some(message) = tungstenite_to_axum_ws(message) else {
                    continue;
                };
                if client_sender.send(message).await.is_err() {
                    break;
                }
                if close {
                    break;
                }
            }
        };

        tokio::select! {
            _ = client_to_upstream => {}
            _ = upstream_to_client => {}
        }
    }

    async fn forward_openai_json_request(
        &self,
        route: &GatewayRoute,
        upstream_path: &str,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
    ) -> Result<Response, GatewayRuntimeError> {
        self.forward_openai_body_request(
            route,
            upstream_path,
            body,
            query,
            model_mapping,
            Some(HeaderValue::from_static("application/json")),
        )
        .await
    }

    async fn forward_openai_body_request(
        &self,
        route: &GatewayRoute,
        upstream_path: &str,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        content_type: Option<HeaderValue>,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let endpoint = build_openai_endpoint_url(base_url, upstream_path, query);
        let content_type =
            content_type.unwrap_or_else(|| HeaderValue::from_static("application/json"));
        let upstream_body = if is_json_content_type(&content_type) {
            replace_json_model(body, &model_mapping.upstream_model)?
        } else {
            body
        };

        let upstream = self
            .http_client
            .post(endpoint)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(CONTENT_TYPE, content_type)
            .header(axum::http::header::ACCEPT, "application/json")
            .body(upstream_body)
            .send()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream request failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));
        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_openai_get_request(
        &self,
        route: &GatewayRoute,
        upstream_path: &str,
        query: Option<&str>,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let endpoint = build_openai_endpoint_url(base_url, upstream_path, query);
        let upstream = self
            .http_client
            .get(endpoint)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(axum::http::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream request failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));
        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_gemini_generate_content(
        &self,
        route: &GatewayRoute,
        normalized_path: &str,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        stream: bool,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::Google,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::Google,
            })?;
        let action = gemini_model_action_from_path(normalized_path)
            .map(|(_, action)| action)
            .unwrap_or_else(|| {
                if stream {
                    "streamGenerateContent".to_owned()
                } else {
                    "generateContent".to_owned()
                }
            });
        let upstream_path = format!(
            "/v1beta/models/{}:{}",
            model_mapping.upstream_model.trim(),
            action
        );
        let filtered_query = gemini_upstream_query(query, stream);
        let endpoint =
            build_gemini_endpoint_url(base_url, &upstream_path, filtered_query.as_deref());

        let mut request = self
            .http_client
            .post(endpoint)
            .header("x-goog-api-key", api_key)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }

        let upstream = request.send().await.map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("upstream request failed: {error}"),
            style: GatewayErrorStyle::Google,
        })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| {
                if stream && status.is_success() {
                    HeaderValue::from_static("text/event-stream")
                } else {
                    HeaderValue::from_static("application/json")
                }
            });

        if stream && status.is_success() {
            return Ok((
                status,
                [
                    (CONTENT_TYPE, content_type),
                    (
                        axum::http::header::CACHE_CONTROL,
                        HeaderValue::from_static("no-cache"),
                    ),
                ],
                Body::from_stream(upstream.bytes_stream()),
            )
                .into_response());
        }

        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::Google,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_gemini_get_request(
        &self,
        route: &GatewayRoute,
        upstream_path: &str,
        query: Option<&str>,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::Google,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::Google,
            })?;
        let filtered_query = filter_gateway_auth_query(query);
        let endpoint =
            build_gemini_endpoint_url(base_url, upstream_path, filtered_query.as_deref());
        let upstream = self
            .http_client
            .get(endpoint)
            .header("x-goog-api-key", api_key)
            .header(axum::http::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream request failed: {error}"),
                style: GatewayErrorStyle::Google,
            })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));
        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::Google,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_anthropic_request(
        &self,
        route: &GatewayRoute,
        upstream_path: &str,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        stream: bool,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::Anthropic,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::Anthropic,
            })?;
        let endpoint = build_openai_endpoint_url(base_url, upstream_path, query);
        let upstream_body = replace_json_model_with_style(
            body,
            &model_mapping.upstream_model,
            GatewayErrorStyle::Anthropic,
        )?;

        let mut request = self
            .http_client
            .post(endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header(CONTENT_TYPE, "application/json")
            .body(upstream_body);
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }

        let upstream = request.send().await.map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("upstream request failed: {error}"),
            style: GatewayErrorStyle::Anthropic,
        })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| {
                if stream && status.is_success() {
                    HeaderValue::from_static("text/event-stream")
                } else {
                    HeaderValue::from_static("application/json")
                }
            });

        if stream && status.is_success() {
            return Ok((
                status,
                [
                    (CONTENT_TYPE, content_type),
                    (
                        axum::http::header::CACHE_CONTROL,
                        HeaderValue::from_static("no-cache"),
                    ),
                ],
                Body::from_stream(upstream.bytes_stream()),
            )
                .into_response());
        }

        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::Anthropic,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_openai_responses(
        &self,
        route: &GatewayRoute,
        normalized_path: &str,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        stream: bool,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let upstream_path = openai_responses_upstream_path(normalized_path);
        let endpoint = build_openai_endpoint_url(base_url, &upstream_path, query);
        let upstream_body = replace_json_model(body, &model_mapping.upstream_model)?;

        let mut request = self
            .http_client
            .post(endpoint)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(CONTENT_TYPE, "application/json")
            .body(upstream_body);
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }

        let upstream = request.send().await.map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("upstream request failed: {error}"),
            style: GatewayErrorStyle::OpenAi,
        })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| {
                if stream && status.is_success() {
                    HeaderValue::from_static("text/event-stream")
                } else {
                    HeaderValue::from_static("application/json")
                }
            });

        if stream && status.is_success() {
            return Ok((
                status,
                [
                    (CONTENT_TYPE, content_type),
                    (
                        axum::http::header::CACHE_CONTROL,
                        HeaderValue::from_static("no-cache"),
                    ),
                ],
                Body::from_stream(upstream.bytes_stream()),
            )
                .into_response());
        }

        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_openai_chat_completions(
        &self,
        route: &GatewayRoute,
        body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        stream: bool,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let endpoint = build_openai_endpoint_url(base_url, "/v1/chat/completions", query);
        let upstream_body = replace_json_model(body, &model_mapping.upstream_model)?;

        let upstream = self
            .http_client
            .post(endpoint)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(CONTENT_TYPE, "application/json")
            .body(upstream_body)
            .send()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream request failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        let status = upstream.status();
        let content_type = upstream
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| {
                if stream && status.is_success() {
                    HeaderValue::from_static("text/event-stream")
                } else {
                    HeaderValue::from_static("application/json")
                }
            });

        if stream && status.is_success() {
            return Ok((
                status,
                [
                    (CONTENT_TYPE, content_type),
                    (
                        axum::http::header::CACHE_CONTROL,
                        HeaderValue::from_static("no-cache"),
                    ),
                ],
                Body::from_stream(upstream.bytes_stream()),
            )
                .into_response());
        }

        let bytes = upstream
            .bytes()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;

        Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response())
    }

    async fn forward_openai_responses_via_chat_completions(
        &self,
        route: &GatewayRoute,
        responses_request: &Value,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        stream: bool,
    ) -> Result<Response, GatewayRuntimeError> {
        let chat_request = self
            .build_stateful_responses_chat_request(route, responses_request, model_mapping)
            .await?;
        let chat_body = serde_json::to_vec(&chat_request)
            .map(Bytes::from)
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("failed to build chat completions bridge request: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        if stream {
            return self
                .forward_openai_responses_stream_via_chat_completions(
                    route,
                    chat_body,
                    query,
                    model_mapping,
                    &chat_request,
                )
                .await;
        }
        let chat_response = self
            .forward_openai_chat_completions(route, chat_body, query, model_mapping, false)
            .await?;
        let status = chat_response.status();
        let headers = chat_response.headers().clone();
        let body = axum::body::to_bytes(chat_response.into_body(), usize::MAX)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        if !status.is_success() {
            let content_type = headers
                .get(CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("application/json"));
            return Ok((status, [(CONTENT_TYPE, content_type)], Body::from(body)).into_response());
        }
        let chat_value: Value =
            serde_json::from_slice(&body).map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("failed to parse upstream chat completions response: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let responses_value =
            chat_completions_response_to_responses(&chat_value, &model_mapping.requested_model);
        self.store_responses_chat_state(route, &responses_value, &chat_request, &chat_value)
            .await?;
        Ok(json_response(responses_value))
    }

    async fn forward_openai_responses_stream_via_chat_completions(
        &self,
        route: &GatewayRoute,
        chat_body: Bytes,
        query: Option<&str>,
        model_mapping: &ModelMappingResolution,
        chat_request: &Value,
    ) -> Result<Response, GatewayRuntimeError> {
        let account = &route.account.account;
        let base_url = account
            .base_url
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream base_url is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let api_key = account
            .api_key
            .as_deref()
            .ok_or_else(|| GatewayRuntimeError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "upstream api_key is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let endpoint = build_openai_endpoint_url(base_url, "/v1/chat/completions", query);
        let upstream = self
            .http_client
            .post(endpoint)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(CONTENT_TYPE, "application/json")
            .header(axum::http::header::ACCEPT, "text/event-stream")
            .body(chat_body)
            .send()
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("upstream request failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let status = upstream.status();
        if !status.is_success() {
            let content_type = upstream
                .headers()
                .get(CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("application/json"));
            let bytes = upstream
                .bytes()
                .await
                .map_err(|error| GatewayRuntimeError {
                    status: StatusCode::BAD_GATEWAY,
                    message: format!("upstream response read failed: {error}"),
                    style: GatewayErrorStyle::OpenAi,
                })?;
            return Ok((status, [(CONTENT_TYPE, content_type)], Body::from(bytes)).into_response());
        }

        let body = upstream.text().await.map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("upstream stream read failed: {error}"),
            style: GatewayErrorStyle::OpenAi,
        })?;
        let mut state = ChatToResponsesStreamState::new(&model_mapping.requested_model);
        let mut sse = String::new();
        for payload in extract_sse_data_payloads(&body) {
            if payload == "[DONE]" {
                break;
            }
            let chunk: Value =
                serde_json::from_str(&payload).map_err(|error| GatewayRuntimeError {
                    status: StatusCode::BAD_GATEWAY,
                    message: format!("failed to parse upstream chat stream chunk: {error}"),
                    style: GatewayErrorStyle::OpenAi,
                })?;
            let events = state.apply_chat_chunk(&chunk);
            sse.push_str(&responses_stream_events_to_sse(&events).map_err(|error| {
                GatewayRuntimeError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: error.message,
                    style: GatewayErrorStyle::OpenAi,
                }
            })?);
        }
        let final_events = state.finalize();
        sse.push_str(
            &responses_stream_events_to_sse(&final_events).map_err(|error| {
                GatewayRuntimeError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: error.message,
                    style: GatewayErrorStyle::OpenAi,
                }
            })?,
        );
        sse.push_str("data: [DONE]\n\n");
        self.store_responses_chat_stream_state(route, state.response_id(), chat_request, &state)
            .await?;
        Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, HeaderValue::from_static("text/event-stream")),
                (
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ),
            ],
            Body::from(sse),
        )
            .into_response())
    }

    async fn build_stateful_responses_chat_request(
        &self,
        route: &GatewayRoute,
        responses_request: &Value,
        model_mapping: &ModelMappingResolution,
    ) -> Result<Value, GatewayRuntimeError> {
        let current_messages =
            responses_to_chat_messages(responses_request).map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_REQUEST,
                message: error.message,
                style: GatewayErrorStyle::OpenAi,
            })?;
        let previous_response_id = responses_request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let messages = if let Some(previous_response_id) = previous_response_id {
            let previous = self
                .responses_chat_store
                .get(previous_response_id)
                .await
                .map_err(|error| GatewayRuntimeError {
                    status: StatusCode::BAD_GATEWAY,
                    message: format!("responses chat state lookup failed: {error}"),
                    style: GatewayErrorStyle::OpenAi,
                })?
                .ok_or_else(|| GatewayRuntimeError {
                    status: StatusCode::BAD_REQUEST,
                    message: "previous_response_id was not found or has expired".to_owned(),
                    style: GatewayErrorStyle::OpenAi,
                })?;
            if previous.group_id != route.account.group_id.0
                || previous.account_id != route.account.account.id.0
            {
                return Err(GatewayRuntimeError {
                    status: StatusCode::BAD_REQUEST,
                    message: "previous_response_id does not belong to this route".to_owned(),
                    style: GatewayErrorStyle::OpenAi,
                });
            }
            let mut messages = previous.messages;
            messages.extend(current_messages);
            messages
        } else {
            current_messages
        };
        let compacted_messages = self
            .compact_responses_chat_messages_if_needed(route, messages, model_mapping)
            .await?;
        build_chat_completions_request(
            responses_request,
            &model_mapping.upstream_model,
            compacted_messages,
        )
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_REQUEST,
            message: error.message,
            style: GatewayErrorStyle::OpenAi,
        })
    }

    async fn compact_responses_chat_messages_if_needed(
        &self,
        route: &GatewayRoute,
        messages: Vec<Value>,
        model_mapping: &ModelMappingResolution,
    ) -> Result<Vec<Value>, GatewayRuntimeError> {
        let cfg = ResponsesChatCompactionConfig::from_account(route, &model_mapping.upstream_model);
        let limit = cfg.limit();
        if estimate_messages_tokens(&messages) <= limit {
            return Ok(messages);
        }
        let keep_count = cfg.keep_recent_turns.max(1).min(messages.len());
        let split_at = messages.len().saturating_sub(keep_count);
        let (older, recent) = messages.split_at(split_at);
        if older.is_empty() {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_REQUEST,
                message: "Context length exceeded and there is no older history to compact"
                    .to_owned(),
                style: GatewayErrorStyle::OpenAi,
            });
        }
        let summary = self
            .summarize_responses_chat_history(route, &cfg, older)
            .await?;
        let mut compacted = vec![json!({
            "role": "system",
            "content": format!("Conversation summary for earlier turns:\n{summary}")
        })];
        compacted.extend(recent.iter().cloned());
        if estimate_messages_tokens(&compacted) > limit {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_REQUEST,
                message: "Context length exceeded after LLM compaction".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            });
        }
        Ok(compacted)
    }

    async fn summarize_responses_chat_history(
        &self,
        route: &GatewayRoute,
        cfg: &ResponsesChatCompactionConfig,
        older_messages: &[Value],
    ) -> Result<String, GatewayRuntimeError> {
        if cfg.compaction_model.trim().is_empty() {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: "Responses chat compaction model is not configured".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            });
        }
        let transcript = older_messages
            .iter()
            .map(message_for_summary)
            .collect::<Vec<_>>()
            .join("\n");
        let request = json!({
            "model": cfg.compaction_model,
            "stream": false,
            "messages": [
                {
                    "role": "system",
                    "content": "Summarize the conversation history faithfully for continuing an AI assistant session. Preserve user goals, constraints, decisions, tool results, and unresolved tasks. Do not invent facts."
                },
                {
                    "role": "user",
                    "content": transcript
                }
            ],
            "max_completion_tokens": 1024,
            "temperature": 0.0
        });
        let request_body = serde_json::to_vec(&request)
            .map(Bytes::from)
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("failed to build compaction request: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        let compaction_mapping = ModelMappingResolution {
            requested_model: cfg.compaction_model.clone(),
            upstream_model: cfg.compaction_model.clone(),
            intermediate_models: Vec::new(),
            matched: false,
            matched_source: None,
        };
        let response = self
            .forward_openai_chat_completions(route, request_body, None, &compaction_mapping, false)
            .await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("compaction response read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })?;
        if !status.is_success() {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("LLM compaction upstream returned {status}"),
                style: GatewayErrorStyle::OpenAi,
            });
        }
        let value: Value = serde_json::from_slice(&body).map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("failed to parse compaction response: {error}"),
            style: GatewayErrorStyle::OpenAi,
        })?;
        let summary = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .map(chat_content_to_text)
            .unwrap_or_default();
        if summary.trim().is_empty() {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: "LLM compaction response has no summary".to_owned(),
                style: GatewayErrorStyle::OpenAi,
            });
        }
        Ok(summary)
    }

    async fn store_responses_chat_state(
        &self,
        route: &GatewayRoute,
        responses_value: &Value,
        chat_request: &Value,
        chat_response: &Value,
    ) -> Result<(), GatewayRuntimeError> {
        let Some(response_id) = responses_value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
        else {
            return Ok(());
        };
        let mut messages = chat_request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(assistant) = assistant_message_from_chat_response(chat_response) {
            messages.push(assistant);
        }
        let state = ResponsesChatState {
            group_id: route.account.group_id.0,
            account_id: route.account.account.id.0,
            messages,
        };
        self.responses_chat_store
            .set(&response_id, state)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("responses chat state store failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })
    }

    async fn store_responses_chat_stream_state(
        &self,
        route: &GatewayRoute,
        response_id: &str,
        chat_request: &Value,
        stream_state: &ChatToResponsesStreamState,
    ) -> Result<(), GatewayRuntimeError> {
        let response_id = response_id.trim();
        if response_id.is_empty() {
            return Ok(());
        }
        let mut messages = chat_request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        messages.push(stream_state.assistant_message());
        let state = ResponsesChatState {
            group_id: route.account.group_id.0,
            account_id: route.account.account.id.0,
            messages,
        };
        self.responses_chat_store
            .set(response_id, state)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("responses chat state store failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            })
    }
}

async fn record_gateway_response_usage(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    kind: GatewayEndpointKind,
    endpoint_path: &str,
    model_mapping: &ModelMappingResolution,
    stream: bool,
    response: Response,
) -> Result<Response, GatewayRuntimeError> {
    if stream || !response.status().is_success() {
        record_gateway_response_ops_event(
            state,
            identity,
            route,
            kind,
            endpoint_path,
            model_mapping,
            stream,
            response.status(),
            GatewayTokenUsage::default(),
            if response.status().is_success() {
                "gateway request completed"
            } else {
                "upstream returned error"
            },
            Value::Null,
        );
        return Ok(response);
    }

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("gateway usage response read failed: {error}"),
            style: error_style_for_kind(kind),
        })?;

    let usage = extract_gateway_token_usage(kind, &bytes);
    record_gateway_response_ops_event(
        state,
        identity,
        route,
        kind,
        endpoint_path,
        model_mapping,
        stream,
        status,
        usage,
        "gateway request completed",
        Value::Null,
    );
    let usage_log = state.admin_portal.record_gateway_usage(GatewayUsageRecord {
        user_id: identity.user_id,
        api_key_id: identity.id,
        api_key_name: identity.name.clone(),
        account_id: route.account.account.id.0,
        group_id: identity.group_id.or(Some(route.account.group_id.0)),
        provider: route_provider(route).to_owned(),
        downstream_protocol: route.downstream.as_str().to_owned(),
        upstream_protocol: route_upstream_protocol(route).to_owned(),
        endpoint: endpoint_path.to_owned(),
        request_type: gateway_request_type(kind).to_owned(),
        requested_model: model_mapping.requested_model.clone(),
        upstream_model: model_mapping.upstream_model.clone(),
        model_mapping_chain: model_mapping_chain(model_mapping),
        stream,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        status: "success".to_owned(),
        duration_ms: 0,
    });
    if let Some(actual_cost) = usage_log.get("actual_cost").and_then(Value::as_f64) {
        let updated_auth_snapshot = state
            .auth
            .increment_api_key_quota_used(identity.id, actual_cost);
        state
            .auth
            .increment_api_key_rate_limit_usage(identity.id, actual_cost);
        persist_api_key_usage_snapshot(
            state,
            identity,
            actual_cost,
            updated_auth_snapshot.is_some(),
        )
        .await?;
        increment_shared_api_key_rate_limit_usage(state, kind, identity, actual_cost).await?;
        persist_user_platform_quota_usage(state, identity, route, actual_cost)
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository platform quota write failed: {error}"),
                style: error_style_for_kind(kind),
            })?;
    }
    persist_gateway_usage(
        state,
        identity,
        route,
        endpoint_path,
        model_mapping,
        stream,
        &usage_log,
    )
    .await
    .map_err(|error| GatewayRuntimeError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("repository usage write failed: {error}"),
        style: error_style_for_kind(kind),
    })?;

    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(bytes))
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("gateway usage response rebuild failed: {error}"),
            style: error_style_for_kind(kind),
        })
}

fn gateway_error_with_ops(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    kind: GatewayEndpointKind,
    endpoint_path: &str,
    model_mapping: &ModelMappingResolution,
    stream: bool,
    error: GatewayRuntimeError,
) -> Response {
    record_gateway_response_ops_event(
        state,
        identity,
        route,
        kind,
        endpoint_path,
        model_mapping,
        stream,
        error.status,
        GatewayTokenUsage::default(),
        &error.message,
        Value::Null,
    );
    gateway_error(error).into_response()
}

fn record_gateway_response_ops_event(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    kind: GatewayEndpointKind,
    endpoint_path: &str,
    model_mapping: &ModelMappingResolution,
    stream: bool,
    status: StatusCode,
    usage: GatewayTokenUsage,
    message: &str,
    upstream_response: Value,
) {
    state.admin_ops.record_gateway_request(GatewayOpsEvent {
        request_id: new_gateway_ops_request_id(),
        client_request_id: None,
        user_id: Some(identity.user_id),
        api_key_id: Some(identity.id),
        group_id: identity.group_id.or(Some(route.account.group_id.0)),
        group_name: None,
        account_id: Some(route.account.account.id.0),
        account_name: Some(route.account.account.name.clone()),
        platform: route_provider(route).to_owned(),
        downstream_protocol: Some(route.downstream.as_str().to_owned()),
        upstream_protocol: Some(route_upstream_protocol(route).to_owned()),
        endpoint: endpoint_path.to_owned(),
        method: gateway_method_for_kind(kind).to_owned(),
        path: endpoint_path.to_owned(),
        model: Some(model_mapping.requested_model.clone()),
        upstream_model: Some(model_mapping.upstream_model.clone()),
        stream,
        status_code: status.as_u16(),
        duration_ms: 0,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        total_tokens: usage.total_tokens(),
        message: message.to_owned(),
        upstream_response,
    });
}

fn record_gateway_error_ops_event(
    state: &AppState,
    kind: GatewayEndpointKind,
    endpoint_path: &str,
    path: &str,
    method: &str,
    identity: Option<&GatewayApiKeyIdentity>,
    route: Option<&GatewayRoute>,
    error: &GatewayRuntimeError,
) {
    state.admin_ops.record_gateway_request(GatewayOpsEvent {
        request_id: new_gateway_ops_request_id(),
        client_request_id: None,
        user_id: identity.map(|identity| identity.user_id),
        api_key_id: identity.map(|identity| identity.id),
        group_id: identity
            .and_then(|identity| identity.group_id)
            .or_else(|| route.map(|route| route.account.group_id.0)),
        group_name: None,
        account_id: route.map(|route| route.account.account.id.0),
        account_name: route.map(|route| route.account.account.name.clone()),
        platform: route.map(route_provider).unwrap_or("gateway").to_owned(),
        downstream_protocol: downstream_protocol_for_kind(kind)
            .map(|protocol| protocol.as_str().to_owned()),
        upstream_protocol: route.map(route_upstream_protocol).map(ToOwned::to_owned),
        endpoint: endpoint_path.to_owned(),
        method: method.to_owned(),
        path: path.to_owned(),
        model: None,
        upstream_model: None,
        stream: false,
        status_code: error.status.as_u16(),
        duration_ms: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 0,
        message: error.message.clone(),
        upstream_response: Value::Null,
    });
}

fn new_gateway_ops_request_id() -> String {
    format!("gw-{}", uuid::Uuid::new_v4().simple())
}

fn gateway_method_for_kind(kind: GatewayEndpointKind) -> &'static str {
    match kind {
        GatewayEndpointKind::OpenAiModels
        | GatewayEndpointKind::OpenAiUsage
        | GatewayEndpointKind::GeminiModels
        | GatewayEndpointKind::AntigravityModels
        | GatewayEndpointKind::AntigravityUsage
        | GatewayEndpointKind::AntigravityGeminiModels
        | GatewayEndpointKind::OpenAiResponsesWebSocket => "GET",
        _ => "POST",
    }
}

async fn persist_user_platform_quota_usage(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    actual_cost: f64,
) -> repository::RepositoryResult<()> {
    if actual_cost <= 0.0 || !actual_cost.is_finite() {
        return Ok(());
    }
    let starts = crate::quota_window::current_window_starts();
    state
        .repository
        .increment_user_platform_quota_usage(
            identity.user_id,
            route_quota_platform(route),
            actual_cost,
            starts.daily,
            starts.weekly,
            starts.monthly,
        )
        .await?;
    Ok(())
}

async fn platform_quota_preflight(
    state: &AppState,
    kind: GatewayEndpointKind,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
) -> Result<(), Response> {
    let quota_platform = route_quota_platform(route);
    let quotas = state
        .repository
        .list_user_platform_quotas(identity.user_id)
        .await
        .map_err(|error| {
            gateway_error(GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository platform quota read failed: {error}"),
                style: error_style_for_kind(kind),
            })
            .into_response()
        })?;
    let Some(quota) = quotas
        .iter()
        .find(|quota| quota.platform.eq_ignore_ascii_case(quota_platform))
    else {
        return Ok(());
    };
    if let Some(exhaustion) =
        crate::quota_window::platform_quota_exhaustion(quota, chrono::Utc::now())
    {
        let message = format!(
            "User {quota_platform} {} platform quota exhausted",
            exhaustion.window
        );
        return Err(gateway_quota_error(
            kind,
            &message,
            exhaustion.retry_after_seconds,
            &exhaustion.reset_at,
        ));
    }
    Ok(())
}

async fn persist_gateway_usage(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    endpoint_path: &str,
    model_mapping: &ModelMappingResolution,
    stream: bool,
    usage_log: &Value,
) -> repository::RepositoryResult<UsageRecord> {
    ensure_repository_usage_principals(state, identity, route).await?;
    state
        .repository
        .insert_usage(UsageRecord {
            id: 0,
            user_id: identity.user_id,
            api_key_id: identity.id,
            group_id: identity.group_id.or(Some(route.account.group_id.0)).map(domain::GroupId),
            account_id: Some(route.account.account.id),
            downstream_protocol: route.downstream,
            upstream_protocol: route_upstream_protocol(route).to_owned(),
            provider: route_provider(route).to_owned(),
            endpoint: endpoint_path.to_owned(),
            requested_model: model_mapping.requested_model.clone(),
            upstream_model: model_mapping.upstream_model.clone(),
            input_tokens: usage_log
                .get("input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            output_tokens: usage_log
                .get("output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            cache_creation_tokens: usage_log
                .get("cache_creation_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            cache_read_tokens: usage_log
                .get("cache_read_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            actual_cost: usage_log
                .get("actual_cost")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            status: usage_log
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("success")
                .to_owned(),
            created_at_unix: now_unix_seconds(),
            metadata: json!({
                "admin_usage_id": usage_log.get("id").cloned().unwrap_or(Value::Null),
                "api_key_name": identity.name,
                "request_type": usage_log.get("request_type").cloned().unwrap_or(Value::Null),
                "stream": stream,
                "model_mapping_chain": usage_log.get("model_mapping_chain").cloned().unwrap_or(Value::Null),
                "total_tokens": usage_log.get("total_tokens").cloned().unwrap_or(Value::Null),
                "input_cost": usage_log.get("input_cost").cloned().unwrap_or(Value::Null),
                "output_cost": usage_log.get("output_cost").cloned().unwrap_or(Value::Null),
                "cache_creation_cost": usage_log.get("cache_creation_cost").cloned().unwrap_or(Value::Null),
                "cache_read_cost": usage_log.get("cache_read_cost").cloned().unwrap_or(Value::Null),
                "total_cost": usage_log.get("total_cost").cloned().unwrap_or(Value::Null),
                "cost": usage_log.get("cost").cloned().unwrap_or(Value::Null)
            }),
        })
        .await
}

async fn ensure_repository_usage_principals(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
) -> repository::RepositoryResult<()> {
    state
        .repository
        .upsert_user(repository::UserRecord {
            id: identity.user_id,
            email: if identity.user_id == 1 {
                "admin@example.com".to_owned()
            } else {
                format!("user-{}@local", identity.user_id)
            },
            username: if identity.user_id == 1 {
                "admin".to_owned()
            } else {
                format!("user-{}", identity.user_id)
            },
            role: if identity.user_id == 1 {
                "admin".to_owned()
            } else {
                "user".to_owned()
            },
            status: "active".to_owned(),
        })
        .await?;
    if let Some(group_id) = identity.group_id.or(Some(route.account.group_id.0)) {
        state
            .repository
            .upsert_group(domain::Group {
                id: domain::GroupId(group_id),
                name: format!("Group {group_id}"),
                status: domain::GroupStatus::Active,
            })
            .await?;
    }
    state
        .repository
        .upsert_account(route.account.account.clone())
        .await?;
    state
        .repository
        .bind_account_to_group(route.account.clone())
        .await?;
    let existing_api_key = state
        .repository
        .get_api_key(domain::ApiKeyId(identity.id))
        .await
        .ok();
    let mut api_key = existing_api_key.unwrap_or_else(|| domain::ApiKey {
        id: domain::ApiKeyId(identity.id),
        user_id: identity.user_id,
        key: format!("backend-next-runtime-{}", identity.id),
        name: identity.name.clone(),
        group_id: identity.group_id.map(domain::GroupId),
        status: domain::ApiKeyStatus::Active,
        ..domain::ApiKey::default()
    });
    api_key.user_id = identity.user_id;
    api_key.name = identity.name.clone();
    api_key.group_id = identity.group_id.map(domain::GroupId);
    api_key.status = domain::ApiKeyStatus::Active;
    state.repository.upsert_api_key(api_key).await?;
    Ok(())
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

async fn risk_control_preflight(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    kind: GatewayEndpointKind,
    endpoint_path: &str,
    request: &Value,
    model_mapping: &ModelMappingResolution,
    record_allow: bool,
) -> Result<(), Response> {
    let config = risk_control_config(state).await.map_err(|error| {
        gateway_error(GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("repository risk control config read failed: {error}"),
            style: error_style_for_kind(kind),
        })
        .into_response()
    })?;
    if !config.enabled || config.mode == "off" {
        return Ok(());
    }
    if !config.includes_group(identity.group_id.or(Some(route.account.group_id.0))) {
        return Ok(());
    }
    if !config.includes_model(&model_mapping.requested_model)
        && !config.includes_model(&model_mapping.upstream_model)
    {
        return Ok(());
    }

    let input = extract_risk_control_input(kind, request);
    if input.is_empty() {
        return Ok(());
    }
    let input_hash = risk_control_input_hash(&input);
    let mut action = None;
    let mut upstream_latency_ms = None;
    let mut moderation_error = String::new();
    let mut moderation_log_recorded = false;
    if config.keyword_precheck_enabled() {
        if let Some(keyword) = match_blocked_keyword(&input.text, &config.blocked_keywords) {
            action = Some(RiskControlAction {
                action: "keyword_block".to_owned(),
                category: "keyword".to_owned(),
                score: 1.0,
                category_scores: json!({ "keyword": 1.0, "matched_keyword": keyword }),
                blocked: true,
            });
        }
    }
    if action.is_none() && config.should_call_moderation_api() {
        let started = std::time::Instant::now();
        match call_risk_control_moderation_api(&state.gateway_runtime.http_client, &config, &input)
            .await
        {
            Ok(result) => {
                upstream_latency_ms =
                    Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
                let (flagged, category, score, scores) =
                    evaluate_moderation_scores(&result.category_scores, &config.thresholds);
                if flagged {
                    let blocked = matches!(config.mode.as_str(), "pre_block" | "block");
                    action = Some(RiskControlAction {
                        action: if blocked { "block" } else { "allow" }.to_owned(),
                        category,
                        score,
                        category_scores: scores,
                        blocked,
                    });
                } else if record_allow && config.record_non_hits {
                    let allow_action = RiskControlAction {
                        action: "allow".to_owned(),
                        category,
                        score,
                        category_scores: scores,
                        blocked: false,
                    };
                    record_risk_control_log(
                        state,
                        identity,
                        route,
                        endpoint_path,
                        &model_mapping.requested_model,
                        &config,
                        &input,
                        &allow_action,
                        false,
                        "",
                        upstream_latency_ms,
                    )
                    .await
                    .map_err(|error| {
                        gateway_error(GatewayRuntimeError {
                            status: StatusCode::INTERNAL_SERVER_ERROR,
                            message: format!("repository risk control log write failed: {error}"),
                            style: error_style_for_kind(kind),
                        })
                        .into_response()
                    })?;
                    moderation_log_recorded = true;
                }
            }
            Err(error) => {
                upstream_latency_ms =
                    Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
                moderation_error = error;
                if record_allow && config.record_non_hits {
                    let error_action = RiskControlAction {
                        action: "error".to_owned(),
                        category: String::new(),
                        score: 0.0,
                        category_scores: json!({}),
                        blocked: false,
                    };
                    record_risk_control_log(
                        state,
                        identity,
                        route,
                        endpoint_path,
                        &model_mapping.requested_model,
                        &config,
                        &input,
                        &error_action,
                        false,
                        &moderation_error,
                        upstream_latency_ms,
                    )
                    .await
                    .map_err(|error| {
                        gateway_error(GatewayRuntimeError {
                            status: StatusCode::INTERNAL_SERVER_ERROR,
                            message: format!("repository risk control log write failed: {error}"),
                            style: error_style_for_kind(kind),
                        })
                        .into_response()
                    })?;
                    moderation_log_recorded = true;
                }
            }
        }
    }
    if action.is_none() && config.pre_hash_check_enabled {
        let hashes = risk_control_flagged_hashes(state).await.map_err(|error| {
            gateway_error(GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository risk control hash read failed: {error}"),
                style: error_style_for_kind(kind),
            })
            .into_response()
        })?;
        if hashes
            .iter()
            .any(|hash| hash.trim().eq_ignore_ascii_case(&input_hash))
        {
            action = Some(RiskControlAction {
                action: "hash_block".to_owned(),
                category: "hash".to_owned(),
                score: 1.0,
                category_scores: json!({ "hash": 1.0, "input_hash": input_hash }),
                blocked: true,
            });
        }
    }

    if let Some(action) = action {
        if action.blocked || action.action == "block" {
            record_risk_control_flagged_hash(state, &input_hash)
                .await
                .map_err(|error| {
                    gateway_error(GatewayRuntimeError {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        message: format!("repository risk control hash write failed: {error}"),
                        style: error_style_for_kind(kind),
                    })
                    .into_response()
                })?;
        }
        record_risk_control_log(
            state,
            identity,
            route,
            endpoint_path,
            &model_mapping.requested_model,
            &config,
            &input,
            &action,
            true,
            &moderation_error,
            upstream_latency_ms,
        )
        .await
        .map_err(|error| {
            gateway_error(GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository risk control log write failed: {error}"),
                style: error_style_for_kind(kind),
            })
            .into_response()
        })?;
        if action.blocked {
            return Err(gateway_error(GatewayRuntimeError {
                status: config.block_status,
                message: config.block_message,
                style: error_style_for_kind(kind),
            })
            .into_response());
        }
        return Ok(());
    }

    if record_allow && config.record_non_hits && !moderation_log_recorded {
        let action = RiskControlAction {
            action: "allow".to_owned(),
            category: String::new(),
            score: 0.0,
            category_scores: json!({}),
            blocked: false,
        };
        record_risk_control_log(
            state,
            identity,
            route,
            endpoint_path,
            &model_mapping.requested_model,
            &config,
            &input,
            &action,
            false,
            "",
            None,
        )
        .await
        .map_err(|error| {
            gateway_error(GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository risk control log write failed: {error}"),
                style: error_style_for_kind(kind),
            })
            .into_response()
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct RiskControlConfig {
    enabled: bool,
    mode: String,
    base_url: String,
    model: String,
    api_keys: Vec<String>,
    timeout_ms: u64,
    retry_count: usize,
    all_groups: bool,
    group_ids: Vec<i64>,
    record_non_hits: bool,
    thresholds: std::collections::HashMap<String, f64>,
    block_status: StatusCode,
    block_message: String,
    pre_hash_check_enabled: bool,
    blocked_keywords: Vec<String>,
    keyword_blocking_mode: String,
    model_filter: RiskControlModelFilter,
    raw: Value,
}

#[derive(Debug, Clone)]
struct RiskControlModelFilter {
    filter_type: String,
    models: Vec<String>,
}

#[derive(Debug, Clone)]
struct RiskControlInput {
    text: String,
}

impl RiskControlInput {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
struct RiskControlAction {
    action: String,
    category: String,
    score: f64,
    category_scores: Value,
    blocked: bool,
}

impl RiskControlConfig {
    fn from_value(raw: Value) -> Self {
        let mode = value_string(&raw, "mode")
            .map(normalize_risk_control_mode)
            .unwrap_or_else(|| "off".to_owned());
        let block_status = value_i64(&raw, "block_status")
            .and_then(|value| u16::try_from(value).ok())
            .and_then(|value| StatusCode::from_u16(value).ok())
            .filter(|status| !status.is_success())
            .unwrap_or(StatusCode::BAD_REQUEST);
        Self {
            enabled: value_bool(&raw, "enabled"),
            mode,
            base_url: value_string(&raw, "base_url")
                .unwrap_or_else(|| "https://api.openai.com".to_owned()),
            model: value_string(&raw, "model")
                .unwrap_or_else(|| "omni-moderation-latest".to_owned()),
            api_keys: risk_control_api_keys(&raw),
            timeout_ms: value_i64(&raw, "timeout_ms")
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| (1..=30_000).contains(value))
                .unwrap_or(5_000),
            retry_count: value_i64(&raw, "retry_count")
                .and_then(|value| usize::try_from(value).ok())
                .map(|value| value.min(3))
                .unwrap_or(0),
            all_groups: raw
                .get("all_groups")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            group_ids: value_i64_array(&raw, "group_ids"),
            record_non_hits: value_bool(&raw, "record_non_hits"),
            thresholds: risk_control_thresholds(raw.get("thresholds")),
            block_status,
            block_message: value_string(&raw, "block_message")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "content moderation blocked this request".to_owned()),
            pre_hash_check_enabled: value_bool(&raw, "pre_hash_check_enabled"),
            blocked_keywords: value_string_array(&raw, "blocked_keywords"),
            keyword_blocking_mode: value_string(&raw, "keyword_blocking_mode")
                .map(normalize_keyword_blocking_mode)
                .unwrap_or_else(|| "keyword_and_api".to_owned()),
            model_filter: RiskControlModelFilter::from_value(raw.get("model_filter")),
            raw,
        }
    }

    fn includes_group(&self, group_id: Option<i64>) -> bool {
        if self.all_groups {
            return true;
        }
        group_id.is_some_and(|id| self.group_ids.contains(&id))
    }

    fn includes_model(&self, model: &str) -> bool {
        self.model_filter.includes(model)
    }

    fn keyword_precheck_enabled(&self) -> bool {
        matches!(self.mode.as_str(), "pre_block" | "block")
            && self.keyword_blocking_mode != "api_only"
            && !self.blocked_keywords.is_empty()
    }

    fn should_call_moderation_api(&self) -> bool {
        self.keyword_blocking_mode != "keyword_only" && !self.api_keys.is_empty()
    }
}

impl RiskControlModelFilter {
    fn from_value(value: Option<&Value>) -> Self {
        let filter_type = value
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                "include" => "include",
                "exclude" => "exclude",
                _ => "all",
            })
            .unwrap_or("all")
            .to_owned();
        let models = value
            .and_then(|value| value.get("models"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| value.trim().to_ascii_lowercase())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let models = if filter_type == "all" {
            Vec::new()
        } else {
            models
        };
        Self {
            filter_type,
            models,
        }
    }

    fn includes(&self, model: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        if model.is_empty() {
            return true;
        }
        let listed = self.models.iter().any(|candidate| candidate == &model);
        match self.filter_type.as_str() {
            "include" => listed,
            "exclude" => !listed,
            _ => true,
        }
    }
}

async fn risk_control_config(state: &AppState) -> repository::RepositoryResult<RiskControlConfig> {
    let default = default_risk_control_config_value();
    let value = match state
        .repository
        .get_system_setting("risk_control", "config")
        .await
    {
        Ok(record) => merge_json(default, record.value),
        Err(repository::RepositoryError::NotFound { .. }) => default,
        Err(error) => return Err(error),
    };
    Ok(RiskControlConfig::from_value(value))
}

async fn risk_control_flagged_hashes(
    state: &AppState,
) -> repository::RepositoryResult<Vec<String>> {
    match state
        .repository
        .get_system_setting("risk_control", "flagged_hashes")
        .await
    {
        Ok(record) => Ok(record
            .value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToOwned::to_owned))
            .collect()),
        Err(repository::RepositoryError::NotFound { .. }) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ModerationApiResponse {
    results: Vec<ModerationApiResult>,
}

#[derive(Debug, serde::Deserialize)]
struct ModerationApiResult {
    #[serde(default)]
    category_scores: std::collections::HashMap<String, f64>,
}

async fn call_risk_control_moderation_api(
    client: &reqwest::Client,
    config: &RiskControlConfig,
    input: &RiskControlInput,
) -> Result<ModerationApiResult, String> {
    let base_url = config.base_url.trim_end_matches('/');
    let endpoint = format!("{base_url}/v1/moderations");
    let payload = json!({
        "model": config.model,
        "input": input.text
    });
    let attempts = config.retry_count.saturating_add(1).clamp(1, 4);
    let mut last_error = None;
    for attempt in 0..attempts {
        let api_key = &config.api_keys[attempt % config.api_keys.len()];
        let result = client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .json(&payload)
            .send()
            .await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(error.to_string());
                continue;
            }
        };
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::new())
                .chars()
                .take(512)
                .collect::<String>();
            last_error = Some(format!("moderation api status {status}: {}", body.trim()));
            if status == StatusCode::BAD_REQUEST {
                break;
            }
            continue;
        }
        let payload = response
            .json::<ModerationApiResponse>()
            .await
            .map_err(|error| error.to_string())?;
        return payload
            .results
            .into_iter()
            .next()
            .ok_or_else(|| "moderation api returned empty results".to_owned());
    }
    Err(last_error.unwrap_or_else(|| "no moderation api key available".to_owned()))
}

fn evaluate_moderation_scores(
    scores: &std::collections::HashMap<String, f64>,
    thresholds: &std::collections::HashMap<String, f64>,
) -> (bool, String, f64, Value) {
    let mut flagged = false;
    let mut highest_category = String::new();
    let mut highest_score = 0.0;
    for category in RISK_CONTROL_CATEGORIES {
        let score = scores.get(*category).copied().unwrap_or(0.0);
        if score > highest_score || highest_category.is_empty() {
            highest_score = score;
            highest_category = (*category).to_owned();
        }
        if let Some(threshold) = thresholds.get(*category) {
            if score >= *threshold {
                flagged = true;
            }
        }
    }
    for (category, score) in scores {
        if *score > highest_score || highest_category.is_empty() {
            highest_score = *score;
            highest_category = category.clone();
        }
    }
    let category_scores = serde_json::to_value(scores).unwrap_or_else(|_| json!({}));
    (flagged, highest_category, highest_score, category_scores)
}

async fn record_risk_control_flagged_hash(
    state: &AppState,
    input_hash: &str,
) -> repository::RepositoryResult<()> {
    let input_hash = input_hash.trim().to_ascii_lowercase();
    if input_hash.is_empty() {
        return Ok(());
    }
    let mut hashes = risk_control_flagged_hashes(state).await?;
    if !hashes
        .iter()
        .any(|hash| hash.trim().eq_ignore_ascii_case(&input_hash))
    {
        hashes.push(input_hash);
    }
    hashes.sort();
    hashes.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    state
        .repository
        .upsert_system_setting(repository::SystemSettingRecord {
            namespace: "risk_control".to_owned(),
            key: "flagged_hashes".to_owned(),
            value: json!(hashes),
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .await?;
    Ok(())
}

async fn record_risk_control_log(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
    endpoint_path: &str,
    model: &str,
    config: &RiskControlConfig,
    input: &RiskControlInput,
    action: &RiskControlAction,
    flagged: bool,
    error: &str,
    upstream_latency_ms: Option<i64>,
) -> repository::RepositoryResult<repository::ContentModerationLogRecord> {
    let user = state.repository.get_user(identity.user_id).await.ok();
    let group_id = identity.group_id.or(Some(route.account.group_id.0));
    let group = match group_id {
        Some(group_id) => state
            .repository
            .get_group(domain::GroupId(group_id))
            .await
            .ok(),
        None => None,
    };
    state
        .repository
        .insert_content_moderation_log(repository::ContentModerationLogRecord {
            id: 0,
            request_id: format!("risk-{}", uuid::Uuid::new_v4().simple()),
            user_id: Some(identity.user_id),
            user_email: user
                .as_ref()
                .map(|user| user.email.clone())
                .unwrap_or_else(|| format!("user-{}@local", identity.user_id)),
            api_key_id: (identity.id > 0).then_some(identity.id),
            api_key_name: identity.name.clone(),
            group_id,
            group_name: group
                .as_ref()
                .map(|group| group.name.clone())
                .unwrap_or_else(|| group_id.map_or_else(String::new, |id| format!("Group {id}"))),
            endpoint: endpoint_path.to_owned(),
            provider: route_provider(route).to_owned(),
            model: model.to_owned(),
            mode: config.mode.clone(),
            action: action.action.to_owned(),
            flagged,
            highest_category: action.category.to_owned(),
            highest_score: action.score,
            category_scores: action.category_scores.clone(),
            threshold_snapshot: config
                .raw
                .get("thresholds")
                .cloned()
                .unwrap_or_else(|| json!({})),
            input_excerpt: risk_control_excerpt(&input.text),
            upstream_latency_ms,
            error: error.to_owned(),
            violation_count: i64::from(flagged),
            auto_banned: false,
            email_sent: false,
            user_status: user
                .as_ref()
                .map(|user| user.status.clone())
                .unwrap_or_else(|| "active".to_owned()),
            queue_delay_ms: None,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })
        .await
}

fn extract_risk_control_input(kind: GatewayEndpointKind, request: &Value) -> RiskControlInput {
    let mut parts = Vec::new();
    match kind {
        GatewayEndpointKind::OpenAiChatCompletions => {
            collect_chat_messages_text(request.get("messages"), &mut parts);
        }
        GatewayEndpointKind::OpenAiResponses | GatewayEndpointKind::OpenAiResponsesWebSocket => {
            collect_responses_input_text(request.get("input"), &mut parts);
            collect_string_field(request, "instructions", &mut parts);
        }
        GatewayEndpointKind::AnthropicMessages | GatewayEndpointKind::AntigravityMessages => {
            collect_string_field(request, "system", &mut parts);
            collect_anthropic_messages_text(request.get("messages"), &mut parts);
        }
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => {
            collect_gemini_contents_text(request.get("contents"), &mut parts);
        }
        _ => collect_json_strings(request, &mut parts),
    }
    if parts.is_empty() {
        collect_json_strings(request, &mut parts);
    }
    RiskControlInput {
        text: normalize_moderation_text(&parts.join("\n")),
    }
}

fn collect_chat_messages_text(value: Option<&Value>, parts: &mut Vec<String>) {
    let Some(messages) = value.and_then(Value::as_array) else {
        return;
    };
    for message in messages {
        collect_content_text(message.get("content"), parts);
    }
}

fn collect_responses_input_text(value: Option<&Value>, parts: &mut Vec<String>) {
    match value {
        Some(Value::String(text)) => push_text(parts, text),
        Some(Value::Array(items)) => {
            for item in items {
                collect_content_text(Some(item), parts);
                collect_content_text(item.get("content"), parts);
            }
        }
        Some(Value::Object(_)) => collect_content_text(value, parts),
        _ => {}
    }
}

fn collect_anthropic_messages_text(value: Option<&Value>, parts: &mut Vec<String>) {
    let Some(messages) = value.and_then(Value::as_array) else {
        return;
    };
    for message in messages {
        collect_content_text(message.get("content"), parts);
    }
}

fn collect_gemini_contents_text(value: Option<&Value>, parts: &mut Vec<String>) {
    let Some(contents) = value.and_then(Value::as_array) else {
        return;
    };
    for content in contents {
        if let Some(parts_value) = content.get("parts").and_then(Value::as_array) {
            for part in parts_value {
                collect_string_field(part, "text", parts);
            }
        }
    }
}

fn collect_content_text(value: Option<&Value>, parts: &mut Vec<String>) {
    match value {
        Some(Value::String(text)) => push_text(parts, text),
        Some(Value::Array(items)) => {
            for item in items {
                collect_content_text(Some(item), parts);
            }
        }
        Some(Value::Object(object)) => {
            for key in [
                "text",
                "content",
                "input_text",
                "output_text",
                "message",
                "summary",
            ] {
                if let Some(value) = object.get(key).and_then(Value::as_str) {
                    push_text(parts, value);
                }
            }
        }
        _ => {}
    }
}

fn collect_string_field(value: &Value, key: &str, parts: &mut Vec<String>) {
    if let Some(text) = value.get(key).and_then(Value::as_str) {
        push_text(parts, text);
    }
}

fn collect_json_strings(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::String(text) => push_text(parts, text),
        Value::Array(items) => {
            for item in items {
                collect_json_strings(item, parts);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_json_strings(value, parts);
            }
        }
        _ => {}
    }
}

fn push_text(parts: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_owned());
    }
}

fn normalize_moderation_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn risk_control_input_hash(input: &RiskControlInput) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"text:");
    hasher.update(input.text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn risk_control_excerpt(text: &str) -> String {
    text.chars().take(240).collect()
}

fn match_blocked_keyword(text: &str, keywords: &[String]) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    keywords
        .iter()
        .map(|keyword| keyword.trim())
        .filter(|keyword| !keyword.is_empty())
        .find(|keyword| lower.contains(&keyword.to_ascii_lowercase()))
        .map(ToOwned::to_owned)
}

fn normalize_risk_control_mode(mode: String) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "observe" => "observe".to_owned(),
        "pre_block" | "block" => "pre_block".to_owned(),
        _ => "off".to_owned(),
    }
}

fn normalize_keyword_blocking_mode(mode: String) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "keyword_only" => "keyword_only".to_owned(),
        "api_only" => "api_only".to_owned(),
        _ => "keyword_and_api".to_owned(),
    }
}

fn value_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn value_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn value_i64_array(value: &Value, key: &str) -> Vec<i64> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

fn value_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn risk_control_api_keys(value: &Value) -> Vec<String> {
    let mut keys = value_string_array(value, "api_keys");
    if let Some(api_key) = value_string(value, "api_key") {
        keys.push(api_key);
    }
    let mut out = Vec::new();
    for key in keys {
        if !out.iter().any(|existing: &String| existing == &key) {
            out.push(key);
        }
    }
    out
}

fn risk_control_thresholds(overrides: Option<&Value>) -> std::collections::HashMap<String, f64> {
    let mut thresholds = [
        ("harassment", 0.98),
        ("harassment/threatening", 0.90),
        ("hate", 0.65),
        ("hate/threatening", 0.65),
        ("illicit", 0.95),
        ("illicit/violent", 0.95),
        ("self-harm", 0.65),
        ("self-harm/intent", 0.85),
        ("self-harm/instructions", 0.65),
        ("sexual", 0.65),
        ("sexual/minors", 0.65),
        ("violence", 0.95),
        ("violence/graphic", 0.95),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect::<std::collections::HashMap<_, _>>();
    if let Some(object) = overrides.and_then(Value::as_object) {
        for category in RISK_CONTROL_CATEGORIES {
            if let Some(value) = object.get(*category).and_then(Value::as_f64) {
                thresholds.insert((*category).to_owned(), value.clamp(0.0, 1.0));
            }
        }
    }
    thresholds
}

fn default_risk_control_config_value() -> Value {
    json!({
        "enabled": false,
        "mode": "off",
        "base_url": "https://api.openai.com",
        "model": "omni-moderation-latest",
        "api_keys": [],
        "timeout_ms": 5000,
        "retry_count": 0,
        "record_non_hits": false,
        "block_status": 400,
        "block_message": "content moderation blocked this request",
        "pre_hash_check_enabled": false,
        "blocked_keywords": [],
        "keyword_blocking_mode": "keyword_and_api",
        "model_filter": {
            "type": "all",
            "models": []
        },
        "all_groups": true,
        "group_ids": [],
        "thresholds": {}
    })
}

fn merge_json(mut base: Value, patch: Value) -> Value {
    match (&mut base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                let merged = merge_json(base.remove(&key).unwrap_or(Value::Null), value);
                base.insert(key, merged);
            }
            Value::Object(base.clone())
        }
        (_, value) => value,
    }
}

fn extract_gateway_token_usage(kind: GatewayEndpointKind, body: &[u8]) -> GatewayTokenUsage {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return GatewayTokenUsage::default();
    };
    match kind {
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => {
            extract_gemini_token_usage(&value)
        }
        GatewayEndpointKind::AnthropicMessages
        | GatewayEndpointKind::AnthropicCountTokens
        | GatewayEndpointKind::AntigravityMessages
        | GatewayEndpointKind::AntigravityCountTokens => extract_anthropic_token_usage(&value),
        _ => extract_openai_token_usage(&value),
    }
}

fn extract_openai_token_usage(value: &Value) -> GatewayTokenUsage {
    let usage = value.get("usage").or_else(|| {
        value
            .get("response")
            .and_then(|response| response.get("usage"))
    });
    let Some(usage) = usage else {
        return GatewayTokenUsage::default();
    };
    let input_tokens = int_field_any(usage, &["input_tokens", "prompt_tokens"]);
    let output_tokens = int_field_any(usage, &["output_tokens", "completion_tokens"]);
    let cache_read_tokens = int_path_any(
        usage,
        &[
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
            &["cache_read_input_tokens"],
        ],
    );
    let cache_creation_tokens = int_path_any(
        usage,
        &[
            &["cache_creation_input_tokens"],
            &["cache_creation", "ephemeral_5m_input_tokens"],
            &["cache_creation", "ephemeral_1h_input_tokens"],
        ],
    );
    GatewayTokenUsage {
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    }
}

fn extract_anthropic_token_usage(value: &Value) -> GatewayTokenUsage {
    let usage = value
        .get("usage")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("usage"))
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        });
    let Some(usage) = usage else {
        return GatewayTokenUsage::default();
    };
    GatewayTokenUsage {
        input_tokens: int_field_any(usage, &["input_tokens", "prompt_tokens"]),
        output_tokens: int_field_any(usage, &["output_tokens", "completion_tokens"]),
        cache_creation_tokens: int_field_any(usage, &["cache_creation_input_tokens"])
            + int_path(usage, &["cache_creation", "ephemeral_5m_input_tokens"])
            + int_path(usage, &["cache_creation", "ephemeral_1h_input_tokens"]),
        cache_read_tokens: int_field_any(usage, &["cache_read_input_tokens"]),
    }
}

fn extract_gemini_token_usage(value: &Value) -> GatewayTokenUsage {
    let Some(usage) = value.get("usageMetadata") else {
        return GatewayTokenUsage::default();
    };
    GatewayTokenUsage {
        input_tokens: int_field_any(usage, &["promptTokenCount"]),
        output_tokens: int_field_any(usage, &["candidatesTokenCount"]),
        cache_creation_tokens: 0,
        cache_read_tokens: int_field_any(usage, &["cachedContentTokenCount"]),
    }
}

fn int_field_any(value: &Value, fields: &[&str]) -> i64 {
    fields
        .iter()
        .find_map(|field| number_to_i64(value.get(*field)?))
        .unwrap_or(0)
}

fn int_path_any(value: &Value, paths: &[&[&str]]) -> i64 {
    paths.iter().map(|path| int_path(value, path)).sum()
}

fn int_path(value: &Value, path: &[&str]) -> i64 {
    let mut current = value;
    for segment in path {
        let Some(next) = current.get(*segment) else {
            return 0;
        };
        current = next;
    }
    number_to_i64(current).unwrap_or(0)
}

fn number_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn model_mapping_chain(model_mapping: &ModelMappingResolution) -> Vec<String> {
    let mut chain = vec![model_mapping.requested_model.clone()];
    chain.extend(model_mapping.intermediate_models.iter().cloned());
    if chain
        .last()
        .map(|last| last != &model_mapping.upstream_model)
        .unwrap_or(true)
    {
        chain.push(model_mapping.upstream_model.clone());
    }
    chain
}

fn gateway_request_type(kind: GatewayEndpointKind) -> &'static str {
    match kind {
        GatewayEndpointKind::OpenAiResponses | GatewayEndpointKind::OpenAiResponsesWebSocket => {
            "responses"
        }
        GatewayEndpointKind::OpenAiChatCompletions => "chat_completions",
        GatewayEndpointKind::AnthropicMessages | GatewayEndpointKind::AntigravityMessages => {
            "anthropic_messages"
        }
        GatewayEndpointKind::AnthropicCountTokens | GatewayEndpointKind::AntigravityCountTokens => {
            "count_tokens"
        }
        GatewayEndpointKind::OpenAiEmbeddings => "embeddings",
        GatewayEndpointKind::OpenAiImageGenerations | GatewayEndpointKind::OpenAiImageEdits => {
            "images"
        }
        GatewayEndpointKind::GeminiGenerateContent
        | GatewayEndpointKind::AntigravityGeminiGenerateContent => "gemini_generate_content",
        GatewayEndpointKind::OpenAiModels
        | GatewayEndpointKind::AntigravityModels
        | GatewayEndpointKind::GeminiModels
        | GatewayEndpointKind::AntigravityGeminiModels => "models",
        GatewayEndpointKind::OpenAiUsage | GatewayEndpointKind::AntigravityUsage => "usage",
    }
}

fn should_apply_api_key_rate_limit(kind: GatewayEndpointKind) -> bool {
    !matches!(
        kind,
        GatewayEndpointKind::OpenAiUsage
            | GatewayEndpointKind::AntigravityUsage
            | GatewayEndpointKind::OpenAiModels
            | GatewayEndpointKind::AntigravityModels
            | GatewayEndpointKind::GeminiModels
            | GatewayEndpointKind::AntigravityGeminiModels
    )
}

async fn api_key_rate_limit_preflight(
    state: &AppState,
    kind: GatewayEndpointKind,
    identity: &GatewayApiKeyIdentity,
) -> Result<(), GatewayRuntimeError> {
    if !state.repository.uses_shared_consistency_backend() {
        return state
            .auth
            .check_api_key_rate_limits(identity.id)
            .map_err(|error| GatewayRuntimeError {
                status: error.status(),
                message: error.message().to_owned(),
                style: error_style_for_kind(kind),
            });
    }
    let Some(snapshot) = api_key_usage_view(state, identity).await? else {
        return Ok(());
    };
    for window in shared_api_key_rate_limit_windows(&snapshot) {
        let usage = state
            .repository
            .get_rate_limit_usage_fixed_window(
                &window.scope,
                window.limit,
                window.window_start_unix,
                window.window_seconds,
            )
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository API key rate limit read failed: {error}"),
                style: error_style_for_kind(kind),
            })?;
        if usage.usage >= window.limit {
            return Err(GatewayRuntimeError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: format!("API key {} rate limit exceeded", window.label),
                style: error_style_for_kind(kind),
            });
        }
    }
    Ok(())
}

async fn api_key_usage_view(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
) -> Result<Option<ApiKeyUsageView>, GatewayRuntimeError> {
    if let Some(snapshot) = state.auth.api_key_usage_snapshot(identity.id) {
        return Ok(Some(snapshot.into()));
    }
    match state
        .repository
        .get_api_key(domain::ApiKeyId(identity.id))
        .await
    {
        Ok(api_key) => Ok(Some(api_key.into())),
        Err(repository::RepositoryError::NotFound { .. }) => Ok(None),
        Err(error) => Err(GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("repository API key read failed: {error}"),
            style: GatewayErrorStyle::OpenAi,
        }),
    }
}

async fn increment_shared_api_key_rate_limit_usage(
    state: &AppState,
    kind: GatewayEndpointKind,
    identity: &GatewayApiKeyIdentity,
    amount: f64,
) -> Result<(), GatewayRuntimeError> {
    if !state.repository.uses_shared_consistency_backend() || !amount.is_finite() || amount <= 0.0 {
        return Ok(());
    }
    let Some(snapshot) = api_key_usage_view(state, identity).await? else {
        return Ok(());
    };
    for window in shared_api_key_rate_limit_windows(&snapshot) {
        state
            .repository
            .add_rate_limit_usage_fixed_window(
                &window.scope,
                amount,
                window.limit,
                window.window_start_unix,
                window.window_seconds,
            )
            .await
            .map_err(|error| GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository API key rate limit write failed: {error}"),
                style: error_style_for_kind(kind),
            })?;
    }
    Ok(())
}

async fn persist_api_key_usage_snapshot(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    amount: f64,
    auth_snapshot_updated: bool,
) -> Result<(), GatewayRuntimeError> {
    let mut api_key = match state
        .repository
        .get_api_key(domain::ApiKeyId(identity.id))
        .await
    {
        Ok(api_key) => api_key,
        Err(error) => {
            if state.auth.api_key_usage_snapshot(identity.id).is_none() {
                return Ok(());
            }
            return Err(GatewayRuntimeError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("repository API key usage read failed: {error}"),
                style: GatewayErrorStyle::OpenAi,
            });
        }
    };
    if auth_snapshot_updated {
        let Some(snapshot) = state.auth.api_key_usage_snapshot(identity.id) else {
            return Ok(());
        };
        api_key.user_id = snapshot.user_id;
        api_key.name = snapshot.name;
        api_key.group_id = snapshot.group_id.map(domain::GroupId);
        api_key.status = match snapshot.status.as_str() {
            "active" => domain::ApiKeyStatus::Active,
            "quota_exhausted" => domain::ApiKeyStatus::QuotaExhausted,
            "expired" => domain::ApiKeyStatus::Expired,
            _ => domain::ApiKeyStatus::Disabled,
        };
        api_key.quota = snapshot.quota;
        api_key.quota_used = snapshot.quota_used;
        api_key.rate_limit_5h = snapshot.rate_limit_5h;
        api_key.rate_limit_1d = snapshot.rate_limit_1d;
        api_key.rate_limit_7d = snapshot.rate_limit_7d;
        api_key.usage_5h = snapshot.usage_5h;
        api_key.usage_1d = snapshot.usage_1d;
        api_key.usage_7d = snapshot.usage_7d;
        api_key.window_5h_start = snapshot.window_5h_start;
        api_key.window_1d_start = snapshot.window_1d_start;
        api_key.window_7d_start = snapshot.window_7d_start;
    } else if amount.is_finite() && amount > 0.0 {
        let now = now_unix_seconds();
        api_key.quota_used += amount;
        if api_key.quota > 0.0 && api_key.quota_used >= api_key.quota {
            api_key.status = domain::ApiKeyStatus::QuotaExhausted;
        }
        apply_repository_api_key_rate_limit_increment(&mut api_key, amount, now);
    }
    state
        .repository
        .upsert_api_key(api_key)
        .await
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("repository API key usage write failed: {error}"),
            style: GatewayErrorStyle::OpenAi,
        })?;
    Ok(())
}

fn apply_repository_api_key_rate_limit_increment(api_key: &mut ApiKey, amount: f64, now: i64) {
    if repository_rate_window_expired(api_key.window_5h_start, now, 5 * 60 * 60) {
        api_key.usage_5h = 0.0;
        api_key.window_5h_start = None;
    }
    if repository_rate_window_expired(api_key.window_1d_start, now, 24 * 60 * 60) {
        api_key.usage_1d = 0.0;
        api_key.window_1d_start = None;
    }
    if repository_rate_window_expired(api_key.window_7d_start, now, 7 * 24 * 60 * 60) {
        api_key.usage_7d = 0.0;
        api_key.window_7d_start = None;
    }
    if api_key.window_5h_start.is_none() {
        api_key.window_5h_start = Some(now);
    }
    if api_key.window_1d_start.is_none() {
        api_key.window_1d_start = Some(now);
    }
    if api_key.window_7d_start.is_none() {
        api_key.window_7d_start = Some(now);
    }
    api_key.usage_5h += amount;
    api_key.usage_1d += amount;
    api_key.usage_7d += amount;
}

fn repository_rate_window_expired(start: Option<i64>, now: i64, window_seconds: i64) -> bool {
    start.is_some_and(|start| start.saturating_add(window_seconds) <= now)
}

#[derive(Debug, Clone)]
struct SharedApiKeyRateLimitWindow {
    label: &'static str,
    scope: String,
    limit: f64,
    window_start_unix: i64,
    window_seconds: i64,
}

fn shared_api_key_rate_limit_windows(
    snapshot: &ApiKeyUsageView,
) -> Vec<SharedApiKeyRateLimitWindow> {
    [
        ("5h", snapshot.rate_limit_5h, 5 * 60 * 60),
        ("1d", snapshot.rate_limit_1d, 24 * 60 * 60),
        ("7d", snapshot.rate_limit_7d, 7 * 24 * 60 * 60),
    ]
    .into_iter()
    .filter(|(_, limit, _)| limit.is_finite() && *limit > 0.0)
    .map(
        |(label, limit, window_seconds)| SharedApiKeyRateLimitWindow {
            label,
            scope: format!("api-key:{}:{label}", snapshot.id),
            limit,
            window_start_unix: fixed_window_start(now_unix_seconds(), window_seconds),
            window_seconds,
        },
    )
    .collect()
}

fn fixed_window_start(now: i64, window_seconds: i64) -> i64 {
    let window_seconds = window_seconds.max(1);
    now - now.rem_euclid(window_seconds)
}

fn extract_sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("data:"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn estimate_messages_tokens(messages: &[Value]) -> usize {
    let chars = messages.iter().map(message_text_len).sum::<usize>();
    chars.div_ceil(4)
}

fn message_text_len(message: &Value) -> usize {
    message
        .get("content")
        .map(value_text_len)
        .unwrap_or_default()
        + message
            .get("tool_calls")
            .map(value_text_len)
            .unwrap_or_default()
}

fn value_text_len(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::Bool(_) | Value::Number(_) => value.to_string().len(),
        Value::String(text) => text.chars().count(),
        Value::Array(items) => items.iter().map(value_text_len).sum(),
        Value::Object(object) => object.values().map(value_text_len).sum(),
    }
}

fn message_for_summary(message: &Value) -> String {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let content = message
        .get("content")
        .map(chat_content_to_text)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            message
                .get("tool_calls")
                .map(Value::to_string)
                .unwrap_or_default()
        });
    format!("{role}: {content}")
}

fn chat_content_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => value.to_string(),
    }
}

fn extra_usize(extra: &Value, key: &str) -> Option<usize> {
    extra_u64(extra, key).map(|value| value as usize)
}

fn extra_u64(extra: &Value, key: &str) -> Option<u64> {
    extra.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
    })
}

fn extra_string(extra: &Value, key: &str) -> Option<String> {
    extra
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn replace_json_model(body: Bytes, upstream_model: &str) -> Result<Bytes, GatewayRuntimeError> {
    replace_json_model_with_style(body, upstream_model, GatewayErrorStyle::OpenAi)
}

fn replace_json_model_with_style(
    body: Bytes,
    upstream_model: &str,
    style: GatewayErrorStyle,
) -> Result<Bytes, GatewayRuntimeError> {
    if upstream_model.trim().is_empty() {
        return Ok(body);
    }
    let mut value: Value = serde_json::from_slice(&body).map_err(|error| GatewayRuntimeError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid JSON request body: {error}"),
        style,
    })?;
    value["model"] = json!(upstream_model);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("failed to rewrite upstream request model: {error}"),
            style,
        })
}

fn is_json_content_type(content_type: &HeaderValue) -> bool {
    content_type
        .to_str()
        .map(|value| value.to_ascii_lowercase().contains("json"))
        .unwrap_or(false)
}

fn openai_responses_upstream_path(normalized_path: &str) -> String {
    let path = normalized_path.trim().trim_end_matches('/');
    for prefix in [
        "/v1/responses",
        "/responses",
        "/backend-api/codex/responses",
    ] {
        if path == prefix {
            return "/v1/responses".to_owned();
        }
        if let Some(rest) = path.strip_prefix(prefix) {
            if rest.starts_with('/') {
                return format!("/v1/responses{rest}");
            }
        }
    }
    "/v1/responses".to_owned()
}

fn gemini_model_action_from_path(normalized_path: &str) -> Option<(String, String)> {
    let path = normalized_path.trim().trim_end_matches('/');
    let rest = path
        .strip_prefix("/v1beta/models/")
        .or_else(|| path.strip_prefix("/antigravity/v1beta/models/"))?;
    let (model, action) = rest.rsplit_once(':').or_else(|| rest.rsplit_once('/'))?;
    let model = model.trim();
    let action = action.trim();
    if model.is_empty() || action.is_empty() {
        return None;
    }
    let canonical_action = match action.to_ascii_lowercase().as_str() {
        "generatecontent" => "generateContent",
        "streamgeneratecontent" => "streamGenerateContent",
        "counttokens" => "countTokens",
        _ => return None,
    };
    Some((model.to_owned(), canonical_action.to_owned()))
}

fn gemini_models_upstream_path(normalized_path: &str) -> String {
    let path = normalized_path.trim().trim_end_matches('/');
    for prefix in ["/v1beta/models", "/antigravity/v1beta/models"] {
        if path == prefix {
            return "/v1beta/models".to_owned();
        }
        if let Some(rest) = path.strip_prefix(prefix) {
            if rest.starts_with('/') && !rest.contains(':') {
                return format!("/v1beta/models{rest}");
            }
        }
    }
    "/v1beta/models".to_owned()
}

fn filter_gateway_auth_query(query: Option<&str>) -> Option<String> {
    let query = query?.trim();
    if query.is_empty() {
        return None;
    }
    let parts: Vec<&str> = query
        .split('&')
        .filter(|part| {
            let key = part.split_once('=').map(|(key, _)| key).unwrap_or(*part);
            !matches!(key, "key" | "api_key")
        })
        .filter(|part| !part.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("&"))
}

fn gemini_upstream_query(query: Option<&str>, stream: bool) -> Option<String> {
    let mut parts = filter_gateway_auth_query(query)
        .map(|query| {
            query
                .split('&')
                .filter(|part| {
                    let key = part.split_once('=').map(|(key, _)| key).unwrap_or(*part);
                    key != "alt"
                })
                .filter(|part| !part.trim().is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if stream {
        parts.push("alt=sse".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join("&"))
}

fn build_openai_endpoint_url(base: &str, endpoint: &str, query: Option<&str>) -> String {
    let normalized = base.trim().trim_end_matches('/');
    let endpoint = format!("/{}", endpoint.trim().trim_start_matches('/'));
    let relative = endpoint.strip_prefix("/v1").unwrap_or(endpoint.as_str());
    let mut url = if normalized.ends_with(&endpoint) || normalized.ends_with(relative) {
        normalized.to_owned()
    } else if openai_base_url_has_version_suffix(normalized) {
        format!("{normalized}{relative}")
    } else {
        format!("{normalized}{endpoint}")
    };
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn build_openai_responses_ws_url(
    route: &GatewayRoute,
    normalized_path: &str,
    query: Option<&str>,
) -> Result<String, GatewayRuntimeError> {
    let account = &route.account.account;
    let base_url = account
        .base_url
        .as_deref()
        .ok_or_else(|| GatewayRuntimeError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "upstream base_url is not configured".to_owned(),
            style: GatewayErrorStyle::OpenAi,
        })?;
    let upstream_path = openai_responses_upstream_path(normalized_path);
    let http_url = build_openai_endpoint_url(base_url, &upstream_path, query);
    openai_http_url_to_ws_url(&http_url)
}

fn openai_http_url_to_ws_url(http_url: &str) -> Result<String, GatewayRuntimeError> {
    let Some((scheme, rest)) = http_url.trim().split_once("://") else {
        return Err(GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: "invalid upstream WebSocket URL".to_owned(),
            style: GatewayErrorStyle::OpenAi,
        });
    };
    let ws_scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" => "wss",
        "http" => "ws",
        "wss" => "wss",
        "ws" => "ws",
        other => {
            return Err(GatewayRuntimeError {
                status: StatusCode::BAD_GATEWAY,
                message: format!("unsupported upstream WebSocket scheme: {other}"),
                style: GatewayErrorStyle::OpenAi,
            });
        }
    };
    Ok(format!("{ws_scheme}://{rest}"))
}

fn build_openai_ws_request_headers(route: &GatewayRoute, client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(api_key) = route
        .account
        .account
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
            headers.insert(AUTHORIZATION, value);
        }
    }
    headers.insert(
        HeaderName::from_static("openai-beta"),
        HeaderValue::from_static(OPENAI_WS_BETA_V2),
    );
    copy_ws_passthrough_header(client_headers, &mut headers, ACCEPT_LANGUAGE);
    copy_ws_passthrough_header(client_headers, &mut headers, USER_AGENT);
    copy_ws_passthrough_header(
        client_headers,
        &mut headers,
        HeaderName::from_static("session_id"),
    );
    copy_ws_passthrough_header(
        client_headers,
        &mut headers,
        HeaderName::from_static("conversation_id"),
    );
    copy_ws_passthrough_header(
        client_headers,
        &mut headers,
        HeaderName::from_static(OPENAI_WS_TURN_STATE_HEADER),
    );
    copy_ws_passthrough_header(
        client_headers,
        &mut headers,
        HeaderName::from_static(OPENAI_WS_TURN_METADATA_HEADER),
    );
    headers
}

fn copy_ws_passthrough_header(source: &HeaderMap, target: &mut HeaderMap, name: HeaderName) {
    if let Some(value) = source.get(&name).cloned() {
        target.insert(name, value);
    }
}

fn build_tungstenite_ws_request(
    upstream_url: &str,
    headers: &HeaderMap,
) -> Result<Request<()>, GatewayRuntimeError> {
    let mut request = upstream_url
        .into_client_request()
        .map_err(|error| GatewayRuntimeError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("invalid upstream WebSocket request: {error}"),
            style: GatewayErrorStyle::OpenAi,
        })?;
    for (name, value) in headers {
        request.headers_mut().insert(name.clone(), value.clone());
    }
    Ok(request)
}

fn axum_ws_to_tungstenite(message: AxumWsMessage) -> Option<TungsteniteMessage> {
    match message {
        AxumWsMessage::Text(text) => Some(TungsteniteMessage::Text(text)),
        AxumWsMessage::Binary(bytes) => Some(TungsteniteMessage::Binary(bytes)),
        AxumWsMessage::Ping(bytes) => Some(TungsteniteMessage::Ping(bytes)),
        AxumWsMessage::Pong(bytes) => Some(TungsteniteMessage::Pong(bytes)),
        AxumWsMessage::Close(_) => Some(TungsteniteMessage::Close(None)),
    }
}

fn tungstenite_to_axum_ws(message: TungsteniteMessage) -> Option<AxumWsMessage> {
    match message {
        TungsteniteMessage::Text(text) => Some(AxumWsMessage::Text(text)),
        TungsteniteMessage::Binary(bytes) => Some(AxumWsMessage::Binary(bytes)),
        TungsteniteMessage::Ping(bytes) => Some(AxumWsMessage::Ping(bytes)),
        TungsteniteMessage::Pong(bytes) => Some(AxumWsMessage::Pong(bytes)),
        TungsteniteMessage::Close(_) => Some(AxumWsMessage::Close(None)),
        TungsteniteMessage::Frame(_) => None,
    }
}

fn build_gemini_endpoint_url(base: &str, endpoint: &str, query: Option<&str>) -> String {
    let normalized = base.trim().trim_end_matches('/');
    let endpoint = format!("/{}", endpoint.trim().trim_start_matches('/'));
    let relative = endpoint
        .strip_prefix("/v1beta")
        .unwrap_or(endpoint.as_str());
    let mut url = if normalized.ends_with(&endpoint) || normalized.ends_with(relative) {
        normalized.to_owned()
    } else if normalized.ends_with("/v1beta") {
        format!("{normalized}{relative}")
    } else {
        format!("{normalized}{endpoint}")
    };
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn openai_base_url_has_version_suffix(raw: &str) -> bool {
    let path = match raw.split_once("://") {
        Some((_, rest)) => rest
            .split_once('/')
            .map(|(_, path)| format!("/{path}"))
            .unwrap_or_default(),
        None => raw
            .split_once('/')
            .map(|(_, path)| format!("/{path}"))
            .unwrap_or_default(),
    };
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return false;
    }
    let segment = path.rsplit('/').next().unwrap_or(path);
    is_openai_api_version_segment(segment)
}

fn is_openai_api_version_segment(segment: &str) -> bool {
    let segment = segment.trim().to_ascii_lowercase();
    let bytes = segment.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'v' || !bytes[1].is_ascii_digit() {
        return false;
    }
    let mut index = 1;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == bytes.len() {
        return true;
    }
    if bytes[index] == b'.' {
        index += 1;
        if index == bytes.len() || !bytes[index].is_ascii_digit() {
            return false;
        }
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        return index == bytes.len();
    }
    let suffix = &segment[index..];
    suffix.starts_with("alpha") || suffix.starts_with("beta") || suffix.starts_with("preview")
}

fn json_response(value: Value) -> Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        axum::Json(value),
    )
        .into_response()
}

fn gateway_error(error: GatewayRuntimeError) -> Response {
    match error.style {
        GatewayErrorStyle::OpenAi => openai_error(error.status, &error.message),
        GatewayErrorStyle::Anthropic => anthropic_error(error.status, &error.message),
        GatewayErrorStyle::Google => google_error(error.status, &error.message),
    }
}

fn attach_attempt_guard_to_response(
    response: Response,
    attempt_guard: AccountAttemptGuard,
) -> Response {
    let (parts, body) = response.into_parts();
    let guarded_stream = stream::unfold(
        (body.into_data_stream(), attempt_guard),
        |(mut data_stream, attempt_guard)| async move {
            data_stream
                .next()
                .await
                .map(|item| (item, (data_stream, attempt_guard)))
        },
    );
    Response::from_parts(parts, Body::from_stream(guarded_stream))
}

fn gateway_quota_error(
    kind: GatewayEndpointKind,
    message: &str,
    retry_after_seconds: u64,
    reset_at: &str,
) -> Response {
    let mut response = gateway_error(GatewayRuntimeError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message: message.to_owned(),
        style: error_style_for_kind(kind),
    });
    if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    if let Ok(value) = HeaderValue::from_str(reset_at) {
        response
            .headers_mut()
            .insert("x-quota-window-resets-at", value);
    }
    response
}

fn openai_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        axum::Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "param": null,
                "code": null
            }
        })),
    )
        .into_response()
}

fn anthropic_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        axum::Json(json!({
            "type": "error",
            "error": {
                "type": anthropic_error_type(status),
                "message": message
            }
        })),
    )
        .into_response()
}

fn anthropic_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        _ if status.is_server_error() => "api_error",
        _ => "invalid_request_error",
    }
}

fn google_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        axum::Json(json!({
            "error": {
                "code": status.as_u16(),
                "message": message,
                "status": google_status(status)
            }
        })),
    )
        .into_response()
}

fn google_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        _ if status.is_server_error() => "INTERNAL",
        _ => "UNKNOWN",
    }
}

fn openai_models_response() -> Value {
    json!({
        "object": "list",
        "data": [
            { "id": "gpt-5.4", "object": "model", "created": 0, "owned_by": "backend_next" },
            { "id": "deepseek-v4-pro", "object": "model", "created": 0, "owned_by": "deepseek" },
            { "id": "claude-sonnet-4-6", "object": "model", "created": 0, "owned_by": "anthropic" },
            { "id": "gemini-3-pro", "object": "model", "created": 0, "owned_by": "google" }
        ]
    })
}

async fn openai_usage_response(
    state: &AppState,
    identity: &GatewayApiKeyIdentity,
    route: &GatewayRoute,
) -> Result<Value, GatewayRuntimeError> {
    let snapshot = api_key_usage_view(state, identity).await?;
    let usage_summary = state.admin_portal.api_key_usage_summary(identity.id);
    let mut response = json!({
        "object": "usage",
        "user_id": identity.user_id,
        "api_key_id": identity.id,
        "api_key_name": identity.name,
        "group_id": identity.group_id,
        "account_id": route.account.account.id.0,
        "provider": route_provider(route),
        "downstream_protocol": route.downstream.as_str(),
        "upstream_protocol": route_upstream_protocol(route),
        "usage": usage_summary.get("usage").cloned().unwrap_or_else(usage_summary_zero),
        "daily_usage": usage_summary.get("daily_usage").cloned().unwrap_or_else(|| json!([])),
        "model_stats": usage_summary.get("model_stats").cloned().unwrap_or_else(|| json!([]))
    });

    let Some(snapshot) = snapshot else {
        response["mode"] = json!("unrestricted");
        response["isValid"] = json!(true);
        response["status"] = json!("active");
        response["planName"] = json!("Wallet Balance");
        response["remaining"] = json!(0.0);
        response["unit"] = json!("USD");
        response["balance"] = json!(0.0);
        return Ok(response);
    };

    response["status"] = json!(snapshot.status);
    response["isValid"] = json!(
        snapshot.status == "active"
            || snapshot.status == "quota_exhausted"
            || snapshot.status == "expired"
    );
    response["api_key_name"] = json!(snapshot.name);
    response["api_key_user_id"] = json!(snapshot.user_id);
    response["api_key_group_id"] = json!(snapshot.group_id);

    let has_quota = snapshot.quota > 0.0;
    let rate_limits = api_key_rate_limits(&snapshot);
    if has_quota || !rate_limits.is_empty() {
        response["mode"] = json!("quota_limited");
        response["unit"] = json!("USD");
        if has_quota {
            let remaining = (snapshot.quota - snapshot.quota_used).max(0.0);
            response["quota"] = json!({
                "limit": snapshot.quota,
                "used": snapshot.quota_used,
                "remaining": remaining,
                "unit": "USD"
            });
            response["remaining"] = json!(remaining);
        }
        if !rate_limits.is_empty() {
            response["rate_limits"] = json!(rate_limits);
        }
    } else {
        response["mode"] = json!("unrestricted");
        response["planName"] = json!("Wallet Balance");
        response["remaining"] = json!(0.0);
        response["unit"] = json!("USD");
        response["balance"] = json!(0.0);
    }
    Ok(response)
}

fn usage_summary_zero() -> Value {
    json!({
        "today": {
            "requests": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_read_tokens": 0,
            "total_tokens": 0,
            "cost": 0.0,
            "actual_cost": 0.0
        },
        "total": {
            "requests": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_read_tokens": 0,
            "total_tokens": 0,
            "cost": 0.0,
            "actual_cost": 0.0
        },
        "average_duration_ms": 0,
        "rpm": 0,
        "tpm": 0
    })
}

fn api_key_rate_limits(snapshot: &ApiKeyUsageView) -> Vec<Value> {
    [
        (
            "5h",
            snapshot.rate_limit_5h,
            snapshot.usage_5h,
            snapshot.window_5h_start,
            5 * 60 * 60,
        ),
        (
            "1d",
            snapshot.rate_limit_1d,
            snapshot.usage_1d,
            snapshot.window_1d_start,
            24 * 60 * 60,
        ),
        (
            "7d",
            snapshot.rate_limit_7d,
            snapshot.usage_7d,
            snapshot.window_7d_start,
            7 * 24 * 60 * 60,
        ),
    ]
    .into_iter()
    .filter(|(_, limit, _, _, _)| *limit > 0.0)
    .map(|(window, limit, used, window_start, window_seconds)| {
        let remaining = (limit - used).max(0.0);
        json!({
            "window": window,
            "limit": limit,
            "used": used,
            "remaining": remaining,
            "window_start": window_start,
            "reset_at": window_start.map(|start| start + window_seconds)
        })
    })
    .collect()
}

fn gemini_models_response(path: &str) -> Value {
    if let Some(model) = path.rsplit('/').next().filter(|value| *value != "models") {
        return json!({
            "name": model,
            "version": "001",
            "displayName": model,
            "supportedGenerationMethods": ["generateContent", "streamGenerateContent"]
        });
    }
    json!({
        "models": [
            {
                "name": "models/gemini-3-pro",
                "version": "001",
                "displayName": "Gemini 3 Pro",
                "supportedGenerationMethods": ["generateContent", "streamGenerateContent"]
            }
        ]
    })
}

fn route_provider(route: &GatewayRoute) -> &'static str {
    match route.account.account.provider {
        domain::Provider::OpenAi => "openai",
        domain::Provider::DeepSeek => "deepseek",
        domain::Provider::Anthropic => "anthropic",
        domain::Provider::Gemini => "gemini",
        domain::Provider::Vertex => "vertex",
        domain::Provider::Antigravity => "antigravity",
    }
}

fn route_quota_platform(route: &GatewayRoute) -> &'static str {
    match route.account.account.provider {
        domain::Provider::OpenAi | domain::Provider::DeepSeek => "openai",
        domain::Provider::Anthropic => "anthropic",
        domain::Provider::Gemini | domain::Provider::Vertex => "gemini",
        domain::Provider::Antigravity => "antigravity",
    }
}

fn route_upstream_protocol(route: &GatewayRoute) -> &'static str {
    match route.account.upstream_protocol() {
        domain::UpstreamProtocol::OpenAiResponses => "openai_responses",
        domain::UpstreamProtocol::OpenAiChatCompletions => "openai_chat_completions",
        domain::UpstreamProtocol::AnthropicMessages => "anthropic_messages",
        domain::UpstreamProtocol::GeminiGenerateContent => "gemini_generate_content",
    }
}
