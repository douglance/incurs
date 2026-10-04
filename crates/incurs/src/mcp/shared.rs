//! Transport-neutral MCP tool serving.
//!
//! The native `rmcp` server and the portable [`super::McpHttpServer`] both
//! resolve, list, and call tools through this module. It produces MCP wire
//! values as `serde_json::Value`; the native server converts them into `rmcp`
//! model types, and the portable server writes them directly.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use incurs_mcp_protocol::structured::{
    McpStructuredShape, project_output_schema, project_structured_content, projection_metadata,
};
use serde_json::{Map, Value, json};

use super::{McpDiscovery, McpServeOptions, McpToolFilter};
use crate::cli::ConfigOptions;
use crate::command::McpResultContent;
use crate::schema::FieldMeta;
use crate::tool::{
    ConfigSource, EnvironmentSource, ToolCallControl, ToolCallOptions, ToolCallOutcome,
    ToolCatalog, ToolDefinition, ToolEvent,
};

/// A resolved tool metadata entry. Execution goes through [`ToolCatalog`].
pub(crate) struct SharedTool {
    /// Original transport-neutral definition supplied to result mappers.
    definition: ToolDefinition,
    /// MCP-only structured representation; the catalog keeps its original schema.
    output_shape: Option<McpStructuredShape>,
    /// Tool name (path segments joined with `_`).
    pub(crate) name: String,
    /// Human-readable description.
    pub(crate) description: String,
    /// Merged JSON Schema for the tool's input.
    pub(crate) input_schema: Map<String, Value>,
    /// JSON Schema for structured MCP output when object-shaped.
    pub(crate) output_schema: Option<Map<String, Value>>,
    /// Behavioral annotations in MCP wire form.
    pub(crate) annotations: Option<Value>,
    /// Whether the annotations mark the tool read-only.
    pub(crate) read_only: bool,
    /// Tool-specific instructions exposed through metadata.
    pub(crate) instructions: Option<String>,
    /// Rich content derived from the successful structured result.
    pub(crate) result_content: Vec<McpResultContent>,
}

fn shared_tool(definition: &ToolDefinition) -> SharedTool {
    let projection = definition.output_schema.as_ref().map(project_output_schema);
    let annotations = definition.annotations.as_ref().map(|annotations| {
        let mut wire = Map::new();
        if let Some(title) = &annotations.title {
            wire.insert("title".to_string(), json!(title));
        }
        for (key, hint) in [
            ("readOnlyHint", annotations.read_only_hint),
            ("destructiveHint", annotations.destructive_hint),
            ("idempotentHint", annotations.idempotent_hint),
            ("openWorldHint", annotations.open_world_hint),
        ] {
            if let Some(hint) = hint {
                wire.insert(key.to_string(), Value::Bool(hint));
            }
        }
        Value::Object(wire)
    });
    SharedTool {
        definition: definition.clone(),
        output_shape: projection.as_ref().map(|projection| projection.shape),
        name: definition.name.clone(),
        description: definition.description.clone(),
        input_schema: definition
            .input_schema
            .as_object()
            .cloned()
            .unwrap_or_default(),
        output_schema: projection
            .as_ref()
            .and_then(|projection| projection.schema.as_object().cloned()),
        read_only: definition
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint)
            == Some(true),
        annotations,
        instructions: definition.instructions.clone(),
        result_content: definition.result_content.clone(),
    }
}

/// Borrowed CLI parts a server is built from.
pub(crate) struct ServerSource<'a> {
    pub(crate) name: &'a str,
    pub(crate) version: &'a str,
    pub(crate) commands: &'a BTreeMap<String, crate::cli::CommandEntry>,
    pub(crate) root_middleware: &'a [crate::middleware::MiddlewareFn],
    pub(crate) env_fields: &'a [FieldMeta],
    pub(crate) globals_fields: &'a [FieldMeta],
    pub(crate) config: Option<&'a ConfigOptions>,
}

