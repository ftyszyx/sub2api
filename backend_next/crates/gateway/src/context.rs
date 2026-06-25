use domain::{AccountId, ApiKeyId, GroupId, Provider, UpstreamProtocol};
use protocol::DownstreamProtocol;

#[derive(Debug, Clone)]
pub struct GatewayContext {
    pub request_id: String,
    pub user_id: i64,
    pub api_key_id: ApiKeyId,
    pub group_id: Option<GroupId>,
    pub account_id: Option<AccountId>,
    pub downstream_protocol: DownstreamProtocol,
    pub upstream_protocol: Option<UpstreamProtocol>,
    pub provider: Option<Provider>,
    pub original_model: Option<String>,
    pub upstream_model: Option<String>,
    pub stream: bool,
    pub previous_response_id: Option<String>,
}
