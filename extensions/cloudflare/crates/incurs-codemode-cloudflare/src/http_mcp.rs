use std::collections::BTreeMap;

use incurs::command::RequestContext;
use incurs_codemode::{CodeModeRunOptions, ExecutionState, SearchOutput};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Current MCP protocol revision supported by the Worker HTTP adapter.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// MCP revisions accepted by the stateless HTTP adapter.
pub const SUPPORTED_MCP_PROTOCOL_VERSIONS: [&str; 3] =
    ["2025-03-26", "2025-06-18", MCP_PROTOCOL_VERSION];

/// Stable Code Mode lifecycle tools exposed by the HTTP MCP adapter.
pub const CODEMODE_MCP_TOOL_NAMES: [&str; 5] = [
    "codemode_search",
    "codemode_execute",
    "codemode_execution",
    "codemode_decide",
    "codemode_cancel",
];

/// Single-thread service boundary used by a Worker isolate.
#[async_trait::async_trait(?Send)]
pub trait WorkerCodeModeService {
    /// Searches available methods and snippets.
    async fn search(&self, query: String) -> Result<SearchOutput, String>;
    /// Starts one durable execution.
    async fn execute(
        &self,
        code: String,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String>;
    /// Reads one execution snapshot.
    async fn execution(&self, execution_id: String) -> Result<ExecutionState, String>;
    /// Reads one execution-owned artifact.
    async fn artifact(&self, execution_id: String, artifact_id: String) -> Result<Value, String>;
    /// Approves one pending action.
    async fn approve(
        &self,
        execution_id: String,
        seq: u64,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String>;
    /// Rejects one pending action.
    async fn reject(&self, execution_id: String, seq: u64) -> Result<ExecutionState, String>;
    /// Cancels one live execution.
    async fn cancel(&self, execution_id: String) -> Result<ExecutionState, String>;
}

/// One HTTP request passed from a Worker adapter into the MCP protocol handler.
#[derive(Debug, Clone)]
pub struct McpHttpRequest {
    /// HTTP method.
    pub method: String,
    /// Request path inherited by tool calls.
    pub path: String,
    /// Case-insensitive HTTP headers, normalized by the handler.
    pub headers: BTreeMap<String, String>,
    /// UTF-8 request body, when present.
    pub body: Option<String>,
}

/// Security and resource policy for the stateless HTTP MCP endpoint.
#[derive(Debug, Clone)]
pub struct McpHttpOptions {
    /// Origins explicitly allowed when an `Origin` header is present.
    pub allowed_origins: Vec<String>,
    /// Maximum accepted JSON body size in bytes.
    pub max_body_bytes: usize,
}

impl Default for McpHttpOptions {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            max_body_bytes: 1024 * 1024,
        }
    }
}

/// Transport-neutral response returned to a Worker HTTP adapter.
#[derive(Debug, Clone)]
pub struct McpHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// HTTP response headers.
    pub headers: BTreeMap<String, String>,
    /// JSON-RPC body, or no body for a notification.
    pub body: Option<Value>,
}

/// Returns the exact five lifecycle tool definitions.
pub fn mcp_tool_definitions() -> Vec<Value> {
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
                    "decision": {
                        "type": "string",
                        "enum": ["approve", "reject"]
                    }
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

/// Handles one stateless Streamable HTTP MCP request.
pub async fn handle_mcp_request(
    service: &dyn WorkerCodeModeService,
    request: McpHttpRequest,
    options: &McpHttpOptions,
) -> McpHttpResponse {
    let headers = normalized_headers(&request.headers);
    let origin = headers.get("origin").cloned();
    if let Some(origin) = origin.as_ref()
        && !origin_allowed(origin, &options.allowed_origins)
    {
        return http_rpc_error(403, None, -32600, "Origin is not allowed");
    }
    let response = handle_mcp_transport(service, request, options, headers).await;
    with_cors(response, origin.as_deref())
}

async fn handle_mcp_transport(
    service: &dyn WorkerCodeModeService,
    request: McpHttpRequest,
    options: &McpHttpOptions,
    headers: BTreeMap<String, String>,
) -> McpHttpResponse {
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        return cors_preflight();
    }
    if request.method.eq_ignore_ascii_case("GET") {
        return method_not_allowed();
    }
    if !request.method.eq_ignore_ascii_case("POST") {
        return method_not_allowed();
    }
    if !media_type(&headers, "content-type", "application/json") {
        return http_rpc_error(415, None, -32600, "Content-Type must be application/json");
    }
    if !media_type(&headers, "accept", "application/json")
        || !media_type(&headers, "accept", "text/event-stream")
    {
        return http_rpc_error(
            406,
            None,
            -32600,
            "Accept must include application/json and text/event-stream",
        );
    }
    let body = request.body.as_deref().unwrap_or_default();
    if body.len() > options.max_body_bytes {
        return http_rpc_error(413, None, -32600, "MCP request body is too large");
    }
    let value = match serde_json::from_str::<Value>(body) {
        Ok(value) => value,
        Err(_) => return http_rpc_error(400, None, -32700, "Parse error"),
    };
    handle_rpc_request(service, value, request, headers).await
}

