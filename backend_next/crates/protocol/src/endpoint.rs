use crate::{DownstreamProtocol, ProtocolParseError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayEndpoint {
    pub kind: GatewayEndpointKind,
    pub downstream_protocol: Option<DownstreamProtocol>,
    pub normalized_path: String,
}

impl GatewayEndpoint {
    pub fn from_path(path: &str) -> Result<Self, GatewayEndpointParseError> {
        Self::from_method_path("POST", path)
    }

    pub fn from_method_path(method: &str, path: &str) -> Result<Self, GatewayEndpointParseError> {
        let normalized_path = normalize_path(path);
        let kind = GatewayEndpointKind::from_method_and_normalized_path(method, &normalized_path)
            .ok_or_else(|| {
            GatewayEndpointParseError::UnsupportedPath(path.trim().to_owned())
        })?;
        let downstream_protocol = kind.downstream_protocol();

        Ok(Self {
            kind,
            downstream_protocol,
            normalized_path,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayEndpointKind {
    OpenAiResponses,
    OpenAiResponsesWebSocket,
    OpenAiChatCompletions,
    AnthropicMessages,
    AnthropicCountTokens,
    OpenAiEmbeddings,
    OpenAiImageGenerations,
    OpenAiImageEdits,
    OpenAiModels,
    OpenAiUsage,
    GeminiModels,
    GeminiGenerateContent,
    AntigravityMessages,
    AntigravityCountTokens,
    AntigravityModels,
    AntigravityUsage,
    AntigravityGeminiModels,
    AntigravityGeminiGenerateContent,
}

impl GatewayEndpointKind {
    fn from_method_and_normalized_path(method: &str, path: &str) -> Option<Self> {
        if method.eq_ignore_ascii_case("GET") && is_openai_responses_path(path) {
            return Some(Self::OpenAiResponsesWebSocket);
        }
        Self::from_normalized_path(path)
    }

    fn from_normalized_path(path: &str) -> Option<Self> {
        match path {
            p if is_openai_responses_path(p) => Some(Self::OpenAiResponses),
            "/v1/chat/completions" | "/chat/completions" => Some(Self::OpenAiChatCompletions),
            "/v1/messages" => Some(Self::AnthropicMessages),
            "/v1/messages/count_tokens" => Some(Self::AnthropicCountTokens),
            "/v1/embeddings" | "/embeddings" => Some(Self::OpenAiEmbeddings),
            "/v1/images/generations" | "/images/generations" => Some(Self::OpenAiImageGenerations),
            "/v1/images/edits" | "/images/edits" => Some(Self::OpenAiImageEdits),
            "/v1/models" => Some(Self::OpenAiModels),
            "/v1/usage" => Some(Self::OpenAiUsage),
            "/v1beta/models" => Some(Self::GeminiModels),
            p if is_gemini_generate_content_path(p, "/v1beta/models") => {
                Some(Self::GeminiGenerateContent)
            }
            p if has_prefix_segment(p, "/v1beta/models") => Some(Self::GeminiModels),
            "/antigravity/v1/messages" => Some(Self::AntigravityMessages),
            "/antigravity/v1/messages/count_tokens" => Some(Self::AntigravityCountTokens),
            "/antigravity/v1/models" | "/antigravity/models" => Some(Self::AntigravityModels),
            "/antigravity/v1/usage" => Some(Self::AntigravityUsage),
            "/antigravity/v1beta/models" => Some(Self::AntigravityGeminiModels),
            p if is_gemini_generate_content_path(p, "/antigravity/v1beta/models") => {
                Some(Self::AntigravityGeminiGenerateContent)
            }
            p if has_prefix_segment(p, "/antigravity/v1beta/models") => {
                Some(Self::AntigravityGeminiModels)
            }
            _ => None,
        }
    }

    pub const fn downstream_protocol(self) -> Option<DownstreamProtocol> {
        match self {
            Self::OpenAiResponses | Self::OpenAiResponsesWebSocket => {
                Some(DownstreamProtocol::OpenAiResponses)
            }
            Self::OpenAiChatCompletions => Some(DownstreamProtocol::OpenAiChatCompletions),
            Self::AnthropicMessages | Self::AntigravityMessages => {
                Some(DownstreamProtocol::AnthropicMessages)
            }
            Self::OpenAiEmbeddings => Some(DownstreamProtocol::OpenAiEmbeddings),
            Self::OpenAiImageGenerations | Self::OpenAiImageEdits => {
                Some(DownstreamProtocol::OpenAiImages)
            }
            Self::GeminiGenerateContent | Self::AntigravityGeminiGenerateContent => {
                Some(DownstreamProtocol::GeminiGenerateContent)
            }
            Self::AnthropicCountTokens
            | Self::OpenAiModels
            | Self::OpenAiUsage
            | Self::GeminiModels
            | Self::AntigravityCountTokens
            | Self::AntigravityModels
            | Self::AntigravityUsage
            | Self::AntigravityGeminiModels => None,
        }
    }
}

impl TryFrom<&str> for GatewayEndpoint {
    type Error = GatewayEndpointParseError;

    fn try_from(path: &str) -> Result<Self, Self::Error> {
        Self::from_path(path)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GatewayEndpointParseError {
    #[error("unsupported gateway path: {0}")]
    UnsupportedPath(String),
    #[error(transparent)]
    Protocol(#[from] ProtocolParseError),
}

fn normalize_path(path: &str) -> String {
    let mut path = path.trim().to_ascii_lowercase();
    if let Some(index) = path.find('?') {
        path.truncate(index);
    }
    path.trim_end_matches('/').to_owned()
}

fn has_prefix_segment(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn is_gemini_generate_content_path(path: &str, prefix: &str) -> bool {
    has_prefix_segment(path, prefix)
        && (path.ends_with(":generatecontent")
            || path.ends_with(":streamgeneratecontent")
            || path.ends_with(":counttokens"))
}

fn is_openai_responses_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/responses" | "/responses" | "/backend-api/codex/responses"
    ) || has_prefix_segment(path, "/v1/responses")
        || has_prefix_segment(path, "/responses")
        || has_prefix_segment(path, "/backend-api/codex/responses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_openai_responses_aliases_and_subpaths() {
        for path in [
            "/v1/responses",
            "/responses",
            "/backend-api/codex/responses",
            "/backend-api/codex/responses/compact",
        ] {
            let endpoint = GatewayEndpoint::from_path(path).unwrap();
            assert_eq!(endpoint.kind, GatewayEndpointKind::OpenAiResponses);
            assert_eq!(
                endpoint.downstream_protocol,
                Some(DownstreamProtocol::OpenAiResponses)
            );
        }
    }

    #[test]
    fn maps_get_openai_responses_aliases_to_websocket() {
        for path in [
            "/v1/responses",
            "/responses",
            "/backend-api/codex/responses",
            "/backend-api/codex/responses/compact",
        ] {
            let endpoint = GatewayEndpoint::from_method_path("GET", path).unwrap();
            assert_eq!(endpoint.kind, GatewayEndpointKind::OpenAiResponsesWebSocket);
            assert_eq!(
                endpoint.downstream_protocol,
                Some(DownstreamProtocol::OpenAiResponses)
            );
        }
    }

    #[test]
    fn maps_core_protocol_paths() {
        let cases = [
            (
                "/v1/chat/completions",
                GatewayEndpointKind::OpenAiChatCompletions,
                Some(DownstreamProtocol::OpenAiChatCompletions),
            ),
            (
                "/v1/messages",
                GatewayEndpointKind::AnthropicMessages,
                Some(DownstreamProtocol::AnthropicMessages),
            ),
            (
                "/v1beta/models/gemini-pro:streamGenerateContent",
                GatewayEndpointKind::GeminiGenerateContent,
                Some(DownstreamProtocol::GeminiGenerateContent),
            ),
            (
                "/v1beta/models/gemini-pro:countTokens",
                GatewayEndpointKind::GeminiGenerateContent,
                Some(DownstreamProtocol::GeminiGenerateContent),
            ),
            (
                "/v1beta/models/gemini-pro",
                GatewayEndpointKind::GeminiModels,
                None,
            ),
            (
                "/v1/images/generations",
                GatewayEndpointKind::OpenAiImageGenerations,
                Some(DownstreamProtocol::OpenAiImages),
            ),
        ];

        for (path, kind, protocol) in cases {
            let endpoint = GatewayEndpoint::from_path(path).unwrap();
            assert_eq!(endpoint.kind, kind);
            assert_eq!(endpoint.downstream_protocol, protocol);
        }
    }

    #[test]
    fn maps_local_non_forwarding_paths_without_downstream_protocol() {
        for path in ["/v1/models", "/v1/usage", "/antigravity/models"] {
            let endpoint = GatewayEndpoint::from_path(path).unwrap();
            assert_eq!(endpoint.downstream_protocol, None);
        }
    }

    #[test]
    fn maps_antigravity_paths() {
        let cases = [
            (
                "/antigravity/v1/messages",
                GatewayEndpointKind::AntigravityMessages,
                Some(DownstreamProtocol::AnthropicMessages),
            ),
            (
                "/antigravity/v1beta/models/gemini-pro:generateContent",
                GatewayEndpointKind::AntigravityGeminiGenerateContent,
                Some(DownstreamProtocol::GeminiGenerateContent),
            ),
            (
                "/antigravity/v1beta/models/gemini-pro",
                GatewayEndpointKind::AntigravityGeminiModels,
                None,
            ),
        ];

        for (path, kind, protocol) in cases {
            let endpoint = GatewayEndpoint::from_path(path).unwrap();
            assert_eq!(endpoint.kind, kind);
            assert_eq!(endpoint.downstream_protocol, protocol);
        }
    }
}
