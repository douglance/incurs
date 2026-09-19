//! Reachability state, in a shape that cannot carry a credential.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use incurs_mcp_discovery::ServerId;
use serde::{Deserialize, Serialize};

/// Last known reachability of one MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerHealth {
    /// Not yet contacted, and not yet needed.
    Unknown,
    /// Initialization and tool listing both succeeded.
    Healthy,
    /// The server could not be reached at all.
    Unavailable,
    /// The entry cannot be launched as written.
    InvalidConfiguration,
    /// The server answered but demanded or rejected a credential.
    AuthenticationRequired,
    /// A connection opened, but the MCP handshake did not complete.
    InitializationFailed,
    /// The handshake succeeded but the tool schema could not be refreshed.
    ///
    /// Distinct from [`ServerHealth::InitializationFailed`] because a cached
    /// schema stays usable here and does not there.
    SchemaRefreshFailed,
}

/// Why a server is in its current state, from a closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthCode {
    /// The executable named by the configuration does not exist.
    SpawnNotFound,
    /// The executable exists but could not be run.
    SpawnPermissionDenied,
    /// The process could not be started for another reason.
    SpawnFailed,
    /// A connection was refused or the endpoint could not be resolved.
    ConnectFailed,
    /// The MCP handshake did not complete.
    HandshakeFailed,
    /// The peer reported that authentication is required.
    AuthRequired,
    /// Listing tools failed.
    ToolsListFailed,
    /// The entry declared neither a usable command nor a usable endpoint.
    ConfigInvalid,
    /// The entry needs a value that cannot be resolved without a person.
    ConfigNeedsInput,
    /// The declaring host marked the entry disabled.
    ConfigDisabled,
    /// This build cannot speak the configured transport.
    TransportUnsupported,
}

/// A report that is safe to persist and to show a model.
///
/// There is deliberately no free-form message field. Raw failure text — child
/// stderr, an HTTP body, an I/O error's display string — is classified into a
/// [`ServerHealth`] and a [`HealthCode`] at the point of failure and then
/// dropped. It is therefore structurally impossible for a credential, a
/// hostname, or a stack trace to reach a diagnostic through this type, which is
/// a compile-time property rather than a review convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    /// Coarse state.
    pub state: ServerHealth,
    /// Stable reason, when one applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<HealthCode>,
    /// When the state was observed, in Unix milliseconds.
    pub checked_at_ms: u64,
    /// Tools listed at that observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
    /// Whether the tools came from the cache rather than a live connection.
    pub from_cache: bool,
}

impl HealthReport {
    /// Builds a report for a server nothing has needed yet.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            state: ServerHealth::Unknown,
            code: None,
            checked_at_ms: 0,
            tool_count: None,
            from_cache: false,
        }
    }

    /// Returns whether the server is usable.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(
            self.state,
            ServerHealth::Healthy | ServerHealth::SchemaRefreshFailed | ServerHealth::Unknown
        )
    }

    /// Returns a short model-facing explanation.
    ///
    /// Built from fixed text, so it carries no configured value.
    #[must_use]
    pub fn summary(&self) -> &'static str {
        match (self.state, self.code) {
            (ServerHealth::Healthy, _) => "available",
            (ServerHealth::Unknown, _) => "not contacted yet",
            (_, Some(HealthCode::SpawnNotFound)) => "its command was not found on this machine",
            (_, Some(HealthCode::SpawnPermissionDenied)) => "its command could not be executed",
            (_, Some(HealthCode::AuthRequired)) => "it requires authentication",
            (_, Some(HealthCode::ConnectFailed)) => "it could not be reached",
            (_, Some(HealthCode::HandshakeFailed)) => "it did not complete the MCP handshake",
            (_, Some(HealthCode::ToolsListFailed)) => "it did not return a tool list",
            (_, Some(HealthCode::ConfigNeedsInput)) => {
                "its configuration needs a value only a person can supply"
            }
            (_, Some(HealthCode::ConfigDisabled)) => "it is disabled in its own configuration",
            (_, Some(HealthCode::TransportUnsupported)) => "its transport is not supported yet",
            _ => "it is unavailable",
        }
    }
}

/// Shared health for every registered server.
#[derive(Debug, Default)]
pub struct HealthRegistry {
    entries: Mutex<BTreeMap<String, HealthReport>>,
}

impl HealthRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records one server's state.
    pub fn record(&self, id: &ServerId, report: HealthReport) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(id.as_str().to_string(), report);
        }
    }

    /// Returns one server's last known state.
    #[must_use]
    pub fn get(&self, id: &ServerId) -> Option<HealthReport> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(id.as_str()).copied())
    }

    /// Returns every recorded state, keyed by server identity.
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<String, HealthReport> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }
}

/// Classifies a spawn or connection failure without retaining its text.
#[must_use]
pub fn classify_io(error: &std::io::Error) -> HealthCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => HealthCode::SpawnNotFound,
        std::io::ErrorKind::PermissionDenied => HealthCode::SpawnPermissionDenied,
        _ => HealthCode::SpawnFailed,
    }
}

/// Classifies a handshake failure from its rendered form.
///
/// The text is read and discarded here; only the resulting code escapes.
#[must_use]
pub fn classify_handshake(rendered: &str) -> (ServerHealth, HealthCode) {
    let lowered = rendered.to_ascii_lowercase();
    if lowered.contains("401") || lowered.contains("unauthorized") || lowered.contains("forbidden")
    {
        (
            ServerHealth::AuthenticationRequired,
            HealthCode::AuthRequired,
        )
    } else if lowered.contains("connect") || lowered.contains("dns") || lowered.contains("refused")
    {
        (ServerHealth::Unavailable, HealthCode::ConnectFailed)
    } else if lowered.contains("no such file") || lowered.contains("not found") {
        (ServerHealth::Unavailable, HealthCode::SpawnNotFound)
    } else {
        (
            ServerHealth::InitializationFailed,
            HealthCode::HandshakeFailed,
        )
    }
}
