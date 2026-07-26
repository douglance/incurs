use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use incurs::command::RequestContext;
use incurs::tool::{ToolCallControl, ToolCallOptions, ToolCallOutcome, ToolCatalog};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Replay behavior for a connector call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPolicy {
    /// Record the result and replay it without repeating the call.
    #[default]
    Log,
    /// Re-execute the call during every replay pass.
    Reexecute,
}

/// Runtime annotations for one connector method.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// Whether the method is declared read-only.
    pub read_only: Option<bool>,
    /// Whether the method may perform destructive updates.
    pub destructive: Option<bool>,
    /// Whether repeated calls have no additional effect.
    pub idempotent: Option<bool>,
    /// Whether the method may interact with external entities.
    pub open_world: Option<bool>,
}

/// Resolved approval and deterministic replay policy for one method.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Whether the call must pause for approval.
    pub requires_approval: bool,
    /// How the result participates in replay.
    pub replay: ReplayPolicy,
}

/// Trust boundary used while resolving tool policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOrigin {
    /// Metadata declared by the local incurs command graph.
    Local,
    /// Metadata received from a remote MCP server.
    RemoteMcp,
    /// Metadata inferred from a remote OpenAPI document.
    OpenApi,
}

/// Resolves approval and replay behavior from tool metadata.
pub trait ToolPolicyResolver: Send + Sync {
    /// Resolves one method's effective policy.
    fn resolve(&self, origin: ToolOrigin, annotations: &ToolAnnotations) -> ToolPolicy;
}

/// Conservative default policy resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultToolPolicyResolver;

impl ToolPolicyResolver for DefaultToolPolicyResolver {
    fn resolve(&self, origin: ToolOrigin, annotations: &ToolAnnotations) -> ToolPolicy {
        let safe_local_read = origin == ToolOrigin::Local
            && annotations.read_only == Some(true)
            && annotations.destructive != Some(true)
            && annotations.open_world != Some(true);
        ToolPolicy {
            requires_approval: !safe_local_read,
            replay: if safe_local_read {
                ReplayPolicy::Reexecute
            } else {
                ReplayPolicy::Log
            },
        }
    }
}

/// One model-facing connector usage example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorExample {
    /// Example invocation.
    pub command: String,
    /// Explanation of the example.
    pub description: Option<String>,
}

/// Metadata for one connector method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorTool {
    /// Method name within its connector namespace.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for arguments.
    pub input_schema: Value,
    /// JSON Schema for successful output.
    pub output_schema: Option<Value>,
    /// Tool-specific model instructions.
    pub instructions: Option<String>,
    /// Usage examples.
    pub examples: Vec<ConnectorExample>,
    /// Source behavioral annotations.
    pub annotations: ToolAnnotations,
    /// Resolved approval and replay behavior.
    pub policy: ToolPolicy,
}

/// Model-visible description of a connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorDescription {
    /// JavaScript namespace used in sandbox code.
    pub name: String,
    /// Connector-level model guidance.
    pub instructions: Option<String>,
    /// Methods exposed by the connector.
    pub tools: Vec<ConnectorTool>,
}

/// Stable execution context passed to connector calls and compensation.
#[derive(Clone)]
pub struct ToolContext {
    /// Durable execution identifier, stable across replay passes.
    pub execution_id: String,
    /// Execution-scoped cancellation and ordered event delivery.
    pub control: ToolCallControl,
    /// Transport request metadata inherited by connector calls.
    pub request: Option<RequestContext>,
}

/// A transport-neutral source of sandbox-callable tools.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Returns connector metadata and schemas.
    async fn describe(&self) -> Result<ConnectorDescription, String>;

    /// Executes one connector method.
    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<Value, String>;

    /// Compensates a previously applied connector action.
    async fn revert(
        &self,
        _method: &str,
        _arguments: Value,
        _result: Value,
        _context: &ToolContext,
    ) -> Result<bool, String> {
        Ok(false)
    }

    /// Releases resources scoped to one sandbox pass.
    async fn pass_ended(&self, _execution_id: &str, _status: &str) {}

    /// Releases resources scoped to a complete execution.
    async fn execution_ended(&self, _execution_id: &str, _status: &str) {}
}

/// Adapts an incurs command graph into one Code Mode connector.
#[derive(Clone)]
pub struct IncurConnector {
    catalog: ToolCatalog,
    name: String,
    instructions: Option<String>,
    options: ToolCallOptions,
    policy: Arc<dyn ToolPolicyResolver>,
}

impl IncurConnector {
    /// Creates a connector from a resolved incurs tool catalog.
    pub fn new(catalog: ToolCatalog) -> Self {
        let name = sanitize_namespace(catalog.name());
        Self {
            catalog,
            name,
            instructions: None,
            options: ToolCallOptions::default(),
            policy: Arc::new(DefaultToolPolicyResolver),
        }
    }

