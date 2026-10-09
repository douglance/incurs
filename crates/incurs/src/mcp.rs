//! MCP (Model Context Protocol) stdio server.
//!
//! Exposes CLI commands as MCP tools over a stdio
//! transport. The actual server implementation uses the `rmcp` crate. It is
//! built on every target except wasm32, where there is no stdio to serve on;
//! a wasm32 host such as a Cloudflare Worker serves tools through its own
//! adapter over [`crate::tool::ToolCatalog`].

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::schema::FieldMeta;
use futures::stream::BoxStream;
use serde_json::Value;

#[cfg(all(test, feature = "http", not(target_arch = "wasm32")))]
mod parity_tests;
mod portable_server;
mod shared;

pub use portable_server::{
    DEFAULT_MAX_REQUEST_BODY_BYTES, McpHttpBody, McpHttpConfig, McpHttpRequest, McpHttpResponse,
    McpHttpServer,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A resolved tool entry from the command tree.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    /// Tool name (path segments joined with `_`).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Merged JSON Schema for the tool's input.
    pub input_schema: serde_json::Value,
    /// JSON Schema for the tool's output.
    pub output_schema: Option<serde_json::Value>,
}

/// A command entry in the command tree for MCP tool collection.
///
/// This is a minimal representation to avoid depending on the `command` module
/// which is written by another engineer in parallel.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Whether this entry is a group (has subcommands).
    pub is_group: bool,
    /// Human-readable description.
    pub description: Option<String>,
    /// Subcommands (only populated for groups).
    pub commands: BTreeMap<String, CommandEntry>,
    /// Positional argument field metadata.
    pub args_fields: Vec<FieldMeta>,
    /// Named option field metadata.
    pub options_fields: Vec<FieldMeta>,
    /// JSON Schema for the command's output.
    pub output_schema: Option<serde_json::Value>,
    /// Exact MCP `inputSchema` to publish for this command, overriding the
    /// schema [`collect_tools`] would otherwise derive from `args_fields`
    /// and `options_fields`. See
    /// [`McpCommandOptions::input_schema`](crate::command::McpCommandOptions::input_schema)
    /// for the constraint on top-level property names.
    pub input_schema: Option<serde_json::Value>,
}

/// MCP tool discovery strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum McpDiscovery {
    /// Expose search, inspect, and execute tools that discover commands lazily.
    #[default]
    Progressive,
    /// Expose every command as a direct MCP tool.
    Direct,
}

/// Filters which command tools are exposed to MCP clients.
#[derive(Debug, Clone, Default)]
pub struct McpToolFilter {
    /// Discovery strategy. Defaults to progressive discovery.
    pub discovery: McpDiscovery,
    /// Glob patterns selecting tools to include.
    pub include: Vec<String>,
    /// Glob patterns selecting tools to exclude. Excludes win.
    pub exclude: Vec<String>,
}

/// Outcome supplied to an application-defined MCP result mapper.
pub struct McpResultContext<'a> {
    /// Tool whose completed outcome is being projected.
    pub tool: &'a crate::tool::ToolDefinition,
    /// Completed command outcome before MCP projection.
    pub outcome: &'a crate::tool::ToolCallOutcome,
}

/// Application overrides for the MCP result envelope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpResultMapping {
    /// Override for `isError`; `None` keeps the normal classification.
    pub is_error: Option<bool>,
    /// Replacement MCP `structuredContent`; `None` keeps the command output projection.
    pub structured_content: Option<Value>,
    /// Replacement MCP `content` blocks; `None` keeps the generated content.
    pub content: Option<Vec<Value>>,
    /// Dynamic namespaced result metadata merged into `_meta`.
    pub meta: BTreeMap<String, Value>,
}
impl McpResultMapping {
    /// Keeps normal MCP outcome classification.
    pub fn unchanged() -> Self {
        Self::default()
    }
    /// Projects this outcome as a successful MCP result.
    pub fn success() -> Self {
        Self {
            is_error: Some(false),
            structured_content: None,
            content: None,
            meta: BTreeMap::new(),
        }
    }
    /// Projects this outcome as a failed MCP result.
    pub fn error() -> Self {
        Self {
            is_error: Some(true),
            structured_content: None,
            content: None,
            meta: BTreeMap::new(),
        }
    }
}

/// Shared application hook for classifying completed MCP tool results.
#[derive(Clone)]
pub struct McpResultMapper(
    std::sync::Arc<
        dyn for<'a> Fn(McpResultContext<'a>) -> McpResultMapping + Send + Sync + 'static,
    >,
);
impl std::fmt::Debug for McpResultMapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpResultMapper").finish_non_exhaustive()
    }
}
impl McpResultMapper {
    /// Creates a mapper called with the tool and its completed outcome.
    pub fn new(
        mapper: impl for<'a> Fn(McpResultContext<'a>) -> McpResultMapping + Send + Sync + 'static,
    ) -> Self {
        Self(std::sync::Arc::new(mapper))
    }
    pub(super) fn map(&self, context: McpResultContext<'_>) -> McpResultMapping {
        (self.0)(context)
    }
}

/// A resource advertised by an MCP server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    /// URI identifying this resource.
    pub uri: String,
    /// Programmatic resource name.
    pub name: String,
    /// Optional display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional MIME type.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional raw byte size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Optional icon descriptors.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub icons: Vec<Value>,
    /// Namespaced resource metadata.
    #[serde(rename = "_meta", skip_serializing_if = "BTreeMap::is_empty", default)]
    pub meta: BTreeMap<String, Value>,
}

/// A URI template advertised by an MCP server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceTemplate {
    /// RFC 6570 URI template.
    pub uri_template: String,
    /// Programmatic template name.
    pub name: String,
    /// Optional display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional MIME type for matching resources.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional icon descriptors.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub icons: Vec<Value>,
    /// Namespaced template metadata.
    #[serde(rename = "_meta", skip_serializing_if = "BTreeMap::is_empty", default)]
    pub meta: BTreeMap<String, Value>,
}

/// One content item returned from reading an MCP resource.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum McpResourceContents {
    /// UTF-8 text resource contents.
    Text {
        /// Resource URI for this content item.
        uri: String,
        /// Optional MIME type.
        #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// Text payload.
        text: String,
        /// Namespaced content metadata.
        #[serde(rename = "_meta", skip_serializing_if = "BTreeMap::is_empty", default)]
        meta: BTreeMap<String, Value>,
    },
    /// Base64-encoded binary resource contents.
    Blob {
        /// Resource URI for this content item.
        uri: String,
        /// Optional MIME type.
        #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// Base64-encoded payload.
        blob: String,
        /// Namespaced content metadata.
        #[serde(rename = "_meta", skip_serializing_if = "BTreeMap::is_empty", default)]
        meta: BTreeMap<String, Value>,
    },
}

/// Result returned by an MCP resource reader.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceReadResult {
    /// Content items for the requested resource.
    pub contents: Vec<McpResourceContents>,
    /// Namespaced result metadata.
    #[serde(rename = "_meta", skip_serializing_if = "BTreeMap::is_empty", default)]
    pub meta: BTreeMap<String, Value>,
}

/// Metadata passed to an MCP resource handler.
#[derive(Debug, Clone, Default)]
pub struct McpResourceRequest {
    /// JSON-RPC request id for the request, when the transport exposes one.
    pub request_id: Option<Value>,
    /// Trusted transport request metadata, when served over a request transport.
    pub request: Option<crate::command::RequestContext>,
    /// Selected protocol version for the request.
    pub protocol_version: Option<String>,
    /// Complete request `_meta` object, when the client supplied one.
    pub request_meta: Option<Value>,
    /// Client capabilities copied from request metadata or initialization.
    pub client_capabilities: Option<Value>,
    /// Client responses for a retry after an input-required result.
    pub input_responses: Option<Value>,
    /// Opaque request state echoed by the client for a retry.
    pub request_state: Option<String>,
    /// Optional provider-neutral request and notification hook for the connected peer.
    pub peer: Option<crate::command::McpPeer>,
}

/// Request passed to an MCP resource reader.
#[derive(Debug, Clone)]
pub struct McpResourceReadRequest {
    /// URI requested by the client.
    pub uri: String,
    /// Shared request metadata.
    pub context: McpResourceRequest,
}

/// Request passed to an MCP resource subscription handler.
#[derive(Debug, Clone)]
pub struct McpResourceSubscriptionRequest {
    /// URI whose subscription state is changing.
    pub uri: String,
    /// Shared request metadata.
    pub context: McpResourceRequest,
}

/// Failure returned by an MCP resource handler.
#[derive(Debug, Clone)]
pub struct McpResourceError {
    /// JSON-RPC error code, shared with the native transport's signed 32-bit code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    pub data: Option<Value>,
}

/// Future returned by an MCP resource reader.
pub type McpResourceReadFuture =
    Pin<Box<dyn Future<Output = Result<McpResourceReadResult, McpResourceError>> + Send + 'static>>;

/// Future returned by an MCP resource subscription hook.
pub type McpResourceSubscriptionFuture =
    Pin<Box<dyn Future<Output = Result<(), McpResourceError>> + Send + 'static>>;

/// Async callback used to read resource contents.
#[derive(Clone)]
pub struct McpResourceReader(
    Arc<dyn Fn(McpResourceReadRequest) -> McpResourceReadFuture + Send + Sync>,
);

impl std::fmt::Debug for McpResourceReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpResourceReader").finish_non_exhaustive()
    }
}

impl McpResourceReader {
    /// Creates a resource reader from an async function.
    pub fn new<F, Fut>(reader: F) -> Self
    where
        F: Fn(McpResourceReadRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<McpResourceReadResult, McpResourceError>> + Send + 'static,
    {
        Self(Arc::new(move |request| Box::pin(reader(request))))
    }

    pub(super) async fn read(
        &self,
        request: McpResourceReadRequest,
    ) -> Result<McpResourceReadResult, McpResourceError> {
        (self.0)(request).await
    }
}

/// Async callback used to subscribe or unsubscribe from resource updates.
#[derive(Clone)]
pub struct McpResourceSubscriptionHandler(
    Arc<dyn Fn(McpResourceSubscriptionRequest) -> McpResourceSubscriptionFuture + Send + Sync>,
);

impl std::fmt::Debug for McpResourceSubscriptionHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpResourceSubscriptionHandler")
            .finish_non_exhaustive()
    }
}

impl McpResourceSubscriptionHandler {
    /// Creates a resource subscription hook from an async function.
    pub fn new<F, Fut>(handler: F) -> Self
    where
        F: Fn(McpResourceSubscriptionRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), McpResourceError>> + Send + 'static,
    {
        Self(Arc::new(move |request| Box::pin(handler(request))))
    }

    pub(super) async fn handle(
        &self,
        request: McpResourceSubscriptionRequest,
    ) -> Result<(), McpResourceError> {
        (self.0)(request).await
    }
}

