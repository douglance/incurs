//! MCP 2025-11-25 profile.

use crate::core::McpLifecycleFamily;
use crate::wire::McpWireCodecKind;

use crate::generated::{V_2025_11_25_NOTIFICATIONS, V_2025_11_25_REQUESTS};

use super::{McpStandardFeatures, McpStandardProfile};

/// MCP 2025-11-25 profile value.
pub static PROFILE: Profile = Profile;

/// MCP 2025-11-25 standard profile.
pub struct Profile;

impl McpStandardProfile for Profile {
    fn version(&self) -> &'static str {
        "2025-11-25"
    }

    fn lifecycle(&self) -> McpLifecycleFamily {
        McpLifecycleFamily::Legacy
    }

    fn codec(&self) -> McpWireCodecKind {
        McpWireCodecKind::Legacy
    }

    fn request_methods(&self) -> &'static [&'static str] {
        V_2025_11_25_REQUESTS
    }

    fn notification_methods(&self) -> &'static [&'static str] {
        V_2025_11_25_NOTIFICATIONS
    }

    fn schema(&self) -> &'static str {
        include_str!("../../schema/2025-11-25/schema.json")
    }

    fn features(&self) -> McpStandardFeatures {
        McpStandardFeatures::default()
    }
}