    /// Overrides the JavaScript namespace exposed to sandbox programs.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Adds connector-level instructions to model-facing documentation.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Sets the call options shared by every incurs command invocation.
    pub fn with_call_options(mut self, options: ToolCallOptions) -> Self {
        self.options = options;
        self
    }

    /// Overrides policy resolution for local incurs tools.
    pub fn with_policy_resolver(mut self, resolver: Arc<dyn ToolPolicyResolver>) -> Self {
        self.policy = resolver;
        self
    }
}

#[async_trait]
impl Connector for IncurConnector {
    async fn describe(&self) -> Result<ConnectorDescription, String> {
        Ok(ConnectorDescription {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            tools: self
                .catalog
                .definitions()
                .into_iter()
                .map(|tool| {
                    let annotations = ToolAnnotations {
                        read_only: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.read_only_hint),
                        destructive: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.destructive_hint),
                        idempotent: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.idempotent_hint),
                        open_world: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.open_world_hint),
                    };
                    ConnectorTool {
                        name: tool.name,
                        description: (!tool.description.is_empty()).then_some(tool.description),
                        input_schema: tool.input_schema,
                        output_schema: tool.output_schema,
                        instructions: tool.instructions,
                        examples: tool
                            .examples
                            .into_iter()
                            .map(|example| ConnectorExample {
                                command: example.command,
                                description: example.description,
                            })
                            .collect(),
                        policy: self.policy.resolve(ToolOrigin::Local, &annotations),
                        annotations,
                    }
                })
                .collect(),
        })
    }

    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<Value, String> {
        let arguments = arguments
            .as_object()
            .ok_or_else(|| format!("Arguments to {method} must be an object"))?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut options = self.options.clone();
        options.control = context.control.clone();
        if context.request.is_some() {
            options.request = context.request.clone();
        }
        match self.catalog.call(method, arguments, options).await {
            ToolCallOutcome::Ok { data, cta } => {
                if cta.is_some() {
                    Ok(serde_json::json!({ "data": data, "cta": cta }))
                } else {
                    Ok(data)
                }
            }
            ToolCallOutcome::Error {
                code,
                message,
                retryable,
                field_errors,
                cta,
                exit_code,
            } => Err(serde_json::json!({
                "code": code,
                "message": message,
                "retryable": retryable,
                "fieldErrors": field_errors,
                "cta": cta,
                "exitCode": exit_code,
            })
            .to_string()),
        }
    }
}

/// Validates and normalizes a namespace from a CLI or connector name.
pub fn sanitize_namespace(value: &str) -> String {
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        if (index == 0 && !(ch == '_' || ch == '$' || ch.is_ascii_alphabetic()))
            || (index > 0 && !(ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
        {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    if result.is_empty() {
        "tools".to_string()
    } else {
        result
    }
}

/// MCP tool metadata used by the transport-neutral MCP adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Original MCP tool name.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// MCP input schema.
    pub input_schema: Value,
    /// Optional MCP output schema.
    pub output_schema: Option<Value>,
    /// MCP behavioral annotations.
    pub annotations: Option<incurs::command::McpAnnotations>,
}

/// Minimal MCP client contract required by Code Mode.
#[async_trait]
pub trait McpClient: Send + Sync {
    /// Lists tools from the remote MCP server.
    async fn list_tools(&self) -> Result<Vec<McpTool>, String>;

    /// Calls one remote MCP tool with object arguments.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, String>;
}

/// Exposes a remote MCP connection as a Code Mode connector.
pub struct McpConnector {
    name: String,
    instructions: Option<String>,
    client: Arc<dyn McpClient>,
    tools: tokio::sync::OnceCell<Vec<(String, McpTool)>>,
    policy: Arc<dyn ToolPolicyResolver>,
}

impl McpConnector {
    /// Creates an MCP-backed connector.
    pub fn new(name: impl Into<String>, client: Arc<dyn McpClient>) -> Self {
        Self {
            name: name.into(),
            instructions: None,
            client,
            tools: tokio::sync::OnceCell::new(),
            policy: Arc::new(DefaultToolPolicyResolver),
        }
    }

    /// Adds server-level model instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Overrides policy resolution for remote MCP tools.
    pub fn with_policy_resolver(mut self, resolver: Arc<dyn ToolPolicyResolver>) -> Self {
        self.policy = resolver;
        self
    }

    async fn tools(&self) -> Result<&Vec<(String, McpTool)>, String> {
        self.tools
            .get_or_try_init(|| async {
                let mut names = BTreeMap::new();
                let mut tools = Vec::new();
                for tool in self.client.list_tools().await? {
                    let name = sanitize_namespace(&tool.name);
                    if let Some(existing) = names.insert(name.clone(), tool.name.clone()) {
                        return Err(format!(
                            "MCP tools \"{existing}\" and \"{}\" both map to \"{name}\"",
                            tool.name
                        ));
                    }
                    tools.push((name, tool));
                }
                Ok(tools)
            })
            .await
    }
}

#[async_trait]
impl Connector for McpConnector {
    async fn describe(&self) -> Result<ConnectorDescription, String> {
        Ok(ConnectorDescription {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            tools: self
                .tools()
                .await?
                .iter()
                .map(|(name, tool)| {
                    let annotations = ToolAnnotations {
                        read_only: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.read_only_hint),
                        destructive: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.destructive_hint),
                        idempotent: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.idempotent_hint),
                        open_world: tool
                            .annotations
                            .as_ref()
                            .and_then(|annotations| annotations.open_world_hint),
                    };
                    ConnectorTool {
                        name: name.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                        output_schema: tool.output_schema.clone(),
                        instructions: None,
                        examples: Vec::new(),
                        policy: self.policy.resolve(ToolOrigin::RemoteMcp, &annotations),
                        annotations,
                    }
                })
                .collect(),
        })
    }

    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        _context: &ToolContext,
    ) -> Result<Value, String> {
        let (_, tool) = self
            .tools()
            .await?
            .iter()
            .find(|(name, _)| name == method)
            .ok_or_else(|| format!("Tool \"{method}\" not found on {}", self.name))?;
        self.client.call_tool(&tool.name, arguments).await
    }
}

