use crate::group::GroupId;
use protocol::DownstreamProtocol;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenAi,
    DeepSeek,
    Anthropic,
    Gemini,
    Vertex,
    Antigravity,
}

impl Provider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Vertex => "vertex",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub name: String,
    pub provider: Provider,
    pub default_upstream_protocol: UpstreamProtocol,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model_mapping: Vec<ModelMappingRule>,
    pub extra: Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMappingRule {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMappingResolution {
    pub requested_model: String,
    pub upstream_model: String,
    pub intermediate_models: Vec<String>,
    pub matched: bool,
    pub matched_source: Option<String>,
}

impl Account {
    pub fn resolve_mapped_model(&self, requested_model: &str) -> ModelMappingResolution {
        resolve_model_mapping(&self.model_mapping, requested_model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountGroupBinding {
    pub account: Account,
    pub group_id: GroupId,
    pub supported_downstream_protocols: Vec<DownstreamProtocol>,
    pub upstream_protocol_override: Option<UpstreamProtocol>,
    pub priority: i32,
}

impl AccountGroupBinding {
    pub fn upstream_protocol(&self) -> UpstreamProtocol {
        self.upstream_protocol_override
            .unwrap_or(self.account.default_upstream_protocol)
    }

    pub fn supports(&self, protocol: DownstreamProtocol) -> bool {
        self.account.enabled
            && (self.supported_downstream_protocols.is_empty()
                || self.supported_downstream_protocols.contains(&protocol))
    }
}

pub fn resolve_model_mapping(
    rules: &[ModelMappingRule],
    requested_model: &str,
) -> ModelMappingResolution {
    let requested_model = requested_model.trim().to_owned();
    if requested_model.is_empty() {
        return ModelMappingResolution {
            requested_model,
            upstream_model: String::new(),
            intermediate_models: Vec::new(),
            matched: false,
            matched_source: None,
        };
    }

    if let Some(rule) = rules.iter().find(|rule| rule.source == requested_model) {
        return ModelMappingResolution {
            requested_model,
            upstream_model: rule.target.clone(),
            intermediate_models: Vec::new(),
            matched: true,
            matched_source: Some(rule.source.clone()),
        };
    }

    let wildcard = rules
        .iter()
        .filter(|rule| {
            rule.source.contains('*') && wildcard_matches(&rule.source, &requested_model)
        })
        .max_by_key(|rule| rule.source.len());
    if let Some(rule) = wildcard {
        return ModelMappingResolution {
            requested_model,
            upstream_model: rule.target.clone(),
            intermediate_models: Vec::new(),
            matched: true,
            matched_source: Some(rule.source.clone()),
        };
    }

    ModelMappingResolution {
        upstream_model: requested_model.clone(),
        requested_model,
        intermediate_models: Vec::new(),
        matched: false,
        matched_source: None,
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return value.starts_with(prefix) && value.ends_with(suffix);
    }
    pattern == value
}

pub fn select_account_for_downstream(
    candidates: &[AccountGroupBinding],
    downstream: DownstreamProtocol,
) -> Result<AccountGroupBinding, AccountSelectionError> {
    candidates
        .iter()
        .filter(|candidate| candidate.supports(downstream))
        .min_by_key(|candidate| candidate.priority)
        .cloned()
        .ok_or(AccountSelectionError::NoAccountSupportsProtocol(downstream))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccountSelectionError {
    #[error("no account in group supports downstream protocol {0}")]
    NoAccountSupportsProtocol(DownstreamProtocol),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(id: i64, priority: i32, supported: Vec<DownstreamProtocol>) -> AccountGroupBinding {
        AccountGroupBinding {
            account: Account {
                id: AccountId(id),
                name: format!("account-{id}"),
                provider: Provider::DeepSeek,
                default_upstream_protocol: UpstreamProtocol::OpenAiChatCompletions,
                base_url: None,
                api_key: None,
                model_mapping: Vec::new(),
                extra: Value::Null,
                enabled: true,
            },
            group_id: GroupId(1),
            supported_downstream_protocols: supported,
            upstream_protocol_override: None,
            priority,
        }
    }

    #[test]
    fn selects_lowest_priority_account_supporting_downstream_protocol() {
        let candidates = vec![
            binding(1, 50, vec![DownstreamProtocol::OpenAiChatCompletions]),
            binding(2, 10, vec![DownstreamProtocol::OpenAiResponses]),
            binding(3, 20, vec![DownstreamProtocol::OpenAiResponses]),
        ];

        let selected =
            select_account_for_downstream(&candidates, DownstreamProtocol::OpenAiResponses)
                .unwrap();

        assert_eq!(selected.account.id, AccountId(2));
    }

    #[test]
    fn fails_when_no_account_supports_protocol() {
        let candidates = vec![binding(
            1,
            10,
            vec![DownstreamProtocol::OpenAiChatCompletions],
        )];

        let error =
            select_account_for_downstream(&candidates, DownstreamProtocol::AnthropicMessages)
                .unwrap_err();

        assert_eq!(
            error,
            AccountSelectionError::NoAccountSupportsProtocol(DownstreamProtocol::AnthropicMessages)
        );
    }

    #[test]
    fn empty_supported_protocols_means_unrestricted_for_legacy_bindings() {
        let candidates = vec![binding(1, 10, vec![])];

        let selected =
            select_account_for_downstream(&candidates, DownstreamProtocol::OpenAiResponses)
                .unwrap();

        assert_eq!(selected.account.id, AccountId(1));
    }

    #[test]
    fn resolves_model_mapping_exact_before_wildcard() {
        let rules = vec![
            ModelMappingRule {
                source: "gpt-*".to_owned(),
                target: "wildcard-target".to_owned(),
            },
            ModelMappingRule {
                source: "gpt-5.4".to_owned(),
                target: "exact-target".to_owned(),
            },
        ];

        let resolved = resolve_model_mapping(&rules, "gpt-5.4");

        assert!(resolved.matched);
        assert_eq!(resolved.upstream_model, "exact-target");
        assert_eq!(resolved.matched_source.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn resolves_model_mapping_longest_wildcard() {
        let rules = vec![
            ModelMappingRule {
                source: "gpt-*".to_owned(),
                target: "generic".to_owned(),
            },
            ModelMappingRule {
                source: "gpt-5.*".to_owned(),
                target: "gpt5-family".to_owned(),
            },
        ];

        let resolved = resolve_model_mapping(&rules, "gpt-5.4");

        assert!(resolved.matched);
        assert_eq!(resolved.upstream_model, "gpt5-family");
        assert_eq!(resolved.matched_source.as_deref(), Some("gpt-5.*"));
    }
}
