//! Persistent tool-schema storage, keyed by configuration fingerprint.

use std::path::{Path, PathBuf};

use incurs_codemode::McpTool;
use incurs_mcp_discovery::{ConfigFingerprint, ServerId};
use serde::{Deserialize, Serialize};

use crate::health::{HealthCode, ServerHealth};

/// Cache layout version.
///
/// A bump discards the whole tree rather than migrating it. The cache is fully
/// reconstructible from the servers themselves, so migration code would be
/// liability with no corresponding benefit.
pub const CACHE_SCHEMA_VERSION: u32 = 1;

/// How long a cached schema is served before a refresh is wanted.
///
/// A stale entry is still served immediately. Blocking capability listing on a
/// cold start of every configured server would cost tens of seconds, which is
/// the cost this cache exists to avoid.
pub const DEFAULT_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// How long a recorded failure suppresses another connection attempt.
///
/// Without this, a machine with twenty unreachable servers pays their full
/// connect timeout on every single execution, because a failure lives only in
/// the process that saw it. Ten minutes is short on purpose: a server the
/// developer has just fixed must come back on its own, and `cmpst refresh`
/// ignores this entirely for anyone unwilling to wait.
pub const NEGATIVE_TTL_MS: u64 = 10 * 60 * 1000;

/// A recorded failure to reach one server.
///
/// Held on the same entry as the schema rather than in a second file, so one
/// server's whole story is one atomic write and a configuration change
/// invalidates the failure exactly as it invalidates the tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreachableRecord {
    /// Coarse state recorded at the time.
    pub state: ServerHealth,
    /// Why it could not be reached, from the closed vocabulary.
    pub code: HealthCode,
    /// When the attempt failed, in Unix milliseconds.
    pub checked_at_ms: u64,
}

impl UnreachableRecord {
    /// Returns whether this failure is still young enough to trust.
    #[must_use]
    pub fn is_fresh(&self, now_ms: u64, ttl_ms: u64) -> bool {
        now_ms.saturating_sub(self.checked_at_ms) < ttl_ms
    }
}

/// One server's cached capabilities.
///
/// This type is `Serialize`, which is only sound because every field is either
/// a digest or schema metadata. A configured environment value cannot appear
/// here: [`incurs_mcp_discovery::SecretValue`] implements no serializer, so
/// adding a field that carried one would not compile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    /// Layout version this file was written with.
    pub cache_schema_version: u32,
    /// Server the entry belongs to.
    pub server_id: String,
    /// Fingerprint of the configuration that produced these tools.
    pub config_fingerprint: String,
    /// Server-level instructions reported at initialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Protocol version negotiated when the schema was fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// When the schema was fetched, in Unix milliseconds.
    pub fetched_at_ms: u64,
    /// The server's tools.
    pub tools: Vec<McpTool>,
    /// Why the last attempt to reach this server failed, when one did.
    ///
    /// Optional and defaulted, so an entry written before this field existed
    /// still loads. Present with an empty `tools` means "known unreachable";
    /// present alongside tools means the schema is cached but the server was
    /// unreachable when last contacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable: Option<UnreachableRecord>,
}

impl CacheEntry {
    /// Returns whether a refresh is wanted, without making the entry unusable.
    #[must_use]
    pub fn is_stale(&self, now_ms: u64, ttl_ms: u64) -> bool {
        now_ms.saturating_sub(self.fetched_at_ms) > ttl_ms
    }
}

/// On-disk tool-schema cache.
///
/// One file per server, so refreshing one cannot corrupt or race another, and
/// every write is a staged write followed by a rename so a reader sees either
/// the old document or the new one.
#[derive(Debug, Clone)]
pub struct ToolCacheStore {
    root: PathBuf,
}

impl ToolCacheStore {
    /// Opens a cache rooted at a directory.
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Opens the cache in this user's data directory.
    ///
    /// Honours `COMPOSITE_DATA_HOME`, then `INCURS_DATA_HOME`, then the
    /// platform's local data directory. The second is the convention the incurs
    /// plugin installer already uses, so a machine that set it keeps working,
    /// and either variable gives tests one place to redirect.
    ///
    /// # Errors
    /// Returns an error when no user data directory can be resolved.
    pub fn from_env() -> Result<Self, String> {
        let base = std::env::var_os("COMPOSITE_DATA_HOME")
            .or_else(|| std::env::var_os("INCURS_DATA_HOME"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| "cannot resolve a user data directory".to_string())?;
        Ok(Self::new(base.join("composite")))
    }

    /// Returns the directory holding per-server schema files.
    #[must_use]
    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }

    /// Returns the file backing one server.
    #[must_use]
    pub fn entry_path(&self, id: &ServerId) -> PathBuf {
        self.tools_dir().join(format!("{}.json", id.as_str()))
    }

    /// Loads a usable entry for one server.
    ///
    /// Returns `None` for a missing, unreadable, malformed, superseded, or
    /// fingerprint-mismatched file. Every read failure is a miss rather than an
    /// error: the cache is an optimization, and a corrupt file must never be
    /// able to take the runtime down with it.
    #[must_use]
    pub fn load(&self, id: &ServerId, fingerprint: &ConfigFingerprint) -> Option<CacheEntry> {
        let path = self.entry_path(id);
        let text = std::fs::read_to_string(&path).ok()?;
        let entry: CacheEntry = serde_json::from_str(&text).ok()?;
        if entry.cache_schema_version != CACHE_SCHEMA_VERSION
            || entry.server_id != id.as_str()
            || entry.config_fingerprint != fingerprint.as_str()
        {
            return None;
        }
        Some(entry)
    }

    /// Writes one server's capabilities.
    ///
    /// # Errors
    /// Returns an error when the file cannot be created or renamed into place.
    pub fn store(&self, entry: &CacheEntry) -> Result<(), String> {
        let directory = self.tools_dir();
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let target = directory.join(format!("{}.json", entry.server_id));
        // Staged in the same directory, so the rename stays on one filesystem
        // and is therefore atomic.
        let staged = directory.join(format!(
            "{}.json.tmp.{}",
            entry.server_id,
            std::process::id()
        ));
        let bytes = serde_json::to_vec_pretty(entry).map_err(|error| error.to_string())?;
        std::fs::write(&staged, &bytes).map_err(|error| error.to_string())?;
        std::fs::rename(&staged, &target).map_err(|error| {
            let _ = std::fs::remove_file(&staged);
            error.to_string()
        })
    }

    /// Removes one server's entry, ignoring a missing file.
    pub fn forget(&self, id: &ServerId) {
        let _ = std::fs::remove_file(self.entry_path(id));
    }
}

/// Returns the current Unix time in milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
