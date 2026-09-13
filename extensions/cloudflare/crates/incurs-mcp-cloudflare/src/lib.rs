//! Stateless Streamable HTTP MCP transport for an incurs [`ToolCatalog`].

use std::collections::{BTreeMap, HashSet};

use incurs::command::{McpAnnotations, RequestContext};
use incurs::tool::{ToolCallOptions, ToolCallOutcome, ToolCatalog, ToolDefinition};
use incurs_mcp_protocol::structured::{
    McpStructuredShape, project_output_schema, project_structured_content, projection_metadata,
};
use incurs_mcp_protocol::{McpLifecycleFamily, McpVersion, known_standard, known_standards};
use serde_json::{Map, Value, json};

/// Newest MCP revision this adapter serves.
///
/// The adapter implements the legacy lifecycle only: `initialize`, `ping`,
/// `tools/list`, and `tools/call`. It deliberately does not advertise the modern
/// family, which replaces `initialize` with stateless `server/discover`, requires
/// per-request client metadata, and uses a different wire codec. Advertising a
/// modern revision here would promise semantics this transport does not
/// implement.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Preferred protocol for clients that negotiate an unknown or modern revision.
pub const MCP_LEGACY_PROTOCOL_VERSION: &str = MCP_PROTOCOL_VERSION;

/// The revisions this adapter actually serves, newest first.
///
/// Narrower than [`known_standards`], which lists every published standard the
/// protocol crate knows about, including ones no transport here implements.
pub fn supported_versions() -> Vec<&'static str> {
    let mut versions: Vec<_> = known_standards()
        .iter()
        .filter(|standard| standard.lifecycle() == McpLifecycleFamily::Legacy)
        .map(|standard| standard.version())
        .collect();
    versions.sort_unstable_by(|a, b| b.cmp(a));
    versions
}

/// One HTTP request passed from a Worker router into the MCP handler.
#[derive(Debug, Clone)]
pub struct McpHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

/// Security and resource limits for one MCP endpoint.
#[derive(Debug, Clone)]
pub struct McpHttpOptions {
    pub allowed_origins: Vec<String>,
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

/// Transport-neutral response returned to the Worker router.
#[derive(Debug, Clone)]
pub struct McpHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
}

/// Handles one stateless Streamable HTTP MCP request.
pub async fn handle_mcp_request(
    catalog: &ToolCatalog,
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
    let response = handle_transport(catalog, request, options, headers).await;
    with_cors(response, origin.as_deref())
}

async fn handle_transport(
    catalog: &ToolCatalog,
    request: McpHttpRequest,
    options: &McpHttpOptions,
    headers: BTreeMap<String, String>,
) -> McpHttpResponse {
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        return cors_preflight();
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
    handle_rpc(catalog, value, request, headers).await
}

async fn handle_rpc(
    catalog: &ToolCatalog,
    value: Value,
    http: McpHttpRequest,
    headers: BTreeMap<String, String>,
) -> McpHttpResponse {
    let Some(request) = value.as_object() else {
        return http_rpc_error(400, None, -32600, "Invalid Request");
    };
    let id = request.get("id").cloned();
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || id.as_ref().is_some_and(|id| !valid_request_id(id))
    {
        return http_rpc_error(400, valid_id(id), -32600, "Invalid Request");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return http_rpc_error(400, valid_id(id), -32600, "Invalid Request");
    };
    let protocol = protocol_header(&headers);
    if method != "initialize" && !supports_protocol(protocol) {
        return unsupported_protocol_version(valid_id(id), protocol);
    }
    if id.is_none() {
        return accepted();
    }
    let result = match method {
        "initialize" => initialize(catalog, request),
        "ping" => optional_params(request).map(|_| json!({})),
        "tools/list" => optional_params(request).map(|_| definitions(catalog)),
        "tools/call" => {
            call_tool(
                catalog,
                required_params(request),
                RequestContext {
                    headers: headers.into_iter().collect(),
                    method: http.method,
                    path: http.path,
                },
            )
            .await
        }
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    match result {
        Ok(result) => rpc_result(id, result),
        Err(fault) => rpc_error(id, fault.code, &fault.message),
    }
}

fn initialize(catalog: &ToolCatalog, request: &Map<String, Value>) -> Result<Value, RpcFault> {
    let params = required_params(request)?;
    let requested = string(params, "protocolVersion")?;
    object(params, "capabilities")?;
    let client = object(params, "clientInfo")?;
    string(client, "name")?;
    string(client, "version")?;
    let protocol = if known_standard(&McpVersion::from(requested))
        .is_some_and(|standard| standard.lifecycle() == McpLifecycleFamily::Legacy)
    {
        requested
    } else {
        MCP_LEGACY_PROTOCOL_VERSION
    };
    Ok(json!({
        "protocolVersion": protocol,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": catalog.name(),
            "version": catalog.version().unwrap_or(env!("CARGO_PKG_VERSION"))
        }
    }))
}

