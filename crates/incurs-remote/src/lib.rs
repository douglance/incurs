//! Provider-neutral remote capability protocol for Incurs.
//!
//! This crate defines the reusable remote invocation seam: manifests describe
//! callable capabilities, [`RemoteToolCall`] carries a provider-neutral call,
//! [`RemoteToolResult`] reports success or structured failure, and
//! [`RemoteToolRuntime`] is the execution boundary implemented by host adapters.
//! The included [`ToolCatalogRemoteRuntime`] adapts an [`incurs::tool::ToolCatalog`]
//! without adding provider-specific authentication, device schemas, or transport
//! assumptions.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use incurs::command::{McpAnnotations, McpResultContent, RequestContext};
use incurs::output::{CtaBlock, FieldErrorOutput};
use incurs::tool::{
    ConfigSource, EnvironmentSource, ToolCallControl, ToolCallOptions, ToolCallOutcome,
    ToolCatalog, ToolDefinition, ToolEventSink, ToolExample,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Schema identifier for the current remote capability manifest shape.
pub const REMOTE_CAPABILITY_MANIFEST_SCHEMA_VERSION: &str = "incurs.remote.capabilities.v1";

/// Provider-neutral protocol version implemented by this crate.
pub const REMOTE_PROTOCOL_VERSION: &str = "incurs.remote.v1";

/// Describes the capabilities exposed by one remote runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Manifest schema identifier.
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    /// Provider-neutral protocol version implemented by this manifest.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Runtime or application name that owns these capabilities.
    pub name: String,
    /// Runtime or application version, when known.
    pub version: Option<String>,
    /// Stable provider-neutral capability definitions.
    pub capabilities: Vec<CapabilityDefinition>,
    /// Extension data reserved for adapters that need extra metadata.
    pub metadata: BTreeMap<String, Value>,
}

impl CapabilityManifest {
    /// Builds a capability manifest from an Incurs tool catalog.
    pub fn from_catalog(catalog: &ToolCatalog) -> Self {
        Self {
            schema_version: REMOTE_CAPABILITY_MANIFEST_SCHEMA_VERSION.to_string(),
            protocol_version: REMOTE_PROTOCOL_VERSION.to_string(),
            name: catalog.name().to_string(),
            version: catalog.version().map(ToOwned::to_owned),
            capabilities: catalog
                .definitions()
                .into_iter()
                .map(CapabilityDefinition::from)
                .collect(),
            metadata: BTreeMap::new(),
        }
    }
}

/// Provider-neutral description of one callable remote capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDefinition {
    /// Stable capability identifier used by [`RemoteToolCall::capability`].
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for call arguments.
    pub input_schema: Value,
    /// JSON Schema for successful output, when declared.
    pub output_schema: Option<Value>,
    /// Behavioral hints copied from the tool contract.
    pub annotations: Option<McpAnnotations>,
    /// Capability-specific instructions for agent clients.
    pub instructions: Option<String>,
    /// Usage examples copied from the tool definition.
    pub examples: Vec<ToolExample>,
    /// Rich result content declarations copied from the tool definition.
    pub result_content: Vec<McpResultContent>,
    /// Extension data reserved for adapters that need extra metadata.
    pub metadata: BTreeMap<String, Value>,
}

impl From<ToolDefinition> for CapabilityDefinition {
    fn from(definition: ToolDefinition) -> Self {
        Self {
            id: definition.name,
            description: definition.description,
            input_schema: definition.input_schema,
            output_schema: definition.output_schema,
            annotations: definition.annotations,
            instructions: definition.instructions,
            examples: definition.examples,
            result_content: definition.result_content,
            metadata: BTreeMap::new(),
        }
    }
}

/// Durable reference to data used or produced by a remote tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHandle {
    /// Stable artifact identifier in the owning runtime.
    pub id: String,
    /// MIME type or provider-neutral media type.
    pub media_type: String,
    /// Optional human-readable artifact name.
    pub name: Option<String>,
    /// Optional URI that can be dereferenced by an authorized consumer.
    pub uri: Option<String>,
    /// Extension data reserved for adapter-specific addressing.
    pub metadata: BTreeMap<String, String>,
}

/// Transport request facts associated with a remote tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRequestMetadata {
    /// Request headers as lowercase names and string values.
    pub headers: HashMap<String, String>,
    /// Request method or transport operation.
    pub method: String,
    /// Request path, route, or upstream tool name.
    pub path: String,
    /// Optional caller identifier supplied by the host runtime.
    pub caller: Option<String>,
    /// Extension data reserved for provider-specific request metadata.
    pub metadata: BTreeMap<String, String>,
}

impl RemoteRequestMetadata {
    /// Converts this remote metadata into the Incurs command request context.
    pub fn to_request_context(&self) -> RequestContext {
        RequestContext {
            headers: self.headers.clone(),
            method: self.method.clone(),
            path: self.path.clone(),
        }
    }
}

impl From<RequestContext> for RemoteRequestMetadata {
    fn from(request: RequestContext) -> Self {
        Self {
            headers: request.headers,
            method: request.method,
            path: request.path,
            caller: None,
            metadata: BTreeMap::new(),
        }
    }
}

