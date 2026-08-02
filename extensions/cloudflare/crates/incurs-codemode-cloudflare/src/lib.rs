//! Cloudflare WorkerLoader and Durable Object adapters for incurs Code Mode.
//!
//! The public API remains Rust. This crate emits a small internal JavaScript
//! module because Dynamic Workers currently accept JavaScript or Python source,
//! not a nested Rust/Wasm Worker.

mod http_mcp;
mod lifecycle;
mod module;

pub use http_mcp::{
    CODEMODE_MCP_TOOL_NAMES, MCP_LEGACY_PROTOCOL_VERSION, MCP_PROTOCOL_VERSION, McpHttpOptions,
    McpHttpRequest, McpHttpResponse, WorkerCodeModeService, handle_mcp_request,
    mcp_tool_definitions, tenant_key,
};
pub use lifecycle::{drive_with_terminal_failure, persist_terminal_failure};
pub use module::{DynamicWorkerOptions, build_executor_module};

#[cfg(target_arch = "wasm32")]
mod executor;
#[cfg(target_arch = "wasm32")]
mod sql_store;

#[cfg(target_arch = "wasm32")]
pub use executor::{CloudflareClock, DynamicWorkerExecutor, WorkerLoader};
#[cfg(target_arch = "wasm32")]
pub use sql_store::DurableSqlStore;
