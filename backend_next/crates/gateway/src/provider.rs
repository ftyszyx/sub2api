use crate::bridge::{UpstreamRequest, UpstreamResponse};
use crate::context::GatewayContext;
use async_trait::async_trait;
use domain::{Provider, UpstreamProtocol};

#[async_trait]
pub trait ProviderClient: Send + Sync {
    fn provider(&self) -> Provider;
    fn protocols(&self) -> &'static [UpstreamProtocol];

    async fn call(
        &self,
        ctx: &GatewayContext,
        request: UpstreamRequest,
    ) -> anyhow::Result<UpstreamResponse>;
}
