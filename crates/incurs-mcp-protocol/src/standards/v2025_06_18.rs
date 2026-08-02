//! MCP 2025-06-18 profile.

use crate::core::McpLifecycleFamily;
use crate::wire::McpWireCodecKind;

use crate::generated::{V_2025_06_18_NOTIFICATIONS, V_2025_06_18_REQUESTS};

use super::{McpStandardFeatures, McpStandardProfile};

/// MCP 2025-06-18 profile value.
pub static PROFILE: Profile = Profile;

/// MCP 2025-06-18 standard profile.
pub struct Profile;

impl McpStandardProfile for Profile {
    fn version(&self) -> &'static str {
        "2025-06-18"
    }

    fn lifecycle(&self) -> McpLifecycleFamily {
        McpLifecycleFamily::Legacy
    }

    fn codec(&self) -> McpWireCodecKind {
        McpWireCodecKind::Legacy
    }

    fn request_methods(&self) -> &'static [&'static str] {
        V_2025_06_18_REQUESTS
    }

    fn notification_methods(&self) -> &'static [&'static str] {
        V_2025_06_18_NOTIFICATIONS
    }

    fn schema(&self) -> &'static str {
        include_str!("../../schema/2025-06-18/schema.json")
    }

    fn features(&self) -> McpStandardFeatures {
        McpStandardFeatures::default()
    }
}
