//! MCP 2026-07-28 profile.

use crate::core::McpLifecycleFamily;
use crate::wire::McpWireCodecKind;

use crate::generated::{V_2026_07_28_NOTIFICATIONS, V_2026_07_28_REQUESTS};

use super::{McpStandardFeatures, McpStandardProfile};

/// MCP 2026-07-28 profile value.
pub static PROFILE: Profile = Profile;

/// MCP 2026-07-28 standard profile.
pub struct Profile;

impl McpStandardProfile for Profile {
    fn version(&self) -> &'static str {
        "2026-07-28"
    }

    fn lifecycle(&self) -> McpLifecycleFamily {
        McpLifecycleFamily::Modern
    }

    fn codec(&self) -> McpWireCodecKind {
        McpWireCodecKind::Modern
    }

    fn request_methods(&self) -> &'static [&'static str] {
        V_2026_07_28_REQUESTS
    }

    fn notification_methods(&self) -> &'static [&'static str] {
        V_2026_07_28_NOTIFICATIONS
    }

    fn schema(&self) -> &'static str {
        include_str!("../../schema/2026-07-28/schema.json")
    }

    fn features(&self) -> McpStandardFeatures {
        McpStandardFeatures {
            stateless_lifecycle: true,
            per_request_metadata: true,
            result_type: true,
            cache_controls: true,
            standard_http_headers: true,
            multi_round_tool_results: true,
            subscription_listen: true,
        }
    }
}