impl<'a> ServerSource<'a> {
    /// Borrows every part of a complete CLI.
    pub(crate) fn from_cli(cli: &'a crate::cli::Cli) -> Self {
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

    /// Borrows a command tree without globals or config.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn from_parts(
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

pub(crate) fn wildcard_matches(pattern: &str, value: &str) -> bool {
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

pub(crate) fn filter_tools(tools: Vec<SharedTool>, filter: &McpToolFilter) -> Vec<SharedTool> {
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

/// The MCP `Tool` wire value for a directly exposed command.
pub(crate) fn direct_tool(tool: &SharedTool) -> Value {
    let mut wire = Map::new();
    wire.insert("name".to_string(), json!(tool.name));
    wire.insert("description".to_string(), json!(tool.description));
    wire.insert(
        "inputSchema".to_string(),
        Value::Object(tool.input_schema.clone()),
    );
    if let Some(schema) = &tool.output_schema {
        wire.insert("outputSchema".to_string(), Value::Object(schema.clone()));
    }
    if let Some(annotations) = &tool.annotations {
        wire.insert("annotations".to_string(), annotations.clone());
    }
    if let Some(metadata) = tool_metadata(tool) {
        wire.insert("_meta".to_string(), Value::Object(metadata));
    }
    Value::Object(wire)
}

/// Metadata shared by direct tool listing and detailed discovery.
fn tool_metadata(tool: &SharedTool) -> Option<Map<String, Value>> {
    let mut metadata = tool.output_shape.and_then(projection_metadata);
    if let Some(instructions) = &tool.instructions {
        metadata
            .get_or_insert_default()
            .insert("instructions".to_string(), json!(instructions));
    }
    metadata
}

/// The four MCP `Tool` wire values served under progressive discovery.
pub(crate) fn progressive_tools() -> Vec<Value> {
    let search = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "default": "" },
            "limit": { "type": "number", "default": 5 },
            "offset": { "type": "number", "default": 0 }
        }
    });
    let inspect = json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"]
    });
    let execute = json!({
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
        json!({
            "name": name,
            "description": description,
            "inputSchema": schema,
            "annotations": {
                "readOnlyHint": read_only,
                "destructiveHint": !read_only,
                "idempotentHint": read_only,
                "openWorldHint": !matches!(name, "search_tools" | "get_tool_details"),
            },
        })
    })
    .collect()
}

fn text_block(text: String) -> Value {
    json!({ "type": "text", "text": text })
}

/// A `CallToolResult` wire value, before any protocol-version adjustment.
fn call_tool_result(
    content: Vec<Value>,
    structured: Option<Value>,
    is_error: bool,
    meta: Option<Value>,
) -> Value {
    let mut wire = Map::new();
    wire.insert("resultType".to_string(), json!("complete"));
    wire.insert("content".to_string(), Value::Array(content));
    if let Some(structured) = structured {
        wire.insert("structuredContent".to_string(), structured);
    }
    wire.insert("isError".to_string(), Value::Bool(is_error));
    if let Some(meta) = meta {
        wire.insert("_meta".to_string(), meta);
    }
    Value::Object(wire)
}

fn structured_result(value: Value) -> Value {
    call_tool_result(
        vec![text_block(value.to_string())],
        Some(value),
        false,
        None,
    )
}

fn error_text_result(message: String) -> Value {
    call_tool_result(
        vec![text_block(json!({ "error": message }).to_string())],
        None,
        true,
        None,
    )
}