/// Request passed to a modern MCP subscription listener.
#[derive(Debug, Clone)]
pub struct McpSubscriptionListenRequest {
    /// Requested notification families and options.
    pub notifications: Value,
    /// Resource URI filters requested by the client.
    pub resource_uris: Vec<String>,
    /// Last delivery cursor observed by the client.
    pub cursor: Option<String>,
    /// Shared request metadata.
    pub context: McpResourceRequest,
}

/// Stream returned by a modern MCP subscription listener.
pub type McpSubscriptionListenStream = BoxStream<'static, Value>;

/// Future returned by a modern MCP subscription listener.
pub type McpSubscriptionListenFuture = Pin<
    Box<
        dyn Future<Output = Result<McpSubscriptionListenStream, McpResourceError>> + Send + 'static,
    >,
>;

/// Async callback used to open a modern `subscriptions/listen` stream.
#[derive(Clone)]
pub struct McpSubscriptionListenHandler(
    Arc<dyn Fn(McpSubscriptionListenRequest) -> McpSubscriptionListenFuture + Send + Sync>,
);

impl std::fmt::Debug for McpSubscriptionListenHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpSubscriptionListenHandler")
            .finish_non_exhaustive()
    }
}

impl McpSubscriptionListenHandler {
    /// Creates a modern subscription listener from an async function.
    pub fn new<F, Fut>(handler: F) -> Self
    where
        F: Fn(McpSubscriptionListenRequest) -> Fut + Send + Sync + 'static,
        Fut:
            Future<Output = Result<McpSubscriptionListenStream, McpResourceError>> + Send + 'static,
    {
        Self(Arc::new(move |request| Box::pin(handler(request))))
    }

    pub(super) async fn listen(
        &self,
        request: McpSubscriptionListenRequest,
    ) -> Result<McpSubscriptionListenStream, McpResourceError> {
        (self.0)(request).await
    }
}

/// Resources and handlers owned by the MCP host runtime.
#[derive(Debug, Clone, Default)]
pub struct McpResourceRegistry {
    /// Static resources returned by `resources/list`.
    pub resources: Vec<McpResource>,
    /// Static URI templates returned by `resources/templates/list`.
    pub templates: Vec<McpResourceTemplate>,
    /// Optional handler for `resources/read`.
    pub read: Option<McpResourceReader>,
    /// Optional handler for `resources/subscribe`.
    pub subscribe: Option<McpResourceSubscriptionHandler>,
    /// Optional handler for `resources/unsubscribe`.
    pub unsubscribe: Option<McpResourceSubscriptionHandler>,
    /// Optional handler for modern `subscriptions/listen`.
    pub listen: Option<McpSubscriptionListenHandler>,
}

/// Options for the MCP server.
#[derive(Debug, Clone, Default)]
pub struct McpServeOptions {
    /// Application hook for overriding the result envelope's error classification.
    pub result_mapper: Option<McpResultMapper>,
    /// CLI version string.
    pub version: Option<String>,
    /// Instructions describing how clients should use the server.
    pub instructions: Option<String>,
    /// Tool discovery and filtering configuration.
    pub tools: McpToolFilter,
    /// Additional server capabilities merged into initialize and discovery results.
    pub capabilities: BTreeMap<String, Value>,
    /// Host-owned resource registry and runtime handlers.
    pub resources: McpResourceRegistry,
    /// Optional provider-neutral peer request handle exposed to command contexts.
    pub peer: Option<crate::command::McpPeer>,
    /// Exact MCP standards served concurrently. Preference order is used for
    /// legacy fallback and modern discovery.
    pub standards: incurs_mcp_protocol::McpStandardSet,
}

/// Default for [`McpRemoteOptions::request_timeout`]: 60 seconds.
pub const DEFAULT_REMOTE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Most `tools/list` pages, or progressive catalog `search_tools` pages, a
/// remote projection follows before failing with `MCP_TOOL_LIMIT_EXCEEDED`.
pub const MAX_REMOTE_TOOL_PAGES: usize = 256;

/// Most tools a remote projection accepts before failing with
/// `MCP_TOOL_LIMIT_EXCEEDED`.
pub const MAX_REMOTE_TOOLS: usize = 16 * 1024;

/// Options for projecting a remote MCP server as incurs commands.
#[derive(Clone)]
pub struct McpRemoteOptions {
    /// Exact MCP standards the client accepts, in preference order.
    pub standards: incurs_mcp_protocol::McpStandardSet,
    /// Bearer token sent as `Authorization: Bearer <token>`.
    ///
    /// Supply the token alone, without the `Bearer ` prefix. Most private remote
    /// servers need nothing more than this.
    pub auth_token: Option<String>,
    /// Additional headers sent with every request, as `(name, value)` pairs.
    ///
    /// Plain strings rather than `http` types so this stays usable without
    /// taking a dependency on a specific `http` version. Malformed names or
    /// values are reported when the transport is built.
    pub headers: Vec<(String, String)>,
    /// The client every request goes through.
    ///
    /// When set, the portable [`crate::mcp_client::McpHttpClient`] connects
    /// through it on every target. When unset, native builds connect with
    /// `rmcp`, and wasm32 builds, which have no default client, fail with
    /// `HTTP_CLIENT_REQUIRED`.
    pub http_client: Option<crate::outbound::SharedHttpClient>,
    /// Deadline for one request of the portable
    /// [`crate::mcp_client::McpHttpClient`], from sending it until its
    /// response arrives, including waiting for a legacy stream lock. A catalog
    /// projection shares this deadline across all pages and detail requests.
    /// Defaults to [`DEFAULT_REMOTE_REQUEST_TIMEOUT`];
    /// `None` waits without a deadline.
    ///
    /// A request past its deadline fails with the retryable `MCP_TIMEOUT`
    /// error. The timer comes from [`crate::outbound::HttpClient::sleep`];
    /// a client without a timer enforces no deadline. The long-lived legacy
    /// SSE event stream itself has no deadline, only each request on it.
    pub request_timeout: Option<std::time::Duration>,
    /// Largest response body the portable client buffers, in bytes.
    /// Defaults to [`crate::outbound::DEFAULT_MAX_RESPONSE_BYTES`]; a longer
    /// body fails with `HTTP_BODY_TOO_LARGE`.
    pub max_response_bytes: usize,
}

impl Default for McpRemoteOptions {
    fn default() -> Self {
        Self {
            standards: incurs_mcp_protocol::McpStandardSet::default(),
            auth_token: None,
            headers: Vec::new(),
            http_client: None,
            request_timeout: Some(DEFAULT_REMOTE_REQUEST_TIMEOUT),
            max_response_bytes: crate::outbound::DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

impl std::fmt::Debug for McpRemoteOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRemoteOptions")
            .field("standards", &self.standards)
            .field("auth_token", &self.auth_token)
            .field("headers", &self.headers)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "dyn HttpClient"),
            )
            .field("request_timeout", &self.request_timeout)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish()
    }
}

impl McpRemoteOptions {
    /// Options carrying only a bearer token.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            auth_token: Some(token.into()),
            ..Self::default()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn rmcp_protocol_versions(
    standards: &incurs_mcp_protocol::McpStandardSet,
) -> Vec<rmcp::model::ProtocolVersion> {
    standards
        .versions()
        .iter()
        .map(|version| match version.as_str() {
            "2024-11-05" => rmcp::model::ProtocolVersion::V_2024_11_05,
            "2025-03-26" => rmcp::model::ProtocolVersion::V_2025_03_26,
            "2025-06-18" => rmcp::model::ProtocolVersion::V_2025_06_18,
            "2025-11-25" => rmcp::model::ProtocolVersion::V_2025_11_25,
            "2026-07-28" => rmcp::model::ProtocolVersion::V_2026_07_28,
            version => unreachable!("McpStandardSet admitted unknown standard {version}"),
        })
        .collect()
}

/// Returns whether a tool name passes MCP include/exclude filters.
pub fn matches_tool_filter(name: &str, filter: &McpToolFilter) -> bool {
    fn matches(pattern: &str, value: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        let parts = pattern.split('*').collect::<Vec<_>>();
        if parts.len() == 1 {
            return pattern == value;
        }
        let mut offset = 0;
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let Some(found) = value[offset..].find(part) else {
                return false;
            };
            if index == 0 && !pattern.starts_with('*') && found != 0 {
                return false;
            }
            offset += found + part.len();
        }
        pattern.ends_with('*') || parts.last().is_some_and(|part| value.ends_with(part))
    }

    let included =
        filter.include.is_empty() || filter.include.iter().any(|pattern| matches(pattern, name));
    let excluded = filter.exclude.iter().any(|pattern| matches(pattern, name));
    included && !excluded
}

/// A remote tool definition, independent of the client that listed it.
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
struct RemoteTool {
    name: String,
    description: Option<String>,
    input_schema: Value,
    output_schema: Option<Value>,
    read_only: bool,
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
impl RemoteTool {
    /// Reads an MCP tool object.
    fn from_value(value: &Value) -> Result<Self, crate::errors::Error> {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| remote_error("MCP tool has no name"))?;
        let input_schema = value
            .get("inputSchema")
            .filter(|schema| schema.is_object())
            .cloned()
            .ok_or_else(|| remote_error(format!("MCP tool `{name}` has no inputSchema object")))?;
        Ok(Self {
            name: name.to_string(),
            description: value
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            input_schema,
            output_schema: value
                .get("outputSchema")
                .filter(|schema| schema.is_object())
                .cloned()
                .map(|schema| {
                    incurs_mcp_protocol::structured::restore_output_schema(
                        schema,
                        value.get("_meta").and_then(Value::as_object),
                    )
                }),
            read_only: value
                .pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                == Some(true),
        })
    }
}

#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
impl From<rmcp::model::Tool> for RemoteTool {
    fn from(tool: rmcp::model::Tool) -> Self {
        let wire = serde_json::to_value(&tool).unwrap_or(Value::Null);
        Self {
            name: tool.name.to_string(),
            description: tool.description.map(|description| description.to_string()),
            input_schema: Value::Object((*tool.input_schema).clone()),
            output_schema: tool.output_schema.map(|schema| {
                incurs_mcp_protocol::structured::restore_output_schema(
                    Value::Object((*schema).clone()),
                    wire.get("_meta").and_then(Value::as_object),
                )
            }),
            read_only: tool
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint)
                == Some(true),
        }
    }
}

/// A remote tool call result, reduced to what projection reads.
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
struct RemoteCallResult {
    is_error: bool,
    structured_content: Option<Value>,
    first_text: Option<String>,
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
impl RemoteCallResult {
    /// Reads an MCP `CallToolResult` object.
    fn from_value(value: &Value) -> Self {
        Self {
            is_error: value.get("isError").and_then(Value::as_bool) == Some(true),
            structured_content: value.get("structuredContent").cloned().map(|content| {
                incurs_mcp_protocol::structured::restore_structured_content(
                    content,
                    value.get("_meta").and_then(Value::as_object),
                )
            }),
            first_text: value
                .pointer("/content/0")
                .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
        }
    }

