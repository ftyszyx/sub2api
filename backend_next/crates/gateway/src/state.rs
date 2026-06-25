use async_trait::async_trait;
use domain::GroupId;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseState {
    pub response_id: String,
    pub messages_json: String,
}

#[async_trait]
pub trait ResponseStateStore: Send + Sync {
    async fn get(
        &self,
        group_id: GroupId,
        response_id: &str,
    ) -> anyhow::Result<Option<ResponseState>>;

    async fn set(
        &self,
        group_id: GroupId,
        response_id: &str,
        state: ResponseState,
        ttl: Duration,
    ) -> anyhow::Result<()>;

    async fn delete(&self, group_id: GroupId, response_id: &str) -> anyhow::Result<()>;
}