/// Answers a progressive discovery tool call.
///
/// Returns `Ok(None)` when the call must be forwarded to the named command,
/// and `Err(message)` for a JSON-RPC invalid-params error.
pub(crate) fn discovery_result(
    name: &str,
    arguments: Option<Map<String, Value>>,
    tools: &HashMap<String, Arc<SharedTool>>,
) -> Result<Option<Value>, String> {
    let arguments = arguments.unwrap_or_default();
    match name {
        "search_tools" => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            // Saturate rather than truncate: `usize` is 32 bits on wasm32, and
            // a wrapped offset would page differently than a native server.
            let page_arg = |name: &str, default: u64| {
                let value = arguments
                    .get(name)
                    .and_then(Value::as_u64)
                    .unwrap_or(default);
                usize::try_from(value).unwrap_or(usize::MAX)
            };
            let offset = page_arg("offset", 0);
            let limit = page_arg("limit", 5);
            let mut matches = tools
                .values()
                .filter(|tool| {
                    query.is_empty()
                        || tool.name.to_lowercase().contains(&query)
                        || tool.description.to_lowercase().contains(&query)
                })
                .map(|tool| {
                    json!({
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
            Ok(Some(structured_result(json!({
                "tools": page,
                "nextOffset": (offset + page.len() < total).then_some(offset + page.len()),
            }))))
        }
        "get_tool_details" => {
            let tool_name = arguments
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing tool name".to_string())?;
            let tool = tools
                .get(tool_name)
                .ok_or_else(|| format!("Unknown tool: {tool_name}"))?;
            let mut details = json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": Value::Object(tool.input_schema.clone()),
                "outputSchema": tool.output_schema.clone().map(Value::Object),
                "annotations": tool.annotations,
                "instructions": tool.instructions,
            });
            if let Some(metadata) = tool_metadata(tool) {
                details["_meta"] = Value::Object(metadata);
            }
            Ok(Some(structured_result(details)))
        }
        "call_read_tool" | "call_write_tool" => {
            let tool_name = arguments
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing tool name".to_string())?;
            let tool = tools
                .get(tool_name)
                .ok_or_else(|| format!("Unknown tool: {tool_name}"))?;
            if name == "call_read_tool" && !tool.read_only {
                return Ok(Some(error_text_result(format!(
                    "Tool is not read-only: {tool_name}"
                ))));
            }
            if name == "call_write_tool" && tool.read_only {
                return Ok(Some(error_text_result(format!(
                    "Tool is read-only: {tool_name}"
                ))));
            }
            Ok(None)
        }
        _ => Err(format!("Unknown discovery tool: {name}")),
    }
}

fn formatted_cta(name: &str, cta: crate::output::CtaBlock) -> Value {
    let commands = cta
        .commands
        .into_iter()
        .map(|entry| match entry {
            crate::output::CtaEntry::Simple(command) => json!({
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
                json!({ "command": command, "description": description })
            }
        })
        .collect::<Vec<_>>();
    json!({
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

/// A successful `CallToolResult` wire value for a command's data.
pub(crate) fn tool_result_success(
    name: &str,
    data: Value,
    cta: Option<crate::output::CtaBlock>,
    structured: bool,
    presentation: &[McpResultContent],
) -> Value {
    let text = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
    let cta = cta.map(|cta| formatted_cta(name, cta));
    let text = cta
        .as_ref()
        .map(|cta| format!("{text}\n\n{}", render_cta(cta)))
        .unwrap_or(text);
    let mut content = vec![text_block(text)];
    for item in presentation {
        match item {
            McpResultContent::Image {
                data_pointer,
                mime_type_pointer,
            }
            | McpResultContent::Audio {
                data_pointer,
                mime_type_pointer,
            } => {
                let Some(image_data) = data.pointer(data_pointer).and_then(Value::as_str) else {
                    continue;
                };
                let Some(mime_type) = data.pointer(mime_type_pointer).and_then(Value::as_str)
                else {
                    continue;
                };
                content.push(json!({
                    "type": if matches!(item, McpResultContent::Audio { .. }) { "audio" } else { "image" },
                    "data": image_data,
                    "mimeType": mime_type,
                }));
            }
            McpResultContent::ResourceLink {
                uri_pointer,
                name_pointer,
                mime_type_pointer,
            } => {
                let Some(uri) = data.pointer(uri_pointer).and_then(Value::as_str) else {
                    continue;
                };
                let Some(name) = data.pointer(name_pointer).and_then(Value::as_str) else {
                    continue;
                };
                let Some(mime_type) = data.pointer(mime_type_pointer).and_then(Value::as_str)
                else {
                    continue;
                };
                content.push(json!({
                    "type": "resource_link", "uri": uri, "name": name, "mimeType": mime_type,
                }));
            }
        }
    }
    call_tool_result(
        content,
        structured.then_some(data),
        false,
        cta.map(|cta| json!({ "cta": cta })),
    )
}

fn tool_result_error(
    name: &str,
    data: Value,
    cta: Option<crate::output::CtaBlock>,
    structured: bool,
) -> Value {
    let cta = cta.map(|cta| formatted_cta(name, cta));
    let mut content = vec![text_block(data.to_string())];
    if let Some(cta) = &cta {
        content.push(text_block(render_cta(cta)));
    }
    call_tool_result(
        content,
        structured.then_some(data),
        true,
        cta.map(|cta| json!({ "cta": cta })),
    )
}

/// The `CallToolResult` wire value for a finished tool invocation.
pub(crate) fn tool_call_result(
    name: &str,
    outcome: ToolCallOutcome,
    structured: bool,
    presentation: &[McpResultContent],
) -> Value {
    match outcome {
        ToolCallOutcome::Ok { data, cta } => {
            tool_result_success(name, data, cta, structured, presentation)
        }
        ToolCallOutcome::Error {
            code,
            message,
            retryable,
            field_errors,
            cta,
            exit_code,
        } => {
            let message = if message.is_empty() {
                "Command failed".to_string()
            } else {
                message
            };
            let mut data = json!(crate::output::ExecuteError {
                code,
                message,
                retryable,
                field_errors,
            });
            if let Some(exit_code) = exit_code {
                data["exit_code"] = json!(exit_code);
            }
            tool_result_error(name, data, cta, structured)
        }
    }
}

/// The progress-notification message for one tool event.
pub(crate) fn progress_message(event: ToolEvent) -> String {
    match event {
        ToolEvent::Chunk { data } => {
            serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string())
        }
        ToolEvent::Progress { message, .. } | ToolEvent::Log { message, .. } => message,
    }
}

/// Per-call inputs that come from the transport rather than the MCP request.
pub(crate) struct CallContext {
    /// Exact negotiated protocol version used to choose compatible content.
    pub(crate) protocol_version: String,
    /// Transport request metadata handed to the command.
    pub(crate) request: Option<crate::command::RequestContext>,
    /// Cancellation and event delivery for this call.
    pub(crate) control: ToolCallControl,
    /// Environment values for command environment fields.
    pub(crate) environment: EnvironmentSource,
}

/// Tool listing and invocation shared by every MCP server transport.
#[derive(Clone)]
pub(crate) struct ToolServer {
    /// Server name (CLI name).
    pub(crate) server_name: String,
    /// Server version (CLI version).
    pub(crate) server_version: String,
    /// Transport-neutral command catalog used for execution.
    catalog: ToolCatalog,
    /// Resolved tools indexed by name.
    tools_by_name: Arc<HashMap<String, Arc<SharedTool>>>,
    /// `tools/list` entries in MCP wire form.
    pub(crate) tool_list: Arc<Vec<Value>>,
    /// Instructions returned during MCP initialization.
    pub(crate) instructions: Option<String>,
    /// Active discovery strategy.
    discovery: McpDiscovery,
    /// Optional application classification applied after default rendering.
    result_mapper: Option<super::McpResultMapper>,
    /// Exact standards enabled for this server.
    pub(crate) standards: incurs_mcp_protocol::McpStandardSet,
}

impl ToolServer {
    /// Resolves a CLI's catalog and applies the MCP tool filter.
    pub(crate) fn new(
        source: &ServerSource<'_>,
        options: &McpServeOptions,
    ) -> Result<Self, crate::errors::Error> {
        let catalog = ToolCatalog::from_parts(
            source.name.to_string(),
            Some(source.version.to_string()),
            source.commands,
            source.root_middleware,
            source.env_fields,
            source.globals_fields,
            source.config,
        )
        .map_err(|error| {
            crate::errors::Error::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                error.to_string(),
            )))
        })?;
        // `ToolCatalog::resolved` borrows definitions but is native-only;
        // wasm32 reads the same definitions, in the same order, by value.
        #[cfg(not(target_arch = "wasm32"))]
        let resolved = catalog
            .resolved()
            .map(|tool| shared_tool(&tool.definition))
            .collect::<Vec<_>>();
        #[cfg(target_arch = "wasm32")]
        let resolved = catalog
            .definitions()
            .iter()
            .map(shared_tool)
            .collect::<Vec<_>>();
        let resolved = filter_tools(resolved, &options.tools);
        let mut names = HashSet::new();
        for tool in &resolved {
            if !names.insert(tool.name.clone()) {
                return Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Duplicate MCP tool name: {}", tool.name),
                ))));
            }
        }
        let tool_list = if options.tools.discovery == McpDiscovery::Direct {
            resolved.iter().map(direct_tool).collect()
        } else {
            progressive_tools()
        };
        let tools_by_name = resolved
            .into_iter()
            .map(|tool| (tool.name.clone(), Arc::new(tool)))
            .collect();
        Ok(Self {
            server_name: source.name.to_string(),
            server_version: source.version.to_string(),
            catalog,
            tools_by_name: Arc::new(tools_by_name),
            tool_list: Arc::new(tool_list),
            instructions: options.instructions.clone(),
            discovery: options.tools.discovery,
            result_mapper: options.result_mapper.clone(),
            standards: options.standards.clone(),
        })
    }

    /// Handles one `tools/call` request.
    ///
    /// Returns the `CallToolResult` wire value, or `Err(message)` for a
    /// JSON-RPC invalid-params error.
    pub(crate) async fn call_tool(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
        context: CallContext,
    ) -> Result<Value, String> {
        let mut tool_name = name;
        let mut arguments = arguments;
        if self.discovery == McpDiscovery::Progressive {
            if let Some(result) =
                discovery_result(&tool_name, arguments.clone(), &self.tools_by_name)?
            {
                return Ok(result);
            }
            let mut execute = arguments.unwrap_or_default();
            tool_name = execute
                .remove("name")
                .and_then(|value| value.as_str().map(ToString::to_string))
                .ok_or_else(|| "Missing tool name".to_string())?;
            arguments = execute
                .remove("arguments")
                .and_then(|value| value.as_object().cloned());
        }
        let tool = self
            .tools_by_name
            .get(&tool_name)
            .ok_or_else(|| format!("Unknown tool: {tool_name}"))?;
        let input_options: BTreeMap<String, Value> =
            arguments.unwrap_or_default().into_iter().collect();
        let outcome = self
            .catalog
            .call(
                &tool_name,
                input_options,
                ToolCallOptions {
                    environment: context.environment,
                    config: ConfigSource::Auto,
                    globals: None,
                    request: context.request,
                    control: context.control,
                },
            )
            .await;
        let mapping = self
            .result_mapper
            .as_ref()
            .map(|mapper| {
                mapper.map(super::McpResultContext {
                    tool: &tool.definition,
                    outcome: &outcome,
                })
            })
            .unwrap_or_default();
        let success = matches!(outcome, ToolCallOutcome::Ok { .. });
        let mut result = tool_call_result(
            &self.server_name,
            outcome,
            tool.output_schema.is_some(),
            &tool.result_content,
        );
        if success && let Some(shape) = tool.output_shape {
            let data = result["structuredContent"].take();
            match project_structured_content(data, shape) {
                Ok(projected) => {
                    result["structuredContent"] = projected;
                    if let Some(metadata) = projection_metadata(shape) {
                        if !result["_meta"].is_object() {
                            result["_meta"] = json!({});
                        }
                        result["_meta"].as_object_mut().unwrap().extend(metadata);
                    }
                }
                Err(error) => {
                    return Ok(tool_call_result(
                        &self.server_name,
                        ToolCallOutcome::Error {
                            code: "MCP_OUTPUT_SHAPE_INVALID".to_string(),
                            message: error.to_string(),
                            retryable: Some(false),
                            field_errors: None,
                            cta: None,
                            exit_code: None,
                        },
                        false,
                        &[],
                    ));
                }
            }
        }
        if let Some(is_error) = mapping.is_error {
            result["isError"] = json!(is_error);
        }
        if let Some(content) = result["content"].as_array_mut() {
            content.retain(|block| match block["type"].as_str() {
                Some("audio") => context.protocol_version.as_str() >= "2025-03-26",
                Some("resource_link") => context.protocol_version.as_str() >= "2025-06-18",
                _ => true,
            });
        }
        Ok(result)
    }
}
