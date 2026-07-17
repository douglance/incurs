//! MCP (Model Context Protocol) stdio server.
//!
//! Ported from `src/Mcp.ts`. Exposes CLI commands as MCP tools over a stdio
//! transport. The actual server implementation uses the `rmcp` crate and is
//! gated behind the `mcp` feature flag.

use std::collections::BTreeMap;

use crate::schema::FieldMeta;
#[cfg(all(feature = "mcp", feature = "http"))]
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

#[cfg(all(feature = "mcp", feature = "http"))]
struct RemoteToolHandler {
    client: std::sync::Arc<rmcp::service::RunningService<rmcp::RoleClient, ()>>,
    tool: String,
    wrapper: Option<String>,
}

#[cfg(all(feature = "mcp", feature = "http"))]
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
            .call_tool(rmcp::model::CallToolRequestParam {
                name: name.into(),
                arguments: Some(arguments),
            })
            .await;
        match result {
            Ok(result) if result.is_error != Some(true) => {
                let data = result.structured_content.unwrap_or_else(|| {
                    result
                        .content
                        .first()
                        .and_then(|content| content.raw.as_text())
                        .and_then(|text| serde_json::from_str(&text.text).ok())
                        .unwrap_or(Value::Null)
                });
                crate::output::CommandResult::Ok { data, cta: None }
            }
            Ok(result) => crate::output::CommandResult::Error {
                code: "REMOTE_MCP_ERROR".to_string(),
                message: result
                    .content
                    .first()
                    .and_then(|content| content.raw.as_text())
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
    use rmcp::ServiceExt;
    use rmcp::transport::StreamableHttpClientTransport;

    let client =
        ().serve(StreamableHttpClientTransport::from_uri(uri.into()))
            .await
            .map_err(|error| {
                crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
            })?;
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
                output_schema: tool
                    .output_schema
                    .map(|schema| Value::Object((*schema).clone())),
            },
        );
    }
    Ok(commands)
}

