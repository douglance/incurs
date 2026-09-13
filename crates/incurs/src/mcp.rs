//! MCP (Model Context Protocol) stdio server.
//!
//! Ported from `src/Mcp.ts`. Exposes CLI commands as MCP tools over a stdio
//! transport. The actual server implementation uses the `rmcp` crate and is
//! gated behind the `mcp` feature flag.

use std::collections::BTreeMap;

use crate::schema::FieldMeta;
#[cfg(feature = "mcp")]
use serde_json::Value;

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

/// Options for the MCP server.
#[derive(Debug, Clone, Default)]
pub struct McpServeOptions {
    /// CLI version string.
    pub version: Option<String>,
    /// Instructions describing how clients should use the server.
    pub instructions: Option<String>,
    /// Tool discovery and filtering configuration.
    pub tools: McpToolFilter,
    /// Exact MCP standards served concurrently. Preference order is used for
    /// legacy fallback and modern discovery.
    pub standards: incurs_mcp_protocol::McpStandardSet,
}

/// Options for projecting a remote MCP server as incurs commands.
#[derive(Debug, Clone, Default)]
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

#[cfg(feature = "mcp")]
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

#[cfg(feature = "mcp")]
struct RemoteToolHandler {
    client:
        std::sync::Arc<rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>>,
    tool: String,
    wrapper: Option<String>,
}

#[cfg(feature = "mcp")]
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
        let result = self
            .client
            .call_tool(rmcp::model::CallToolRequestParams::new(name).with_arguments(arguments))
            .await;
        match result {
            Ok(result) if result.is_error != Some(true) => {
                let data = remote_success_value(result);
                crate::output::CommandResult::Ok {
                    data,
                    cta: None,
                    exit_code: None,
                }
            }
            Ok(result) => crate::output::CommandResult::Error {
                code: "REMOTE_MCP_ERROR".to_string(),
                message: result
                    .content
                    .first()
                    .and_then(|content| content.as_text())
                    .map(|text| text.text.clone())
                    .unwrap_or_else(|| "Remote MCP tool failed".to_string()),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
            Err(error) => crate::output::CommandResult::Error {
                code: "REMOTE_MCP_ERROR".to_string(),
                message: error.to_string(),
                retryable: true,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

/// Connects to a remote MCP-over-HTTP server and projects its tools as commands.
#[cfg(all(feature = "mcp", feature = "http"))]
pub async fn remote_commands(
    uri: impl Into<String>,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    remote_commands_with(uri, &McpRemoteOptions::default()).await
}

/// Connects to a remote MCP-over-HTTP server using explicit exact standards
/// and projects its tools as commands.
#[cfg(all(feature = "mcp", feature = "http"))]
pub async fn remote_commands_with(
    uri: impl Into<String>,
    options: &McpRemoteOptions,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut config = StreamableHttpClientTransportConfig::with_uri(uri.into());
    if let Some(token) = &options.auth_token {
        config = config.auth_header(token.clone());
    }
    if !options.headers.is_empty() {
        config = config.custom_headers(remote_header_map(&options.headers)?);
    }

    remote_commands_from_transport(StreamableHttpClientTransport::from_config(config), options)
        .await
}

/// Converts plain `(name, value)` pairs into the transport's header map.
///
/// Reports the offending header by name, since a rejected header is otherwise
/// indistinguishable from an authentication failure at the far end.
#[cfg(all(feature = "mcp", feature = "http"))]
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

#[cfg(feature = "mcp")]
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
        .map_err(|error| {
            crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
        })?;
    project_remote_commands(client).await
}

#[cfg(feature = "mcp")]
async fn project_remote_commands(
    client: rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
) -> Result<std::collections::BTreeMap<String, crate::command::CommandDef>, crate::errors::Error> {
    let listed = client.list_all_tools().await.map_err(|error| {
        crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
    })?;
    let progressive = {
        let names = listed
            .iter()
            .map(|tool| tool.name.as_ref())
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
        discover_remote_tools(&client).await?
    } else {
        listed
    };
    let client = std::sync::Arc::new(client);
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
        let name = tool.name.to_string();
        commands.insert(
            name.clone(),
            crate::command::CommandDef {
                name: name.clone(),
                description: tool.description.map(|description| description.to_string()),
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
                        if tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.read_only_hint)
                            == Some(true)
                        {
                            "call_read_tool".to_string()
                        } else {
                            "call_write_tool".to_string()
                        }
                    }),
                    tool: name,
                }),
                middleware: Vec::new(),
                output_schema: tool.output_schema.map(|schema| {
                    incurs_mcp_protocol::structured::restore_output_schema(
                        Value::Object((*schema).clone()),
                        tool.meta.as_ref().map(|meta| &meta.0),
                    )
                }),
            },
        );
    }
    Ok(commands)
}