/// Authenticated request derived from an OpenAPI operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiRequest {
    /// Path or URL with path parameters substituted.
    pub path: String,
    /// HTTP method.
    pub method: String,
    /// Query parameters.
    pub parameters: BTreeMap<String, Value>,
    /// Optional request body.
    pub body: Option<Value>,
    /// Request headers derived from header parameters.
    pub headers: BTreeMap<String, String>,
}

/// Host operations required by the OpenAPI connector.
#[async_trait]
pub trait OpenApiClient: Send + Sync {
    /// Returns the OpenAPI document.
    async fn specification(&self) -> Result<Value, String>;

    /// Executes an authenticated request.
    async fn request(&self, request: OpenApiRequest) -> Result<Value, String>;
}

#[derive(Clone)]
struct OpenApiOperation {
    name: String,
    method: String,
    path: String,
    description: String,
    input_schema: Value,
    parameters: Vec<(String, String)>,
}

/// Derives typed Code Mode tools from an OpenAPI document.
pub struct OpenApiConnector {
    name: String,
    instructions: Option<String>,
    client: Arc<dyn OpenApiClient>,
    operations: tokio::sync::OnceCell<Vec<OpenApiOperation>>,
    policy: Arc<dyn ToolPolicyResolver>,
}

impl OpenApiConnector {
    /// Creates an OpenAPI-backed connector.
    pub fn new(name: impl Into<String>, client: Arc<dyn OpenApiClient>) -> Self {
        Self {
            name: name.into(),
            instructions: None,
            client,
            operations: tokio::sync::OnceCell::new(),
            policy: Arc::new(DefaultToolPolicyResolver),
        }
    }

    /// Adds API-level model instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Overrides policy resolution for OpenAPI tools.
    pub fn with_policy_resolver(mut self, resolver: Arc<dyn ToolPolicyResolver>) -> Self {
        self.policy = resolver;
        self
    }

    async fn operations(&self) -> Result<&Vec<OpenApiOperation>, String> {
        self.operations
            .get_or_try_init(|| async {
                derive_openapi_operations(&self.client.specification().await?)
            })
            .await
    }
}

#[async_trait]
impl Connector for OpenApiConnector {
    async fn describe(&self) -> Result<ConnectorDescription, String> {
        let mut tools = vec![ConnectorTool {
            name: "request".to_string(),
            description: Some(
                "Perform an authenticated request when no derived operation fits.".to_string(),
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "method": {"type": "string"},
                    "parameters": {"type": "object", "additionalProperties": true},
                    "body": {},
                    "headers": {"type": "object", "additionalProperties": {"type": "string"}}
                },
                "required": ["path"]
            }),
            output_schema: None,
            instructions: None,
            examples: Vec::new(),
            annotations: ToolAnnotations {
                open_world: Some(true),
                ..ToolAnnotations::default()
            },
            policy: ToolPolicy {
                requires_approval: true,
                replay: ReplayPolicy::Log,
            },
        }];
        tools.extend(self.operations().await?.iter().map(|operation| {
            let annotations = ToolAnnotations {
                read_only: Some(operation.method == "get" || operation.method == "head"),
                open_world: Some(true),
                ..ToolAnnotations::default()
            };
            ConnectorTool {
                name: operation.name.clone(),
                description: Some(operation.description.clone()),
                input_schema: operation.input_schema.clone(),
                output_schema: None,
                instructions: None,
                examples: Vec::new(),
                policy: self.policy.resolve(ToolOrigin::OpenApi, &annotations),
                annotations,
            }
        }));
        Ok(ConnectorDescription {
            name: self.name.clone(),
            instructions: self.instructions.clone(),
            tools,
        })
    }

    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        _context: &ToolContext,
    ) -> Result<Value, String> {
        if method == "request" {
            return self.client.request(parse_raw_request(arguments)?).await;
        }
        let operation = self
            .operations()
            .await?
            .iter()
            .find(|operation| operation.name == method)
            .ok_or_else(|| format!("Tool \"{method}\" not found on {}", self.name))?;
        self.client
            .request(operation_request(operation, arguments)?)
            .await
    }
}