#[cfg(all(feature = "mcp", feature = "http"))]
async fn discover_remote_tools(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
) -> Result<Vec<rmcp::model::Tool>, crate::errors::Error> {
    let mut tools = Vec::new();
    let mut offset = 0_u64;
    loop {
        let search = client
            .call_tool(rmcp::model::CallToolRequestParam {
                name: "search_tools".into(),
                arguments: Some(serde_json::Map::from_iter([
                    ("query".to_string(), Value::String(String::new())),
                    ("limit".to_string(), Value::from(20)),
                    ("offset".to_string(), Value::from(offset)),
                ])),
            })
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
                .call_tool(rmcp::model::CallToolRequestParam {
                    name: "get_tool_details".into(),
                    arguments: Some(serde_json::Map::from_iter([(
                        "name".to_string(),
                        Value::String(name.to_string()),
                    )])),
                })
                .await
                .map_err(remote_error)?;
            tools.push(
                serde_json::from_value(remote_result_value(details)?)
                    .map_err(|error| remote_error(error))?,
            );
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

#[cfg(all(feature = "mcp", feature = "http"))]
fn remote_result_value(result: rmcp::model::CallToolResult) -> Result<Value, crate::errors::Error> {
    if result.is_error == Some(true) {
        return Err(remote_error(std::io::Error::other(
            result
                .content
                .first()
                .and_then(|content| content.raw.as_text())
                .map(|text| text.text.clone())
                .unwrap_or_else(|| "Remote MCP tool failed".to_string()),
        )));
    }
    Ok(result.structured_content.unwrap_or_else(|| {
        result
            .content
            .first()
            .and_then(|content| content.raw.as_text())
            .and_then(|text| serde_json::from_str(&text.text).ok())
            .unwrap_or(Value::Null)
    }))
}

#[cfg(all(feature = "mcp", feature = "http"))]
fn remote_error(error: impl std::fmt::Display) -> crate::errors::Error {
    crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
}

#[cfg(all(feature = "mcp", feature = "http"))]
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

    use futures::StreamExt;
    use serde_json::Value;

    use rmcp::ErrorData as McpError;
    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult, Meta,
        PaginatedRequestParam, ProgressNotificationParam, ServerCapabilities, ServerInfo, Tool,
        ToolAnnotations,
    };
    use rmcp::service::{RequestContext, RoleServer};

    use crate::command::{self, CommandDef, ExecuteOptions, ParseMode};
    use crate::middleware::MiddlewareFn;
    use crate::output::Format;
    use crate::schema::FieldMeta;

    use super::{McpDiscovery, McpServeOptions, McpToolFilter, build_tool_schema};

    // -----------------------------------------------------------------------
    // Tool resolution from the CLI command tree
    // -----------------------------------------------------------------------

    /// A resolved tool with both metadata and the `CommandDef` needed for
    /// execution. This is the server-side counterpart of `ToolEntry`.
    struct ResolvedTool {
        /// Tool name (path segments joined with `_`).
        name: String,
        /// Human-readable description.
        description: String,
        /// Merged JSON Schema for the tool's input (as a JSON Map).
        input_schema: Arc<serde_json::Map<String, Value>>,
        /// The command definition for execution.
        command: Arc<CommandDef>,
        /// Middleware inherited from parent groups.
        middleware: Vec<MiddlewareFn>,
        /// JSON Schema for structured MCP output when object-shaped.
        output_schema: Option<Arc<serde_json::Map<String, Value>>>,
        /// Behavioral annotations exposed to clients.
        annotations: Option<ToolAnnotations>,
        /// Tool-specific instructions exposed through metadata.
        instructions: Option<String>,
    }

    /// Recursively collects leaf commands from the CLI command tree,
    /// preserving `Arc<CommandDef>` references for execution and inheriting
    /// group middleware.
    fn collect_resolved_tools(
        commands: &BTreeMap<String, crate::cli::CommandEntry>,
        prefix: &[String],
        parent_middleware: &[MiddlewareFn],
    ) -> Vec<ResolvedTool> {
        let mut result = Vec::new();

        for (name, entry) in commands {
            let mut path = prefix.to_vec();
            path.push(name.clone());

            match entry {
                crate::cli::CommandEntry::Leaf(def) => {
                    let mcp = def.handler.mcp_options().cloned().unwrap_or_default();
                    if !mcp.enabled {
                        continue;
                    }
                    let tool_name = mcp.name.clone().unwrap_or_else(|| path.join("_"));
                    let schema_value =
                        def.handler.mcp_input_schema().cloned().unwrap_or_else(|| {
                            build_tool_schema(&def.args_fields, &def.options_fields)
                        });
                    let input_schema = match schema_value {
                        Value::Object(map) => Arc::new(map),
                        _ => Arc::new(serde_json::Map::new()),
                    };
                    result.push(ResolvedTool {
                        name: tool_name,
                        description: mcp
                            .description
                            .clone()
                            .or_else(|| def.description.clone())
                            .unwrap_or_default(),
                        input_schema,
                        command: Arc::clone(def),
                        middleware: parent_middleware.to_vec(),
                        output_schema: def
                            .output_schema
                            .as_ref()
                            .and_then(|schema| schema.as_object().cloned().map(Arc::new)),
                        annotations: mcp.annotations.as_ref().map(|annotations| ToolAnnotations {
                            title: annotations.title.clone(),
                            read_only_hint: annotations.read_only_hint,
                            destructive_hint: annotations.destructive_hint,
                            idempotent_hint: annotations.idempotent_hint,
                            open_world_hint: annotations.open_world_hint,
                        }),
                        instructions: mcp.instructions.clone(),
                    });
                }
                crate::cli::CommandEntry::Group {
                    commands: sub,
                    middleware,
                    ..
                } => {
                    let mut merged_mw = parent_middleware.to_vec();
                    merged_mw.extend(middleware.iter().cloned());
                    result.extend(collect_resolved_tools(sub, &path, &merged_mw));
                }
                crate::cli::CommandEntry::FetchGateway { .. } => {
                    // Fetch gateways are not exposed as MCP tools.
                }
            }
        }

        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
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
        if let Some(instructions) = &tool.instructions {
            result.meta = Some(Meta(serde_json::Map::from_iter([(
                "instructions".to_string(),
                Value::String(instructions.clone()),
            )])));
        }
        result
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
            tool.annotations = Some(ToolAnnotations {
                title: None,
                read_only_hint: Some(read_only),
                destructive_hint: Some(!read_only),
                idempotent_hint: Some(read_only),
                open_world_hint: Some(!matches!(name, "search_tools" | "get_tool_details")),
            });
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
                    return Ok(Some(CallToolResult::error(vec![Content::text(
                        serde_json::json!({ "error": format!("Tool is not read-only: {tool_name}") }).to_string(),
                    )])));
                }
                if name == "call_write_tool" && read_only {
                    return Ok(Some(CallToolResult::error(vec![Content::text(
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

    fn tool_result_success(
        name: &str,
        data: Value,
        cta: Option<crate::output::CtaBlock>,
        structured: bool,
    ) -> CallToolResult {
        let text = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
        let cta = cta.map(|cta| formatted_cta(name, cta));
        let text = cta
            .as_ref()
            .map(|cta| format!("{text}\n\n{}", render_cta(cta)))
            .unwrap_or(text);
        CallToolResult {
            content: vec![Content::text(text)],
            structured_content: structured.then_some(data),
            is_error: Some(false),
            meta: cta.map(|cta| Meta(serde_json::Map::from_iter([("cta".to_string(), cta)]))),
        }
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
        CallToolResult {
            content: vec![Content::text(text)],
            structured_content: None,
            is_error: Some(true),
            meta: cta.map(|cta| Meta(serde_json::Map::from_iter([("cta".to_string(), cta)]))),
        }
    }

    // -----------------------------------------------------------------------
    // ServerHandler implementation
    // -----------------------------------------------------------------------

    /// The MCP server handler. Implements `rmcp::handler::server::ServerHandler`
    /// to respond to `initialize`, `tools/list`, and `tools/call` requests.
    #[derive(Clone)]
    pub(crate) struct IncurMcpServer {
        /// CLI name used in command execution context.
        cli_name: String,
        /// CLI version used in command execution context.
        cli_version: Option<String>,
        /// Server name (CLI name).
        server_name: String,
        /// Server version (CLI version).
        server_version: String,
        /// Resolved tools indexed by name for O(1) lookup during `tools/call`.
        tools_by_name: Arc<HashMap<String, Arc<ResolvedTool>>>,
        /// Pre-built list of `rmcp::model::Tool` for `tools/list` responses.
        tool_list: Arc<Vec<Tool>>,
        /// Root-level middleware from the CLI.
        root_middleware: Vec<MiddlewareFn>,
        /// CLI-level env field metadata.
        env_fields: Vec<FieldMeta>,
        /// Instructions returned during MCP initialization.
        instructions: Option<String>,
        /// Active discovery strategy.
        discovery: McpDiscovery,
    }

    impl IncurMcpServer {
        fn new(
            name: String,
            version: String,
            resolved_tools: Vec<ResolvedTool>,
            root_middleware: Vec<MiddlewareFn>,
            env_fields: Vec<FieldMeta>,
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
                cli_name: name.clone(),
                cli_version: Some(version.clone()),
                server_name: name,
                server_version: version,
                tools_by_name: Arc::new(tools_by_name),
                tool_list: Arc::new(tool_list),
                root_middleware,
                env_fields,
                instructions: options.instructions.clone(),
                discovery: options.tools.discovery,
            })
        }
    }

    impl ServerHandler for IncurMcpServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo {
                protocol_version: Default::default(),
                capabilities: ServerCapabilities::builder().enable_tools().build(),
                server_info: Implementation {
                    name: self.server_name.clone(),
                    version: self.server_version.clone(),
                    icons: None,
                    title: None,
                    website_url: None,
                },
                instructions: self.instructions.clone(),
            }
        }

        fn list_tools(
            &self,
            _request: Option<PaginatedRequestParam>,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_
        {
            let tools = (*self.tool_list).clone();
            std::future::ready(Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            }))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParam,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_
        {
            let tools_by_name = Arc::clone(&self.tools_by_name);
            let cli_name = self.cli_name.clone();
            let cli_version = self.cli_version.clone();
            let root_middleware = self.root_middleware.clone();
            let env_fields = self.env_fields.clone();
            let discovery = self.discovery;
            let progress_token = context.meta.get_progress_token();
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
            let peer = context.peer;
            let cancellation = context.ct;

            async move {
                let mut tool_name = request.name.to_string();
                let mut arguments = request.arguments;
                if discovery == McpDiscovery::Progressive {
                    if let Some(result) =
                        discovery_result(&tool_name, arguments.clone(), &tools_by_name)?
                    {
                        return Ok(result);
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

                // Convert arguments to BTreeMap<String, Value> for ExecuteOptions.
                let input_options: BTreeMap<String, Value> =
                    arguments.unwrap_or_default().into_iter().collect();

                // Collect all middleware: root + group + command.
                let mut all_middleware = root_middleware.clone();
                all_middleware.extend(tool.middleware.iter().cloned());
                all_middleware.extend(tool.command.middleware.iter().cloned());

                // Build the environment source from actual env vars.
                let env_source: HashMap<String, String> = std::env::vars().collect();

                let result = command::execute(
                    Arc::clone(&tool.command),
                    ExecuteOptions {
                        agent: true,
                        argv: vec![],
                        defaults: None,
                        display_name: cli_name.clone(),
                        env_fields: env_fields.clone(),
                        env_source,
                        format: Format::Json,
                        format_explicit: true,
                        globals: Value::Object(serde_json::Map::new()),
                        input_options,
                        middlewares: all_middleware,
                        name: cli_name.clone(),
                        parse_mode: ParseMode::Flat,
                        path: tool_name.clone(),
                        request: transport_request,
                        vars_fields: vec![],
                        version: cli_version,
                    },
                )
                .await;

                match result {
                    command::InternalResult::Ok { data, cta } => Ok(tool_result_success(
                        &cli_name,
                        data,
                        cta,
                        tool.output_schema.is_some(),
                    )),
                    command::InternalResult::Error {
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
                            text.push_str(
                                &serde_json::to_string(
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
                                .unwrap_or_default(),
                            );
                        }
                        Ok(tool_result_error(&cli_name, text, cta))
                    }
                    command::InternalResult::Stream(stream) => {
                        let mut stream = stream;
                        let mut chunks = Vec::new();
                        loop {
                            let chunk = tokio::select! {
                                _ = cancellation.cancelled() => break,
                                chunk = stream.next() => chunk,
                            };
                            let Some(chunk) = chunk else {
                                break;
                            };
                            chunks.push(chunk.clone());
                            if let Some(progress_token) = progress_token.clone() {
                                let _ = peer
                                    .notify_progress(ProgressNotificationParam {
                                        progress_token,
                                        progress: chunks.len() as f64,
                                        total: None,
                                        message: Some(
                                            serde_json::to_string(&chunk)
                                                .unwrap_or_else(|_| "null".to_string()),
                                        ),
                                    })
                                    .await;
                            }
                        }
                        let text = serde_json::to_string(&chunks).unwrap_or_else(|_| "[]".into());
                        Ok(CallToolResult::success(vec![Content::text(text)]))
                    }
                    command::InternalResult::RecordStream(stream) => {
                        let mut stream = stream;
                        let mut chunks = Vec::new();
                        let mut terminal = None;
                        loop {
                            let record = tokio::select! {
                                _ = cancellation.cancelled() => break,
                                record = stream.next() => record,
                            };
                            let Some(record) = record else { break };
                            match record {
                                crate::output::StreamRecord::Chunk(chunk) => {
                                    chunks.push(chunk.clone());
                                    if let Some(progress_token) = progress_token.clone() {
                                        let _ = peer
                                            .notify_progress(ProgressNotificationParam {
                                                progress_token,
                                                progress: chunks.len() as f64,
                                                total: None,
                                                message: Some(
                                                    serde_json::to_string(&chunk)
                                                        .unwrap_or_else(|_| "null".to_string()),
                                                ),
                                            })
                                            .await;
                                    }
                                }
                                record => {
                                    terminal = Some(record);
                                    break;
                                }
                            }
                        }
                        match terminal {
                            Some(crate::output::StreamRecord::Error { message, cta, .. }) => {
                                Ok(tool_result_error(&cli_name, message, cta))
                            }
                            terminal => {
                                let cta = match terminal {
                                    Some(crate::output::StreamRecord::Ok { cta }) => cta,
                                    _ => None,
                                };
                                Ok(tool_result_success(
                                    &cli_name,
                                    Value::Array(chunks),
                                    cta,
                                    tool.output_schema.is_some(),
                                ))
                            }
                        }
                    }
                }
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
    /// Each tool call executes the corresponding command via
    /// `command::execute()` with `ParseMode::Flat`.
    pub async fn serve(
        name: &str,
        version: &str,
        commands: &BTreeMap<String, crate::cli::CommandEntry>,
        root_middleware: &[MiddlewareFn],
        env_fields: &[FieldMeta],
        options: &McpServeOptions,
    ) -> Result<(), crate::errors::Error> {
        use rmcp::ServiceExt;
        use rmcp::transport::io::stdio;

        let resolved = collect_resolved_tools(commands, &[], &[]);

        let server = IncurMcpServer::new(
            name.to_string(),
            version.to_string(),
            resolved,
            root_middleware.to_vec(),
            env_fields.to_vec(),
            options,
        )?;

        let transport = stdio();

        let running = server.serve(transport).await.map_err(|e| {
            crate::errors::Error::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("MCP server failed to start: {e}"),
            )))
        })?;

        // Block until the client disconnects or the server is cancelled.
        let _quit_reason = running.waiting().await.map_err(|e| {
            crate::errors::Error::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("MCP server task failed: {e}"),
            )))
        })?;

        Ok(())
    }

    #[cfg(feature = "http")]
    pub(crate) fn http_service(
        name: &str,
        version: &str,
        commands: &BTreeMap<String, crate::cli::CommandEntry>,
        root_middleware: &[MiddlewareFn],
        env_fields: &[FieldMeta],
        options: &McpServeOptions,
    ) -> Result<
        rmcp::transport::StreamableHttpService<
            IncurMcpServer,
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
        >,
        crate::errors::Error,
    > {
        use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

        let resolved = collect_resolved_tools(commands, &[], &[]);
        let server = IncurMcpServer::new(
            name.to_string(),
            version.to_string(),
            resolved,
            root_middleware.to_vec(),
            env_fields.to_vec(),
            options,
        )?;
        Ok(StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            StreamableHttpServerConfig {
                stateful_mode: false,
                ..Default::default()
            },
        ))
    }
}

/// Builds a stateless MCP-over-HTTP service for a CLI.
#[cfg(all(feature = "mcp", feature = "http"))]
pub(crate) fn http_service(
    cli: &crate::cli::Cli,
) -> Result<
    rmcp::transport::StreamableHttpService<
        impl rmcp::Service<rmcp::RoleServer>,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    >,
    crate::errors::Error,
> {
    server::http_service(
        &cli.name,
        cli.version.as_deref().unwrap_or("0.0.0"),
        &cli.commands,
        &cli.middleware,
        &cli.env_fields,
        &cli.mcp_options,
    )
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
        name,
        version,
        commands,
        root_middleware,
        env_fields,
        options,
    )
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, to_kebab};

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
}
