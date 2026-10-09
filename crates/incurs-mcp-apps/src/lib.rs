//! Provider-neutral MCP App lifecycle and transport primitives.
//!
//! The crate models the shared app runtime surface: JSON-RPC request
//! correlation, notifications, lifecycle state, host capabilities, host
//! context changes, tool input, tool result, and disposal. Provider-specific
//! helpers build on [`McpApp`] and keep their own metadata keys outside this
//! crate.

#![deny(missing_docs)]

mod app;
#[cfg(target_arch = "wasm32")]
pub mod browser;
mod error;
pub mod transport;

pub use app::{
    HOST_CONTEXT_CHANGED_METHOD, HostCapabilities, HostContextListener, INITIALIZE_METHOD,
    INITIALIZED_NOTIFICATION_METHOD, InitializeParams, InitializeResult,
    MODEL_CONTEXT_UPDATE_METHOD, McpApp, McpAppSnapshot, REQUEST_DISPLAY_MODE_METHOD,
    REQUEST_TEARDOWN_NOTIFICATION_METHOD, RESOURCE_READ_METHOD, RequestCancellation,
    RequestOptions, SEND_MESSAGE_METHOD, TOOL_CANCELLED_NOTIFICATION_METHOD,
    TOOL_INPUT_NOTIFICATION_METHOD, TOOL_INPUT_PARTIAL_NOTIFICATION_METHOD,
    TOOL_RESULT_NOTIFICATION_METHOD,
};
pub use error::{AppError, AppResult};
pub use transport::{
    AppTransport, AppTransportFuture, InMemoryTransport, Listener, ListenerRegistration,
    RequestHandler, empty_result, value_handler,
};
