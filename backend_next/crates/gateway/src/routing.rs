use async_trait::async_trait;
use domain::{
    select_account_for_downstream, AccountGroupBinding, AccountSelectionError, ApiKey, Group,
    GroupId,
};
use thiserror::Error;

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn get_group(&self, group_id: GroupId) -> Result<Option<Group>, RoutingError>;
}

#[async_trait]
pub trait AccountPoolRepository: Send + Sync {
    async fn list_group_accounts(
        &self,
        group_id: GroupId,
    ) -> Result<Vec<AccountGroupBinding>, RoutingError>;
}

pub async fn resolve_api_key_group<R>(repo: &R, api_key: &ApiKey) -> Result<Group, RoutingError>
where
    R: GroupRepository,
{
    let group_id = api_key
        .active_group_id()
        .ok_or(RoutingError::ApiKeyGroupMissing)?;
    let group = repo
        .get_group(group_id)
        .await?
        .ok_or(RoutingError::GroupNotFound(group_id))?;
    if !group.is_active() {
        return Err(RoutingError::GroupUnavailable(group_id));
    }
    Ok(group)
}

pub async fn select_group_account<R>(
    repo: &R,
    group_id: GroupId,
    downstream: protocol::DownstreamProtocol,
) -> Result<AccountGroupBinding, RoutingError>
where
    R: AccountPoolRepository,
{
    let candidates = repo.list_group_accounts(group_id).await?;
    select_account_for_downstream(&candidates, downstream).map_err(RoutingError::AccountSelection)
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("api key is not bound to an active group")]
    ApiKeyGroupMissing,
    #[error("group {0:?} was not found")]
    GroupNotFound(GroupId),
    #[error("group {0:?} is unavailable")]
    GroupUnavailable(GroupId),
    #[error(transparent)]
    AccountSelection(#[from] AccountSelectionError),
    #[error("repository error: {0}")]
    Repository(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ApiKeyId, ApiKeyStatus, GroupStatus};

    struct Groups {
        group: Option<Group>,
    }

    #[async_trait]
    impl GroupRepository for Groups {
        async fn get_group(&self, _group_id: GroupId) -> Result<Option<Group>, RoutingError> {
            Ok(self.group.clone())
        }
    }

    #[tokio::test]
    async fn resolves_active_api_key_group() {
        let api_key = ApiKey {
            id: ApiKeyId(1),
            user_id: 1,
            key: "sk-test".to_owned(),
            name: "test".to_owned(),
            group_id: Some(GroupId(2)),
            status: ApiKeyStatus::Active,
            ..ApiKey::default()
        };
        let repo = Groups {
            group: Some(Group {
                id: GroupId(2),
                name: "group".to_owned(),
                status: GroupStatus::Active,
            }),
        };

        let group = resolve_api_key_group(&repo, &api_key).await.unwrap();

        assert_eq!(group.id, GroupId(2));
    }
}
