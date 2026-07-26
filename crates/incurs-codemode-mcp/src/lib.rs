//! MCP transport for the generic incurs Code Mode lifecycle.
//!
//! The server exposes exactly five stable tools. Hosts can serve the handler
//! over stdio or reuse it with another `rmcp` server transport.

use std::collections::HashMap;
use std::sync::Arc;

use incurs::command::RequestContext as IncurRequestContext;
use incurs_codemode::{CodeModeRunOptions, CodeModeService};
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

/// Stable names of the Code Mode lifecycle tools.
pub const TOOL_NAMES: [&str; 5] = [
    "codemode_search",
    "codemode_execute",
    "codemode_execution",
    "codemode_decide",
    "codemode_cancel",
];

/// Reusable MCP server handler for the Code Mode lifecycle.
#[derive(Clone)]
pub struct CodeModeMcpServer {
    service: Arc<dyn CodeModeService>,
    tools: Arc<Vec<Tool>>,
}

impl CodeModeMcpServer {
    /// Creates an MCP handler over a send-safe Code Mode service.
    pub fn new(service: Arc<dyn CodeModeService>) -> Self {
        Self {
            service,
            tools: Arc::new(definitions()),
        }
    }

    /// Returns the exact lifecycle tools exposed to MCP clients.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }
}

impl ServerHandler for CodeModeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "incurs-codemode",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Search for available methods, execute JavaScript, then inspect, decide, or cancel by execution ID.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            next_cursor: None,
            meta: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = request.name.as_ref();
        let arguments = request.arguments.unwrap_or_default();
        let request = incur_request_context(name, context.extensions.get::<http::request::Parts>());
        let options = CodeModeRunOptions {
            cancellation: context.ct,
            request: Some(request),
        };
        let result = match name {
            "codemode_search" => self
                .service
                .search(string(&arguments, "query")?.to_string())
                .await
                .and_then(to_value),
            "codemode_execute" => self
                .service
                .execute(string(&arguments, "code")?.to_string(), options)
                .await
                .and_then(to_value),
            "codemode_execution" => {
                let execution_id = string(&arguments, "id")?.to_string();
                if let Some(artifact_id) = optional_string(&arguments, "artifact_id")? {
                    self.service
                        .artifact(execution_id, artifact_id.to_string())
                        .await
                } else {
                    self.service
                        .execution(execution_id)
                        .await
                        .and_then(to_value)
                }
            }
            "codemode_decide" => {
                let execution_id = string(&arguments, "id")?.to_string();
                let seq = unsigned(&arguments, "seq")?;
                match string(&arguments, "decision")? {
                    "approve" => self
                        .service
                        .approve(execution_id, seq, options)
                        .await
                        .and_then(to_value),
                    "reject" => self
                        .service
                        .reject(execution_id, seq)
                        .await
                        .and_then(to_value),
                    decision => {
                        return Err(ErrorData::invalid_params(
                            format!("Unknown decision \"{decision}\""),
                            None,
                        ));
                    }
                }
            }
            "codemode_cancel" => self
                .service
                .cancel(string(&arguments, "id")?.to_string())
                .await
                .and_then(to_value),
            _ => return Err(ErrorData::method_not_found::<CallToolRequestMethod>()),
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(json!({ "error": error })),
        })
    }
}

/// Serves the Code Mode MCP handler over process stdio until disconnected.
pub async fn serve_stdio(service: Arc<dyn CodeModeService>) -> Result<(), String> {
    CodeModeMcpServer::new(service)
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|error| error.to_string())?
        .waiting()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn definitions() -> Vec<Tool> {
    vec![
        tool(
            "codemode_search",
            "Search available Code Mode methods and saved snippets.",
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        tool(
            "codemode_execute",
            "Start durable JavaScript execution and return its running state.",
            json!({
                "type": "object",
                "properties": {"code": {"type": "string"}},
                "required": ["code"],
                "additionalProperties": false
            }),
        ),
        tool(
            "codemode_execution",
            "Read an execution or one oversized artifact owned by it.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "artifact_id": {"type": "string"}
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        ),
        tool(
            "codemode_decide",
            "Approve or reject one pending Code Mode action.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "seq": {"type": "integer", "minimum": 0},
                    "decision": {"type": "string", "enum": ["approve", "reject"]}
                },
                "required": ["id", "seq", "decision"],
                "additionalProperties": false
            }),
        ),
        tool(
            "codemode_cancel",
            "Cancel one running or paused Code Mode execution.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
    Tool::new(name, description, object(schema))
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn incur_request_context(tool: &str, parts: Option<&http::request::Parts>) -> IncurRequestContext {
    let Some(parts) = parts else {
        return IncurRequestContext {
            headers: HashMap::new(),
            method: "tools/call".to_string(),
            path: tool.to_string(),
        };
    };
    IncurRequestContext {
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
        method: parts.method.as_str().to_string(),
        path: parts
            .uri
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_else(|| parts.uri.path().to_string()),
    }
}

fn string<'a>(arguments: &'a JsonObject, name: &str) -> Result<&'a str, ErrorData> {
    arguments.get(name).and_then(Value::as_str).ok_or_else(|| {
        ErrorData::invalid_params(format!("Missing string argument \"{name}\""), None)
    })
}

