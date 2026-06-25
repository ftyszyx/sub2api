use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownstreamProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    OpenAiEmbeddings,
    OpenAiImages,
}

impl DownstreamProtocol {
    pub fn from_path(path: &str) -> Result<Self, ProtocolParseError> {
        let normalized = normalize_path(path);
        match normalized.as_str() {
            p if p.contains("/chat/completions") => Ok(Self::OpenAiChatCompletions),
            p if p.contains("/messages") => Ok(Self::AnthropicMessages),
            p if p.contains("/responses") => Ok(Self::OpenAiResponses),
            p if p.contains("/v1beta/models") || p.contains("/models/") => {
                Ok(Self::GeminiGenerateContent)
            }
            p if p.contains("/embeddings") => Ok(Self::OpenAiEmbeddings),
            p if p.contains("/images/") => Ok(Self::OpenAiImages),
            _ => Err(ProtocolParseError::UnsupportedPath(path.trim().to_owned())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai_responses",
            Self::OpenAiChatCompletions => "openai_chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::OpenAiEmbeddings => "openai_embeddings",
            Self::OpenAiImages => "openai_images",
        }
    }
}

impl fmt::Display for DownstreamProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DownstreamProtocol {
    type Err = ProtocolParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "openai_responses" => Ok(Self::OpenAiResponses),
            "openai_chat_completions" => Ok(Self::OpenAiChatCompletions),
            "anthropic_messages" => Ok(Self::AnthropicMessages),
            "gemini_generate_content" => Ok(Self::GeminiGenerateContent),
            "openai_embeddings" => Ok(Self::OpenAiEmbeddings),
            "openai_images" => Ok(Self::OpenAiImages),
            other => Err(ProtocolParseError::UnsupportedProtocol(other.to_owned())),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolParseError {
    #[error("unsupported downstream path: {0}")]
    UnsupportedPath(String),
    #[error("unsupported downstream protocol: {0}")]
    UnsupportedProtocol(String),
}

fn normalize_path(path: &str) -> String {
    let mut path = path.trim().to_ascii_lowercase();
    if let Some(index) = path.find('?') {
        path.truncate(index);
    }
    path.trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_openai_responses_aliases() {
        for path in [
            "/responses",
            "/v1/responses",
            "/backend-api/codex/responses",
            "/openai/v1/responses/compact",
        ] {
            assert_eq!(
                DownstreamProtocol::from_path(path).unwrap(),
                DownstreamProtocol::OpenAiResponses
            );
        }
    }

    #[test]
    fn maps_core_gateway_paths() {
        let cases = [
            (
                "/v1/chat/completions",
                DownstreamProtocol::OpenAiChatCompletions,
            ),
            ("/v1/messages", DownstreamProtocol::AnthropicMessages),
            (
                "/v1beta/models/gemini:generateContent",
                DownstreamProtocol::GeminiGenerateContent,
            ),
            ("/v1/embeddings", DownstreamProtocol::OpenAiEmbeddings),
            ("/v1/images/generations", DownstreamProtocol::OpenAiImages),
        ];

        for (path, expected) in cases {
            assert_eq!(DownstreamProtocol::from_path(path).unwrap(), expected);
        }
    }
}
