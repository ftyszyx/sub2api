use protocol::{DownstreamProtocol, ProtocolParseError};

pub fn resolve_downstream_protocol(path: &str) -> Result<DownstreamProtocol, ProtocolParseError> {
    DownstreamProtocol::from_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_codex_alias_to_responses() {
        assert_eq!(
            resolve_downstream_protocol("/backend-api/codex/responses").unwrap(),
            DownstreamProtocol::OpenAiResponses
        );
    }
}
