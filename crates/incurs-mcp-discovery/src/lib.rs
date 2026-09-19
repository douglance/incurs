//! Reads the MCP server configuration a developer's coding agents already have.
//!
//! This crate is deliberately inert. It reads files, and that is all: it never
//! starts a process, resolves a `PATH` entry, or opens a network connection.
//! Apart from [`HostPaths::from_env`] it does not read the environment either,
//! so everything it returns is a pure function of file bytes plus one resolved
//! path set — which is what makes it testable against a fixture tree without
//! `std::env::set_var`, an operation that is `unsafe` in edition 2024 and races
//! across the test harness's threads.
//!
//! Configured credentials are separated from configuration at parse time. A
//! value classified as a credential is held in a [`SecretValue`], which
//! implements neither `Serialize` nor `Deserialize`, so persisting one is a
//! compile error rather than something a reviewer has to catch.

pub mod error;
pub mod hosts;
pub mod identity;
pub mod parse;
pub mod secret;
pub mod transport;

pub use error::{DiscoveryDiagnostic, DiscoveryError, Severity};
pub use hosts::{
    ConfigEncoding, ConfigLocation, ConfigScope, HostPaths, McpConfigSource, McpDiscoverySource,
    locations,
};
pub use identity::{ConfigFingerprint, ServerId};
pub use secret::{EnvClass, EnvValue, SecretValue};
pub use transport::{AuthClass, HttpTransport, McpTransport, ProgramToken, StdioTransport};

/// One MCP server entry parsed from exactly one host configuration file.
///
/// The same server configured in several hosts yields several of these, all
/// sharing one [`ServerId`]. Collapsing them is the registry's job.
#[derive(Debug, Clone)]
pub struct DiscoveredMcpServer {
    /// Identity derived from the transport alone.
    pub id: ServerId,
    /// Digest of every launch input, used to invalidate a cached tool schema.
    pub fingerprint: ConfigFingerprint,
    /// The name this server has in its own configuration file.
    pub local_name: String,
    /// Where the entry came from.
    pub source: McpConfigSource,
    /// Transport and its configuration, with credentials held separately.
    pub transport: McpTransport,
    /// Whether the declaring host marked the entry enabled.
    pub enabled: bool,
    /// Placeholder identifiers the entry needs and discovery cannot resolve.
    pub requires_input: Vec<String>,
}

/// Everything one discovery pass found.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Every server entry parsed, in host precedence order.
    pub servers: Vec<DiscoveredMcpServer>,
    /// Non-fatal findings about individual entries.
    pub diagnostics: Vec<DiscoveryDiagnostic>,
    /// Files that could not be read or parsed at all.
    pub unreadable: Vec<(std::path::PathBuf, String)>,
}

/// Reads every known host configuration file.
///
/// A file that is missing is skipped silently; a file that exists but cannot be
/// parsed is recorded in [`Discovery::unreadable`] and never aborts the pass, so
/// one broken configuration cannot hide every healthy server on the machine.
pub fn discover(paths: &HostPaths) -> Discovery {
    let mut result = Discovery::default();
    for location in locations(paths) {
        if !location.path.exists() {
            continue;
        }
        match parse::parse_location(&location, paths, &mut result.diagnostics) {
            Ok(servers) => result.servers.extend(servers),
            Err(error) => result
                .unreadable
                .push((location.path.clone(), error.to_string())),
        }
    }
    result
}