fn definitions(catalog: &ToolCatalog) -> Value {
    let tools = catalog
        .definitions()
        .into_iter()
        .map(tool_definition)
        .collect::<Vec<_>>();
    json!({"tools": tools})
}

fn tool_definition(definition: ToolDefinition) -> Value {
    let mut value = json!({
        "name": definition.name,
        "description": definition.description,
        "inputSchema": definition.input_schema,
    });
    let object = value.as_object_mut().expect("tool definition is an object");
    if let Some(schema) = definition.output_schema {
        let projection = project_output_schema(&schema);
        object.insert("outputSchema".to_string(), projection.schema);
        if let Some(meta) = projection_metadata(projection.shape) {
            object.insert("_meta".to_string(), Value::Object(meta));
        }
    }
    if let Some(annotations) = definition.annotations {
        object.insert("annotations".to_string(), tool_annotations(annotations));
    }
    value
}

fn tool_annotations(annotations: McpAnnotations) -> Value {
    let mut values = Map::new();
    if let Some(title) = annotations.title {
        values.insert("title".to_string(), Value::String(title));
    }
    for (name, value) in [
        ("readOnlyHint", annotations.read_only_hint),
        ("destructiveHint", annotations.destructive_hint),
        ("idempotentHint", annotations.idempotent_hint),
        ("openWorldHint", annotations.open_world_hint),
    ] {
        if let Some(value) = value {
            values.insert(name.to_string(), Value::Bool(value));
        }
    }
    Value::Object(values)
}

async fn call_tool(
    catalog: &ToolCatalog,
    params: Result<&Map<String, Value>, RpcFault>,
    request: RequestContext,
) -> Result<Value, RpcFault> {
    let params = params?;
    only_keys(params, &["name", "arguments", "_meta"])?;
    let name = string(params, "name")?;
    let definition = catalog
        .get(name)
        .ok_or_else(|| RpcFault::invalid_params(format!("Unknown tool: {name}")))?;
    let shape = definition
        .output_schema
        .as_ref()
        .map(|schema| project_output_schema(schema).shape);
    let arguments = match params.get("arguments") {
        Some(value) => value
            .as_object()
            .ok_or_else(|| RpcFault::invalid_params("\"arguments\" must be an object"))?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        None => BTreeMap::new(),
    };
    let outcome = catalog
        .call(
            name,
            arguments,
            ToolCallOptions {
                request: Some(request),
                ..ToolCallOptions::isolated()
            },
        )
        .await;
    Ok(match outcome {
        ToolCallOutcome::Ok { data, .. } => tool_result(data, false, shape),
        ToolCallOutcome::Error { code, message, .. } => tool_result(
            json!({"code": code, "message": message}),
            true,
            Some(McpStructuredShape::Object),
        ),
    })
}

fn tool_result(value: Value, is_error: bool, shape: Option<McpStructuredShape>) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_default();
    let structured = match shape {
        Some(shape) => match project_structured_content(value, shape) {
            Ok(value) => Some(value),
            Err(error) => {
                return tool_result(
                    json!({"code":"INVALID_TOOL_OUTPUT", "message":error.to_string()}),
                    true,
                    Some(McpStructuredShape::Object),
                );
            }
        },
        None => value.is_object().then_some(value),
    };
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    });
    if let Some(value) = structured {
        result["structuredContent"] = value;
    }
    if let Some(meta) = shape.and_then(projection_metadata) {
        result["_meta"] = Value::Object(meta);
    }
    result
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
    let allowed = allowed.iter().copied().collect::<HashSet<_>>();
    if let Some(name) = values.keys().find(|name| !allowed.contains(name.as_str())) {
        return Err(RpcFault::invalid_params(format!(
            "Unknown property \"{name}\""
        )));
    }
    Ok(())
}

