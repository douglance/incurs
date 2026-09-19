//! An in-process MCP server for exercising the client without spawning anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use incurs_mcp_discovery::McpTransport;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptMessage, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, Role,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

use crate::connect::Connection;
use crate::health::{HealthCode, HealthReport, ServerHealth};
use crate::lazy::TransportFactory;

/// How a fixture server behaves when called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureBehavior {
    /// Echo the arguments back.
    Echo,
    /// Never respond, so only cancellation ends the call.
    Hang,
    /// Refuse the connection, as an unreachable server would.
    RefuseConnection,
}

/// A scriptable in-process MCP server.
///
/// Serving over `tokio::io::duplex` gives a real handshake, a real `tools/list`,
/// and a real `tools/call` with no filesystem, no subprocess, and no port, which
/// also keeps these tests working on every platform.
#[derive(Clone)]
pub struct FixtureServer {
    name: String,
    tools: Arc<Vec<Tool>>,
    behavior: FixtureBehavior,
    calls: Arc<AtomicUsize>,
    connects: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
    open: Arc<AtomicUsize>,
    resources: Arc<Vec<Resource>>,
    prompts: Arc<Vec<Prompt>>,
    supports_resources_and_prompts: bool,
}

impl FixtureServer {
    /// Creates a fixture exposing one echo tool per supplied name.
    #[must_use]
    pub fn new(name: &str, tools: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            tools: Arc::new(tools.iter().map(|tool| echo_tool(tool)).collect()),
            behavior: FixtureBehavior::Echo,
            calls: Arc::new(AtomicUsize::new(0)),
            connects: Arc::new(AtomicUsize::new(0)),
            attempts: Arc::new(AtomicUsize::new(0)),
            open: Arc::new(AtomicUsize::new(0)),
            resources: Arc::new(Vec::new()),
            prompts: Arc::new(Vec::new()),
            supports_resources_and_prompts: true,
        }
    }

    /// Makes the fixture answer resource and prompt requests the way a
    /// tools-only server does: with JSON-RPC method-not-found.
    #[must_use]
    pub fn without_resources_or_prompts(mut self) -> Self {
        self.supports_resources_and_prompts = false;
        self
    }

    /// Adds resources this fixture will list and read.
    #[must_use]
    pub fn with_resources(mut self, uris: &[&str]) -> Self {
        self.resources = Arc::new(
            uris.iter()
                .map(|uri| {
                    Resource::new((*uri).to_string(), (*uri).to_string())
                })
                .collect(),
        );
        self
    }

    /// Adds prompts this fixture will list and render.
    #[must_use]
    pub fn with_prompts(mut self, names: &[&str]) -> Self {
        self.prompts = Arc::new(
            names
                .iter()
                .map(|name| Prompt::new((*name).to_string(), None::<String>, None))
                .collect(),
        );
        self
    }

    /// Sets how the server responds.
    #[must_use]
    pub fn with_behavior(mut self, behavior: FixtureBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    /// Returns how many tool calls this fixture has served.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Returns how many times a client has connected to this fixture.
    ///
    /// Zero is the assertion that a configured server was never started.
    #[must_use]
    pub fn connect_count(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    /// Returns how many connections to this fixture are still open.
    ///
    /// Falling back to zero is the assertion that a close actually reached the
    /// server, which a caller cannot observe from its own side.
    #[must_use]
    pub fn open_connections(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    /// Returns how many times a client has *tried* to connect.
    ///
    /// Unlike [`Self::connect_count`] this counts refusals too, which is what a
    /// test of the negative cache needs: the whole point is that a second
    /// attempt never happens, and a refused attempt never reaches the counter
    /// that only successes increment.
    #[must_use]
    pub fn attempt_count(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

/// Builds a tool that returns its own arguments.
fn echo_tool(name: &str) -> Tool {
    Tool::new(
        name.to_string(),
        format!("Echoes its arguments back, for tests ({name})"),
        Arc::new(
            json!({"type": "object", "properties": {"value": {"type": "string"}}})
                .as_object()
                .expect("schema is an object")
                .clone(),
        ),
    )
    .annotate(rmcp::model::ToolAnnotations::new().read_only(true))
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
            .with_server_info(Implementation::new(self.name.clone(), "0.0.0"))
    }

    async fn list_tools(
        &self,
        _params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.behavior == FixtureBehavior::Hang {
            std::future::pending::<()>().await;
        }
        let arguments = request.arguments.unwrap_or_default();
        Ok(CallToolResult::structured(Value::Object(arguments)).into())
    }

    async fn list_resources(
        &self,
        _params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        if !self.supports_resources_and_prompts {
            return Err(ErrorData::method_not_found::<
                rmcp::model::ListResourcesRequestMethod,
            >());
        }
        Ok(ListResourcesResult {
            resources: self.resources.as_ref().clone(),
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            format!("contents of {}", request.uri),
            request.uri,
        )])
        .into())
    }

    async fn list_prompts(
        &self,
        _params: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        if !self.supports_resources_and_prompts {
            return Err(ErrorData::method_not_found::<
                rmcp::model::ListPromptsRequestMethod,
            >());
        }
        Ok(ListPromptsResult {
            prompts: self.prompts.as_ref().clone(),
            ..Default::default()
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!("prompt {} rendered", request.name),
        )])
        .with_description(format!("rendered {}", request.name))
        .into())
    }
}

/// A transport factory serving one fixture per configured server.
pub struct FixtureFactory {
    server: FixtureServer,
}

impl FixtureFactory {
    /// Wraps one fixture as a connection source.
    #[must_use]
    pub fn new(server: FixtureServer) -> Arc<Self> {
        Arc::new(Self { server })
    }
}

#[async_trait]
impl TransportFactory for FixtureFactory {
    async fn connect(&self, _transport: &McpTransport) -> Result<Arc<Connection>, HealthReport> {
        self.server.attempts.fetch_add(1, Ordering::SeqCst);
        if self.server.behavior == FixtureBehavior::RefuseConnection {
            return Err(HealthReport {
                state: ServerHealth::Unavailable,
                code: Some(HealthCode::SpawnNotFound),
                checked_at_ms: crate::cache::now_ms(),
                tool_count: None,
                from_cache: false,
            });
        }
        self.server.connects.fetch_add(1, Ordering::SeqCst);

        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = self.server.clone();
        let open = Arc::clone(&self.server.open);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_side).await {
                open.fetch_add(1, Ordering::SeqCst);
                let _ = running.waiting().await;
                open.fetch_sub(1, Ordering::SeqCst);
            }
        });
        crate::connect::Connection::from_client_transport(client_side)
            .await
            .map_err(|_| HealthReport {
                state: ServerHealth::InitializationFailed,
                code: Some(HealthCode::HandshakeFailed),
                checked_at_ms: crate::cache::now_ms(),
                tool_count: None,
                from_cache: false,
            })
    }
}