/// Produces a stable, non-reversible Durable Object tenant key.
pub fn tenant_key(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    format!("{digest:x}")
}

async fn handle_rpc_request(
    service: &dyn WorkerCodeModeService,
    request: Value,
    http: McpHttpRequest,
    headers: BTreeMap<String, String>,
) -> McpHttpResponse {
    let Some(request) = request.as_object() else {
        return http_rpc_error(400, None, -32600, "Invalid Request");
    };
    let id = request.get("id").cloned();
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return http_rpc_error(400, valid_id(id), -32600, "Invalid Request");
    }
    if let Some(id) = id.as_ref()
        && !id.is_string()
        && !id.is_number()
    {
        return http_rpc_error(400, None, -32600, "Invalid Request");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        if valid_client_response(request) {
            if let Err(message) = validate_protocol_header(&headers) {
                return http_rpc_error(400, valid_id(id), -32600, message);
            }
            return accepted();
        }
        return http_rpc_error(400, valid_id(id), -32600, "Invalid Request");
    };
    if method == "initialize" {
        if id.is_none() {
            return http_rpc_error(400, None, -32600, "initialize must be a request");
        }
    } else if let Err(message) = validate_protocol_header(&headers) {
        return http_rpc_error(400, valid_id(id), -32600, message);
    }
    if id.is_none() {
        return accepted();
    }
    let result = match method {
        "initialize" => initialize(request),
        "ping" => optional_params(request).map(|_| json!({})),
        "tools/list" => optional_params(request).map(|_| json!({"tools": mcp_tool_definitions()})),
        "tools/call" => match required_params(request) {
            Ok(params) => {
                call_tool(
                    service,
                    params,
                    Some(RequestContext {
                        headers: headers.into_iter().collect(),
                        method: http.method,
                        path: http.path,
                    }),
                )
                .await
            }
            Err(error) => Err(error),
        },
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    match result {
        Ok(result) => rpc_result(id, result),
        Err(error) => rpc_error(id, error.code, &error.message),
    }
}

fn initialize(request: &Map<String, Value>) -> Result<Value, RpcFault> {
    let params = required_params(request)?;
    let requested = string(params, "protocolVersion")?;
    object(params, "capabilities")?;
    let client = object(params, "clientInfo")?;
    string(client, "name")?;
    string(client, "version")?;
    let protocol = if SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        MCP_PROTOCOL_VERSION
    };
    Ok(json!({
        "protocolVersion": protocol,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "incurs-codemode",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Search for available methods, execute JavaScript, then inspect, decide, or cancel by execution ID."
    }))
}