fn normalized_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    origin != "null"
        && allowed.iter().any(|candidate| {
            candidate
                .trim()
                .trim_end_matches('/')
                .eq_ignore_ascii_case(origin.trim().trim_end_matches('/'))
        })
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

fn protocol_header(headers: &BTreeMap<String, String>) -> &str {
    headers
        .get("mcp-protocol-version")
        .map(String::as_str)
        .unwrap_or("2025-03-26")
}

/// Whether this adapter can serve `protocol`.
///
/// A revision being *published* is not the same as this transport implementing
/// it. Accepting a modern revision here would let a client proceed under
/// stateless-lifecycle rules that the handler below does not honour.
fn supports_protocol(protocol: &str) -> bool {
    known_standard(&McpVersion::from(protocol))
        .is_some_and(|standard| standard.lifecycle() == McpLifecycleFamily::Legacy)
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
        Some(json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result})),
    )
}

fn rpc_error(id: Option<Value>, code: i64, message: &str) -> McpHttpResponse {
    http_rpc_error(200, valid_id(id), code, message)
}

fn unsupported_protocol_version(id: Option<Value>, requested: &str) -> McpHttpResponse {
    json_response(
        400,
        Some(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {
                "code": -32022,
                "message": "Unsupported MCP protocol version",
                "data": {
                    "requested": requested,
                    "supported": supported_versions()
                }
            }
        })),
    )
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
    use incurs::cli::Cli;
    use incurs::command::{
        CommandDef, McpAnnotations, McpCommandOptions, TypedContext, TypedResult,
    };
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Deserialize, incurs::Args)]
    struct EchoArgs {
        text: String,
    }

    #[derive(JsonSchema, Serialize)]
    struct EchoOutput {
        text: String,
        path: String,
    }

    fn catalog() -> ToolCatalog {
        catalog_with_annotations(McpAnnotations {
            title: Some("Echo".to_string()),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        })
    }

    fn catalog_with_annotations(annotations: McpAnnotations) -> ToolCatalog {
        let command = CommandDef::typed::<EchoArgs, (), (), EchoOutput, _, _>(
            "echo",
            |ctx: TypedContext<EchoArgs, (), ()>| async move {
                TypedResult::ok(EchoOutput {
                    text: ctx.args.text,
                    path: ctx.request.map(|request| request.path).unwrap_or_default(),
                })
            },
        )
        .description("Echo text")
        .mcp(McpCommandOptions {
            annotations: Some(annotations),
            ..McpCommandOptions::default()
        })
        .done();
        Cli::create("fixture")
            .version("1.2.3")
            .command("echo", command)
            .command(
                "values",
                CommandDef::typed::<(), (), (), Vec<String>, _, _>("values", |_| async {
                    TypedResult::ok(vec!["record".to_string()])
                })
                .done(),
            )
            .tool_catalog()
    }

    fn request(body: Value) -> McpHttpRequest {
        McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "accept".to_string(),
                    "application/json, text/event-stream".to_string(),
                ),
                ("mcp-protocol-version".to_string(), "2025-11-25".to_string()),
            ]),
            body: Some(body.to_string()),
        }
    }

    /// Builds a request with one header replaced or removed, so a test can vary
    /// exactly one thing.
    fn request_with(body: Value, header: &str, value: Option<&str>) -> McpHttpRequest {
        let mut req = request(body);
        match value {
            Some(v) => {
                req.headers.insert(header.to_string(), v.to_string());
            }
            None => {
                req.headers.remove(header);
            }
        }
        req
    }

    fn initialize_body(version: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.0.0"}
            }
        })
    }

    #[tokio::test]
    async fn initialize_echoes_a_supported_legacy_revision() {
        for version in supported_versions() {
            let response = handle_mcp_request(
                &catalog(),
                request(initialize_body(version)),
                &McpHttpOptions::default(),
            )
            .await;
            let body = response.body.expect("body");
            assert_eq!(
                body["result"]["protocolVersion"], version,
                "a supported revision must be echoed back unchanged",
            );
        }
    }

    #[tokio::test]
    async fn initialize_downgrades_a_modern_revision_it_cannot_serve() {
        // The modern family replaces `initialize` with stateless `server/discover`
        // and requires per-request metadata. This adapter implements neither, so it
        // must answer with the newest revision it can actually honour rather than
        // agreeing to terms it will not meet.
        let response = handle_mcp_request(
            &catalog(),
            request(initialize_body("2026-07-28")),
            &McpHttpOptions::default(),
        )
        .await;
        let body = response.body.expect("body");
        assert_eq!(body["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_ne!(body["result"]["protocolVersion"], "2026-07-28");
    }

    #[tokio::test]
    async fn a_modern_protocol_header_is_refused_on_later_calls() {
        let response = handle_mcp_request(
            &catalog(),
            request_with(
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
                "mcp-protocol-version",
                Some("2026-07-28"),
            ),
            &McpHttpOptions::default(),
        )
        .await;

        assert_eq!(response.status, 400);
        let body = response.body.expect("body");
        assert_eq!(body["error"]["code"], -32022);
        let supported = body["error"]["data"]["supported"]
            .as_array()
            .expect("supported list")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect::<Vec<_>>();
        assert!(
            !supported.iter().any(|v| v == "2026-07-28"),
            "the error must not advertise a revision this adapter cannot serve: {supported:?}",
        );
        assert_eq!(supported, supported_versions());
    }

    #[tokio::test]
    async fn supported_versions_are_legacy_only_and_newest_first() {
        let versions = supported_versions();
        assert!(!versions.is_empty());
        assert!(!versions.contains(&"2026-07-28"));
        assert_eq!(versions.first().copied(), Some(MCP_PROTOCOL_VERSION));

        let mut sorted = versions.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(versions, sorted, "newest first");
    }

    #[tokio::test]
    async fn an_unlisted_origin_is_refused() {
        let options = McpHttpOptions {
            allowed_origins: vec!["https://ledger.example".to_string()],
            ..McpHttpOptions::default()
        };
        let response = handle_mcp_request(
            &catalog(),
            request_with(
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}),
                "origin",
                Some("https://evil.example"),
            ),
            &options,
        )
        .await;
        assert_eq!(response.status, 403);
    }

    #[tokio::test]
    async fn a_listed_origin_is_allowed() {
        let options = McpHttpOptions {
            allowed_origins: vec!["https://ledger.example".to_string()],
            ..McpHttpOptions::default()
        };
        let response = handle_mcp_request(
            &catalog(),
            request_with(
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {}}),
                "origin",
                Some("https://ledger.example"),
            ),
            &options,
        )
        .await;
        assert_eq!(response.status, 200);
    }

    #[tokio::test]
    async fn the_wrong_content_type_is_refused() {
        let response = handle_mcp_request(
            &catalog(),
            request_with(
                json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list", "params": {}}),
                "content-type",
                Some("text/plain"),
            ),
            &McpHttpOptions::default(),
        )
        .await;
        assert_eq!(response.status, 415);
    }

    #[tokio::test]
    async fn a_non_post_method_is_refused() {
        let mut req = request(json!({"jsonrpc": "2.0", "id": 6, "method": "ping"}));
        req.method = "GET".to_string();
        let response = handle_mcp_request(&catalog(), req, &McpHttpOptions::default()).await;
        assert_eq!(response.status, 405);
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused() {
        let options = McpHttpOptions {
            max_body_bytes: 8,
            ..McpHttpOptions::default()
        };
        let response = handle_mcp_request(
            &catalog(),
            request(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"})),
            &options,
        )
        .await;
        assert!(
            response.status >= 400,
            "a body past the cap must be refused, got {}",
            response.status
        );
    }

    #[tokio::test]
    async fn the_default_allowlist_is_empty_and_fails_closed() {
        // A host that forgets to configure origins must not silently accept
        // every browser origin.
        let response = handle_mcp_request(
            &catalog(),
            request_with(
                json!({"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {}}),
                "origin",
                Some("https://anything.example"),
            ),
            &McpHttpOptions::default(),
        )
        .await;
        assert_eq!(
            response.status, 403,
            "an empty allowlist must reject a browser origin"
        );
    }

    #[tokio::test]
    async fn a_request_without_an_origin_is_unaffected_by_the_allowlist() {
        // Non-browser callers send no Origin at all; the allowlist is a browser
        // control and must not block them.
        let response = handle_mcp_request(
            &catalog(),
            request(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list", "params": {}})),
            &McpHttpOptions::default(),
        )
        .await;
        assert_eq!(response.status, 200);
    }

    #[tokio::test]
    async fn lists_catalog_definitions() {
        let response = handle_mcp_request(
            &catalog(),
            request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
            &McpHttpOptions::default(),
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.body.unwrap()["result"]["tools"][0]["name"], "echo");
    }

    #[tokio::test]
    async fn annotation_hints_use_mcp_wire_names() {
        let response = handle_mcp_request(
            &catalog(),
            request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
            &McpHttpOptions::default(),
        )
        .await;
        let body = response.body.expect("body");
        assert_eq!(
            body["result"]["tools"][0]["annotations"],
            json!({
                "title": "Echo",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            })
        );
    }

    #[tokio::test]
    async fn unspecified_annotation_hints_are_omitted() {
        let response = handle_mcp_request(
            &catalog_with_annotations(McpAnnotations::default()),
            request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
            &McpHttpOptions::default(),
        )
        .await;
        assert_eq!(
            response.body.expect("body")["result"]["tools"][0]["annotations"],
            json!({})
        );
    }

    #[tokio::test]
    async fn array_output_has_object_schema_and_marked_structured_envelope() {
        let catalog = catalog();
        let listed = handle_mcp_request(
            &catalog,
            request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
            &McpHttpOptions::default(),
        )
        .await
        .body
        .expect("list body");
        let definition = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "values")
            .unwrap();
        assert_eq!(definition["outputSchema"]["type"], "object");
        assert_eq!(
            definition["outputSchema"]["properties"]["data"]["type"],
            "array"
        );
        assert!(definition["_meta"]["io.incurs.outputProjection"].is_object());
        let response = handle_mcp_request(
            &catalog,
            request(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call", "params":{"name":"values"}}),
            ),
            &McpHttpOptions::default(),
        )
        .await
        .body
        .expect("call body");
        assert_eq!(
            response["result"]["structuredContent"],
            json!({"data":["record"]})
        );
        assert_eq!(response["result"]["content"][0]["text"], "[\"record\"]");
        assert_eq!(response["result"]["_meta"], definition["_meta"]);
        assert_eq!(
            catalog
                .get("values")
                .unwrap()
                .output_schema
                .as_ref()
                .unwrap()["type"],
            "array"
        );
    }

    #[tokio::test]
    async fn calls_catalog_and_forwards_request_context() {
        let response = handle_mcp_request(
            &catalog(),
            request(json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"echo","arguments":{"text":"hello"}}
            })),
            &McpHttpOptions::default(),
        )
        .await;
        let body = response.body.unwrap();
        assert_eq!(body["result"]["structuredContent"]["text"], "hello");
        assert_eq!(body["result"]["structuredContent"]["path"], "/mcp");
        assert_eq!(body["result"]["isError"], false);
    }

    #[tokio::test]
    async fn rejects_unknown_origins_and_non_post_requests() {
        let mut origin = request(json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}}));
        origin
            .headers
            .insert("origin".to_string(), "https://evil.example".to_string());
        assert_eq!(
            handle_mcp_request(&catalog(), origin, &McpHttpOptions::default())
                .await
                .status,
            403
        );

        let mut get = request(json!({}));
        get.method = "GET".to_string();
        assert_eq!(
            handle_mcp_request(&catalog(), get, &McpHttpOptions::default())
                .await
                .status,
            405
        );
    }
}