/// Provider-neutral remote tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteToolCall {
    /// Stable call identifier chosen by the caller.
    pub call_id: String,
    /// Optional agent identifier supplied by the caller or host runtime.
    pub agent_id: Option<String>,
    /// Optional remote device or target identifier supplied by the host runtime.
    pub device_id: Option<String>,
    /// Capability identifier from [`CapabilityDefinition::id`].
    pub capability: String,
    /// Optional version of the requested capability contract.
    pub capability_version: Option<String>,
    /// JSON arguments for the capability.
    pub arguments: Value,
    /// Optional Unix deadline timestamp in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Optional idempotency key for runtimes that can deduplicate side effects.
    pub idempotency_key: Option<String>,
    /// Optional trace identifier supplied by the caller or host runtime.
    pub trace_id: Option<String>,
    /// Request metadata visible to the invoked tool.
    pub request: RemoteRequestMetadata,
    /// Artifact handles supplied as inputs to the call.
    pub artifacts: Vec<ArtifactHandle>,
    /// Extension data reserved for adapter-specific call metadata.
    pub metadata: BTreeMap<String, Value>,
}

impl RemoteToolCall {
    /// Creates a call with no arguments and default metadata.
    pub fn new(id: impl Into<String>, capability: impl Into<String>) -> Self {
        Self {
            call_id: id.into(),
            agent_id: None,
            device_id: None,
            capability: capability.into(),
            capability_version: None,
            arguments: Value::Object(serde_json::Map::new()),
            deadline_ms: None,
            idempotency_key: None,
            trace_id: None,
            request: RemoteRequestMetadata::default(),
            artifacts: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

/// Execution-scoped cancellation and event delivery for a remote call.
#[derive(Clone, Default)]
pub struct RemoteToolControl {
    /// Cooperative cancellation signal for the active call.
    pub cancellation: CancellationToken,
    /// Optional ordered event consumer.
    pub events: Option<Arc<dyn ToolEventSink>>,
}

/// Provider-neutral structured failure from a remote tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteToolError {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether retrying the call may succeed.
    pub retryable: Option<bool>,
    /// Per-field validation failures.
    pub field_errors: Option<Vec<FieldErrorOutput>>,
    /// Optional process-style exit code.
    pub exit_code: Option<i32>,
    /// Optional follow-up commands.
    pub cta: Option<CtaBlock>,
    /// Extension data reserved for adapter-specific error details.
    pub details: Option<Value>,
}

impl RemoteToolError {
    /// Creates a remote error with no field errors, follow-up action, or details.
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: Option<bool>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            field_errors: None,
            exit_code: None,
            cta: None,
            details: None,
        }
    }
}

/// Result of one remote tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RemoteToolResult {
    /// Successful remote tool output.
    Ok {
        /// Stable call identifier copied from [`RemoteToolCall::call_id`].
        call_id: String,
        /// Structured command result.
        data: Value,
        /// Call duration in milliseconds as measured by the runtime adapter.
        duration_ms: u64,
        /// Artifact handles produced by the call.
        artifacts: Vec<ArtifactHandle>,
        /// Optional follow-up commands.
        cta: Option<CtaBlock>,
    },
    /// Structured remote tool failure.
    Error {
        /// Stable call identifier copied from [`RemoteToolCall::call_id`].
        call_id: String,
        /// Structured failure payload.
        error: RemoteToolError,
        /// Call duration in milliseconds as measured by the runtime adapter.
        duration_ms: u64,
        /// Artifact handles produced before failure, when any.
        artifacts: Vec<ArtifactHandle>,
    },
}

/// Provider-neutral runtime boundary for remote tool execution.
#[async_trait]
pub trait RemoteToolRuntime: Send + Sync {
    /// Returns the current capability manifest.
    fn manifest(&self) -> CapabilityManifest;

    /// Executes one remote tool call.
    async fn call(&self, call: RemoteToolCall, control: RemoteToolControl) -> RemoteToolResult;
}

/// Remote runtime adapter backed by an Incurs [`ToolCatalog`].
#[derive(Clone)]
pub struct ToolCatalogRemoteRuntime {
    catalog: ToolCatalog,
    options: ToolCatalogRemoteOptions,
}

impl ToolCatalogRemoteRuntime {
    /// Creates a runtime that executes with isolated environment and config.
    pub fn new(catalog: ToolCatalog) -> Self {
        Self {
            catalog,
            options: ToolCatalogRemoteOptions::isolated(),
        }
    }

    /// Creates a runtime with explicit ToolCatalog execution options.
    pub fn with_options(catalog: ToolCatalog, options: ToolCatalogRemoteOptions) -> Self {
        Self { catalog, options }
    }

    /// Returns the wrapped tool catalog.
    pub fn catalog(&self) -> &ToolCatalog {
        &self.catalog
    }
}

/// Execution defaults used by [`ToolCatalogRemoteRuntime`].
#[derive(Debug, Clone)]
pub struct ToolCatalogRemoteOptions {
    /// Environment values used to parse command environment fields.
    pub environment: EnvironmentSource,
    /// Command config defaults.
    pub config: ConfigSource,
    /// CLI-level global option overrides.
    pub globals: Option<Value>,
}

