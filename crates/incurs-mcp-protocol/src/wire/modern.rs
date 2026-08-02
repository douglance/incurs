use serde_json::{Map, Value};

use crate::core::{McpClientMetadata, McpVersion};

use super::{McpValidation, McpWireCodec, McpWireCodecKind};

pub(super) static MODERN_CODEC: ModernCodec = ModernCodec;

pub(super) struct ModernCodec;

impl McpWireCodec for ModernCodec {
    fn kind(&self) -> McpWireCodecKind {
        McpWireCodecKind::Modern
    }

    fn validate_request(&self, standard: &McpVersion, value: &Value) -> McpValidation {
        let validation = super::validate_request(standard, value);
        if validation != McpValidation::Valid {
            return validation;
        }
        if value
            .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
            .and_then(Value::as_str)
            != Some(standard.as_str())
        {
            return McpValidation::Invalid {
                message: "request _meta is missing the selected protocol version".to_string(),
            };
        }
        if !value
            .pointer("/params/_meta/io.modelcontextprotocol~1clientCapabilities")
            .is_some_and(Value::is_object)
        {
            return McpValidation::Invalid {
                message: "request _meta is missing client capabilities".to_string(),
            };
        }
        McpValidation::Valid
    }

    fn project_outbound_request(
        &self,
        standard: &McpVersion,
        client: &McpClientMetadata,
        mut value: Value,
    ) -> Value {
        let Some(object) = value.as_object_mut() else {
            return value;
        };
        let params = object
            .entry("params")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(params) = params.as_object_mut() else {
            return value;
        };
        let meta = params
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(
                "io.modelcontextprotocol/protocolVersion".to_string(),
                Value::String(standard.as_str().to_string()),
            );
            meta.insert(
                "io.modelcontextprotocol/clientInfo".to_string(),
                serde_json::json!({
                    "name": client.name,
                    "version": client.version,
                }),
            );
            meta.insert(
                "io.modelcontextprotocol/clientCapabilities".to_string(),
                client.capabilities.clone(),
            );
        }
        value
    }

    fn canonicalize_inbound_request(&self, value: Value) -> Value {
        super::remove_reserved_meta(value)
    }
}