fn derive_openapi_operations(document: &Value) -> Result<Vec<OpenApiOperation>, String> {
    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut used = BTreeMap::new();
    let mut operations = Vec::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for method in ["get", "put", "post", "delete", "patch", "options", "head"] {
            let Some(operation) = item.get(method).and_then(Value::as_object) else {
                continue;
            };
            let source_name = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{method}_{path}"));
            let name = sanitize_namespace(&source_name);
            if name == "request" || name == "spec" || used.insert(name.clone(), path).is_some() {
                continue;
            }
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            let mut parameters = Vec::new();
            for parameter in operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(parameter) = parameter.as_object() else {
                    continue;
                };
                let Some(parameter_name) = parameter.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let location = parameter
                    .get("in")
                    .and_then(Value::as_str)
                    .unwrap_or("query");
                properties.insert(
                    parameter_name.to_string(),
                    parameter
                        .get("schema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                );
                parameters.push((parameter_name.to_string(), location.to_string()));
                if parameter.get("required").and_then(Value::as_bool) == Some(true) {
                    required.push(Value::String(parameter_name.to_string()));
                }
            }
            if let Some(body) = operation
                .get("requestBody")
                .and_then(|value| value.get("content"))
                .and_then(|value| value.get("application/json"))
                .and_then(|value| value.get("schema"))
                .cloned()
            {
                properties.insert("body".to_string(), body);
                if operation
                    .get("requestBody")
                    .and_then(|value| value.get("required"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    required.push(Value::String("body".to_string()));
                }
            }
            operations.push(OpenApiOperation {
                name,
                method: method.to_string(),
                path: path.clone(),
                description: operation
                    .get("summary")
                    .or_else(|| operation.get("description"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase())),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }),
                parameters,
            });
        }
    }
    Ok(operations)
}

fn operation_request(
    operation: &OpenApiOperation,
    arguments: Value,
) -> Result<OpenApiRequest, String> {
    let input = arguments
        .as_object()
        .ok_or_else(|| format!("Arguments to {} must be an object", operation.name))?;
    let mut path = operation.path.clone();
    let mut parameters = BTreeMap::new();
    let mut headers = BTreeMap::new();
    for (name, location) in &operation.parameters {
        let Some(value) = input.get(name) else {
            continue;
        };
        match location.as_str() {
            "path" => {
                path = path.replace(
                    &format!("{{{name}}}"),
                    value.as_str().unwrap_or(&value.to_string()),
                )
            }
            "header" => {
                headers.insert(
                    name.clone(),
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
            "query" => {
                parameters.insert(name.clone(), value.clone());
            }
            _ => {}
        }
    }
    Ok(OpenApiRequest {
        path,
        method: operation.method.clone(),
        parameters,
        body: input.get("body").cloned(),
        headers,
    })
}

fn parse_raw_request(arguments: Value) -> Result<OpenApiRequest, String> {
    let input = arguments
        .as_object()
        .ok_or_else(|| "Arguments to request must be an object".to_string())?;
    Ok(OpenApiRequest {
        path: input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "request.path is required".to_string())?
            .to_string(),
        method: input
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_string(),
        parameters: object_map(input.get("parameters")),
        body: input.get("body").cloned(),
        headers: input
            .get("headers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string()),
                )
            })
            .collect(),
    })
}

fn object_map(value: Option<&Value>) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn only_safe_local_reads_skip_approval() {
        let resolver = DefaultToolPolicyResolver;
        let read = ToolAnnotations {
            read_only: Some(true),
            ..ToolAnnotations::default()
        };
        assert_eq!(
            resolver.resolve(ToolOrigin::Local, &read),
            ToolPolicy {
                requires_approval: false,
                replay: ReplayPolicy::Reexecute,
            }
        );
        assert!(
            resolver
                .resolve(ToolOrigin::RemoteMcp, &read)
                .requires_approval
        );
        assert!(
            resolver
                .resolve(
                    ToolOrigin::Local,
                    &ToolAnnotations {
                        open_world: Some(true),
                        ..read
                    }
                )
                .requires_approval
        );
    }
}
