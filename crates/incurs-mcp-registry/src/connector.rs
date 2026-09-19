//! A connector that reports its server's unavailability to the model.

use std::sync::Arc;

use async_trait::async_trait;
use incurs_codemode::{
    Connector, ConnectorDescription, ConnectorTool, McpConnector, ReplayPolicy, ToolAnnotations,
    ToolContext, ToolPolicy,
};
use incurs_mcp_client::{HealthRegistry, LazyMcpClient, ServerHealth};
use incurs_mcp_discovery::ServerId;
use serde_json::Value;

/// Wraps an MCP connector so an empty namespace explains itself.
///
/// When a server cannot be reached it exposes no tools, which renders as
/// `declare const github: {};`. That is accurate but silent, and a model faced
/// with an empty namespace will invent an explanation. This wrapper replaces the
/// connector's instructions with the recorded reason, which flows through
/// `generate_types` into `codemode_search` results.
///
/// It is a wrapper rather than a call to `McpConnector::with_instructions` for a
/// concrete reason: the failure is not known at construction time, and
/// `McpConnector` memoizes its tool list through a `OnceCell` that treats an
/// empty list as success — so a server that recovered would stay empty for the
/// life of the process.
pub struct HealthAwareMcpConnector {
    id: ServerId,
    namespace: String,
    inner: McpConnector,
    health: Arc<HealthRegistry>,
    instructions: Option<String>,
    client: Arc<LazyMcpClient>,
}

/// The resource and prompt methods added to every reachable namespace.
///
/// MCP servers expose three kinds of thing and Code Mode only ever reached one
/// of them. A server's resources and prompts were unreachable from a program
/// however it was written, which made "compose everything this machine offers"
/// false for any server whose value is its resources. These names are prefixed
/// because a namespace is flat: a server with its own `prompts` tool must keep
/// it, so a synthetic method is dropped whenever the server already uses the
/// name.
const MCP_METHODS: [(&str, &str); 4] = [
    (
        "mcp_resources",
        "List the resources this MCP server exposes.",
    ),
    (
        "mcp_read_resource",
        "Read one resource from this MCP server by URI.",
    ),
    ("mcp_prompts", "List the prompts this MCP server exposes."),
    ("mcp_get_prompt", "Render one prompt from this MCP server."),
];

/// Builds the synthetic method definitions, skipping any name the server uses.
fn mcp_methods(existing: &[ConnectorTool]) -> Vec<ConnectorTool> {
    MCP_METHODS
        .iter()
        .filter(|(name, _)| !existing.iter().any(|tool| tool.name == *name))
        .map(|(name, description)| {
            let input_schema = match *name {
                "mcp_read_resource" => serde_json::json!({
                    "type": "object",
                    "properties": { "uri": { "type": "string" } },
                    "required": ["uri"],
                    "additionalProperties": false
                }),
                "mcp_get_prompt" => serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "arguments": { "type": "object" }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                _ => serde_json::json!({
                    "type": "object", "properties": {}, "additionalProperties": false
                }),
            };
            ConnectorTool {
                name: (*name).to_string(),
                description: Some((*description).to_string()),
                input_schema,
                output_schema: None,
                instructions: None,
                examples: Vec::new(),
                // Listing and reading are reads by definition in MCP: a resource
                // read has no side effect and a prompt render returns text. That
                // is a property of the protocol rather than a claim by this
                // server, so it does not depend on the server annotating it.
                annotations: ToolAnnotations {
                    read_only: Some(true),
                    destructive: Some(false),
                    idempotent: Some(true),
                    open_world: Some(false),
                },
                policy: ToolPolicy {
                    requires_approval: false,
                    replay: ReplayPolicy::Reexecute,
                },
            }
        })
        .collect()
}

impl HealthAwareMcpConnector {
    /// Wraps one MCP connector.
    #[must_use]
    pub fn new(
        id: ServerId,
        namespace: String,
        inner: McpConnector,
        health: Arc<HealthRegistry>,
        client: Arc<LazyMcpClient>,
    ) -> Self {
        Self {
            id,
            namespace,
            inner,
            health,
            instructions: None,
            client,
        }
    }

    /// Adds server-level guidance shown before the declarations.
    #[must_use]
    pub fn with_instructions(mut self, instructions: Option<String>) -> Self {
        self.instructions = instructions;
        self
    }
}

#[async_trait]
impl Connector for HealthAwareMcpConnector {
    fn name(&self) -> &str {
        &self.namespace
    }

    async fn describe(&self) -> Result<ConnectorDescription, String> {
        let mut description = self.inner.describe().await?;
        description.name = self.namespace.clone();
        if description.tools.is_empty() {
            let reason = self
                .health
                .get(&self.id)
                .filter(|report| report.state != ServerHealth::Healthy)
                .map_or("it could not be reached", |report| report.summary());
            description.instructions = Some(format!(
                "UNAVAILABLE: the {} server exposes no tools right now because {reason}. \
                 Run `cmpst check` to see why, and do not invent its methods.",
                self.namespace
            ));
        } else {
            if description.instructions.is_none() {
                description.instructions = self.instructions.clone();
            }
            // Only a reachable server gets them: adding methods to a namespace
            // that already explains it is unavailable would invite calls that
            // cannot succeed.
            description.tools.extend(mcp_methods(&description.tools));
        }
        Ok(description)
    }

    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<Value, String> {
        // Only intercept a synthetic name the server did not claim. `describe`
        // already declines to advertise a name the server uses, and dispatching
        // without the same check would advertise the server's tool and then call
        // something else.
        if MCP_METHODS.iter().any(|(name, _)| *name == method) {
            let server_owns_it = self
                .inner
                .describe()
                .await
                .map(|description| description.tools.iter().any(|tool| tool.name == method))
                .unwrap_or(false);
            if server_owns_it {
                return self.inner.execute(method, arguments, context).await;
            }
        }
        match method {
            "mcp_resources" => self.client.resources().await,
            "mcp_prompts" => self.client.prompts().await,
            "mcp_read_resource" => {
                let uri = arguments
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or("`uri` is required and must be a string")?;
                self.client.read_resource(uri).await
            }
            "mcp_get_prompt" => {
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("`name` is required and must be a string")?;
                let args = arguments.get("arguments").cloned().unwrap_or(Value::Null);
                self.client.get_prompt(name, args).await
            }
            _ => self.inner.execute(method, arguments, context).await,
        }
    }

    async fn revert(
        &self,
        method: &str,
        arguments: Value,
        result: Value,
        context: &ToolContext,
    ) -> Result<bool, String> {
        self.inner.revert(method, arguments, result, context).await
    }
}
