use crate::bridge::{BridgeAdapter, DownstreamResponse};
use crate::context::GatewayContext;
use crate::provider::ProviderClient;
use bytes::Bytes;

pub async fn forward_once(
    ctx: &GatewayContext,
    bridge: &dyn BridgeAdapter,
    provider: &dyn ProviderClient,
    body: Bytes,
) -> anyhow::Result<DownstreamResponse> {
    let upstream_request = bridge.convert_request(ctx, body).await?;
    let upstream_response = provider.call(ctx, upstream_request).await?;
    bridge.convert_response(ctx, upstream_response).await
}
