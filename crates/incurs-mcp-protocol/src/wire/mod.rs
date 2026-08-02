//! Wire-era codecs.

mod legacy;
mod modern;

use serde_json::Value;

use crate::core::{McpClientMetadata, McpVersion};
use crate::standards::known_standard;

/// Shared wire codec family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpWireCodecKind {
    /// MCP standards through 2025-11-25.
    Legacy,
    /// MCP 2026-07-28 and later stateless standards.
    Modern,
}

/// Validation result that distinguishes unsupported standard members from
/// malformed messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum McpValidation {
    /// The value is valid for the selected standard.
    Valid,
    /// The value is well formed but the method does not exist in the standard.
    NotInStandard,
    /// The value is malformed.
    Invalid {
        /// Human-readable validation failure.
        message: String,
    },
}

/// Function-only MCP wire projection boundary.
pub trait McpWireCodec: private::Sealed + Send + Sync {
    /// Returns the codec family.
    fn kind(&self) -> McpWireCodecKind;

    /// Validates a JSON-RPC request against one exact standard.
    fn validate_request(&self, standard: &McpVersion, value: &Value) -> McpValidation;

    /// Adds standard-required outbound metadata without exposing it to the
    /// application invocation boundary.
    fn project_outbound_request(
        &self,
        standard: &McpVersion,
        client: &McpClientMetadata,
        value: Value,
    ) -> Value;

    /// Removes wire-only metadata from an inbound request.
    fn canonicalize_inbound_request(&self, value: Value) -> Value;
}

/// Returns the shared codec for a codec family.
pub fn codec(kind: McpWireCodecKind) -> &'static dyn McpWireCodec {
    match kind {
        McpWireCodecKind::Legacy => &legacy::LEGACY_CODEC,
        McpWireCodecKind::Modern => &modern::MODERN_CODEC,
    }
}

fn validate_request(standard: &McpVersion, value: &Value) -> McpValidation {
    let Some(object) = value.as_object() else {
        return McpValidation::Invalid {
            message: "request must be a JSON object".to_string(),
        };
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return McpValidation::Invalid {
            message: "jsonrpc must be \"2.0\"".to_string(),
        };
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return McpValidation::Invalid {
            message: "method must be a string".to_string(),
        };
    };
    let Some(profile) = known_standard(standard) else {
        return McpValidation::Invalid {
            message: format!("unsupported MCP standard {standard}"),
        };
    };
    if profile.request_methods().contains(&method) {
        McpValidation::Valid
    } else {
        McpValidation::NotInStandard
    }
}

fn remove_reserved_meta(mut value: Value) -> Value {
    if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut)
        && let Some(meta) = params.get_mut("_meta").and_then(Value::as_object_mut)
    {
        meta.remove("io.modelcontextprotocol/protocolVersion");
        meta.remove("io.modelcontextprotocol/clientInfo");
        meta.remove("io.modelcontextprotocol/clientCapabilities");
        if meta.is_empty() {
            params.remove("_meta");
        }
    }
    value
}

mod private {
    pub trait Sealed {}

    impl Sealed for super::legacy::LegacyCodec {}
    impl Sealed for super::modern::ModernCodec {}
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reports_method_absence_separately_from_invalid_json_rpc() {
        let version = McpVersion::from("2025-11-25");
        let codec = codec(McpWireCodecKind::Legacy);

        assert_eq!(
            codec.validate_request(
                &version,
                &json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"})
            ),
            McpValidation::NotInStandard
        );
        assert!(matches!(
            codec.validate_request(
                &version,
                &json!({"jsonrpc": "1.0", "id": 1, "method": "tools/list"})
            ),
            McpValidation::Invalid { .. }
        ));
    }

    #[test]
    fn modern_codec_lifts_protocol_version_into_reserved_meta() {
        let version = McpVersion::from("2026-07-28");
        let value = codec(McpWireCodecKind::Modern).project_outbound_request(
            &version,
            &McpClientMetadata::new("test", "1.0.0", json!({"roots": {}})),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
        );

        assert_eq!(
            value.pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion"),
            Some(&json!("2026-07-28"))
        );
        assert_eq!(
            value.pointer("/params/_meta/io.modelcontextprotocol~1clientInfo"),
            Some(&json!({"name": "test", "version": "1.0.0"}))
        );
        assert_eq!(
            codec(McpWireCodecKind::Modern).canonicalize_inbound_request(value),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
        );
    }
}