async fn call_tool(
    service: &dyn WorkerCodeModeService,
    params: &Map<String, Value>,
    request: Option<RequestContext>,
) -> Result<Value, RpcFault> {
    only_keys(params, &["name", "arguments", "_meta"])?;
    let name = string(params, "name")?;
    if !CODEMODE_MCP_TOOL_NAMES.contains(&name) {
        return Err(RpcFault::invalid_params(format!("Unknown tool: {name}")));
    }
    let arguments = match params.get("arguments") {
        Some(value) => value
            .as_object()
            .ok_or_else(|| RpcFault::invalid_params("\"arguments\" must be an object"))?
            .clone(),
        None => Map::new(),
    };
    let options = CodeModeRunOptions {
        request,
        ..CodeModeRunOptions::default()
    };
    let result = match name {
        "codemode_search" => {
            only_keys(&arguments, &["query"])?;
            service
                .search(string(&arguments, "query")?.to_string())
                .await
                .and_then(to_value)
        }
        "codemode_execute" => {
            only_keys(&arguments, &["code"])?;
            service
                .execute(string(&arguments, "code")?.to_string(), options)
                .await
                .and_then(to_value)
        }
        "codemode_execution" => {
            only_keys(&arguments, &["id", "artifact_id"])?;
            let execution_id = string(&arguments, "id")?.to_string();
            if let Some(artifact_id) = arguments.get("artifact_id") {
                service
                    .artifact(
                        execution_id,
                        artifact_id
                            .as_str()
                            .ok_or_else(|| {
                                RpcFault::invalid_params("\"artifact_id\" must be a string")
                            })?
                            .to_string(),
                    )
                    .await
            } else {
                service.execution(execution_id).await.and_then(to_value)
            }
        }
        "codemode_decide" => {
            only_keys(&arguments, &["id", "seq", "decision"])?;
            let execution_id = string(&arguments, "id")?.to_string();
            let sequence = arguments
                .get("seq")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    RpcFault::invalid_params("\"seq\" must be a non-negative integer")
                })?;
            match string(&arguments, "decision")? {
                "approve" => service
                    .approve(execution_id, sequence, options)
                    .await
                    .and_then(to_value),
                "reject" => service
                    .reject(execution_id, sequence)
                    .await
                    .and_then(to_value),
                decision => {
                    return Err(RpcFault::invalid_params(format!(
                        "Unknown decision \"{decision}\""
                    )));
                }
            }
        }
        "codemode_cancel" => {
            only_keys(&arguments, &["id"])?;
            service
                .cancel(string(&arguments, "id")?.to_string())
                .await
                .and_then(to_value)
        }
        _ => unreachable!("tool name checked above"),
    };
    Ok(match result {
        Ok(value) => tool_result(value, false),
        Err(error) => tool_result(json!({"error": error}), true),
    })
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&value).unwrap_or_default()
        }],
        "structuredContent": value,
        "isError": is_error
    })
}

fn required_params(request: &Map<String, Value>) -> Result<&Map<String, Value>, RpcFault> {
    request
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| RpcFault::invalid_params("\"params\" must be an object"))
}

fn optional_params(request: &Map<String, Value>) -> Result<Map<String, Value>, RpcFault> {
    match request.get("params") {
        Some(value) => value
            .as_object()
            .cloned()
            .ok_or_else(|| RpcFault::invalid_params("\"params\" must be an object")),
        None => Ok(Map::new()),
    }
}

fn string<'a>(values: &'a Map<String, Value>, name: &str) -> Result<&'a str, RpcFault> {
    values
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcFault::invalid_params(format!("\"{name}\" must be a string")))
}

fn object<'a>(
    values: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, RpcFault> {
    values
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| RpcFault::invalid_params(format!("\"{name}\" must be an object")))
}

fn only_keys(values: &Map<String, Value>, allowed: &[&str]) -> Result<(), RpcFault> {
    if let Some(name) = values.keys().find(|name| !allowed.contains(&name.as_str())) {
        return Err(RpcFault::invalid_params(format!(
            "Unknown property \"{name}\""
        )));
    }
    Ok(())
}

fn to_value(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn normalized_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    origin != "null"
        && allowed
            .iter()
            .any(|candidate| normalize_origin(candidate) == normalize_origin(origin))
}

fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn media_type(headers: &BTreeMap<String, String>, name: &str, expected: &str) -> bool {
    headers.get(name).is_some_and(|header| {
        header.split(',').any(|value| {
            value
                .trim()
                .split(';')
                .next()
                .is_some_and(|value| value.eq_ignore_ascii_case(expected))
        })
    })
}

fn validate_protocol_header(headers: &BTreeMap<String, String>) -> Result<(), &'static str> {
    let protocol = headers
        .get("mcp-protocol-version")
        .map(String::as_str)
        .unwrap_or("2025-03-26");
    if SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&protocol) {
        Ok(())
    } else {
        Err("Unsupported MCP-Protocol-Version")
    }
}

