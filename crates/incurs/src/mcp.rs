//! MCP (Model Context Protocol) stdio server.
//!
//! Ported from `src/Mcp.ts`. Exposes CLI commands as MCP tools over a stdio
//! transport. The actual server implementation uses the `rmcp` crate and is
//! gated behind the `mcp` feature flag.

use std::collections::BTreeMap;

use crate::schema::FieldMeta;

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

/// Options for the MCP server.
#[cfg(feature = "mcp")]
#[derive(Debug, Clone, Default)]
pub struct McpServeOptions {
    /// CLI version string.
    pub version: Option<String>,
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

    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParam, ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::{Peer, RequestContext, RoleServer};
    use rmcp::Error as McpError;

    use crate::command::{self, CommandDef, ExecuteOptions, ParseMode};
    use crate::middleware::MiddlewareFn;
    use crate::output::Format;
    use crate::schema::FieldMeta;

    use super::{build_tool_schema, McpServeOptions};

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
                    let tool_name = path.join("_");
                    let schema_value =
                        build_tool_schema(&def.args_fields, &def.options_fields);
                    let input_schema = match schema_value {
                        Value::Object(map) => Arc::new(map),
                        _ => Arc::new(serde_json::Map::new()),
                    };
                    result.push(ResolvedTool {
                        name: tool_name,
                        description: def.description.clone().unwrap_or_default(),
                        input_schema,
                        command: Arc::clone(def),
                        middleware: parent_middleware.to_vec(),
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

    // -----------------------------------------------------------------------
    // ServerHandler implementation
    // -----------------------------------------------------------------------

    /// The MCP server handler. Implements `rmcp::handler::server::ServerHandler`
    /// to respond to `initialize`, `tools/list`, and `tools/call` requests.
    #[derive(Clone)]
    struct IncurMcpServer {
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
        /// Peer handle for sending notifications (set after initialization).
        peer: Option<Peer<RoleServer>>,
    }

    impl IncurMcpServer {
        fn new(
            name: String,
            version: String,
            resolved_tools: Vec<ResolvedTool>,
            root_middleware: Vec<MiddlewareFn>,
            env_fields: Vec<FieldMeta>,
        ) -> Self {
            let tool_list: Vec<Tool> = resolved_tools
                .iter()
                .map(|t| {
                    Tool::new(
                        Cow::Owned(t.name.clone()),
                        Cow::Owned(t.description.clone()),
                        Arc::clone(&t.input_schema),
                    )
                })
                .collect();

            let mut tools_by_name = HashMap::new();
            for tool in resolved_tools {
                let name = tool.name.clone();
                tools_by_name.insert(name, Arc::new(tool));
            }

            IncurMcpServer {
                cli_name: name.clone(),
                cli_version: Some(version.clone()),
                server_name: name,
                server_version: version,
                tools_by_name: Arc::new(tools_by_name),
                tool_list: Arc::new(tool_list),
                root_middleware,
                env_fields,
                peer: None,
            }
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
                },
                instructions: None,
            }
        }

        fn get_peer(&self) -> Option<Peer<RoleServer>> {
            self.peer.clone()
        }

        fn set_peer(&mut self, peer: Peer<RoleServer>) {
            self.peer = Some(peer);
        }

        fn list_tools(
            &self,
            _request: PaginatedRequestParam,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
            let tools = (*self.tool_list).clone();
            std::future::ready(Ok(ListToolsResult {
                tools,
                next_cursor: None,
            }))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParam,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
            let tools_by_name = Arc::clone(&self.tools_by_name);
            let cli_name = self.cli_name.clone();
            let cli_version = self.cli_version.clone();
            let root_middleware = self.root_middleware.clone();
            let env_fields = self.env_fields.clone();

            async move {
                let tool_name = request.name.to_string();
                let tool = tools_by_name.get(&tool_name).ok_or_else(|| {
                    McpError::invalid_params(format!("Unknown tool: {tool_name}"), None)
                })?;

                // Convert arguments to BTreeMap<String, Value> for ExecuteOptions.
                let input_options: BTreeMap<String, Value> =
                    request.arguments.unwrap_or_default().into_iter().collect();

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
                        env_fields: env_fields.clone(),
                        env_source,
                        format: Format::Json,
                        format_explicit: true,
                        input_options,
                        middlewares: all_middleware,
                        name: cli_name,
                        parse_mode: ParseMode::Flat,
                        path: tool_name.clone(),
                        vars_fields: vec![],
                        version: cli_version,
                    },
                )
                .await;

                match result {
                    command::InternalResult::Ok { data, .. } => {
                        let text =
                            serde_json::to_string(&data).unwrap_or_else(|_| "null".into());
                        Ok(CallToolResult::success(vec![Content::text(text)]))
                    }
                    command::InternalResult::Error { message, .. } => {
                        let text = if message.is_empty() {
                            "Command failed".to_string()
                        } else {
                            message
                        };
                        Ok(CallToolResult::error(vec![Content::text(text)]))
                    }
                    command::InternalResult::Stream(stream) => {
                        // Buffer all stream chunks, then return the collected result.
                        let chunks: Vec<Value> = stream.collect().await;
                        let text =
                            serde_json::to_string(&chunks).unwrap_or_else(|_| "[]".into());
                        Ok(CallToolResult::success(vec![Content::text(text)]))
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
        _options: &McpServeOptions,
    ) -> Result<(), crate::errors::Error> {
        use rmcp::transport::io::stdio;
        use rmcp::ServiceExt;

        let resolved = collect_resolved_tools(commands, &[], &[]);

        let server = IncurMcpServer::new(
            name.to_string(),
            version.to_string(),
            resolved,
            root_middleware.to_vec(),
            env_fields.to_vec(),
        );

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
    server::serve(name, version, commands, root_middleware, env_fields, options).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{to_kebab, FieldType};

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
}
