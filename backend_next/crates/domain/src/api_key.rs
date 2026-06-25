use crate::group::GroupId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApiKeyId(pub i64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: ApiKeyId,
    pub user_id: i64,
    pub key: String,
    pub name: String,
    pub group_id: Option<GroupId>,
    pub status: ApiKeyStatus,
    pub quota: f64,
    pub quota_used: f64,
    pub rate_limit_5h: f64,
    pub rate_limit_1d: f64,
    pub rate_limit_7d: f64,
    pub usage_5h: f64,
    pub usage_1d: f64,
    pub usage_7d: f64,
    pub window_5h_start: Option<i64>,
    pub window_1d_start: Option<i64>,
    pub window_7d_start: Option<i64>,
}

impl ApiKey {
    pub fn active_group_id(&self) -> Option<GroupId> {
        if self.status == ApiKeyStatus::Active {
            self.group_id
        } else {
            None
        }
    }
}

impl Default for ApiKey {
    fn default() -> Self {
        Self {
            id: ApiKeyId(0),
            user_id: 0,
            key: String::new(),
            name: String::new(),
            group_id: None,
            status: ApiKeyStatus::Active,
            quota: 0.0,
            quota_used: 0.0,
            rate_limit_5h: 0.0,
            rate_limit_1d: 0.0,
            rate_limit_7d: 0.0,
            usage_5h: 0.0,
            usage_1d: 0.0,
            usage_7d: 0.0,
            window_5h_start: None,
            window_1d_start: None,
            window_7d_start: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyStatus {
    Active,
    Disabled,
    QuotaExhausted,
    Expired,
}
