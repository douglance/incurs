//! Published MCP standard registry.

mod private;
pub mod v2024_11_05;
pub mod v2025_03_26;
pub mod v2025_06_18;
pub mod v2025_11_25;
pub mod v2026_07_28;

use crate::core::{McpLifecycleFamily, McpVersion};
use crate::wire::McpWireCodecKind;

/// JSON-RPC error code for an unsupported modern protocol version.
pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;

/// Feature changes attached to one exact standard profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct McpStandardFeatures {
    /// Uses stateless `server/discover` lifecycle negotiation.
    pub stateless_lifecycle: bool,
    /// Requires client identity and protocol version on each request.
    pub per_request_metadata: bool,
    /// Requires a result-type discriminator.
    pub result_type: bool,
    /// Supports result cache TTL and scope.
    pub cache_controls: bool,
    /// Uses the standard MCP HTTP headers.
    pub standard_http_headers: bool,
    /// Supports multi-round tool results.
    pub multi_round_tool_results: bool,
    /// Supports `subscriptions/listen`.
    pub subscription_listen: bool,
}

/// Exact behavior supplied by one published MCP standard.
pub trait McpStandardProfile: private::Sealed + Send + Sync {
    /// Returns the exact wire protocol version.
    fn version(&self) -> &'static str;

    /// Returns the lifecycle family.
    fn lifecycle(&self) -> McpLifecycleFamily;

    /// Returns the shared wire codec family.
    fn codec(&self) -> McpWireCodecKind;

    /// Returns request methods admitted by this standard.
    fn request_methods(&self) -> &'static [&'static str];

    /// Returns notification methods admitted by this standard.
    fn notification_methods(&self) -> &'static [&'static str];

    /// Returns the pinned official JSON Schema.
    fn schema(&self) -> &'static str;

    /// Returns features introduced or active in this standard.
    fn features(&self) -> McpStandardFeatures;
}

static STANDARDS: [&dyn McpStandardProfile; 5] = [
    &v2024_11_05::PROFILE,
    &v2025_03_26::PROFILE,
    &v2025_06_18::PROFILE,
    &v2025_11_25::PROFILE,
    &v2026_07_28::PROFILE,
];

/// Returns all official standards in publication order.
pub fn known_standards() -> &'static [&'static dyn McpStandardProfile] {
    &STANDARDS
}

/// Resolves one exact official standard.
pub fn known_standard(version: &McpVersion) -> Option<&'static dyn McpStandardProfile> {
    known_standards()
        .iter()
        .copied()
        .find(|profile| profile.version() == version.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_each_official_standard_once() {
        assert_eq!(
            known_standards()
                .iter()
                .map(|standard| standard.version())
                .collect::<Vec<_>>(),
            [
                "2024-11-05",
                "2025-03-26",
                "2025-06-18",
                "2025-11-25",
                "2026-07-28",
            ]
        );
    }

    #[test]
    fn lifecycle_changes_only_at_the_modern_standard() {
        for standard in &known_standards()[..4] {
            assert_eq!(standard.lifecycle(), McpLifecycleFamily::Legacy);
            assert_eq!(standard.codec(), McpWireCodecKind::Legacy);
        }
        assert_eq!(known_standards()[4].lifecycle(), McpLifecycleFamily::Modern);
        assert_eq!(known_standards()[4].codec(), McpWireCodecKind::Modern);
        assert_eq!(
            known_standards()[4].features(),
            McpStandardFeatures {
                stateless_lifecycle: true,
                per_request_metadata: true,
                result_type: true,
                cache_controls: true,
                standard_http_headers: true,
                multi_round_tool_results: true,
                subscription_listen: true,
            }
        );
    }

    #[test]
    fn exact_profiles_embed_their_pinned_schema_and_generated_methods() {
        for standard in known_standards() {
            let schema: serde_json::Value =
                serde_json::from_str(standard.schema()).expect("valid pinned schema");
            assert_eq!(
                schema["properties"]["method"].as_str(),
                None,
                "schema root is a union rather than a flattened dialect"
            );
            assert!(standard.request_methods().contains(&"tools/list"));
        }
        assert!(
            !known_standards()[3]
                .request_methods()
                .contains(&"tasks/get")
        );
        assert!(
            known_standards()[4]
                .request_methods()
                .contains(&"subscriptions/listen")
        );
    }
}
