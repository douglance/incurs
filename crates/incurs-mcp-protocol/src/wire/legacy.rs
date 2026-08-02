use serde_json::Value;

use crate::core::{McpClientMetadata, McpVersion};

use super::{McpValidation, McpWireCodec, McpWireCodecKind};

pub(super) static LEGACY_CODEC: LegacyCodec = LegacyCodec;

pub(super) struct LegacyCodec;

impl McpWireCodec for LegacyCodec {
    fn kind(&self) -> McpWireCodecKind {
        McpWireCodecKind::Legacy
    }

    fn validate_request(&self, standard: &McpVersion, value: &Value) -> McpValidation {
        super::validate_request(standard, value)
    }

    fn project_outbound_request(
        &self,
        _standard: &McpVersion,
        _client: &McpClientMetadata,
        value: Value,
    ) -> Value {
        value
    }

    fn canonicalize_inbound_request(&self, value: Value) -> Value {
        super::remove_reserved_meta(value)
    }
}
