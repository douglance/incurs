//! Establishing one MCP connection, and nothing before it is needed.

use std::sync::Arc;

use incurs_mcp_discovery::{McpTransport, StdioTransport};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo};
use rmcp::service::RunningService;
use serde_json::{Map, Value};

use crate::health::{HealthCode, ServerHealth, classify_handshake, classify_io};

/// A live connection to one MCP server.
pub struct Connection {
    service: RunningService<rmcp::RoleClient, ClientInfo>,
}

impl Connection {
    /// Lists the server's tools.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, String> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .service
                .list_tools(Some(
                    rmcp::model::PaginatedRequestParams::default().with_cursor(cursor),
                ))
                .await
                .map_err(|error| error.to_string())?;
            out.extend(page.tools);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// Calls one tool.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<CallToolResult, String> {
        self.service
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
            .await
            .map_err(|error| error.to_string())
    }

    /// Lists the server's resources.
    ///
    /// A server that declares no resource capability answers with an error
    /// rather than an empty page, so the caller treats any failure here as "this
    /// server has no resources" rather than as a fault.
    pub async fn list_resources(&self) -> Result<Vec<rmcp::model::Resource>, String> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .service
                .list_resources(Some(
                    rmcp::model::PaginatedRequestParams::default().with_cursor(cursor),
                ))
                .await
                .map_err(|error| error.to_string())?;
            out.extend(page.resources);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// Reads one resource by URI.
    pub async fn read_resource(
        &self,
        uri: &str,
    ) -> Result<rmcp::model::ReadResourceResult, String> {
        self.service
            .read_resource(rmcp::model::ReadResourceRequestParams::new(uri))
            .await
            .map_err(|error| error.to_string())
    }

    /// Lists the server's prompts.
    pub async fn list_prompts(&self) -> Result<Vec<rmcp::model::Prompt>, String> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .service
                .list_prompts(Some(
                    rmcp::model::PaginatedRequestParams::default().with_cursor(cursor),
                ))
                .await
                .map_err(|error| error.to_string())?;
            out.extend(page.prompts);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// Renders one prompt with arguments.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<rmcp::model::GetPromptResult, String> {
        self.service
            .get_prompt(
                rmcp::model::GetPromptRequestParams::new(name).with_arguments(arguments),
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Returns server-declared instructions, if any.
    pub fn instructions(&self) -> Option<String> {
        self.service
            .peer_info()
            .and_then(|info| info.instructions.clone())
    }

    /// Returns the negotiated protocol version.
    pub fn protocol_version(&self) -> Option<String> {
        self.service
            .peer_info()
            .map(|info| info.protocol_version.to_string())
    }

    /// Serves a client over an already-open byte stream.
    ///
    /// Used by in-process fixtures, which get a real handshake without a
    /// subprocess or a port.
    ///
    /// # Errors
    /// Returns an error when the MCP handshake does not complete.
    pub async fn from_client_transport<T>(transport: T) -> Result<Arc<Self>, String>
    where
        T: rmcp::transport::IntoTransport<
                rmcp::RoleClient,
                std::io::Error,
                rmcp::transport::async_rw::TransportAdapterAsyncCombinedRW,
            > + Send
            + 'static,
    {
        use rmcp::ServiceExt;
        ClientInfo::default()
            .serve(transport)
            .await
            .map(|service| Arc::new(Self { service }))
            .map_err(|error| error.to_string())
    }

    /// Closes the connection, reaping a child process where there is one.
    ///
    /// Takes `&self` because cancelling a token needs no ownership, and taking
    /// `self` made this unreachable: the only caller holds the connection in a
    /// `OnceCell`, so it could never be the last `Arc` and the close never ran.
    pub fn shutdown(&self) {
        self.service.cancellation_token().cancel();
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Connection")
    }
}

/// Why a connection could not be established.
#[derive(Debug, Clone, Copy)]
pub struct ConnectFailure {
    /// Coarse state to record.
    pub state: ServerHealth,
    /// Stable reason.
    pub code: HealthCode,
}

/// Builds the child process for a stdio server.
///
/// The environment is the parent's, overlaid with the configured values. That
/// inheritance is what makes a credential in the developer's shell profile keep
/// working without being copied anywhere: the server sees exactly what it would
/// have seen under the agent that configured it.
fn stdio_command(stdio: &StdioTransport) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&stdio.command);
    command.args(&stdio.args);
    if let Some(cwd) = &stdio.resolved_cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &stdio.env {
        command.env(key, value.expose());
    }
    command
}

