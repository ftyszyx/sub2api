use crate::context::GatewayContext;
use async_trait::async_trait;
use bytes::Bytes;
use domain::UpstreamProtocol;
use protocol::DownstreamProtocol;

#[derive(Debug, Clone)]
pub struct UpstreamRequest {
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct UpstreamResponse {
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct DownstreamResponse {
    pub body: Bytes,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait]
pub trait BridgeAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn downstream(&self) -> DownstreamProtocol;
    fn upstream(&self) -> UpstreamProtocol;

    async fn convert_request(
        &self,
        _ctx: &GatewayContext,
        body: Bytes,
    ) -> anyhow::Result<UpstreamRequest> {
        Ok(UpstreamRequest { body })
    }

    async fn convert_response(
        &self,
        _ctx: &GatewayContext,
        response: UpstreamResponse,
    ) -> anyhow::Result<DownstreamResponse> {
        Ok(DownstreamResponse {
            body: response.body,
            usage: None,
        })
    }
}

pub struct PassthroughBridge {
    downstream: DownstreamProtocol,
    upstream: UpstreamProtocol,
}

impl PassthroughBridge {
    pub fn new(downstream: DownstreamProtocol, upstream: UpstreamProtocol) -> Self {
        Self {
            downstream,
            upstream,
        }
    }
}

#[async_trait]
impl BridgeAdapter for PassthroughBridge {
    fn name(&self) -> &'static str {
        "passthrough"
    }

    fn downstream(&self) -> DownstreamProtocol {
        self.downstream
    }

    fn upstream(&self) -> UpstreamProtocol {
        self.upstream
    }
}
