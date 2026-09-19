//! The production MCP client for incurs Code Mode.
//!
//! A configured server is not a running server. Constructing a client starts
//! nothing: the process or connection is created on the first call that
//! genuinely needs it, and tool schemas normally come from a persistent cache
//! keyed by a configuration fingerprint, so listing the capabilities of twenty
//! servers usually costs no processes at all.

pub mod bridge;
pub mod cache;
pub mod connect;
pub mod health;
pub mod lazy;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use bridge::{BridgeError, IoBridge};
pub use cache::{
    CACHE_SCHEMA_VERSION, CacheEntry, DEFAULT_TTL_MS, NEGATIVE_TTL_MS, ToolCacheStore,
    UnreachableRecord,
};
pub use health::{HealthCode, HealthRegistry, HealthReport, ServerHealth};
pub use lazy::{ClientLimits, LazyMcpClient};