fn optional_string<'a>(
    arguments: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, ErrorData> {
    match arguments.get(name) {
        Some(value) => value.as_str().map(Some).ok_or_else(|| {
            ErrorData::invalid_params(format!("Argument \"{name}\" must be a string"), None)
        }),
        None => Ok(None),
    }
}

fn unsigned(arguments: &JsonObject, name: &str) -> Result<u64, ErrorData> {
    arguments.get(name).and_then(Value::as_u64).ok_or_else(|| {
        ErrorData::invalid_params(
            format!("Missing unsigned integer argument \"{name}\""),
            None,
        )
    })
}

fn to_value(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use incurs_codemode::{
        CodeMode, CodeModeRunOptions, CodeModeService, ExecutionState, MemoryStore, SearchOutput,
    };
    use incurs_codemode_local::{LocalCodeModeService, LocalExecutor};
    use rmcp::model::{CallToolRequestParams, ClientInfo};

    use super::*;

    #[test]
    fn exposes_only_the_stable_lifecycle_surface() {
        let service = local_service();
        let server = CodeModeMcpServer::new(service);
        assert_eq!(
            server
                .tools()
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>(),
            TOOL_NAMES
        );
    }

    #[test]
    fn inherits_http_request_metadata_when_available() {
        let (parts, ()) = http::Request::builder()
            .method("POST")
            .uri("/mcp?tenant=one")
            .header("x-request-id", "request-1")
            .body(())
            .unwrap()
            .into_parts();
        let request = incur_request_context("codemode_execute", Some(&parts));
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp?tenant=one");
        assert_eq!(
            request.headers.get("x-request-id").map(String::as_str),
            Some("request-1")
        );
    }

    #[tokio::test]
    async fn serves_the_lifecycle_over_a_real_mcp_transport() {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server = CodeModeMcpServer::new(local_service());
        let task = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ClientInfo::default().serve(client_transport).await.unwrap();

        let listed = client.list_tools(None).await.unwrap();
        assert_eq!(
            listed
                .tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect::<Vec<_>>(),
            TOOL_NAMES
        );

        let executed = client
            .call_tool(call(
                "codemode_execute",
                json!({"code": "await new Promise(() => {})"}),
            ))
            .await
            .unwrap();
        let execution = executed.structured_content.unwrap();
        assert_eq!(execution["status"], json!("running"));
        let id = execution["id"].as_str().unwrap();

        let cancelled = client
            .call_tool(call("codemode_cancel", json!({"id": id})))
            .await
            .unwrap();
        assert_eq!(
            cancelled.structured_content.unwrap()["status"],
            json!("cancelled")
        );

        client.cancel().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn validates_arguments_and_returns_tool_errors() {
        let service = local_service();
        let server = CodeModeMcpServer::new(service);
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let task = tokio::spawn(async move {
            let _ = server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await;
        });
        let client = ClientInfo::default().serve(client_transport).await.unwrap();

        let missing = client
            .call_tool(call("codemode_execution", json!({"id": "missing"})))
            .await
            .unwrap();
        assert_eq!(missing.is_error, Some(true));
        assert_eq!(
            missing.structured_content.unwrap(),
            json!({"error": "Execution \"missing\" not found"})
        );

        let invalid = client
            .call_tool(call(
                "codemode_decide",
                json!({"id": "missing", "seq": 0, "decision": "later"}),
            ))
            .await;
        assert!(invalid.is_err());

        client.cancel().await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn retrieves_an_execution_owned_artifact() {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server = CodeModeMcpServer::new(Arc::new(ArtifactService));
        let task = tokio::spawn(async move {
            let _ = server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await;
        });
        let client = ClientInfo::default().serve(client_transport).await.unwrap();

        let result = client
            .call_tool(call(
                "codemode_execution",
                json!({"id": "execution-1", "artifact_id": "artifact-1"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            result.structured_content.unwrap(),
            json!({"execution_id": "execution-1", "artifact_id": "artifact-1"})
        );

        client.cancel().await.unwrap();
        task.await.unwrap();
    }

    struct ArtifactService;

    #[async_trait::async_trait]
    impl CodeModeService for ArtifactService {
        async fn search(&self, _query: String) -> Result<SearchOutput, String> {
            Err("unexpected search".to_string())
        }

        async fn execute(
            &self,
            _code: String,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("unexpected execute".to_string())
        }

        async fn execution(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("unexpected execution lookup".to_string())
        }

        async fn artifact(
            &self,
            execution_id: String,
            artifact_id: String,
        ) -> Result<Value, String> {
            Ok(json!({
                "execution_id": execution_id,
                "artifact_id": artifact_id
            }))
        }

        async fn approve(
            &self,
            _execution_id: String,
            _seq: u64,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("unexpected approval".to_string())
        }

        async fn reject(&self, _execution_id: String, _seq: u64) -> Result<ExecutionState, String> {
            Err("unexpected rejection".to_string())
        }

        async fn cancel(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("unexpected cancellation".to_string())
        }
    }

    fn local_service() -> Arc<dyn CodeModeService> {
        Arc::new(
            LocalCodeModeService::spawn(|| {
                CodeMode::new(
                    Arc::new(MemoryStore::default()),
                    LocalExecutor::default(),
                    vec![],
                )
            })
            .unwrap(),
        )
    }

    fn call(name: &'static str, arguments: Value) -> CallToolRequestParams {
        CallToolRequestParams::new(name).with_arguments(object(arguments))
    }
}
