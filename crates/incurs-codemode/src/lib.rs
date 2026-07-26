//! Portable durable Code Mode runtime for incurs tools.
//!
//! This crate owns connector discovery, model-facing type generation, approval
//! policy, deterministic replay, snippets, and lifecycle state. Sandbox
//! execution and durable persistence are supplied through traits so the same
//! runtime can run through local or remote executors.

mod codec;
mod codemode;
mod connector;
mod dispatch;
mod engine;
mod normalize;
mod program;
mod runtime;
mod search;
mod service;
mod typescript;

pub use codec::{CodeValue, decode_value, encode_value};
pub use codemode::{CodeMode, CodeModeRunOptions};
pub use connector::{
    Connector, ConnectorDescription, ConnectorExample, ConnectorTool, DefaultToolPolicyResolver,
    IncurConnector, McpClient, McpConnector, McpTool, OpenApiClient, OpenApiConnector,
    OpenApiRequest, ReplayPolicy, ToolAnnotations, ToolContext, ToolOrigin, ToolPolicy,
    ToolPolicyResolver,
};
pub use dispatch::{DispatchResponse, DispatchSession, StepResponse};
pub use engine::{Clock, CodeExecutor, DispatchRequest, ExecuteResult, ExecutionHost, SystemClock};
pub use normalize::normalize_code;
pub use program::{ProgramSourceOptions, build_program_source};
pub use runtime::{
    ArtifactRef, ArtifactStore, CapabilitySnapshot, CodeModeRuntime, DEFAULT_MAX_EVENTS,
    DEFAULT_MAX_EXECUTIONS, DEFAULT_PAUSED_TTL_MS, ExecutionEvent, ExecutionState, ExecutionStatus,
    LogEntry, LogEntryState, MAX_DURABLE_VALUE_BYTES, MemoryArtifactStore, MemoryStore,
    PendingAction, RuntimeError, RuntimeStore, Snippet, ToolDecision,
};
pub use search::{DescribeOutput, SearchOutput, SearchResult, describe, search};
pub use service::CodeModeService;
pub use typescript::{generate_types, json_schema_to_type, sanitize_identifier};