    /// The command data of a successful call: structured content, else the
    /// first text block parsed as JSON, else null.
    fn data(self) -> Value {
        self.structured_content.unwrap_or_else(|| {
            self.first_text
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null)
        })
    }

    /// The message of a failed call.
    fn failure_message(self) -> String {
        self.first_text
            .unwrap_or_else(|| "Remote MCP tool failed".to_string())
    }
}

#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
impl From<rmcp::model::CallToolResult> for RemoteCallResult {
    fn from(result: rmcp::model::CallToolResult) -> Self {
        let wire = serde_json::to_value(&result).unwrap_or(Value::Null);
        Self {
            is_error: result.is_error == Some(true),
            first_text: result
                .content
                .first()
                .and_then(|content| content.as_text())
                .map(|text| text.text.clone()),
            structured_content: result.structured_content.map(|content| {
                incurs_mcp_protocol::structured::restore_structured_content(
                    content,
                    wire.get("_meta").and_then(Value::as_object),
                )
            }),
        }
    }
}

/// A connected MCP client that projected remote commands call through.
///
/// Native builds implement it with `rmcp`; every target implements it with
/// the portable [`crate::mcp_client::McpHttpClient`].
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
#[async_trait::async_trait]
#[allow(
    clippy::double_must_use,
    reason = "async_trait annotates generated futures"
)]
trait RemoteToolClient: Send + Sync {
    /// Lists every tool the server exposes.
    async fn remote_list_tools(&self) -> Result<Vec<RemoteTool>, crate::errors::Error>;

    /// Host timer for the complete projection, when one is available.
    fn remote_deadline(&self) -> Option<(std::time::Duration, crate::outbound::Sleep)> {
        None
    }

    /// Reads one tool object returned by a progressive catalog.
    fn remote_parse_tool(&self, value: Value) -> Result<RemoteTool, crate::errors::Error>;

    /// Calls one tool. A failed request keeps its code, retryability, and
    /// hint as [`crate::errors::Error::Incur`].
    async fn remote_call_tool(
        &self,
        name: String,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<RemoteCallResult, crate::errors::Error>;
}

#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
#[async_trait::async_trait]
impl RemoteToolClient for rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo> {
    fn remote_deadline(&self) -> Option<(std::time::Duration, crate::outbound::Sleep)> {
        Some((
            DEFAULT_REMOTE_REQUEST_TIMEOUT,
            Box::pin(tokio::time::sleep(DEFAULT_REMOTE_REQUEST_TIMEOUT)),
        ))
    }

    async fn remote_list_tools(&self) -> Result<Vec<RemoteTool>, crate::errors::Error> {
        let mut tools = Vec::new();
        let mut cursor = None;
        for page in 1..=MAX_REMOTE_TOOL_PAGES {
            let params = cursor.clone().map(|cursor| {
                rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
            });
            let result = rmcp::service::Peer::<rmcp::RoleClient>::list_tools(self, params)
                .await
                .map_err(rmcp_service_error)?;
            if result.tools.len() > MAX_REMOTE_TOOLS.saturating_sub(tools.len()) {
                return Err(tool_limit_error(format!(
                    "MCP tool catalog lists more than {MAX_REMOTE_TOOLS} tools"
                )));
            }
            tools.extend(result.tools.into_iter().map(RemoteTool::from));
            match result.next_cursor {
                Some(next) if cursor.as_deref() == Some(next.as_str()) => {
                    return Err(remote_error("tools/list returned a non-advancing cursor"));
                }
                Some(_) if page == MAX_REMOTE_TOOL_PAGES => {
                    return Err(tool_limit_error(format!(
                        "tools/list returned more than {MAX_REMOTE_TOOL_PAGES} pages"
                    )));
                }
                Some(next) => cursor = Some(next),
                None => return Ok(tools),
            }
        }
        unreachable!("the final page returns a result or the page-limit error")
    }

    fn remote_parse_tool(&self, value: Value) -> Result<RemoteTool, crate::errors::Error> {
        serde_json::from_value::<rmcp::model::Tool>(value)
            .map(RemoteTool::from)
            .map_err(remote_error)
    }