/// Opens one connection.
///
/// Every transport is constructed inside this function, which callers only ever
/// invoke through [`crate::IoBridge`], so no transport is ever built on a thread
/// without an I/O driver.
pub async fn connect(transport: &McpTransport) -> Result<Arc<Connection>, ConnectFailure> {
    use rmcp::ServiceExt;

    let service = match transport {
        McpTransport::Stdio(stdio) => {
            let child =
                rmcp::transport::TokioChildProcess::new(stdio_command(stdio)).map_err(|error| {
                    ConnectFailure {
                        state: ServerHealth::Unavailable,
                        code: classify_io(&error),
                    }
                })?;
            ClientInfo::default().serve(child).await
        }
        // Not reachable through `LazyMcpClient`, which refuses an SSE entry
        // before it gets here, but `connect` is public and speaking the wrong
        // protocol at a server is worse than declining to speak at all.
        McpTransport::Sse(_) => {
            return Err(ConnectFailure {
                state: ServerHealth::InvalidConfiguration,
                code: HealthCode::TransportUnsupported,
            });
        }
        McpTransport::StreamableHttp(http) => {
            let mut config =
                rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                    http.url.clone(),
                );
            let mut headers = std::collections::HashMap::new();
            for (name, value) in &http.headers {
                let Ok(name) = http::HeaderName::try_from(name.as_str()) else {
                    return Err(ConnectFailure {
                        state: ServerHealth::InvalidConfiguration,
                        code: HealthCode::ConfigInvalid,
                    });
                };
                let Ok(parsed) = http::HeaderValue::from_str(value.expose()) else {
                    return Err(ConnectFailure {
                        state: ServerHealth::InvalidConfiguration,
                        code: HealthCode::ConfigInvalid,
                    });
                };
                headers.insert(name, parsed);
            }
            if !headers.is_empty() {
                config = config.custom_headers(headers);
            }
            ClientInfo::default()
                .serve(rmcp::transport::StreamableHttpClientTransport::from_config(
                    config,
                ))
                .await
        }
    };

    match service {
        Ok(service) => Ok(Arc::new(Connection { service })),
        Err(error) => {
            // The rendered error is read here and discarded; only the code escapes.
            let (state, code) = classify_handshake(&error.to_string());
            Err(ConnectFailure { state, code })
        }
    }
}

/// Converts one MCP call result into the value a Code Mode program receives.
///
/// Unlike the projection used for incurs' own remote commands, non-JSON text is
/// preserved as a string rather than collapsed to null, and every content block
/// is kept rather than only the first. A program that asked for a document
/// should receive the document.
pub fn tool_value(result: CallToolResult) -> Result<Value, String> {
    if result.is_error == Some(true) {
        return Err(result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_else(|| "the MCP tool reported an error".to_string()));
    }
    if let Some(structured) = result.structured_content {
        return Ok(structured);
    }
    let mut blocks: Vec<Value> = result
        .content
        .iter()
        .map(|block| {
            block.as_text().map_or_else(
                || serde_json::to_value(block).unwrap_or(Value::Null),
                |text| {
                    serde_json::from_str(&text.text)
                        .unwrap_or_else(|_| Value::String(text.text.clone()))
                },
            )
        })
        .collect();
    Ok(match blocks.len() {
        0 => Value::Null,
        1 => blocks.remove(0),
        _ => Value::Array(blocks),
    })
}

/// Converts an rmcp tool declaration into Code Mode's tool metadata.
pub fn to_mcp_tool(tool: rmcp::model::Tool) -> incurs_codemode::McpTool {
    let annotations = tool
        .annotations
        .map(|annotations| incurs::command::McpAnnotations {
            title: annotations.title.clone(),
            read_only_hint: annotations.read_only_hint,
            destructive_hint: annotations.destructive_hint,
            idempotent_hint: annotations.idempotent_hint,
            open_world_hint: annotations.open_world_hint,
        });
    incurs_codemode::McpTool {
        name: tool.name.to_string(),
        description: tool.description.map(|text| text.to_string()),
        input_schema: serde_json::to_value(&tool.input_schema).unwrap_or(Value::Null),
        output_schema: tool
            .output_schema
            .and_then(|schema| serde_json::to_value(&schema).ok()),
        annotations,
    }
}
