//! Canonical protocol types shared by all MCP standards.

use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An open MCP protocol-version identifier.
///
/// Version preference is defined by the standard registry, never by lexical
/// comparison of this value.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct McpVersion(String);

impl McpVersion {
    /// Creates a protocol-version identifier.
    pub fn new(version: impl Into<String>) -> Self {
        Self(version.into())
    }

    /// Returns the wire representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for McpVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<&str> for McpVersion {
    fn from(version: &str) -> Self {
        Self::new(version)
    }
}

impl From<String> for McpVersion {
    fn from(version: String) -> Self {
        Self::new(version)
    }
}

/// MCP lifecycle family used by a published standard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpLifecycleFamily {
    /// Stateful `initialize` and `notifications/initialized`.
    Legacy,
    /// Stateless `server/discover` and per-request protocol metadata.
    Modern,
}

/// Canonical client identity projected into modern request metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpClientMetadata {
    /// Client implementation name.
    pub name: String,
    /// Client implementation version.
    pub version: String,
    /// Client capabilities object.
    pub capabilities: Value,
}

impl McpClientMetadata {
    /// Creates modern client metadata.
    pub fn new(name: impl Into<String>, version: impl Into<String>, capabilities: Value) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            capabilities,
        }
    }
}