    async fn remote_call_tool(
        &self,
        name: String,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<RemoteCallResult, crate::errors::Error> {
        rmcp::service::Peer::<rmcp::RoleClient>::call_tool(
            self,
            rmcp::model::CallToolRequestParams::new(name).with_arguments(arguments),
        )
        .await
        .map(RemoteCallResult::from)
        .map_err(rmcp_service_error)
    }
}

/// A coded remote error with the given code and retryability, whose message
/// is `error`'s display text.
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
fn coded_remote_error(
    code: &str,
    retryable: bool,
    hint: Option<String>,
    error: impl std::fmt::Display,
) -> crate::errors::Error {
    crate::errors::Error::Incur(crate::errors::IncurError {
        message: error.to_string(),
        code: code.to_string(),
        hint,
        retryable,
        exit_code: None,
        cause: None,
    })
}

/// Converts an `rmcp` request failure into a coded error whose message is
/// the failure's display text.
#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
fn rmcp_service_error(error: rmcp::ServiceError) -> crate::errors::Error {
    use rmcp::ServiceError;

    let (code, retryable, hint) = match &error {
        // A JSON-RPC error response.
        ServiceError::McpError(_) => ("MCP_JSONRPC_ERROR", false, None),
        // The request could not be sent. rmcp reports why only as text.
        ServiceError::TransportSend(_) => ("MCP_TRANSPORT_ERROR", true, None),
        // The connection closed under the request.
        ServiceError::TransportClosed => ("MCP_TRANSPORT_ERROR", true, None),
        // No response within rmcp's own deadline.
        ServiceError::Timeout { .. } => ("MCP_TIMEOUT", true, None),
        // The request was cancelled locally.
        ServiceError::Cancelled { .. } => ("MCP_TRANSPORT_ERROR", false, None),
        // A response the protocol does not allow.
        _ => ("MCP_PROTOCOL_ERROR", false, None),
    };
    coded_remote_error(code, retryable, hint, error)
}

/// Converts an `rmcp` connection failure into a coded error whose message is
/// the failure's display text.
#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
fn rmcp_initialize_error(error: rmcp::service::ClientInitializeError) -> crate::errors::Error {
    use rmcp::service::ClientInitializeError;

    let (code, retryable, hint) = match &error {
        // A JSON-RPC error response to the lifecycle request.
        ClientInitializeError::JsonRpcError(_) => ("MCP_JSONRPC_ERROR", false, None),
        // The request could not be sent. rmcp reports why only as text.
        ClientInitializeError::TransportError { .. } => ("MCP_TRANSPORT_ERROR", true, None),
        // The connection closed during the lifecycle.
        ClientInitializeError::ConnectionClosed(_) => ("MCP_TRANSPORT_ERROR", true, None),
        // No common standard.
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => (
            "MCP_NEGOTIATION_FAILED",
            false,
            Some(
                "Enable an MCP standard the server supports in McpRemoteOptions::standards"
                    .to_string(),
            ),
        ),
        // The connection was cancelled locally.
        ClientInitializeError::Cancelled => ("MCP_TRANSPORT_ERROR", false, None),
        // A lifecycle response the protocol does not allow.
        _ => ("MCP_PROTOCOL_ERROR", false, None),
    };
    coded_remote_error(code, retryable, hint, error)
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
#[async_trait::async_trait]
impl RemoteToolClient for crate::mcp_client::McpHttpClient {
    fn remote_deadline(&self) -> Option<(std::time::Duration, crate::outbound::Sleep)> {
        self.deadline()
    }

    async fn remote_list_tools(&self) -> Result<Vec<RemoteTool>, crate::errors::Error> {
        let tools = self.list_tools().await?;
        tools.iter().map(RemoteTool::from_value).collect()
    }

    fn remote_parse_tool(&self, value: Value) -> Result<RemoteTool, crate::errors::Error> {
        RemoteTool::from_value(&value)
    }

    async fn remote_call_tool(
        &self,
        name: String,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<RemoteCallResult, crate::errors::Error> {
        self.call_tool(&name, arguments)
            .await
            .map(|result| RemoteCallResult::from_value(&result))
            .map_err(crate::errors::Error::from)
    }
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
struct RemoteToolHandler {
    client: std::sync::Arc<dyn RemoteToolClient>,
    tool: String,
    wrapper: Option<String>,
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
#[async_trait::async_trait]
impl crate::command::CommandHandler for RemoteToolHandler {
    async fn run(&self, ctx: crate::command::CommandContext) -> crate::output::CommandResult {
        let mut arguments = serde_json::Map::new();
        for source in [ctx.args, ctx.options] {
            if let Some(values) = source.as_object() {
                arguments.extend(values.clone());
            }
        }
        let (name, arguments) = if let Some(wrapper) = &self.wrapper {
            (
                wrapper.clone(),
                serde_json::Map::from_iter([
                    ("name".to_string(), Value::String(self.tool.clone())),
                    ("arguments".to_string(), Value::Object(arguments)),
                ]),
            )
        } else {
            (self.tool.clone(), arguments)
        };
        match self.client.remote_call_tool(name, arguments).await {
            // The tool ran and succeeded.
            Ok(result) if !result.is_error => crate::output::CommandResult::Ok {
                data: result.data(),
                cta: None,
                exit_code: None,
            },
            // The tool ran and reported a failure.
            Ok(result) => crate::output::CommandResult::Error {
                code: "REMOTE_MCP_ERROR".to_string(),
                message: result.failure_message(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
            // The request failed or the server rejected it: keep its code
            // and retryability.
            Err(crate::errors::Error::Incur(error)) => crate::output::CommandResult::Error {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
                exit_code: Some(error.exit_code.unwrap_or(1)),
                cta: None,
            },
            // An uncoded failure.
            Err(error) => crate::output::CommandResult::Error {
                code: "REMOTE_MCP_ERROR".to_string(),
                message: error.to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

/// Connects to a remote MCP-over-HTTP server and projects its tools as commands.
///
/// Native builds connect with `rmcp`. wasm32 builds have no default HTTP
/// client, so this fails there with `HTTP_CLIENT_REQUIRED`; use
/// [`remote_commands_with`] and set [`McpRemoteOptions::http_client`].
#[cfg(feature = "http")]
pub async fn remote_commands(
    uri: impl Into<String>,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    remote_commands_with(uri, &McpRemoteOptions::default()).await
}

/// Connects to a remote MCP-over-HTTP server using explicit exact standards
/// and projects its tools as commands.
///
/// With [`McpRemoteOptions::http_client`] set, the portable
/// [`crate::mcp_client::McpHttpClient`] connects through that client on every
/// target. Without it, native builds connect with `rmcp` and wasm32 builds
/// fail with `HTTP_CLIENT_REQUIRED`. Both clients project identical commands.
#[cfg(feature = "http")]
pub async fn remote_commands_with(
    uri: impl Into<String>,
    options: &McpRemoteOptions,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    let uri = uri.into();
    #[cfg(not(target_arch = "wasm32"))]
    if options.http_client.is_none() {
        use rmcp::transport::StreamableHttpClientTransport;
        use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

        let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
        if let Some(token) = &options.auth_token {
            config = config.auth_header(token.clone());
        }
        if !options.headers.is_empty() {
            config = config.custom_headers(remote_header_map(&options.headers)?);
        }

        return remote_commands_from_transport(
            StreamableHttpClientTransport::from_config(config),
            options,
        )
        .await;
    }
    let client = crate::mcp_client::McpHttpClient::connect(&uri, options).await?;
    remote_commands_from_client(client).await
}

/// Projects the tools of a connected portable client as commands.
///
/// Each command calls its tool through `client`. A server that exposes the
/// incurs progressive catalog (`search_tools`, `get_tool_details`,
/// `call_read_tool`, `call_write_tool`) is expanded into its underlying tools.
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
pub async fn remote_commands_from_client(
    client: crate::mcp_client::McpHttpClient,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    project_remote_commands(std::sync::Arc::new(client)).await
}

/// Converts plain `(name, value)` pairs into the transport's header map.
///
/// Reports the offending header by name, since a rejected header is otherwise
/// indistinguishable from an authentication failure at the far end.
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
fn remote_header_map(
    headers: &[(String, String)],
) -> Result<std::collections::HashMap<http::HeaderName, http::HeaderValue>, crate::errors::Error> {
    let mut map = std::collections::HashMap::with_capacity(headers.len());
    for (name, value) in headers {
        let header = http::HeaderName::try_from(name.as_str()).map_err(|error| {
            crate::errors::Error::Incur(crate::errors::IncurError {
                message: format!("`{name}` is not a valid header name"),
                code: "INVALID_HEADER".to_string(),
                hint: None,
                retryable: false,
                exit_code: None,
                cause: Some(Box::new(error)),
            })
        })?;
        let parsed = http::HeaderValue::from_str(value).map_err(|error| {
            crate::errors::Error::Incur(crate::errors::IncurError {
                message: format!("the value for header `{name}` is not valid"),
                code: "INVALID_HEADER".to_string(),
                hint: None,
                retryable: false,
                exit_code: None,
                cause: Some(Box::new(error)),
            })
        })?;
        map.insert(header, parsed);
    }
    Ok(map)
}

#[cfg(all(
    any(feature = "http", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
pub(crate) async fn remote_commands_from_transport<T, E, A>(
    transport: T,
    options: &McpRemoteOptions,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error>
where
    T: rmcp::transport::IntoTransport<rmcp::RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    use rmcp::model::ClientInfo;
    use rmcp::{ClientLifecycleMode, ClientServiceExt};

    let versions = rmcp_protocol_versions(&options.standards);
    let preferred_versions: Vec<_> = versions
        .iter()
        .filter(|version| version.as_str() == "2026-07-28")
        .cloned()
        .collect();
    let legacy_version = versions
        .into_iter()
        .find(|version| version.as_str() != "2026-07-28");
    let lifecycle = if preferred_versions.is_empty() {
        ClientLifecycleMode::Initialize
    } else if legacy_version.is_none() {
        ClientLifecycleMode::Discover { preferred_versions }
    } else {
        ClientLifecycleMode::Auto {
            preferred_versions,
            legacy_version: legacy_version.clone(),
        }
    };
    let client = ClientInfo::default()
        .with_protocol_version(legacy_version.unwrap_or(rmcp::model::ProtocolVersion::V_2026_07_28))
        .serve_with_lifecycle(transport, lifecycle)
        .await
        .map_err(rmcp_initialize_error)?;
    project_remote_commands(std::sync::Arc::new(client)).await
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
async fn project_remote_commands(
    client: std::sync::Arc<dyn RemoteToolClient>,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    let work = std::pin::pin!(project_remote_commands_unbounded(std::sync::Arc::clone(
        &client
    )));
    if let Some((after, timer)) = client.remote_deadline() {
        match futures::future::select(work, timer).await {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right(((), _)) => {
                Err(crate::mcp_client::McpClientError::Timeout { after }.into())
            }
        }
    } else {
        work.await
    }
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
async fn project_remote_commands_unbounded(
    client: std::sync::Arc<dyn RemoteToolClient>,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    let listed = client.remote_list_tools().await?;
    let progressive = {
        let names = listed
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        listed.len() == 4
            && [
                "search_tools",
                "get_tool_details",
                "call_read_tool",
                "call_write_tool",
            ]
            .iter()
            .all(|name| names.contains(name))
    };
    let tools = if progressive {
        discover_remote_tools(client.as_ref()).await?
    } else {
        listed
    };
    let mut commands = std::collections::BTreeMap::new();
    for tool in tools {
        let required = tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<std::collections::HashSet<_>>();
        let fields = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|properties| properties.iter())
            .map(|(name, schema)| remote_field(name, schema, required.contains(name)))
            .collect();
        let name = tool.name;
        commands.insert(
            name.clone(),
            crate::command::CommandDef {
                name: name.clone(),
                description: tool.description,
                args_fields: Vec::new(),
                options_fields: fields,
                env_fields: Vec::new(),
                aliases: std::collections::HashMap::new(),
                command_aliases: Vec::new(),
                examples: Vec::new(),
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(RemoteToolHandler {
                    client: std::sync::Arc::clone(&client),
                    wrapper: progressive.then(|| {
                        if tool.read_only {
                            "call_read_tool".to_string()
                        } else {
                            "call_write_tool".to_string()
                        }
                    }),
                    tool: name,
                }),
                middleware: Vec::new(),
                output_schema: tool.output_schema,
                raw: false,
                hidden: false,
            },
        );
    }
    Ok(commands)
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
async fn discover_remote_tools(
    client: &dyn RemoteToolClient,
) -> Result<Vec<RemoteTool>, crate::errors::Error> {
    let mut tools = Vec::new();
    let mut offset = 0_u64;
    let mut pages = 0_usize;
    loop {
        pages += 1;
        let search = client
            .remote_call_tool(
                "search_tools".to_string(),
                serde_json::Map::from_iter([
                    ("query".to_string(), Value::String(String::new())),
                    ("limit".to_string(), Value::from(20)),
                    ("offset".to_string(), Value::from(offset)),
                ]),
            )
            .await?;
        let value = remote_result_value(search)?;
        for name in value["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["name"].as_str())
        {
            if tools.len() >= MAX_REMOTE_TOOLS {
                return Err(tool_limit_error(format!(
                    "MCP tool catalog lists more than {MAX_REMOTE_TOOLS} tools"
                )));
            }
            let details = client
                .remote_call_tool(
                    "get_tool_details".to_string(),
                    serde_json::Map::from_iter([(
                        "name".to_string(),
                        Value::String(name.to_string()),
                    )]),
                )
                .await?;
            tools.push(client.remote_parse_tool(remote_result_value(details)?)?);
        }
        let Some(next) = value.get("nextOffset").and_then(Value::as_u64) else {
            break;
        };
        if next <= offset {
            return Err(remote_error(
                "MCP tool catalog returned a non-advancing offset",
            ));
        }
        if pages >= MAX_REMOTE_TOOL_PAGES {
            return Err(tool_limit_error(format!(
                "MCP tool catalog has more than {MAX_REMOTE_TOOL_PAGES} pages"
            )));
        }
        offset = next;
    }
    Ok(tools)
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
fn remote_result_value(result: RemoteCallResult) -> Result<Value, crate::errors::Error> {
    if result.is_error {
        return Err(coded_remote_error(
            "REMOTE_MCP_ERROR",
            false,
            None,
            result.failure_message(),
        ));
    }
    Ok(result.data())
}

/// A remote response that violates the MCP tool contract.
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
fn remote_error(error: impl std::fmt::Display) -> crate::errors::Error {
    coded_remote_error("MCP_PROTOCOL_ERROR", false, None, error)
}

/// A remote catalog past [`MAX_REMOTE_TOOL_PAGES`] or [`MAX_REMOTE_TOOLS`].
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
fn tool_limit_error(message: String) -> crate::errors::Error {
    coded_remote_error("MCP_TOOL_LIMIT_EXCEEDED", false, None, message)
}

#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
fn remote_field(name: &str, schema: &Value, required: bool) -> FieldMeta {
    let field_type = match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => crate::schema::FieldType::Boolean,
        Some("integer" | "number") => crate::schema::FieldType::Number,
        Some("array") => crate::schema::FieldType::Array(Box::new(crate::schema::FieldType::Value)),
        Some("object") => crate::schema::FieldType::Value,
        _ => crate::schema::FieldType::String,
    };
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .map(|description| &*Box::leak(description.to_string().into_boxed_str()));
    FieldMeta {
        name,
        cli_name: crate::schema::to_kebab(name),
        description,
        field_type,
        required,
        default: schema.get("default").cloned(),
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

// ---------------------------------------------------------------------------
// Tool collection
// ---------------------------------------------------------------------------

/// Recursively collects leaf commands as MCP tool entries.
///
/// Groups are traversed but not emitted as tools — only leaf commands become
/// tools. Tool names use underscore-joined path segments (e.g. `deploy_app`).
pub fn collect_tools(
    commands: &BTreeMap<String, CommandEntry>,
    prefix: &[String],
) -> Vec<ToolEntry> {
    let mut result: Vec<ToolEntry> = Vec::new();

    for (name, entry) in commands {
        let mut path = prefix.to_vec();
        path.push(name.clone());

        if entry.is_group {
            result.extend(collect_tools(&entry.commands, &path));
        } else {
            let tool_name = path.join("_");
            let input_schema = entry
                .input_schema
                .clone()
                .unwrap_or_else(|| build_tool_schema(&entry.args_fields, &entry.options_fields));
            result.push(ToolEntry {
                name: tool_name,
                description: entry.description.clone(),
                input_schema,
                output_schema: entry.output_schema.clone(),
            });
        }
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Builds a merged JSON Schema from args and options field metadata.
pub(crate) fn build_tool_schema(
    args_fields: &[FieldMeta],
    options_fields: &[FieldMeta],
) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<String> = Vec::new();

    for field in args_fields.iter().chain(options_fields.iter()) {
        let mut prop = serde_json::Map::new();
        prop.insert(
            "type".to_string(),
            serde_json::Value::String(field_type_to_json_type(&field.field_type)),
        );
        if let crate::schema::FieldType::Enum(values) = &field.field_type {
            prop.insert(
                "enum".to_string(),
                serde_json::Value::Array(
                    values
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(desc) = field.description {
            prop.insert(
                "description".to_string(),
                serde_json::Value::String(desc.to_string()),
            );
        }
        if let Some(default) = &field.default {
            prop.insert("default".to_string(), default.clone());
        }
        properties.insert(field.name.to_string(), serde_json::Value::Object(prop));

        if field.required {
            required.push(field.name.to_string());
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    schema.insert(
        "properties".to_string(),
        serde_json::Value::Object(properties),
    );
    if !required.is_empty() {
        schema.insert(
            "required".to_string(),
            serde_json::Value::Array(
                required
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }

    serde_json::Value::Object(schema)
}

/// Maps a FieldType to its JSON Schema type string.
fn field_type_to_json_type(ft: &crate::schema::FieldType) -> String {
    use crate::schema::FieldType;
    match ft {
        FieldType::String => "string".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array(_) => "array".to_string(),
        FieldType::Enum(_) => "string".to_string(),
        FieldType::Count => "number".to_string(),
        FieldType::Value => "string".to_string(),
    }
}

// ---------------------------------------------------------------------------
// MCP Server (every target except wasm32)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod server {
    use std::borrow::Cow;
    use std::sync::Arc;

    use serde_json::{Map, Value};

    use rmcp::ErrorData as McpError;
    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, CustomNotification,
        CustomRequest, Implementation, InitializeRequestParams, InitializeResult,
        InputRequiredResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ServerCapabilities,
        ServerInfo, ServerNotification, ServerRequest, SubscribeRequestParams, Tool,
        UnsubscribeRequestParams,
    };
    use rmcp::service::{RequestContext, RoleServer};

    #[cfg(test)]
    use crate::command::McpResultContent;
    #[cfg(test)]
    use crate::tool::ToolCallOutcome;
    use crate::tool::{EnvironmentSource, ToolCallControl, ToolEvent, ToolEventSink};

    use super::McpServeOptions;
    pub(super) use super::shared::ServerSource;
    use super::shared::{self, CallContext, ResourceContext, ToolServer};

    fn non_empty_json_object(value: Value) -> Option<Value> {
        if value.as_object().is_some_and(serde_json::Map::is_empty) {
            None
        } else {
            Some(value)
        }
    }

    fn context_protocol_version(context: &RequestContext<RoleServer>) -> Option<String> {
        context
            .protocol_version()
            .or_else(|| context.meta.protocol_version())
            .map(|version| version.as_str().to_string())
    }

    fn context_meta_value(context: &RequestContext<RoleServer>) -> Option<Value> {
        serde_json::to_value(&context.meta)
            .ok()
            .and_then(non_empty_json_object)
    }

    fn context_client_capabilities(context: &RequestContext<RoleServer>) -> Option<Value> {
        context
            .meta
            .client_capabilities()
            .and_then(|capabilities| serde_json::to_value(capabilities).ok())
    }

    fn resource_context<I: serde::Serialize>(
        protocol_version: Option<String>,
        request_meta: Option<Value>,
        client_capabilities: Option<Value>,
        input_responses: &Option<I>,
        request_state: Option<String>,
        peer: Option<crate::command::McpPeer>,
    ) -> ResourceContext {
        ResourceContext {
            request_id: None,
            request: None,
            protocol_version,
            request_meta,
            client_capabilities,
            input_responses: input_responses
                .as_ref()
                .and_then(|responses| serde_json::to_value(responses).ok()),
            request_state,
            peer,
        }
    }

    fn params_with_meta(params: Option<Value>, meta: Option<Value>) -> Option<Value> {
        let Some(meta) = meta else {
            return params;
        };
        match params {
            Some(Value::Object(mut object)) => {
                object.insert("_meta".to_string(), meta);
                Some(Value::Object(object))
            }
            Some(value) => {
                let mut object = Map::new();
                object.insert("value".to_string(), value);
                object.insert("_meta".to_string(), meta);
                Some(Value::Object(object))
            }
            None => {
                let mut object = Map::new();
                object.insert("_meta".to_string(), meta);
                Some(Value::Object(object))
            }
        }
    }

    fn native_peer(peer: rmcp::service::Peer<RoleServer>) -> crate::command::McpPeer {
        let request_peer = peer.clone();
        let notify_peer = peer;
        crate::command::McpPeer::with_notify(
            move |request: crate::command::McpPeerRequest| {
                let peer = request_peer.clone();
                async move {
                    let params = params_with_meta(request.params, request.meta);
                    let result = peer
                        .send_request(ServerRequest::CustomRequest(CustomRequest::new(
                            request.method,
                            params,
                        )))
                        .await
                        .map_err(|error| crate::command::McpPeerError {
                            code: "MCP_PEER_REQUEST_FAILED".to_string(),
                            message: error.to_string(),
                            data: None,
                        })?;
                    serde_json::to_value(result).map_err(|error| crate::command::McpPeerError {
                        code: "MCP_PEER_RESPONSE_SERIALIZATION_FAILED".to_string(),
                        message: error.to_string(),
                        data: None,
                    })
                }
            },
            move |notification: crate::command::McpPeerNotification| {
                let peer = notify_peer.clone();
                async move {
                    let params = params_with_meta(notification.params, notification.meta);
                    peer.send_notification(ServerNotification::CustomNotification(
                        CustomNotification::new(notification.method, params),
                    ))
                    .await
                    .map_err(|error| crate::command::McpPeerError {
                        code: "MCP_PEER_NOTIFICATION_FAILED".to_string(),
                        message: error.to_string(),
                        data: None,
                    })
                }
            },
        )
    }

    fn native_capabilities(tools: &ToolServer) -> ServerCapabilities {
        serde_json::from_value(tools.capabilities()).unwrap_or_else(|_| {
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build()
        })
    }

    fn resource_error(error: super::McpResourceError) -> McpError {
        McpError::new(
            rmcp::model::ErrorCode(error.code),
            error.message,
            error.data,
        )
    }

    async fn resource_call<T>(
        future: impl std::future::Future<Output = Result<T, super::McpResourceError>>,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<T, McpError> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(McpError::new(
                rmcp::model::ErrorCode(-32800), "Resource request cancelled", None,
            )),
            result = future => result.map_err(resource_error),
        }
    }

    /// Converts a shared `CallToolResult` wire value into the `rmcp` model.
    fn call_tool_result(value: Value) -> CallToolResult {
        serde_json::from_value(value).expect("shared CallToolResult wire value is valid")
    }

    /// Converts a shared `CallToolResponse` wire value into the `rmcp` model.
    fn call_tool_response(value: Value) -> CallToolResponse {
        if value.get("resultType") == Some(&Value::String("input_required".to_string())) {
            let result: InputRequiredResult = serde_json::from_value(value)
                .expect("shared InputRequiredResult wire value is valid");
            result.into()
        } else {
            call_tool_result(value).into()
        }
    }

    #[cfg(test)]
    pub(super) fn tool_result_success(
        name: &str,
        data: Value,
        cta: Option<crate::output::CtaBlock>,
        structured: bool,
        presentation: &[McpResultContent],
    ) -> CallToolResult {
        call_tool_result(shared::tool_result_success(
            name,
            data,
            cta,
            structured,
            presentation,
        ))
    }

    #[cfg(test)]
    mod error_tests;

    #[cfg(test)]
    fn tool_call_result(
        name: &str,
        outcome: ToolCallOutcome,
        structured: bool,
        presentation: &[McpResultContent],
    ) -> CallToolResult {
        call_tool_result(shared::tool_call_result(
            name,
            outcome,
            structured,
            presentation,
        ))
    }

    struct McpEventSink {
        peer: rmcp::service::Peer<RoleServer>,
        progress_token: Option<rmcp::model::ProgressToken>,
        count: tokio::sync::Mutex<u64>,
    }

    #[async_trait::async_trait]
    impl ToolEventSink for McpEventSink {
        async fn emit(&self, event: ToolEvent) {
            let Some(progress_token) = self.progress_token.clone() else {
                return;
            };
            let message = shared::progress_message(event);
            let mut count = self.count.lock().await;
            *count += 1;
            let _ = self
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(progress_token, *count as f64)
                        .with_message(message),
                )
                .await;
        }
    }

    // -----------------------------------------------------------------------
    // ServerHandler implementation
    // -----------------------------------------------------------------------

    /// The MCP server handler. Implements `rmcp::handler::server::ServerHandler`
    /// to respond to `initialize`, `tools/list`, and `tools/call` requests.
    #[derive(Clone)]
    pub(crate) struct IncurMcpServer {
        /// Shared tool listing and invocation.
        tools: ToolServer,
        /// Pre-built list of `rmcp::model::Tool` for `tools/list` responses.
        tool_list: Arc<Vec<Tool>>,
    }

    impl IncurMcpServer {
        pub(super) fn new(
            source: &ServerSource<'_>,
            options: &McpServeOptions,
        ) -> Result<Self, crate::errors::Error> {
            let tools = ToolServer::new(source, options)?;
            let tool_list = tools
                .tool_list
                .iter()
                .map(|tool| {
                    serde_json::from_value(tool.clone()).expect("shared Tool wire value is valid")
                })
                .collect();
            Ok(IncurMcpServer {
                tools,
                tool_list: Arc::new(tool_list),
            })
        }

        fn initialize_protocol_versions(&self) -> Vec<ProtocolVersion> {
            super::rmcp_protocol_versions(&self.tools.standards)
                .into_iter()
                .filter(|version| version.as_str() != "2026-07-28")
                .collect()
        }
    }

    impl ServerHandler for IncurMcpServer {
        fn get_info(&self) -> ServerInfo {
            let protocol = self
                .initialize_protocol_versions()
                .into_iter()
                .next()
                .unwrap_or(ProtocolVersion::V_2026_07_28);
            let info = ServerInfo::new(native_capabilities(&self.tools))
                .with_protocol_version(protocol)
                .with_server_info(Implementation::new(
                    self.tools.server_name.clone(),
                    self.tools.server_version.clone(),
                ));
            if let Some(instructions) = &self.tools.instructions {
                info.with_instructions(instructions.clone())
            } else {
                info
            }
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Owned(super::rmcp_protocol_versions(&self.tools.standards))
        }

        fn initialize(
            &self,
            request: InitializeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<InitializeResult, McpError>> + Send + '_
        {
            context.peer.set_peer_info(request.clone());
            let initialize_supported = self.initialize_protocol_versions();
            let selected = initialize_supported
                .iter()
                .find(|version| version.as_str() == request.protocol_version.as_str())
                .cloned();
            let selected = if selected.is_none()
                && ProtocolVersion::KNOWN_VERSIONS.contains(&request.protocol_version)
            {
                None
            } else {
                selected.or_else(|| initialize_supported.first().cloned())
            };
            std::future::ready(match selected {
                Some(selected) => Ok(self.get_info().with_protocol_version(selected)),
                None => Err(McpError::unsupported_protocol_version(
                    request.protocol_version,
                    &initialize_supported,
                )),
            })
        }

        fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_
        {
            let tools = (*self.tool_list).clone();
            let mut result = ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
                ..Default::default()
            };
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() == "2026-07-28")
            {
                result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
            }
            std::future::ready(Ok(result))
        }

        fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_
        {
            let mut result = ListPromptsResult::default();
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() == "2026-07-28")
            {
                result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
            }
            std::future::ready(Ok(result))
        }

        fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send + '_
        {
            let mut result: ListResourcesResult =
                serde_json::from_value(self.tools.list_resources()).unwrap_or_default();
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() == "2026-07-28")
            {
                result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
            }
            std::future::ready(Ok(result))
        }

        fn list_resource_templates(
            &self,
            _request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_
        {
            let mut result: ListResourceTemplatesResult =
                serde_json::from_value(self.tools.list_resource_templates()).unwrap_or_default();
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() == "2026-07-28")
            {
                result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
            }
            std::future::ready(Ok(result))
        }

        fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ReadResourceResponse, McpError>> + Send + '_
        {
            let tools = self.tools.clone();
            let cancellation = context.ct.clone();
            let protocol_version = context_protocol_version(&context);
            let request_meta = context_meta_value(&context).or_else(|| {
                request
                    .meta
                    .as_ref()
                    .and_then(|meta| serde_json::to_value(meta).ok())
                    .and_then(non_empty_json_object)
            });
            let client_capabilities = context_client_capabilities(&context).or_else(|| {
                request_meta
                    .as_ref()
                    .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
                    .cloned()
            });
            let peer = native_peer(context.peer.clone());
            async move {
                let ctx = resource_context(
                    protocol_version,
                    request_meta,
                    client_capabilities,
                    &request.input_responses,
                    request.request_state.clone(),
                    Some(peer),
                );
                let value =
                    resource_call(tools.read_resource(request.uri, ctx), cancellation).await?;
                let result: ReadResourceResult = serde_json::from_value(value)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
                Ok(ReadResourceResponse::from(result))
            }
        }

        #[allow(deprecated)]
        fn subscribe(
            &self,
            request: SubscribeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
            let tools = self.tools.clone();
            let cancellation = context.ct.clone();
            let protocol_version = context
                .protocol_version()
                .map(|version| version.as_str().to_string());
            let peer = native_peer(context.peer.clone());
            async move {
                resource_call(
                    tools.set_resource_subscription(
                        request.uri,
                        true,
                        ResourceContext {
                            protocol_version,
                            peer: Some(peer),
                            ..ResourceContext::default()
                        },
                    ),
                    cancellation,
                )
                .await?;
                Ok(())
            }
        }

        #[allow(deprecated)]
        fn unsubscribe(
            &self,
            request: UnsubscribeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
            let tools = self.tools.clone();
            let cancellation = context.ct.clone();
            let protocol_version = context
                .protocol_version()
                .map(|version| version.as_str().to_string());
            let peer = native_peer(context.peer.clone());
            async move {
                resource_call(
                    tools.set_resource_subscription(
                        request.uri,
                        false,
                        ResourceContext {
                            protocol_version,
                            peer: Some(peer),
                            ..ResourceContext::default()
                        },
                    ),
                    cancellation,
                )
                .await?;
                Ok(())
            }
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResponse, McpError>> + Send + '_
        {
            let tools = self.tools.clone();
            let progress_token = context.meta.get_progress_token();
            #[cfg(feature = "http")]
            let transport_request =
                context
                    .extensions
                    .get::<axum::http::request::Parts>()
                    .map(|parts| crate::command::RequestContext {
                        headers: parts
                            .headers
                            .iter()
                            .filter_map(|(name, value)| {
                                value
                                    .to_str()
                                    .ok()
                                    .map(|value| (name.as_str().to_string(), value.to_string()))
                            })
                            .collect(),
                        method: parts.method.to_string(),
                        path: parts.uri.path().to_string(),
                    });
            #[cfg(not(feature = "http"))]
            let transport_request = None;
            let request_meta = context_meta_value(&context).or_else(|| {
                request
                    .meta
                    .as_ref()
                    .and_then(|meta| serde_json::to_value(meta).ok())
                    .and_then(non_empty_json_object)
            });
            let client_capabilities = context_client_capabilities(&context).or_else(|| {
                request_meta
                    .as_ref()
                    .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
                    .cloned()
            });
            let protocol_version =
                context_protocol_version(&context).unwrap_or_else(|| "2025-03-26".to_string());
            let peer = context.peer;
            let mcp_peer = native_peer(peer.clone());
            let cancellation = context.ct;

            async move {
                let result = tools
                    .call_tool(
                        request.name.to_string(),
                        request.arguments,
                        CallContext {
                            protocol_version,
                            request: transport_request,
                            request_meta,
                            client_capabilities,
                            input_responses: request
                                .input_responses
                                .clone()
                                .map(serde_json::to_value)
                                .transpose()
                                .ok()
                                .flatten(),
                            request_state: request.request_state.clone(),
                            peer: Some(mcp_peer),
                            control: ToolCallControl {
                                cancellation,
                                events: Some(Arc::new(McpEventSink {
                                    peer,
                                    progress_token,
                                    count: tokio::sync::Mutex::new(0),
                                })),
                            },
                            environment: EnvironmentSource::DeclaredHost,
                        },
                    )
                    .await
                    .map_err(|message| McpError::invalid_params(message, None))?;
                Ok(call_tool_response(result))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Starts a stdio MCP server that exposes CLI commands as tools.
    ///
    /// This function:
    /// 1. Walks the CLI command tree to collect leaf commands as tools.
    /// 2. Creates an `rmcp` server implementing `ServerHandler`.
    /// 3. Connects via stdio transport (stdin/stdout).
    /// 4. Blocks until the client disconnects.
    ///
    /// Each tool call executes through the shared transport-neutral
    /// [`crate::tool::ToolCatalog`].
    pub(super) async fn serve(
        source: ServerSource<'_>,
        options: &McpServeOptions,
    ) -> Result<(), crate::errors::Error> {
        use rmcp::ServiceExt;
        use rmcp::transport::io::stdio;

        let server = IncurMcpServer::new(&source, options)?;

        let transport = stdio();

        let running = server.serve(transport).await.map_err(|e| {
            crate::errors::Error::Other(Box::new(std::io::Error::other(format!(
                "MCP server failed to start: {e}"
            ))))
        })?;

        // Block until the client disconnects or the server is cancelled.
        let _quit_reason = running.waiting().await.map_err(|e| {
            crate::errors::Error::Other(Box::new(std::io::Error::other(format!(
                "MCP server task failed: {e}"
            ))))
        })?;

        Ok(())
    }

    #[cfg(feature = "http")]
    pub(crate) fn http_service(
        source: ServerSource<'_>,
        options: &McpServeOptions,
    ) -> Result<
        rmcp::transport::StreamableHttpService<
            IncurMcpServer,
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
        >,
        crate::errors::Error,
    > {
        use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

        let server = IncurMcpServer::new(&source, options)?;
        let mut config = StreamableHttpServerConfig::default();
        config.legacy_session_mode = false;
        Ok(StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            config,
        ))
    }
}

/// Builds a stateless MCP-over-HTTP service for a CLI.
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub(crate) fn http_service(
    cli: &crate::cli::Cli,
) -> Result<
    rmcp::transport::StreamableHttpService<
        impl rmcp::ServerHandler,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    >,
    crate::errors::Error,
> {
    server::http_service(server::ServerSource::from_cli(cli), &cli.mcp_options)
}

/// Starts a stdio MCP server for a complete CLI.
///
/// On wasm32 there is no stdio, so this returns an `MCP_STDIO_UNAVAILABLE`
/// error instead of serving.
pub async fn serve_cli(cli: &crate::cli::Cli) -> Result<(), crate::errors::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        server::serve(server::ServerSource::from_cli(cli), &cli.mcp_options).await
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = cli;
        Err(stdio_unavailable())
    }
}

/// Starts a stdio MCP server that exposes commands as tools.
///
/// Uses the `rmcp` crate for the actual MCP protocol implementation.
/// Each leaf command in the command tree becomes an MCP tool.
///
/// This is the public entry point. It accepts the CLI command tree directly
/// (rather than the standalone `mcp::CommandEntry` tree) so that it can
/// resolve `Arc<CommandDef>` references for command execution.
///
/// On wasm32 there is no stdio, so this returns an `MCP_STDIO_UNAVAILABLE`
/// error instead of serving.
pub async fn serve(
    name: &str,
    version: &str,
    commands: &std::collections::BTreeMap<String, crate::cli::CommandEntry>,
    root_middleware: &[crate::middleware::MiddlewareFn],
    env_fields: &[FieldMeta],
    options: &McpServeOptions,
) -> Result<(), crate::errors::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        server::serve(
            server::ServerSource::from_parts(name, version, commands, root_middleware, env_fields),
            options,
        )
        .await
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (
            name,
            version,
            commands,
            root_middleware,
            env_fields,
            options,
        );
        Err(stdio_unavailable())
    }
}

#[cfg(target_arch = "wasm32")]
fn stdio_unavailable() -> crate::errors::Error {
    crate::errors::Error::Incur(crate::errors::IncurError {
        message: "MCP over stdio is not available on wasm32".to_string(),
        code: "MCP_STDIO_UNAVAILABLE".to_string(),
        hint: Some("Serve tools through the host's HTTP adapter over ToolCatalog".to_string()),
        retryable: false,
        exit_code: None,
        cause: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, to_kebab};

    #[test]
    fn test_tool_result_success_presents_declared_image_content() {
        use crate::command::McpResultContent;

        let data = serde_json::json!({
            "preview": {
                "data": "aW1hZ2U=",
                "mimeType": "image/png"
            },
            "artifactUrl": "https://example.test/artifacts/ui.png"
        });
        let presentation = vec![McpResultContent::Image {
            data_pointer: "/preview/data".to_string(),
            mime_type_pointer: "/preview/mimeType".to_string(),
        }];

        let result = server::tool_result_success("visualize", data, None, true, &presentation);
        let encoded = serde_json::to_value(result.content).expect("content serializes");

        assert_eq!(encoded[0]["type"], "text");
        assert_eq!(encoded[1]["type"], "image");
        assert_eq!(encoded[1]["data"], "aW1hZ2U=");
        assert_eq!(encoded[1]["mimeType"], "image/png");
    }

    fn make_field(name: &'static str, ft: FieldType, required: bool) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: to_kebab(name),
            description: Some("A field"),
            field_type: ft,
            required,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_leaf(desc: &str) -> CommandEntry {
        CommandEntry {
            is_group: false,
            description: Some(desc.to_string()),
            commands: BTreeMap::new(),
            args_fields: vec![],
            options_fields: vec![],
            output_schema: None,
            input_schema: None,
        }
    }

    fn make_leaf_with_input_schema(desc: &str, input_schema: serde_json::Value) -> CommandEntry {
        CommandEntry {
            input_schema: Some(input_schema),
            ..make_leaf(desc)
        }
    }

    fn make_group(desc: &str, commands: BTreeMap<String, CommandEntry>) -> CommandEntry {
        CommandEntry {
            is_group: true,
            description: Some(desc.to_string()),
            commands,
            args_fields: vec![],
            options_fields: vec![],
            output_schema: None,
            input_schema: None,
        }
    }

    #[test]
    fn test_collect_tools_flat() {
        let mut commands = BTreeMap::new();
        commands.insert("deploy".to_string(), make_leaf("Deploy app"));
        commands.insert("status".to_string(), make_leaf("Show status"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "deploy");
        assert_eq!(tools[1].name, "status");
    }

    #[test]
    fn test_collect_tools_nested() {
        let mut sub = BTreeMap::new();
        sub.insert("app".to_string(), make_leaf("Deploy app"));
        sub.insert("config".to_string(), make_leaf("Deploy config"));

        let mut commands = BTreeMap::new();
        commands.insert("deploy".to_string(), make_group("Deploy group", sub));
        commands.insert("status".to_string(), make_leaf("Show status"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "deploy_app");
        assert_eq!(tools[1].name, "deploy_config");
        assert_eq!(tools[2].name, "status");
    }

    #[test]
    fn test_collect_tools_honors_input_schema_override() {
        let published = serde_json::json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {"type": "object"},
                },
            },
        });
        let mut commands = BTreeMap::new();
        commands.insert(
            "run".to_string(),
            make_leaf_with_input_schema("Run steps", published.clone()),
        );
        commands.insert("status".to_string(), make_leaf("Show status"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools[0].name, "run");
        assert_eq!(tools[0].input_schema, published);
        // The fallback path is unaffected: no override still derives from fields.
        assert_ne!(tools[1].input_schema, published);
    }

    #[test]
    fn test_collect_tools_sorted() {
        let mut commands = BTreeMap::new();
        commands.insert("zebra".to_string(), make_leaf("Z"));
        commands.insert("alpha".to_string(), make_leaf("A"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools[0].name, "alpha");
        assert_eq!(tools[1].name, "zebra");
    }

    #[test]
    fn test_build_tool_schema_basic() {
        let args = vec![make_field("target", FieldType::String, true)];
        let opts = vec![make_field("verbose", FieldType::Boolean, false)];

        let schema = build_tool_schema(&args, &opts);
        let obj = schema.as_object().unwrap();
        assert_eq!(obj["type"], "object");

        let props = obj["properties"].as_object().unwrap();
        assert!(props.contains_key("target"));
        assert!(props.contains_key("verbose"));

        let required = obj["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "target");
    }

    #[test]
    fn test_build_tool_schema_no_required() {
        let schema = build_tool_schema(&[], &[make_field("verbose", FieldType::Boolean, false)]);
        let obj = schema.as_object().unwrap();
        assert!(!obj.contains_key("required"));
    }

    #[test]
    fn test_field_type_to_json_type() {
        assert_eq!(field_type_to_json_type(&FieldType::String), "string");
        assert_eq!(field_type_to_json_type(&FieldType::Number), "number");
        assert_eq!(field_type_to_json_type(&FieldType::Boolean), "boolean");
        assert_eq!(
            field_type_to_json_type(&FieldType::Array(Box::new(FieldType::String))),
            "array"
        );
        assert_eq!(
            field_type_to_json_type(&FieldType::Enum(vec!["a".to_string()])),
            "string"
        );
        assert_eq!(field_type_to_json_type(&FieldType::Count), "number");
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn test_remote_commands_discovers_progressive_catalog() {
        struct Ping;

        #[async_trait::async_trait]
        impl crate::command::CommandHandler for Ping {
            async fn run(
                &self,
                _ctx: crate::command::CommandContext,
            ) -> crate::output::CommandResult {
                crate::output::CommandResult::Ok {
                    data: serde_json::json!({ "pong": true }),
                    cta: None,
                    exit_code: None,
                }
            }
        }

        let cli = crate::cli::Cli::create("remote-test").command(
            "ping",
            crate::command::CommandDef {
                name: "ping".to_string(),
                description: Some("Ping the server".to_string()),
                args_fields: Vec::new(),
                options_fields: Vec::new(),
                env_fields: Vec::new(),
                aliases: std::collections::HashMap::new(),
                command_aliases: Vec::new(),
                examples: Vec::new(),
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(Ping),
                middleware: Vec::new(),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "pong": { "type": "boolean" } },
                })),
                raw: false,
                hidden: false,
            },
        );
        let app = axum::Router::new().nest_service("/mcp", http_service(&cli).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let commands = remote_commands(format!("http://{addr}/mcp")).await.unwrap();
        server.abort();

        assert_eq!(commands.keys().cloned().collect::<Vec<_>>(), vec!["ping"]);
        assert_eq!(
            commands["ping"].description.as_deref(),
            Some("Ping the server")
        );
        assert!(commands["ping"].output_schema.is_some());
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn test_mcp_calls_use_tool_catalog_config_defaults() {
        use rmcp::ServiceExt;
        use rmcp::transport::StreamableHttpClientTransport;

        struct EchoOptions;

        #[async_trait::async_trait]
        impl crate::command::CommandHandler for EchoOptions {
            async fn run(
                &self,
                ctx: crate::command::CommandContext,
            ) -> crate::output::CommandResult {
                crate::output::CommandResult::Ok {
                    data: serde_json::json!({ "options": ctx.options }),
                    cta: None,
                    exit_code: None,
                }
            }
        }

        let config_path = std::env::temp_dir().join(format!(
            "incurs-mcp-catalog-defaults-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &config_path,
            serde_json::json!({
                "commands": {
                    "echo": {
                        "options": { "profile": "configured" }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        fn cli(discovery: McpDiscovery, config_path: &std::path::Path) -> crate::cli::Cli {
            let mut command = crate::command::CommandDef::build("echo", EchoOptions).done();
            command.options_fields = vec![make_field("profile", FieldType::String, false)];
            command.output_schema = Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "options": {
                        "type": "object",
                        "properties": { "profile": { "type": "string" } }
                    }
                }
            }));
            crate::cli::Cli::create("catalog-test")
                .config(crate::cli::ConfigOptions {
                    flag: "config".to_string(),
                    files: vec![config_path.to_string_lossy().into_owned()],
                })
                .mcp(McpServeOptions {
                    tools: McpToolFilter {
                        discovery,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .command("echo", command)
        }

        let direct_cli = cli(McpDiscovery::Direct, &config_path);
        let direct_app =
            axum::Router::new().nest_service("/mcp", http_service(&direct_cli).unwrap());
        let direct_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let direct_addr = direct_listener.local_addr().unwrap();
        let direct_server =
            tokio::spawn(async move { axum::serve(direct_listener, direct_app).await.unwrap() });
        let direct_client = ()
            .serve(StreamableHttpClientTransport::from_uri(format!(
                "http://{direct_addr}/mcp"
            )))
            .await
            .unwrap();
        let direct = direct_client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("echo")
                    .with_arguments(serde_json::Map::new()),
            )
            .await
            .unwrap();
        direct_server.abort();

        let progressive_cli = cli(McpDiscovery::Progressive, &config_path);
        let progressive_app =
            axum::Router::new().nest_service("/mcp", http_service(&progressive_cli).unwrap());
        let progressive_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let progressive_addr = progressive_listener.local_addr().unwrap();
        let progressive_server = tokio::spawn(async move {
            axum::serve(progressive_listener, progressive_app)
                .await
                .unwrap()
        });
        let progressive_client = ()
            .serve(StreamableHttpClientTransport::from_uri(format!(
                "http://{progressive_addr}/mcp"
            )))
            .await
            .unwrap();
        let progressive = progressive_client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("call_write_tool").with_arguments(
                    serde_json::Map::from_iter([
                        ("name".to_string(), serde_json::json!("echo")),
                        ("arguments".to_string(), serde_json::json!({})),
                    ]),
                ),
            )
            .await
            .unwrap();
        progressive_server.abort();
        let _ = std::fs::remove_file(config_path);

        assert_eq!(
            direct.structured_content.unwrap()["options"]["profile"],
            "configured"
        );
        assert_eq!(
            progressive.structured_content.unwrap()["options"]["profile"],
            "configured"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn native_duplex_transport_carries_metadata_resources_and_peer_requests() {
        use crate::command::{CommandContext, CommandDef, McpCommandOptions, McpPeerRequest};
        use crate::output::CommandResult;
        use rmcp::model::{
            CallToolRequestParams, ClientInfo, CustomRequest, CustomResult, ErrorData as McpError,
            Implementation, ProtocolVersion, ReadResourceRequestParams,
        };
        use rmcp::service::{RequestContext, RoleClient};
        use rmcp::{ClientHandler, ClientServiceExt, ServiceExt};

        struct ContextTool;

        #[async_trait::async_trait]
        impl crate::command::CommandHandler for ContextTool {
            async fn run(&self, ctx: CommandContext) -> CommandResult {
                let mcp = ctx.mcp.expect("MCP context");
                let peer = mcp.peer.clone().expect("connected peer");
                let peer_response = peer
                    .request(McpPeerRequest {
                        method: "client/echo".to_string(),
                        params: Some(serde_json::json!({ "from": "server" })),
                        meta: None,
                    })
                    .await
                    .expect("peer response");
                CommandResult::Ok {
                    data: serde_json::json!({
                        "protocolVersion": mcp.protocol_version,
                        "requestMeta": mcp.request_meta,
                        "clientCapabilities": mcp.client_capabilities,
                        "peer": peer_response,
                    }),
                    cta: None,
                    exit_code: None,
                }
            }
        }

        struct EchoClient;

        impl ClientHandler for EchoClient {
            fn get_info(&self) -> ClientInfo {
                let mut info = ClientInfo::default();
                info.protocol_version = ProtocolVersion::V_2026_07_28;
                info.client_info = Implementation::new("native-duplex", "1.0.0");
                info
            }

            fn on_custom_request(
                &self,
                request: CustomRequest,
                _context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<CustomResult, McpError>> + rmcp::service::MaybeSendFuture + '_
            {
                std::future::ready(Ok(CustomResult::new(serde_json::json!({
                    "method": request.method,
                    "params": request.params,
                }))))
            }
        }

        let mut command = CommandDef::build("context", ContextTool)
            .description("Return MCP context")
            .mcp(McpCommandOptions {
                name: Some("native.context".to_string()),
                title: Some("Native Context".to_string()),
                meta: BTreeMap::from([("example/tool".to_string(), serde_json::json!(true))]),
                result_meta: BTreeMap::from([(
                    "example/result".to_string(),
                    serde_json::json!(true),
                )]),
                direct: true,
                ..McpCommandOptions::default()
            })
            .done();
        command.output_schema = Some(serde_json::json!({ "type": "object" }));
        let mut resources = McpResourceRegistry::default();
        resources.resources.push(McpResource {
            uri: "memory://native".to_string(),
            name: "native-resource".to_string(),
            title: Some("Native Resource".to_string()),
            description: Some("Resource served over native transport".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(6),
            icons: Vec::new(),
            meta: BTreeMap::from([("example/resource".to_string(), serde_json::json!(true))]),
        });
        struct ResourceDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for ResourceDrop {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let resource_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resource_dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = resource_started.clone();
        let dropped = resource_dropped.clone();
        resources.read = Some(McpResourceReader::new(move |request| {
            let started = started.clone();
            let dropped = dropped.clone();
            async move {
                if request.uri == "memory://pending" {
                    let _guard = ResourceDrop(dropped);
                    started.store(true, std::sync::atomic::Ordering::SeqCst);
                    std::future::pending::<()>().await;
                }
                Ok(McpResourceReadResult {
                    contents: vec![McpResourceContents::Text {
                        uri: request.uri,
                        mime_type: Some("text/plain".to_string()),
                        text: "native".to_string(),
                        meta: BTreeMap::from([(
                            "example/content".to_string(),
                            serde_json::json!(true),
                        )]),
                    }],
                    meta: BTreeMap::from([("example/read".to_string(), serde_json::json!(true))]),
                })
            }
        }));
        let cli = crate::cli::Cli::create("native-duplex")
            .mcp(McpServeOptions {
                tools: McpToolFilter {
                    discovery: McpDiscovery::Direct,
                    ..Default::default()
                },
                resources,
                capabilities: BTreeMap::from([
                    (
                        "extensions".to_string(),
                        serde_json::json!({ "example/extensions": { "enabled": true } }),
                    ),
                    (
                        "experimental".to_string(),
                        serde_json::json!({ "example/feature": { "mode": "on" } }),
                    ),
                ]),
                ..Default::default()
            })
            .command("context", command);

        let (server_io, client_io) = tokio::io::duplex(32 * 1024);
        let (server_read, server_write) = tokio::io::split(server_io);
        let (client_read, client_write) = tokio::io::split(client_io);
        let server =
            server::IncurMcpServer::new(&server::ServerSource::from_cli(&cli), &cli.mcp_options)
                .unwrap();
        let server_task = tokio::spawn(async move {
            let running = server
                .serve((server_read, server_write))
                .await
                .expect("server starts");
            let _ = running.waiting().await;
        });
        let client = EchoClient
            .serve_with_lifecycle(
                (client_read, client_write),
                rmcp::ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("client discovers server");
        let info = client.peer_info().expect("server info");
        let capabilities =
            serde_json::to_value(&info.capabilities).expect("capabilities serialize");
        assert_eq!(
            capabilities["extensions"]["example/extensions"]["enabled"],
            true
        );
        assert_eq!(
            capabilities["experimental"]["example/feature"]["mode"],
            "on"
        );

        let tools = client.list_tools(None).await.expect("tools list");
        let tool = serde_json::to_value(&tools.tools[0]).expect("tool serializes");
        assert_eq!(tool["name"], "native.context");
        assert_eq!(tool["title"], "Native Context");
        assert_eq!(tool["_meta"]["example/tool"], true);

        let resources = client.list_resources(None).await.expect("resources list");
        let resource = serde_json::to_value(&resources.resources[0]).expect("resource serializes");
        assert_eq!(resource["uri"], "memory://native");
        assert_eq!(resource["_meta"]["example/resource"], true);
        let read = client
            .read_resource(ReadResourceRequestParams::new("memory://native"))
            .await
            .expect("resource read");
        let read = serde_json::to_value(read).expect("read result serializes");
        assert_eq!(read["contents"][0]["text"], "native");
        assert_eq!(read["contents"][0]["_meta"]["example/content"], true);
        assert_eq!(read["_meta"]["example/read"], true);

        let result = client
            .call_tool(
                CallToolRequestParams::new("native.context").with_arguments(serde_json::Map::new()),
            )
            .await
            .expect("tool call");
        let result = serde_json::to_value(result).expect("tool result serializes");
        assert_eq!(result["structuredContent"]["protocolVersion"], "2026-07-28");
        assert!(result["structuredContent"]["requestMeta"].is_object());
        assert!(result["structuredContent"]["clientCapabilities"].is_object());
        assert_eq!(result["structuredContent"]["peer"]["method"], "client/echo");
        assert_eq!(
            result["structuredContent"]["peer"]["params"]["from"],
            "server"
        );
        assert_eq!(result["_meta"]["example/result"], true);

        let pending_request = client
            .send_cancellable_request(
                rmcp::model::ClientRequest::ReadResourceRequest(
                    rmcp::model::ReadResourceRequest::new(ReadResourceRequestParams::new(
                        "memory://pending",
                    )),
                ),
                rmcp::service::PeerRequestOptions::no_options(),
            )
            .await
            .expect("pending read is sent");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !resource_started.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("resource callback started");
        pending_request
            .cancel(Some("literal cancellation control".to_string()))
            .await
            .expect("cancel notification sent");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !resource_dropped.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled resource callback is dropped");

        drop(client);
        server_task.abort();
    }

    #[cfg(feature = "http")]
    #[test]
    fn remote_header_map_converts_valid_pairs() {
        let headers = vec![
            ("x-temper-machine".to_string(), "mac-1".to_string()),
            ("x-trace".to_string(), "abc123".to_string()),
        ];
        let map = super::remote_header_map(&headers).expect("valid headers convert");
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&http::HeaderName::from_static("x-temper-machine"))
                .map(|value| value.to_str().unwrap()),
            Some("mac-1"),
        );
    }

    #[cfg(feature = "http")]
    #[test]
    fn remote_header_map_names_the_offending_header() {
        // A rejected header would otherwise be indistinguishable from an auth
        // failure at the far end, so the error has to say which one.
        let error = super::remote_header_map(&[("bad name".to_string(), "v".to_string())])
            .expect_err("a space is not valid in a header name");
        assert!(error.to_string().contains("bad name"), "got: {error}");

        let error = super::remote_header_map(&[("x-ok".to_string(), "bad\nvalue".to_string())])
            .expect_err("a newline is not valid in a header value");
        assert!(error.to_string().contains("x-ok"), "got: {error}");
    }

    #[cfg(feature = "http")]
    #[test]
    fn bearer_options_carry_only_a_token() {
        let options = super::McpRemoteOptions::bearer("secret-token");
        assert_eq!(options.auth_token.as_deref(), Some("secret-token"));
        assert!(options.headers.is_empty());
    }

    #[cfg(all(
        any(feature = "http", feature = "agent-plugins-mcp"),
        not(target_arch = "wasm32")
    ))]
    #[test]
    fn remote_adapters_restore_only_the_exact_output_projection_marker() {
        let payload_schema = serde_json::json!({ "type": "array", "items": { "type": "integer" } });
        let wrapped_schema = serde_json::json!({
            "type": "object", "properties": { "data": payload_schema.clone() },
            "required": ["data"], "additionalProperties": false
        });
        let wrapped_content = serde_json::json!({ "data": [1, 2] });
        let cases = [
            (None, false),
            (
                Some(serde_json::json!({
                    "version": 1, "shape": "value-wrapper", "field": "data",
                    "schemaRefBase": "#/properties/data"
                })),
                true,
            ),
            (
                Some(serde_json::json!({
                    "version": 2, "shape": "value-wrapper", "field": "data",
                    "schemaRefBase": "#/properties/data"
                })),
                false,
            ),
            (
                Some(serde_json::json!({
                    "version": 1, "shape": "value-wrapper", "field": "other",
                    "schemaRefBase": "#/properties/data"
                })),
                false,
            ),
            (
                Some(serde_json::json!({
                    "version": 1, "shape": "value-wrapper", "field": "data",
                    "schemaRefBase": "#/properties/data", "extra": true
                })),
                false,
            ),
        ];
        for (marker, restore) in cases {
            let mut tool = serde_json::json!({
                "name": "echo", "inputSchema": { "type": "object" },
                "outputSchema": wrapped_schema.clone()
            });
            let mut result = serde_json::json!({
                "content": [], "structuredContent": wrapped_content.clone(), "isError": false
            });
            if let Some(marker) = marker {
                let metadata = serde_json::json!({ "io.incurs.outputProjection": marker });
                tool["_meta"] = metadata.clone();
                result["_meta"] = metadata;
            }
            let expected_schema = if restore {
                &payload_schema
            } else {
                &wrapped_schema
            };
            let expected_content = if restore {
                serde_json::json!([1, 2])
            } else {
                wrapped_content.clone()
            };
            let portable_tool = super::RemoteTool::from_value(&tool).unwrap();
            assert_eq!(portable_tool.output_schema.as_ref(), Some(expected_schema));
            let native_tool: super::RemoteTool = serde_json::from_value::<rmcp::model::Tool>(tool)
                .unwrap()
                .into();
            assert_eq!(native_tool.output_schema.as_ref(), Some(expected_schema));
            assert_eq!(
                super::RemoteCallResult::from_value(&result).data(),
                expected_content
            );
            let native_result: super::RemoteCallResult =
                serde_json::from_value::<rmcp::model::CallToolResult>(result)
                    .unwrap()
                    .into();
            assert_eq!(native_result.data(), expected_content);
        }
    }

    /// The native client keeps a stable code, keeps the JSON-RPC error text
    /// unchanged, and calls only transport failures retryable.
    #[cfg(all(feature = "http", not(target_arch = "wasm32")))]
    #[test]
    fn rmcp_failures_keep_a_code_and_only_transport_failures_are_retryable() {
        let coded = |error: crate::errors::Error| match error {
            crate::errors::Error::Incur(error) => (error.code, error.retryable, error.message),
            other => panic!("expected a coded error, got {other}"),
        };
        let (code, retryable, message) =
            coded(super::rmcp_service_error(rmcp::ServiceError::McpError(
                rmcp::model::ErrorData::new(rmcp::model::ErrorCode(-32602), "bad", None),
            )));
        assert_eq!(code, "MCP_JSONRPC_ERROR");
        assert!(!retryable);
        assert_eq!(message, "Mcp error: -32602: bad");
        let (code, retryable, _) = coded(super::rmcp_service_error(
            rmcp::ServiceError::TransportClosed,
        ));
        assert_eq!(code, "MCP_TRANSPORT_ERROR");
        assert!(retryable);
        let (code, retryable, _) = coded(super::rmcp_service_error(
            rmcp::ServiceError::UnexpectedResponse,
        ));
        assert_eq!(code, "MCP_PROTOCOL_ERROR");
        assert!(!retryable);
    }
}