#[cfg(feature = "mcp")]
async fn discover_remote_tools(
    client: &rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>,
) -> Result<Vec<rmcp::model::Tool>, crate::errors::Error> {
    let mut tools = Vec::new();
    let mut offset = 0_u64;
    loop {
        let search = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("search_tools").with_arguments(
                    serde_json::Map::from_iter([
                        ("query".to_string(), Value::String(String::new())),
                        ("limit".to_string(), Value::from(20)),
                        ("offset".to_string(), Value::from(offset)),
                    ]),
                ),
            )
            .await
            .map_err(remote_error)?;
        let value = remote_result_value(search)?;
        for name in value["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["name"].as_str())
        {
            let details = client
                .call_tool(
                    rmcp::model::CallToolRequestParams::new("get_tool_details").with_arguments(
                        serde_json::Map::from_iter([(
                            "name".to_string(),
                            Value::String(name.to_string()),
                        )]),
                    ),
                )
                .await
                .map_err(remote_error)?;
            tools
                .push(serde_json::from_value(remote_result_value(details)?).map_err(remote_error)?);
        }
        let Some(next) = value.get("nextOffset").and_then(Value::as_u64) else {
            break;
        };
        if next <= offset {
            return Err(remote_error(std::io::Error::other(
                "MCP tool catalog returned a non-advancing offset",
            )));
        }
        offset = next;
    }
    Ok(tools)
}

#[cfg(feature = "mcp")]
fn remote_result_value(result: rmcp::model::CallToolResult) -> Result<Value, crate::errors::Error> {
    if result.is_error == Some(true) {
        return Err(remote_error(std::io::Error::other(
            result
                .content
                .first()
                .and_then(|content| content.as_text())
                .map(|text| text.text.clone())
                .unwrap_or_else(|| "Remote MCP tool failed".to_string()),
        )));
    }
    Ok(remote_success_value(result))
}

#[cfg(feature = "mcp")]
fn remote_success_value(result: rmcp::model::CallToolResult) -> Value {
    let value = result.structured_content.unwrap_or_else(|| {
        result
            .content
            .first()
            .and_then(|content| content.as_text())
            .and_then(|text| serde_json::from_str(&text.text).ok())
            .unwrap_or(Value::Null)
    });
    incurs_mcp_protocol::structured::restore_structured_content(
        value,
        result.meta.as_ref().map(|meta| &meta.0),
    )
}

#[cfg(feature = "mcp")]
fn remote_error(error: impl std::fmt::Display) -> crate::errors::Error {
    crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
}

#[cfg(feature = "mcp")]
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
            let input_schema = build_tool_schema(&entry.args_fields, &entry.options_fields);
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
// MCP Server (behind feature flag)
// ---------------------------------------------------------------------------

#[cfg(feature = "mcp")]
mod server {
    use std::borrow::Cow;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use incurs_mcp_protocol::structured::{
        McpStructuredShape, project_output_schema, project_structured_content, projection_metadata,
    };
    use serde_json::Value;