fn valid_client_response(request: &Map<String, Value>) -> bool {
    request.get("id").is_some_and(valid_request_id)
        && (request.contains_key("result") ^ request.contains_key("error"))
}

fn valid_request_id(id: &Value) -> bool {
    id.is_string() || id.is_number()
}

fn valid_id(id: Option<Value>) -> Option<Value> {
    id.filter(valid_request_id)
}

fn rpc_result(id: Option<Value>, result: Value) -> McpHttpResponse {
    json_response(
        200,
        Some(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "result": result
        })),
    )
}

fn rpc_error(id: Option<Value>, code: i64, message: &str) -> McpHttpResponse {
    http_rpc_error(200, valid_id(id), code, message)
}

fn http_rpc_error(
    status: u16,
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
) -> McpHttpResponse {
    json_response(
        status,
        Some(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {"code": code, "message": message.into()}
        })),
    )
}

fn accepted() -> McpHttpResponse {
    json_response(202, None)
}

fn method_not_allowed() -> McpHttpResponse {
    let mut response = json_response(405, None);
    response
        .headers
        .insert("allow".to_string(), "POST".to_string());
    response
}

fn cors_preflight() -> McpHttpResponse {
    let mut response = json_response(204, None);
    response.headers.insert(
        "access-control-allow-methods".to_string(),
        "POST, OPTIONS".to_string(),
    );
    response.headers.insert(
        "access-control-allow-headers".to_string(),
        "authorization, content-type, mcp-protocol-version".to_string(),
    );
    response
}

fn with_cors(mut response: McpHttpResponse, origin: Option<&str>) -> McpHttpResponse {
    if let Some(origin) = origin {
        response.headers.insert(
            "access-control-allow-origin".to_string(),
            origin.to_string(),
        );
        response
            .headers
            .insert("vary".to_string(), "Origin".to_string());
    }
    response
}

fn json_response(status: u16, body: Option<Value>) -> McpHttpResponse {
    let mut headers = BTreeMap::new();
    if body.is_some() {
        headers.insert("content-type".to_string(), "application/json".to_string());
    }
    McpHttpResponse {
        status,
        headers,
        body,
    }
}

struct RpcFault {
    code: i64,
    message: String,
}

impl RpcFault {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use incurs_codemode::CodeModeService;

    use super::*;

    struct StubService;

    #[async_trait::async_trait(?Send)]
    impl WorkerCodeModeService for StubService {
        async fn search(&self, _query: String) -> Result<SearchOutput, String> {
            Err("not called".to_string())
        }

        async fn execute(
            &self,
            _code: String,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn execution(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn artifact(
            &self,
            _execution_id: String,
            _artifact_id: String,
        ) -> Result<Value, String> {
            Err("not called".to_string())
        }

        async fn approve(
            &self,
            _execution_id: String,
            _seq: u64,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn reject(&self, _execution_id: String, _seq: u64) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn cancel(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }
    }

    struct GenericStubService;

    #[async_trait::async_trait]
    impl CodeModeService for GenericStubService {
        async fn search(&self, _query: String) -> Result<SearchOutput, String> {
            Err("not called".to_string())
        }

        async fn execute(
            &self,
            _code: String,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn execution(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn artifact(
            &self,
            _execution_id: String,
            _artifact_id: String,
        ) -> Result<Value, String> {
            Err("not called".to_string())
        }

        async fn approve(
            &self,
            _execution_id: String,
            _seq: u64,
            _options: CodeModeRunOptions,
        ) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn reject(&self, _execution_id: String, _seq: u64) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }

        async fn cancel(&self, _execution_id: String) -> Result<ExecutionState, String> {
            Err("not called".to_string())
        }
    }

    fn mcp_request(body: Value) -> McpHttpRequest {
        McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "accept".to_string(),
                    "application/json, text/event-stream".to_string(),
                ),
                (
                    "mcp-protocol-version".to_string(),
                    MCP_PROTOCOL_VERSION.to_string(),
                ),
            ]),
            body: Some(serde_json::to_string(&body).unwrap()),
        }
    }