impl ToolCatalogRemoteOptions {
    /// Creates options that do not read process environment or filesystem config.
    pub fn isolated() -> Self {
        Self {
            environment: EnvironmentSource::Empty,
            config: ConfigSource::Disabled,
            globals: None,
        }
    }

    /// Creates options that follow the default ToolCatalog host behavior.
    pub fn host_defaults() -> Self {
        Self {
            environment: EnvironmentSource::DeclaredHost,
            config: ConfigSource::Auto,
            globals: None,
        }
    }
}

impl Default for ToolCatalogRemoteOptions {
    fn default() -> Self {
        Self::isolated()
    }
}

#[async_trait]
impl RemoteToolRuntime for ToolCatalogRemoteRuntime {
    fn manifest(&self) -> CapabilityManifest {
        CapabilityManifest::from_catalog(&self.catalog)
    }

    async fn call(&self, call: RemoteToolCall, control: RemoteToolControl) -> RemoteToolResult {
        let started_at = now_unix_ms();
        if control.cancellation.is_cancelled() {
            return remote_error(
                call.call_id,
                cancelled_error(),
                elapsed_ms(started_at),
                Vec::new(),
            );
        }
        if let Some(deadline) = call.deadline_ms
            && now_unix_ms() >= deadline
        {
            return remote_error(
                call.call_id,
                deadline_error(),
                elapsed_ms(started_at),
                Vec::new(),
            );
        }

        let RemoteToolCall {
            call_id,
            capability,
            deadline_ms,
            arguments,
            request,
            artifacts,
            ..
        } = call;
        let Some(arguments) = flat_arguments(arguments) else {
            return remote_error(
                call_id,
                invalid_arguments_error(),
                elapsed_ms(started_at),
                artifacts,
            );
        };
        let cancellation = control.cancellation.child_token();
        let outcome = self.catalog.call(
            &capability,
            arguments,
            ToolCallOptions {
                environment: self.options.environment.clone(),
                config: self.options.config.clone(),
                globals: self.options.globals.clone(),
                request: Some(request.to_request_context()),
                control: ToolCallControl {
                    cancellation: cancellation.clone(),
                    events: control.events,
                },
            },
        );
        tokio::pin!(outcome);
        let outcome = if let Some(deadline_ms) = deadline_ms {
            tokio::select! {
                _ = control.cancellation.cancelled() => {
                    cancellation.cancel();
                    return remote_error(call_id, cancelled_error(), elapsed_ms(started_at), artifacts);
                }
                _ = sleep_until(deadline_ms) => {
                    cancellation.cancel();
                    return remote_error(call_id, deadline_error(), elapsed_ms(started_at), artifacts);
                }
                outcome = &mut outcome => outcome,
            }
        } else {
            tokio::select! {
                _ = control.cancellation.cancelled() => {
                    cancellation.cancel();
                    return remote_error(call_id, cancelled_error(), elapsed_ms(started_at), artifacts);
                }
                outcome = &mut outcome => outcome,
            }
        };
        match outcome {
            ToolCallOutcome::Ok { data, cta } => RemoteToolResult::Ok {
                call_id,
                data,
                duration_ms: elapsed_ms(started_at),
                artifacts,
                cta,
            },
            ToolCallOutcome::Error {
                code,
                message,
                retryable,
                field_errors,
                cta,
                exit_code,
            } => RemoteToolResult::Error {
                call_id,
                error: RemoteToolError {
                    code,
                    message,
                    retryable,
                    field_errors,
                    exit_code,
                    cta,
                    details: None,
                },
                duration_ms: elapsed_ms(started_at),
                artifacts,
            },
        }
    }
}

fn remote_error(
    call_id: String,
    error: RemoteToolError,
    duration_ms: u64,
    artifacts: Vec<ArtifactHandle>,
) -> RemoteToolResult {
    RemoteToolResult::Error {
        call_id,
        error,
        duration_ms,
        artifacts,
    }
}

fn cancelled_error() -> RemoteToolError {
    RemoteToolError::new("CANCELLED", "Remote tool call cancelled", Some(false))
}

fn deadline_error() -> RemoteToolError {
    RemoteToolError::new(
        "DEADLINE_EXCEEDED",
        "Remote tool call deadline exceeded",
        Some(true),
    )
}

fn invalid_arguments_error() -> RemoteToolError {
    RemoteToolError::new(
        "INVALID_ARGUMENTS",
        "Remote tool call arguments must be a JSON object",
        Some(false),
    )
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn sleep_until(deadline_ms: u64) {
    tokio::time::sleep(Duration::from_millis(
        deadline_ms.saturating_sub(now_unix_ms()),
    ))
    .await;
}

fn elapsed_ms(started_at: u64) -> u64 {
    now_unix_ms().saturating_sub(started_at)
}

fn flat_arguments(arguments: Value) -> Option<BTreeMap<String, Value>> {
    let Value::Object(arguments) = arguments else {
        return None;
    };
    Some(arguments.into_iter().collect())
}