    use rmcp::ErrorData as McpError;
    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        Implementation, InitializeRequestParams, InitializeResult, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, MetaObject,
        PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion, ServerCapabilities,
        ServerInfo, Tool, ToolAnnotations,
    };
    use rmcp::service::{RequestContext, RoleServer};

    use crate::cli::ConfigOptions;
    use crate::command::McpResultContent;
    use crate::schema::FieldMeta;
    use crate::tool::{
        ConfigSource, EnvironmentSource, ToolCallControl, ToolCallOptions, ToolCallOutcome,
        ToolCatalog, ToolDefinition, ToolEvent, ToolEventSink,
    };

    use super::{McpDiscovery, McpServeOptions, McpToolFilter};

    // -----------------------------------------------------------------------
    // Tool resolution from the CLI command tree
    // -----------------------------------------------------------------------

    /// A resolved tool metadata entry. Execution goes through [`ToolCatalog`].
    struct ResolvedTool {
        /// Tool name (path segments joined with `_`).
        name: String,
        /// Human-readable description.
        description: String,
        /// Merged JSON Schema for the tool's input (as a JSON Map).
        input_schema: Arc<serde_json::Map<String, Value>>,
        /// JSON Schema for structured MCP output when object-shaped.
        output_schema: Option<Arc<serde_json::Map<String, Value>>>,
        /// MCP-only representation; the source catalog retains its original schema.
        output_shape: Option<McpStructuredShape>,
        /// Behavioral annotations exposed to clients.
        annotations: Option<ToolAnnotations>,
        /// Tool-specific instructions exposed through metadata.
        instructions: Option<String>,
        /// Rich content derived from the successful structured result.
        result_content: Vec<McpResultContent>,
    }

    fn tool_definition(definition: &ToolDefinition) -> ResolvedTool {
        let projection = definition.output_schema.as_ref().map(project_output_schema);
        ResolvedTool {
            name: definition.name.clone(),
            description: definition.description.clone(),
            input_schema: Arc::new(
                definition
                    .input_schema
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ),
            output_schema: projection
                .as_ref()
                .and_then(|projection| projection.schema.as_object().cloned().map(Arc::new)),
            output_shape: projection.as_ref().map(|projection| projection.shape),
            annotations: definition.annotations.as_ref().map(|annotations| {
                ToolAnnotations::from_raw(
                    annotations.title.clone(),
                    annotations.read_only_hint,
                    annotations.destructive_hint,
                    annotations.idempotent_hint,
                    annotations.open_world_hint,
                )
            }),
            instructions: definition.instructions.clone(),
            result_content: definition.result_content.clone(),
        }
    }

    /// Resolves the shared transport-neutral catalog for MCP discovery and calls.
    fn resolve_catalog(
        name: String,
        version: Option<String>,
        commands: &BTreeMap<String, crate::cli::CommandEntry>,
        root_middleware: &[crate::middleware::MiddlewareFn],
        env_fields: &[FieldMeta],
        globals_fields: &[FieldMeta],
        config: Option<&ConfigOptions>,
    ) -> Result<ToolCatalog, crate::errors::Error> {
        ToolCatalog::from_parts(
            name,
            version,
            commands,
            root_middleware,
            env_fields,
            globals_fields,
            config,
        )
        .map_err(|error| {
            crate::errors::Error::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                error.to_string(),
            )))
        })
    }

    fn collect_resolved_tools(catalog: &ToolCatalog) -> Vec<ResolvedTool> {
        catalog
            .resolved()
            .map(|tool| tool_definition(&tool.definition))
            .collect()
    }

    pub(super) struct ServerSource<'a> {
        name: &'a str,
        version: &'a str,
        commands: &'a BTreeMap<String, crate::cli::CommandEntry>,
        root_middleware: &'a [crate::middleware::MiddlewareFn],
        env_fields: &'a [FieldMeta],
        globals_fields: &'a [FieldMeta],
        config: Option<&'a ConfigOptions>,
    }

    impl<'a> ServerSource<'a> {
        pub(super) fn from_cli(cli: &'a crate::cli::Cli) -> Self {
            Self {
                name: &cli.name,
                version: cli.version.as_deref().unwrap_or("0.0.0"),
                commands: &cli.commands,
                root_middleware: &cli.middleware,
                env_fields: &cli.env_fields,
                globals_fields: &cli.globals_fields,
                config: cli.config.as_ref(),
            }
        }

        pub(super) fn from_parts(
            name: &'a str,
            version: &'a str,
            commands: &'a BTreeMap<String, crate::cli::CommandEntry>,
            root_middleware: &'a [crate::middleware::MiddlewareFn],
            env_fields: &'a [FieldMeta],
        ) -> Self {
            Self {
                name,
                version,
                commands,
                root_middleware,
                env_fields,
                globals_fields: &[],
                config: None,
            }
        }
    }

    fn catalog_and_tools(
        source: &ServerSource<'_>,
    ) -> Result<(ToolCatalog, Vec<ResolvedTool>), crate::errors::Error> {
        let catalog = resolve_catalog(
            source.name.to_string(),
            Some(source.version.to_string()),
            source.commands,
            source.root_middleware,
            source.env_fields,
            source.globals_fields,
            source.config,
        )?;
        let resolved = collect_resolved_tools(&catalog);
        Ok((catalog, resolved))
    }

    fn wildcard_matches(pattern: &str, value: &str) -> bool {
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

    fn filter_tools(tools: Vec<ResolvedTool>, filter: &McpToolFilter) -> Vec<ResolvedTool> {
        tools
            .into_iter()
            .filter(|tool| {
                let included = filter.include.is_empty()
                    || filter
                        .include
                        .iter()
                        .any(|pattern| wildcard_matches(pattern, &tool.name));
                let excluded = filter
                    .exclude
                    .iter()
                    .any(|pattern| wildcard_matches(pattern, &tool.name));
                included && !excluded
            })
            .collect()
    }

    fn direct_tool(tool: &ResolvedTool) -> Tool {
        let mut result = Tool::new(
            Cow::Owned(tool.name.clone()),
            Cow::Owned(tool.description.clone()),
            Arc::clone(&tool.input_schema),
        );
        result.output_schema = tool.output_schema.clone();
        result.annotations = tool.annotations.clone();
        result.meta = tool_metadata(tool);
        result
    }

    fn tool_metadata(tool: &ResolvedTool) -> Option<MetaObject> {
        let mut meta = tool.output_shape.and_then(projection_metadata);
        if let Some(instructions) = &tool.instructions {
            meta.get_or_insert_default().insert(
                "instructions".to_string(),
                Value::String(instructions.clone()),
            );
        }
        meta.map(MetaObject)
    }

    fn progressive_tools() -> Vec<Tool> {
        let search = serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "default": "" },
                "limit": { "type": "number", "default": 5 },
                "offset": { "type": "number", "default": 0 }
            }
        });
        let inspect = serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        let execute = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "arguments": { "type": "object", "additionalProperties": true }
            },
            "required": ["name"]
        });
        [
            (
                "search_tools",
                "Search or page through available tools by capability. Returns names and descriptions without loading their schemas. Inspect a result before calling it.",
                search,
                true,
            ),
            (
                "get_tool_details",
                "Inspect one tool returned by search_tools. Returns its complete input schema and metadata.",
                inspect,
                true,
            ),
            (
                "call_read_tool",
                "Execute a tool marked read-only after inspecting its schema with get_tool_details.",
                execute.clone(),
                true,
            ),
            (
                "call_write_tool",
                "Execute a writable or unclassified tool after inspecting its schema with get_tool_details.",
                execute,
                false,
            ),
        ]
        .into_iter()
        .map(|(name, description, schema, read_only)| {
            let mut tool = Tool::new(
                name.to_string(),
                description.to_string(),
                Arc::new(schema.as_object().cloned().unwrap_or_default()),
            );
            tool.annotations = Some(ToolAnnotations::from_raw(
                None,
                Some(read_only),
                Some(!read_only),
                Some(read_only),
                Some(!matches!(name, "search_tools" | "get_tool_details")),
            ));
            tool
        })
        .collect()
    }

    fn discovery_result(
        name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
        tools: &HashMap<String, Arc<ResolvedTool>>,
    ) -> Result<Option<CallToolResult>, McpError> {
        let arguments = arguments.unwrap_or_default();
        match name {
            "search_tools" => {
                let query = arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
                let mut matches = tools
                    .values()
                    .filter(|tool| {
                        query.is_empty()
                            || tool.name.to_lowercase().contains(&query)
                            || tool.description.to_lowercase().contains(&query)
                    })
                    .map(|tool| {
                        serde_json::json!({
                            "name": tool.name,
                            "description": tool.description,
                            "annotations": tool.annotations,
                        })
                    })
                    .collect::<Vec<_>>();
                matches.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                let total = matches.len();
                let page = matches
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .collect::<Vec<_>>();
                Ok(Some(CallToolResult::structured(serde_json::json!({
                    "tools": page,
                    "nextOffset": (offset + page.len() < total).then_some(offset + page.len()),
                }))))
            }
            "get_tool_details" => {
                let tool_name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| McpError::invalid_params("Missing tool name", None))?;
                let tool = tools.get(tool_name).ok_or_else(|| {
                    McpError::invalid_params(format!("Unknown tool: {tool_name}"), None)
                })?;
                Ok(Some(CallToolResult::structured(serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": Value::Object((*tool.input_schema).clone()),
                    "outputSchema": tool.output_schema.as_ref().map(|schema| Value::Object((**schema).clone())),
                    "annotations": tool.annotations,
                    "instructions": tool.instructions,
                    "_meta": tool_metadata(tool),
                }))))
            }
            "call_read_tool" | "call_write_tool" => {
                let tool_name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| McpError::invalid_params("Missing tool name", None))?;
                let tool = tools.get(tool_name).ok_or_else(|| {
                    McpError::invalid_params(format!("Unknown tool: {tool_name}"), None)
                })?;
                let read_only = tool
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.read_only_hint)
                    == Some(true);
                if name == "call_read_tool" && !read_only {
                    return Ok(Some(CallToolResult::error(vec![ContentBlock::text(
                        serde_json::json!({ "error": format!("Tool is not read-only: {tool_name}") }).to_string(),
                    )])));
                }
                if name == "call_write_tool" && read_only {
                    return Ok(Some(CallToolResult::error(vec![ContentBlock::text(
                        serde_json::json!({ "error": format!("Tool is read-only: {tool_name}") })
                            .to_string(),
                    )])));
                }
                Ok(None)
            }
            _ => Err(McpError::invalid_params(
                format!("Unknown discovery tool: {name}"),
                None,
            )),
        }
    }

    fn formatted_cta(name: &str, cta: crate::output::CtaBlock) -> Value {
        let commands = cta
            .commands
            .into_iter()
            .map(|entry| match entry {
                crate::output::CtaEntry::Simple(command) => serde_json::json!({
                    "command": format!("{name} {command}"),
                }),
                crate::output::CtaEntry::Detailed {
                    command,
                    description,
                } => {
                    let command = if command == name || command.starts_with(&format!("{name} ")) {
                        command
                    } else {
                        format!("{name} {command}")
                    };
                    serde_json::json!({ "command": command, "description": description })
                }
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "description": cta.description.unwrap_or_else(|| "Suggested commands:".to_string()),
            "commands": commands,
        })
    }

    fn render_cta(cta: &Value) -> String {
        let mut lines = vec![
            cta["description"]
                .as_str()
                .unwrap_or("Suggested commands:")
                .to_string(),
        ];
        for command in cta["commands"].as_array().into_iter().flatten() {
            let value = command["command"].as_str().unwrap_or("");
            let description = command["description"]
                .as_str()
                .map(|description| format!("  # {description}"))
                .unwrap_or_default();
            lines.push(format!("  {value}{description}"));
        }
        lines.join("\n")
    }

    pub(super) fn tool_result_success(
        name: &str,
        data: Value,
        cta: Option<crate::output::CtaBlock>,
        structured: Option<McpStructuredShape>,
        presentation: &[McpResultContent],
    ) -> CallToolResult {
        let text = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
        let cta = cta.map(|cta| formatted_cta(name, cta));
        let text = cta
            .as_ref()
            .map(|cta| format!("{text}\n\n{}", render_cta(cta)))
            .unwrap_or(text);
        let mut content = vec![ContentBlock::text(text)];
        for item in presentation {
            match item {
                McpResultContent::Image {
                    data_pointer,
                    mime_type_pointer,
                } => {
                    let Some(image_data) = data.pointer(data_pointer).and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let Some(mime_type) = data.pointer(mime_type_pointer).and_then(Value::as_str)
                    else {
                        continue;
                    };
                    content.push(ContentBlock::image(image_data, mime_type));
                }
            }
        }
        let mut result = CallToolResult::success(content);
        if let Some(shape) = structured {
            result.structured_content = match project_structured_content(data, shape) {
                Ok(data) => Some(data),
                Err(error) => {
                    return CallToolResult::error(vec![ContentBlock::text(error.to_string())]);
                }
            };
        }
        let mut meta = structured.and_then(projection_metadata);
        if let Some(cta) = cta {
            meta.get_or_insert_default().insert("cta".to_string(), cta);
        }
        result.meta = meta.map(MetaObject);
        result
    }

    fn tool_result_error(
        name: &str,
        message: String,
        cta: Option<crate::output::CtaBlock>,
    ) -> CallToolResult {
        let cta = cta.map(|cta| formatted_cta(name, cta));
        let text = cta
            .as_ref()
            .map(|cta| format!("{message}\n\n{}", render_cta(cta)))
            .unwrap_or(message);
        CallToolResult::error(vec![ContentBlock::text(text)]).with_meta(
            cta.map(|cta| MetaObject(serde_json::Map::from_iter([("cta".to_string(), cta)]))),
        )
    }

    fn field_errors_text(field_errors: Vec<crate::output::FieldErrorOutput>) -> String {
        serde_json::to_string(
            &field_errors
                .into_iter()
                .map(|error| {
                    serde_json::json!({
                        "path": error.path,
                        "expected": error.expected,
                        "received": error.received,
                        "message": error.message,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default()
    }

    fn tool_call_result(
        name: &str,
        outcome: ToolCallOutcome,
        structured: Option<McpStructuredShape>,
        presentation: &[McpResultContent],
    ) -> CallToolResult {
        match outcome {
            ToolCallOutcome::Ok { data, cta } => {
                tool_result_success(name, data, cta, structured, presentation)
            }
            ToolCallOutcome::Error {
                message,
                field_errors,
                cta,
                ..
            } => {
                let mut text = if message.is_empty() {
                    "Command failed".to_string()
                } else {
                    message
                };
                if let Some(field_errors) = field_errors {
                    text.push_str("\n\n");
                    text.push_str(&field_errors_text(field_errors));
                }
                tool_result_error(name, text, cta)
            }
        }
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
            let message = match event {
                ToolEvent::Chunk { data } => {
                    serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string())
                }
                ToolEvent::Progress { message, .. } | ToolEvent::Log { message, .. } => message,
            };
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
        /// Server name (CLI name).
        server_name: String,
        /// Server version (CLI version).
        server_version: String,
        /// Transport-neutral command catalog used for execution.
        catalog: ToolCatalog,
        /// Resolved tools indexed by name for O(1) lookup during `tools/call`.
        tools_by_name: Arc<HashMap<String, Arc<ResolvedTool>>>,
        /// Pre-built list of `rmcp::model::Tool` for `tools/list` responses.
        tool_list: Arc<Vec<Tool>>,
        /// Instructions returned during MCP initialization.
        instructions: Option<String>,
        /// Active discovery strategy.
        discovery: McpDiscovery,
        /// Exact standards enabled for this server.
        standards: incurs_mcp_protocol::McpStandardSet,
    }

    impl IncurMcpServer {
        fn new(
            name: String,
            version: String,
            catalog: ToolCatalog,
            resolved_tools: Vec<ResolvedTool>,
            options: &McpServeOptions,
        ) -> Result<Self, crate::errors::Error> {
            let resolved_tools = filter_tools(resolved_tools, &options.tools);
            let mut names = std::collections::HashSet::new();
            for tool in &resolved_tools {
                if !names.insert(tool.name.clone()) {
                    return Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Duplicate MCP tool name: {}", tool.name),
                    ))));
                }
            }
            let tool_list = if options.tools.discovery == McpDiscovery::Direct {
                resolved_tools.iter().map(direct_tool).collect()
            } else {
                progressive_tools()
            };

            let mut tools_by_name = HashMap::new();
            for tool in resolved_tools {
                let name = tool.name.clone();
                tools_by_name.insert(name, Arc::new(tool));
            }

            Ok(IncurMcpServer {
                server_name: name,
                server_version: version,
                catalog,
                tools_by_name: Arc::new(tools_by_name),
                tool_list: Arc::new(tool_list),
                instructions: options.instructions.clone(),
                discovery: options.tools.discovery,
                standards: options.standards.clone(),
            })
        }
    }

    impl ServerHandler for IncurMcpServer {
        fn get_info(&self) -> ServerInfo {
            let protocol = super::rmcp_protocol_versions(&self.standards)
                .into_iter()
                .find(|version| version.as_str() != "2026-07-28")
                .unwrap_or(ProtocolVersion::V_2026_07_28);
            let info = ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .enable_resources()
                    .build(),
            )
            .with_protocol_version(protocol)
            .with_server_info(Implementation::new(
                self.server_name.clone(),
                self.server_version.clone(),
            ));
            if let Some(instructions) = &self.instructions {
                info.with_instructions(instructions.clone())
            } else {
                info
            }
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Owned(super::rmcp_protocol_versions(&self.standards))
        }

        fn initialize(
            &self,
            request: InitializeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<InitializeResult, McpError>> + Send + '_
        {
            context.peer.set_peer_info(request.clone());
            let supported = super::rmcp_protocol_versions(&self.standards);
            let selected = supported
                .iter()
                .find(|version| {
                    version.as_str() == request.protocol_version.as_str()
                        && version.as_str() != "2026-07-28"
                })
                .cloned();
            let selected = if selected.is_none()
                && ProtocolVersion::KNOWN_VERSIONS.contains(&request.protocol_version)
            {
                None
            } else {
                selected.or_else(|| {
                    supported
                        .iter()
                        .find(|version| version.as_str() != "2026-07-28")
                        .cloned()
                })
            };
            std::future::ready(match selected {
                Some(selected) => Ok(self.get_info().with_protocol_version(selected)),
                None => Err(McpError::unsupported_protocol_version(
                    request.protocol_version,
                    &supported,
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
            let mut result = ListResourcesResult::default();
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
            let mut result = ListResourceTemplatesResult::default();
            if context
                .protocol_version()
                .is_some_and(|version| version.as_str() == "2026-07-28")
            {
                result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
            }
            std::future::ready(Ok(result))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResponse, McpError>> + Send + '_
        {
            let tools_by_name = Arc::clone(&self.tools_by_name);
            let catalog = self.catalog.clone();
            let server_name = self.server_name.clone();
            let discovery = self.discovery;
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
            let peer = context.peer;
            let cancellation = context.ct;

            async move {
                let mut tool_name = request.name.to_string();
                let mut arguments = request.arguments;
                if discovery == McpDiscovery::Progressive {
                    if let Some(result) =
                        discovery_result(&tool_name, arguments.clone(), &tools_by_name)?
                    {
                        return Ok(result.into());
                    }
                    let mut execute = arguments.unwrap_or_default();
                    tool_name = execute
                        .remove("name")
                        .and_then(|value| value.as_str().map(ToString::to_string))
                        .ok_or_else(|| McpError::invalid_params("Missing tool name", None))?;
                    arguments = execute
                        .remove("arguments")
                        .and_then(|value| value.as_object().cloned());
                }
                let tool = tools_by_name.get(&tool_name).ok_or_else(|| {
                    McpError::invalid_params(format!("Unknown tool: {tool_name}"), None)
                })?;

                let input_options: BTreeMap<String, Value> =
                    arguments.unwrap_or_default().into_iter().collect();
                let outcome = catalog
                    .call(
                        &tool_name,
                        input_options,
                        ToolCallOptions {
                            environment: EnvironmentSource::DeclaredHost,
                            config: ConfigSource::Auto,
                            globals: None,
                            request: transport_request,
                            control: ToolCallControl {
                                cancellation,
                                events: Some(Arc::new(McpEventSink {
                                    peer,
                                    progress_token,
                                    count: tokio::sync::Mutex::new(0),
                                })),
                            },
                        },
                    )
                    .await;
                Ok(tool_call_result(
                    &server_name,
                    outcome,
                    tool.output_shape,
                    &tool.result_content,
                )
                .into())
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
    /// [`ToolCatalog`].
    pub(super) async fn serve(
        source: ServerSource<'_>,
        options: &McpServeOptions,
    ) -> Result<(), crate::errors::Error> {
        use rmcp::ServiceExt;
        use rmcp::transport::io::stdio;

        let (catalog, resolved) = catalog_and_tools(&source)?;

        let server = IncurMcpServer::new(
            source.name.to_string(),
            source.version.to_string(),
            catalog,
            resolved,
            options,
        )?;

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

        let (catalog, resolved) = catalog_and_tools(&source)?;
        let server = IncurMcpServer::new(
            source.name.to_string(),
            source.version.to_string(),
            catalog,
            resolved,
            options,
        )?;
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
#[cfg(all(feature = "mcp", feature = "http"))]
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
#[cfg(feature = "mcp")]
pub async fn serve_cli(cli: &crate::cli::Cli) -> Result<(), crate::errors::Error> {
    server::serve(server::ServerSource::from_cli(cli), &cli.mcp_options).await
}

/// Starts a stdio MCP server that exposes commands as tools.
///
/// Uses the `rmcp` crate for the actual MCP protocol implementation.
/// Each leaf command in the command tree becomes an MCP tool.
///
/// This is the public entry point. It accepts the CLI command tree directly
/// (rather than the standalone `mcp::CommandEntry` tree) so that it can
/// resolve `Arc<CommandDef>` references for command execution.
#[cfg(feature = "mcp")]
pub async fn serve(
    name: &str,
    version: &str,
    commands: &std::collections::BTreeMap<String, crate::cli::CommandEntry>,
    root_middleware: &[crate::middleware::MiddlewareFn],
    env_fields: &[FieldMeta],
    options: &McpServeOptions,
) -> Result<(), crate::errors::Error> {
    server::serve(
        server::ServerSource::from_parts(name, version, commands, root_middleware, env_fields),
        options,
    )
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "mcp", feature = "http"))]
#[path = "mcp_projection_tests.rs"]
mod projection_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, to_kebab};

    #[cfg(feature = "mcp")]
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

        let result = server::tool_result_success(
            "visualize",
            data,
            None,
            Some(incurs_mcp_protocol::structured::McpStructuredShape::Object),
            &presentation,
        );
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

    #[cfg(all(feature = "mcp", feature = "http"))]
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

    #[cfg(all(feature = "mcp", feature = "http"))]
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

    #[cfg(all(feature = "mcp", feature = "http"))]
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

    #[cfg(all(feature = "mcp", feature = "http"))]
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

    #[cfg(all(feature = "mcp", feature = "http"))]
    #[test]
    fn bearer_options_carry_only_a_token() {
        let options = super::McpRemoteOptions::bearer("secret-token");
        assert_eq!(options.auth_token.as_deref(), Some("secret-token"));
        assert!(options.headers.is_empty());
    }
}