    #[test]
    fn exposes_only_the_stable_lifecycle_surface() {
        assert_eq!(
            mcp_tool_definitions()
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect::<Vec<_>>(),
            CODEMODE_MCP_TOOL_NAMES
        );
    }

    #[test]
    fn definitions_match_the_generic_mcp_crate() {
        let generic = incurs_codemode_mcp::CodeModeMcpServer::new(Arc::new(GenericStubService));
        for (worker, generic) in mcp_tool_definitions().iter().zip(generic.tools()) {
            assert_eq!(worker["name"], generic.name.as_ref());
            assert_eq!(
                worker["description"],
                generic.description.as_deref().unwrap_or_default()
            );
            assert_eq!(
                worker["inputSchema"],
                Value::Object(generic.input_schema.as_ref().clone())
            );
        }
    }

    #[tokio::test]
    async fn rejects_unlisted_origins_before_dispatch() {
        let mut request = mcp_request(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
        request
            .headers
            .insert("origin".to_string(), "https://evil.example".to_string());
        let response = handle_mcp_request(&StubService, request, &McpHttpOptions::default()).await;
        assert_eq!(response.status, 403);
    }

    #[tokio::test]
    async fn allows_configured_origins_and_answers_preflight() {
        let mut request = mcp_request(json!({}));
        request.method = "OPTIONS".to_string();
        request.body = None;
        request
            .headers
            .insert("origin".to_string(), "https://client.example".to_string());
        let response = handle_mcp_request(
            &StubService,
            request,
            &McpHttpOptions {
                allowed_origins: vec!["https://client.example".to_string()],
                ..McpHttpOptions::default()
            },
        )
        .await;
        assert_eq!(response.status, 204);
        assert_eq!(
            response.headers["access-control-allow-origin"],
            "https://client.example"
        );
        assert_eq!(
            response.headers["access-control-allow-methods"],
            "POST, OPTIONS"
        );
    }

    #[tokio::test]
    async fn get_returns_method_not_allowed() {
        let mut request = mcp_request(json!({}));
        request.method = "GET".to_string();
        request.body = None;
        let response = handle_mcp_request(&StubService, request, &McpHttpOptions::default()).await;
        assert_eq!(response.status, 405);
        assert_eq!(response.headers["allow"], "POST");
    }

    #[tokio::test]
    async fn rejects_invalid_transport_headers_and_json() {
        let mut request = mcp_request(json!({}));
        request.headers.remove("accept");
        assert_eq!(
            handle_mcp_request(&StubService, request, &McpHttpOptions::default())
                .await
                .status,
            406
        );

        let mut request = mcp_request(json!({}));
        request.body = Some("{bad".to_string());
        let response = handle_mcp_request(&StubService, request, &McpHttpOptions::default()).await;
        assert_eq!(response.status, 400);
        assert_eq!(response.body.unwrap()["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn rejects_invalid_protocol_and_json_rpc_versions() {
        let mut request = mcp_request(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
        request
            .headers
            .insert("mcp-protocol-version".to_string(), "invalid".to_string());
        assert_eq!(
            handle_mcp_request(&StubService, request, &McpHttpOptions::default())
                .await
                .status,
            400
        );

        let request = mcp_request(json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}));
        assert_eq!(
            handle_mcp_request(&StubService, request, &McpHttpOptions::default())
                .await
                .status,
            400
        );
    }

    #[tokio::test]
    async fn validates_and_negotiates_initialize() {
        let invalid = mcp_request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}));
        let response = handle_mcp_request(&StubService, invalid, &McpHttpOptions::default()).await;
        assert_eq!(response.body.unwrap()["error"]["code"], -32602);

        let valid = mcp_request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0.0"}
            }
        }));
        let response = handle_mcp_request(&StubService, valid, &McpHttpOptions::default()).await;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.body.unwrap()["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
    }

    #[test]
    fn tenant_keys_do_not_expose_tokens() {
        let key = tenant_key("secret");
        assert_eq!(key.len(), 64);
        assert!(!key.contains("secret"));
        assert_eq!(key, tenant_key("secret"));
        assert_ne!(key, tenant_key("other"));
    }
}
