//! Portable MCP client tests.
//!
//! Parity tests project the same live server twice, once through the native
//! `rmcp` client and once through [`McpHttpClient`], and require identical
//! command definitions and call outcomes. Every expected value comes from the
//! `rmcp` side.

use super::*;

#[test]
fn sse_parser_joins_data_lines_across_chunks_and_line_endings() {
    let mut parser = SseParser::default();
    parser.push(b": comment\r\nevent: mess").unwrap();
    assert_eq!(parser.next_event(), None);
    parser
        .push(b"age\r\ndata: {\"a\":\r\ndata:1}\r\n\r\ndata: second\n\n")
        .unwrap();
    assert_eq!(
        parser.next_event(),
        Some(SseEvent {
            event: Some("message".to_string()),
            data: "{\"a\":\n1}".to_string(),
        })
    );
    assert_eq!(
        parser.next_event(),
        Some(SseEvent {
            event: None,
            data: "second".to_string(),
        })
    );
    assert_eq!(parser.next_event(), None);
}

#[test]
fn sse_parser_waits_for_the_byte_after_a_trailing_carriage_return() {
    let mut parser = SseParser::default();
    parser.push(b"data: x\r").unwrap();
    assert_eq!(parser.next_event(), None);
    parser.push(b"\n\r\n").unwrap();
    assert_eq!(
        parser.next_event().map(|event| event.data),
        Some("x".into())
    );
}

#[test]
fn accumulated_sse_data_lines_share_the_buffer_limit() {
    let mut parser = SseParser {
        limit: 40,
        ..SseParser::default()
    };
    for _ in 0..3 {
        parser.push(b"data: abcdefgh\n").unwrap();
        assert!(parser.next_event().is_none());
    }
    let error = parser.push(b"data: abcdefgh\n").unwrap_err();
    assert_eq!(error.code(), "HTTP_BODY_TOO_LARGE");
    let mut parser = SseParser {
        limit: 32,
        ..SseParser::default()
    };
    for _ in 0..4 {
        parser.push(b"data: ok\n\n").unwrap();
        assert_eq!(parser.next_event().unwrap().data, "ok");
    }
}

#[test]
fn header_values_outside_visible_ascii_are_base64_wrapped() {
    assert_eq!(encode_header_value("echo_tool"), "echo_tool");
    assert_eq!(encode_header_value(" padded"), "=?base64?IHBhZGRlZA==?=");
    assert_eq!(encode_header_value("é"), "=?base64?w6k=?=");
    assert_eq!(base64(b"Man"), "TWFu");
    assert_eq!(base64(b"Ma"), "TWE=");
}

#[test]
fn responses_are_matched_by_id_in_batches_and_stringified_ids() {
    let batch = serde_json::json!([
        { "jsonrpc": "2.0", "method": "notifications/message", "params": {} },
        { "jsonrpc": "2.0", "id": 6, "result": { "other": true } },
        { "jsonrpc": "2.0", "id": "7", "result": { "mine": true } }
    ]);
    assert_eq!(
        find_response(&batch, 7).unwrap().unwrap(),
        serde_json::json!({ "mine": true })
    );
    let error = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "error": { "code": -32602, "message": "bad", "data": { "field": "x" } }
    });
    assert_eq!(
        find_response(&error, 7).unwrap().unwrap_err().to_string(),
        "Mcp error: -32602: bad({\"field\":\"x\"})"
    );
    assert!(find_response(&error, 8).is_none());
}

#[test]
fn unsafe_param_header_annotations_drop_the_tool() {
    let tool = |annotation: Value| {
        serde_json::json!({
            "name": "t",
            "inputSchema": { "type": "object", "properties": { "p": annotation } }
        })
    };
    assert_eq!(
        param_header_annotations(&tool(
            serde_json::json!({ "type": "string", "x-mcp-header": "Region" })
        )),
        Some(vec![("p".to_string(), "Region".to_string())])
    );
    assert_eq!(
        param_header_annotations(&tool(
            serde_json::json!({ "type": "object", "x-mcp-header": "Region" })
        )),
        None
    );
    assert_eq!(
        param_header_annotations(&tool(
            serde_json::json!({ "type": "string", "x-mcp-header": "" })
        )),
        None
    );
}

/// Runs a projected command's handler directly, bypassing local validation so
/// the remote server sees exactly `options`, and returns a comparable value.
#[cfg(feature = "http")]
pub(crate) async fn run_command(def: &crate::command::CommandDef, options: Value) -> Value {
    let ctx = crate::command::CommandContext {
        agent: true,
        args: Value::Object(Map::new()),
        env: Value::Object(Map::new()),
        display_name: def.name.clone(),
        globals: Value::Object(Map::new()),
        options,
        mcp: None,
        request: None,
        format: crate::output::Format::Json,
        format_explicit: false,
        name: def.name.clone(),
        vars: Value::Object(Map::new()),
        version: None,
    };
    match def.handler.run(ctx).await {
        // Success with data.
        crate::output::CommandResult::Ok {
            data,
            cta,
            exit_code,
        } => serde_json::json!({
            "ok": data,
            "cta": serde_json::to_value(cta).unwrap(),
            "exit_code": exit_code,
        }),
        // Structured failure.
        crate::output::CommandResult::Error {
            code,
            message,
            retryable,
            exit_code,
            cta,
        } => serde_json::json!({
            "error": code,
            "message": message,
            "retryable": retryable,
            "exit_code": exit_code,
            "cta": serde_json::to_value(cta).unwrap(),
        }),
        // Remote commands never stream.
        _ => panic!("a remote command returned a stream"),
    }
}

/// Every observable part of projected command definitions.
#[cfg(feature = "http")]
pub(crate) fn describe(
    commands: &std::collections::BTreeMap<String, crate::command::CommandDef>,
) -> Value {
    Value::Object(
        commands
            .iter()
            .map(|(key, def)| {
                (
                    key.clone(),
                    serde_json::json!({
                        "name": def.name,
                        "description": def.description,
                        "output_schema": def.output_schema,
                        "args_fields": def.args_fields.iter().map(|field| format!("{field:?}")).collect::<Vec<_>>(),
                        "options_fields": def.options_fields.iter().map(|field| format!("{field:?}")).collect::<Vec<_>>(),
                        "env_fields": def.env_fields.len(),
                        "raw": def.raw,
                        "hidden": def.hidden,
                    }),
                )
            })
            .collect(),
    )
}

#[cfg(feature = "http")]
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(30), future)
        .await
        .expect("MCP exchange timed out")
}

#[cfg(feature = "http")]
mod live {
    use std::sync::{Arc, RwLock};

    use serde_json::json;

    use super::*;
    use crate::command::{CommandDef, McpAnnotations, McpCommandOptions, McpResultContent};
    use crate::mcp::{McpDiscovery, McpServeOptions, McpToolFilter};
    use crate::output::{CommandResult, CtaBlock, CtaEntry};
    use crate::schema::{FieldMeta, FieldType, to_kebab};

    const STANDARDS: [&str; 5] = [
        "2024-11-05",
        "2025-03-26",
        "2025-06-18",
        "2025-11-25",
        "2026-07-28",
    ];

    fn field(name: &'static str, field_type: FieldType, required: bool) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: to_kebab(name),
            description: Some("A parity field"),
            field_type,
            required,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn cta() -> CtaBlock {
        CtaBlock {
            commands: vec![CtaEntry::Simple("parity fail".to_string())],
            description: None,
        }
    }

    struct Profile;

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for Profile {
        async fn run(&self, ctx: crate::command::CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: json!({
                    "greeting": format!("hello {}", ctx.options["name"].as_str().unwrap_or("?")),
                    "count": ctx.options["count"],
                    "tags": ctx.options["tags"],
                }),
                cta: Some(cta()),
                exit_code: None,
            }
        }
    }

    struct Fail;

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for Fail {
        async fn run(&self, _ctx: crate::command::CommandContext) -> CommandResult {
            CommandResult::Error {
                code: "DENIED".to_string(),
                message: "access denied".to_string(),
                retryable: false,
                exit_code: Some(3),
                cta: Some(cta()),
            }
        }
    }

    struct Visualize;

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for Visualize {
        async fn run(&self, _ctx: crate::command::CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: json!({ "preview": { "data": "aW1hZ2U=", "mimeType": "image/png" } }),
                cta: None,
                exit_code: None,
            }
        }
    }

    fn fixture_cli(discovery: McpDiscovery, with_ghost: bool) -> crate::cli::Cli {
        let mut profile = CommandDef::build("profile", Profile)
            .description("Greet a profile")
            .done();
        profile.options_fields = vec![
            field("name", FieldType::String, true),
            field("count", FieldType::Number, false),
            field("loud", FieldType::Boolean, false),
            field("tags", FieldType::Array(Box::new(FieldType::String)), false),
        ];
        profile.output_schema = Some(json!({
            "type": "object",
            "properties": {
                "greeting": { "type": "string" },
                "count": { "type": "number" },
                "tags": { "type": "array" }
            }
        }));
        let fail = CommandDef::build("fail", Fail)
            .description("Always fails")
            .done();
        let visualize = CommandDef::build("visualize", Visualize)
            .description("Render a preview")
            .mcp(McpCommandOptions {
                annotations: Some(McpAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                }),
                result_content: vec![McpResultContent::Image {
                    data_pointer: "/preview/data".to_string(),
                    mime_type_pointer: "/preview/mimeType".to_string(),
                }],
                ..Default::default()
            })
            .done();
        let mut cli = crate::cli::Cli::create("parity")
            .mcp(McpServeOptions {
                tools: McpToolFilter {
                    discovery,
                    ..Default::default()
                },
                ..Default::default()
            })
            .command("profile", profile)
            .command("fail", fail)
            .command("visualize", visualize);
        if with_ghost {
            cli = cli.command(
                "ghost",
                CommandDef::build("ghost", Visualize)
                    .description("Removed after projection")
                    .done(),
            );
        }
        cli
    }

    fn router(cli: &crate::cli::Cli) -> axum::Router {
        axum::Router::new().nest_service("/mcp", crate::mcp::http_service(cli).unwrap())
    }

    /// Serves a router that the test can replace while clients stay connected.
    async fn serve_swappable(
        initial: axum::Router,
    ) -> (
        std::net::SocketAddr,
        Arc<RwLock<axum::Router>>,
        tokio::task::JoinHandle<()>,
    ) {
        let current = Arc::new(RwLock::new(initial));
        let state = Arc::clone(&current);
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let router = state.read().expect("router lock").clone();
            async move {
                tower::ServiceExt::oneshot(router, request)
                    .await
                    .expect("router is infallible")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (address, current, server)
    }

    fn only(standard: &str) -> McpRemoteOptions {
        McpRemoteOptions {
            standards: McpStandardSet::from_versions([McpVersion::from(standard)]).unwrap(),
            ..McpRemoteOptions::default()
        }
    }

    /// A contract implementation that is not reqwest: it hands each request,
    /// in process, to the router the live server is currently serving.
    struct RouterClient {
        router: Arc<RwLock<axum::Router>>,
        requests: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::outbound::HttpClient for RouterClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let url = url::Url::parse(&request.url).map_err(HttpClientError::transport)?;
            let mut builder = axum::http::Request::builder()
                .method(request.method.as_str())
                .uri(&url[url::Position::BeforePath..])
                .header(
                    "host",
                    format!(
                        "{}:{}",
                        url.host_str().unwrap_or_default(),
                        url.port_or_known_default().unwrap_or_default()
                    ),
                );
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            let request = builder
                .body(axum::body::Body::from(request.body.unwrap_or_default()))
                .map_err(HttpClientError::transport)?;
            let router = self.router.read().expect("router lock").clone();
            let response = tower::ServiceExt::oneshot(router, request)
                .await
                .unwrap_or_else(|never| match never {});
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let body = response.into_body().into_data_stream().map(|chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(HttpClientError::transport)
            });
            Ok(HttpResponse {
                status,
                headers,
                body: Box::pin(body),
            })
        }
    }

    #[tokio::test]
    async fn portable_client_matches_rmcp_for_every_standard_and_discovery_mode() {
        let forwarded = parity_cases(false).await;
        assert_eq!(forwarded, 0);
    }

    /// The same 12 parity cases, with the portable client sending every request
    /// through [`RouterClient`] instead of reqwest. The native side still
    /// connects over TCP, so both clients reach the same live router.
    #[tokio::test]
    async fn portable_client_through_a_host_contract_matches_rmcp() {
        let forwarded = parity_cases(true).await;
        assert!(forwarded > 0, "the host client was never called");
    }

    /// Runs every parity case and returns how many requests went through a
    /// [`RouterClient`] when `through_contract` is set.
    async fn parity_cases(through_contract: bool) -> usize {
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = [
            (
                "profile",
                json!({ "name": "Ada", "count": 3, "loud": true, "tags": ["x", "y"] }),
            ),
            ("profile", json!({})),
            ("profile", json!({ "name": "Ada", "count": "many" })),
            ("profile", json!({ "name": "Ada", "unknown": 1 })),
            ("fail", json!({})),
            ("visualize", json!({})),
        ];
        let mut cases = 0;
        for discovery in [McpDiscovery::Direct, McpDiscovery::Progressive] {
            for standard in STANDARDS.iter().copied().chain(["all"]) {
                let options = if standard == "all" {
                    McpRemoteOptions::default()
                } else {
                    only(standard)
                };
                let label = format!("{discovery:?} {standard}");
                let (address, current, server) =
                    serve_swappable(router(&fixture_cli(discovery, true))).await;
                let url = format!("http://{address}/mcp");

                let native = within(crate::mcp::remote_commands_with(url.clone(), &options))
                    .await
                    .unwrap_or_else(|error| panic!("{label}: rmcp projection failed: {error}"));
                let portable_options = if through_contract {
                    McpRemoteOptions {
                        http_client: Some(Arc::new(RouterClient {
                            router: Arc::clone(&current),
                            requests: Arc::clone(&requests),
                        })),
                        ..options.clone()
                    }
                } else {
                    options.clone()
                };
                let client = within(McpHttpClient::connect(&url, &portable_options))
                    .await
                    .unwrap_or_else(|error| panic!("{label}: portable connect failed: {error}"));
                let expected_standard = if standard == "all" {
                    "2026-07-28"
                } else {
                    standard
                };
                assert_eq!(client.standard().as_str(), expected_standard, "{label}");
                let portable = within(crate::mcp::remote_commands_from_client(client))
                    .await
                    .unwrap();

                assert_eq!(
                    native.keys().map(String::as_str).collect::<Vec<_>>(),
                    ["fail", "ghost", "profile", "visualize"],
                    "{label}"
                );
                assert_eq!(describe(&portable), describe(&native), "{label}");

                let mut outcomes = Vec::new();
                for (tool, arguments) in &calls {
                    let expected = within(run_command(&native[*tool], arguments.clone())).await;
                    let actual = within(run_command(&portable[*tool], arguments.clone())).await;
                    assert_eq!(actual, expected, "{label}: {tool} {arguments}");
                    outcomes.push(expected);
                }
                // Sanity on the rmcp-derived expectations themselves.
                assert_eq!(outcomes[0]["ok"]["greeting"], "hello Ada", "{label}");
                assert!(outcomes[1]["error"].is_string(), "{label}: {}", outcomes[1]);
                assert!(outcomes[4]["error"].is_string(), "{label}: {}", outcomes[4]);
                assert_eq!(outcomes[5]["ok"]["preview"]["mimeType"], "image/png");

                // The server forgets `ghost`; both clients call it anyway.
                *current.write().unwrap() = router(&fixture_cli(discovery, false));
                let expected = within(run_command(&native["ghost"], json!({}))).await;
                let actual = within(run_command(&portable["ghost"], json!({}))).await;
                assert_eq!(actual, expected, "{label}: unknown tool");
                assert!(expected["error"].is_string(), "{label}: {expected}");

                server.abort();
                cases += 1;
            }
        }
        assert_eq!(cases, 12);
        requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn a_missing_host_client_keeps_its_code_through_the_mcp_client_error() {
        let error =
            McpClientError::Transport(HttpClientError::required("McpRemoteOptions::http_client"));
        assert_eq!(error.code(), "HTTP_CLIENT_REQUIRED");
        assert!(!error.retryable());
        let crate::errors::Error::Incur(error) = crate::errors::Error::from(error) else {
            panic!("expected a coded error");
        };
        assert_eq!(error.code, "HTTP_CLIENT_REQUIRED");
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("McpRemoteOptions::http_client"))
        );
    }

    #[derive(Clone, Default)]
    struct LegacyHttpFixture {
        methods: Arc<std::sync::Mutex<Vec<String>>>,
    }

    async fn legacy_http_post(
        axum::extract::State(state): axum::extract::State<LegacyHttpFixture>,
        headers: axum::http::HeaderMap,
        body: String,
    ) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let message: Value = serde_json::from_str(&body).unwrap();
        let method = message["method"].as_str().unwrap_or_default().to_string();
        state.methods.lock().unwrap().push(method.clone());
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        if header("authorization") != Some("Bearer good") {
            return (
                StatusCode::UNAUTHORIZED,
                [("www-authenticate", "Bearer realm=\"fixture\"")],
                "",
            )
                .into_response();
        }
        if !matches!(method.as_str(), "initialize" | "server/discover")
            && (header("mcp-session-id") != Some("session-abc")
                || header("mcp-protocol-version") != Some("2025-06-18"))
        {
            return (
                StatusCode::BAD_REQUEST,
                "missing session or protocol version",
            )
                .into_response();
        }
        let Some(id) = message.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        match method.as_str() {
            // Initialization issues a session and pins this server's only standard.
            "initialize" => (
                [("mcp-session-id", "session-abc")],
                axum::Json(reply(json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "legacy-http", "version": "1.0.0" }
                }))),
            )
                .into_response(),
            // Listing answers as an event stream with a notification first.
            "tools/list" => {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": { "level": "info", "data": "listing" }
                });
                let response = reply(json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo text",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string", "description": "Text" } },
                            "required": ["text"]
                        }
                    }]
                }));
                (
                    [("content-type", "text/event-stream")],
                    format!(
                        "event: message\ndata: {notification}\n\nevent: message\ndata: {response}\n\n"
                    ),
                )
                    .into_response()
            }
            // Calls answer as JSON.
            "tools/call" => {
                let arguments = message["params"]["arguments"].clone();
                axum::Json(reply(json!({
                    "content": [{ "type": "text", "text": arguments.to_string() }],
                    "isError": false,
                    "structuredContent": { "echo": arguments }
                })))
                .into_response()
            }
            // `server/discover` and everything else is unknown to this server.
            _ => axum::Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            }))
            .into_response(),
        }
    }

    async fn serve_legacy_http() -> (String, LegacyHttpFixture, tokio::task::JoinHandle<()>) {
        let state = LegacyHttpFixture::default();
        let app = axum::Router::new()
            .route("/mcp", axum::routing::post(legacy_http_post))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/mcp"), state, server)
    }

    fn take_methods(state: &LegacyHttpFixture) -> Vec<String> {
        std::mem::take(&mut *state.methods.lock().unwrap())
    }

    #[tokio::test]
    async fn modern_preference_falls_back_to_a_legacy_only_server_like_rmcp() {
        let (url, state, server) = serve_legacy_http().await;
        let options = McpRemoteOptions::bearer("good");

        let native = within(crate::mcp::remote_commands_with(url.clone(), &options))
            .await
            .unwrap();
        let native_call = within(run_command(&native["echo"], json!({ "text": "hi" }))).await;
        let native_methods = take_methods(&state);

        let client = within(McpHttpClient::connect(&url, &options))
            .await
            .unwrap();
        assert_eq!(client.standard().as_str(), "2025-06-18");
        assert_eq!(client.session_id(), Some("session-abc"));
        let portable = within(crate::mcp::remote_commands_from_client(client))
            .await
            .unwrap();
        let portable_call = within(run_command(&portable["echo"], json!({ "text": "hi" }))).await;
        let portable_methods = take_methods(&state);

        assert_eq!(
            &native_methods[..3],
            ["server/discover", "initialize", "notifications/initialized"]
        );
        assert_eq!(
            portable_methods,
            [
                "server/discover",
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call"
            ]
        );
        assert_eq!(describe(&portable), describe(&native));
        assert_eq!(portable_call, native_call);
        assert_eq!(native_call["ok"], json!({ "echo": { "text": "hi" } }));
        server.abort();
    }

    #[tokio::test]
    async fn authentication_failure_during_discovery_fails_closed_without_fallback() {
        let (url, state, server) = serve_legacy_http().await;
        let options = McpRemoteOptions::bearer("bad");

        let native = within(crate::mcp::remote_commands_with(url.clone(), &options)).await;
        assert!(native.is_err());
        assert_eq!(take_methods(&state), ["server/discover"]);

        let error = within(McpHttpClient::connect(&url, &options))
            .await
            .unwrap_err();
        assert_eq!(take_methods(&state), ["server/discover"]);
        assert!(matches!(
            &error,
            McpClientError::Unauthorized { status: 401, www_authenticate: Some(challenge) }
                if challenge == "Bearer realm=\"fixture\""
        ));
        let error = crate::errors::Error::from(error);
        let crate::errors::Error::Incur(error) = error else {
            panic!("expected a coded error");
        };
        assert_eq!(error.code, "MCP_UNAUTHORIZED");
        assert!(error.hint.is_some());
        assert!(!error.retryable);
        server.abort();
    }

    #[tokio::test]
    async fn a_modern_only_client_does_not_fall_back_to_a_legacy_server() {
        let (url, state, server) = serve_legacy_http().await;
        let options = McpRemoteOptions {
            standards: McpStandardSet::modern_only(),
            ..McpRemoteOptions::bearer("good")
        };

        let native = within(crate::mcp::remote_commands_with(url.clone(), &options)).await;
        assert!(native.is_err());
        assert_eq!(take_methods(&state), ["server/discover"]);

        let error = within(McpHttpClient::connect(&url, &options))
            .await
            .unwrap_err();
        assert_eq!(take_methods(&state), ["server/discover"]);
        assert_eq!(error.code(), "MCP_NEGOTIATION_FAILED");
        server.abort();
    }

    #[tokio::test]
    async fn reserved_and_malformed_headers_are_rejected_before_connecting() {
        let error = McpHttpClient::connect(
            "http://127.0.0.1:1/mcp",
            &McpRemoteOptions {
                headers: vec![("Accept".to_string(), "text/plain".to_string())],
                ..McpRemoteOptions::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "INVALID_HEADER");
        let error = McpHttpClient::connect(
            "http://127.0.0.1:1/mcp",
            &McpRemoteOptions {
                headers: vec![("bad name".to_string(), "v".to_string())],
                ..McpRemoteOptions::default()
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("bad name"), "{error}");
        let error = McpHttpClient::connect("ftp://example.test/mcp", &McpRemoteOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "MCP_INVALID_URL");
        let error = McpHttpClient::connect("http://127.0.0.1:1/mcp", &McpRemoteOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "MCP_TRANSPORT_ERROR");
        assert!(error.retryable());
    }
}

/// Hostile and failing servers, scripted in process behind a host client.
/// Every test must end quickly with a coded error rather than hang.
#[cfg(feature = "http")]
mod hostile {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::outbound::{HttpClient, Sleep};

    type Respond = dyn Fn(&str, Value) -> HttpResponse + Send + Sync;

    /// A host client that answers each request from a script and times
    /// deadlines with tokio.
    struct Scripted {
        respond: Box<Respond>,
    }

    #[async_trait::async_trait]
    impl HttpClient for Scripted {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
            let message = request
                .body
                .as_deref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null);
            Ok((self.respond)(&request.method, message))
        }

        fn sleep(&self, duration: Duration) -> Option<Sleep> {
            Some(Box::pin(tokio::time::sleep(duration)))
        }
    }

    fn scripted(
        standard: &str,
        respond: impl Fn(&str, Value) -> HttpResponse + Send + Sync + 'static,
    ) -> McpRemoteOptions {
        McpRemoteOptions {
            standards: McpStandardSet::from_versions([McpVersion::from(standard)]).unwrap(),
            http_client: Some(Arc::new(Scripted {
                respond: Box::new(respond),
            })),
            ..McpRemoteOptions::default()
        }
    }

    fn result(message: &Value, result: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })
    }

    fn json_reply(message: &Value, value: Value) -> HttpResponse {
        HttpResponse::from_bytes(
            200,
            vec![("content-type".to_string(), JSON.to_string())],
            serde_json::to_vec(&result(message, value)).unwrap(),
        )
    }

    fn initialized(standard: &str) -> Value {
        json!({
            "protocolVersion": standard,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "hostile", "version": "1.0.0" }
        })
    }

    /// Answers the 2025-06-18 Streamable HTTP lifecycle.
    fn lifecycle(message: &Value) -> Option<HttpResponse> {
        match message["method"].as_str() {
            // Initialization.
            Some("initialize") => Some(json_reply(message, initialized("2025-06-18"))),
            // Notifications are accepted.
            Some(method) if method.starts_with("notifications/") => {
                Some(HttpResponse::from_bytes(202, Vec::new(), Vec::new()))
            }
            // Everything else is the test's.
            _ => None,
        }
    }

    fn tool(name: &str) -> Value {
        json!({ "name": name, "inputSchema": { "type": "object", "properties": {} } })
    }

    /// A body that never ends. It yields 1 KiB per millisecond, so an
    /// unbounded read cannot exhaust memory before [`bounded`] fails it.
    fn endless(status: u16, content_type: &str, chunk: &'static [u8]) -> HttpResponse {
        let body = futures::stream::unfold((), move |()| async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Some((Ok(chunk.to_vec()), ()))
        });
        HttpResponse {
            status,
            headers: vec![("content-type".to_string(), content_type.to_string())],
            body: Box::pin(body),
        }
    }

    const FILLER: &[u8] = &[b' '; 1024];
    const KEEPALIVE: &[u8] = b": keepalive\n\n";

    async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("a hostile server hung the client")
    }

    /// A legacy HTTP+SSE server. Its event stream sends a comment every
    /// 10 ms forever. `hang` is accepted and never answered, `huge` fails
    /// with an endless error body, and any other tool answers `ok`.
    fn legacy_sse() -> McpRemoteOptions {
        let (events, stream) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        events
            .unbounded_send(b"event: endpoint\ndata: /messages\n\n".to_vec())
            .unwrap();
        let keepalive = events.clone();
        tokio::spawn(async move {
            while keepalive.unbounded_send(KEEPALIVE.to_vec()).is_ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let stream = std::sync::Mutex::new(Some(stream));
        scripted("2024-11-05", move |method, message| {
            if method == "GET" {
                let body = stream.lock().unwrap().take().expect("one event stream");
                return HttpResponse {
                    status: 200,
                    headers: vec![("content-type".to_string(), EVENT_STREAM.to_string())],
                    body: Box::pin(body.map(Ok)),
                };
            }
            let answer = |value: Value| {
                let event = format!("event: message\ndata: {}\n\n", result(&message, value));
                let _ = events.unbounded_send(event.into_bytes());
            };
            match (
                message["method"].as_str(),
                message["params"]["name"].as_str(),
            ) {
                // Initialization is answered on the stream.
                (Some("initialize"), _) => answer(initialized("2024-11-05")),
                // Accepted, never answered.
                (Some("tools/call"), Some("hang")) => {}
                // Rejected with a body that never ends.
                (Some("tools/call"), Some("huge")) => return endless(500, JSON, FILLER),
                // Answered.
                (Some("tools/call"), _) => {
                    answer(json!({ "content": [{ "type": "text", "text": "ok" }] }))
                }
                // Notifications are not answered.
                _ => {}
            }
            HttpResponse::from_bytes(202, Vec::new(), Vec::new())
        })
    }

    #[tokio::test]
    async fn endless_response_bodies_stop_at_the_cap_with_a_coded_error() {
        let options = McpRemoteOptions {
            max_response_bytes: 64 * 1024,
            ..scripted("2025-06-18", |_, message| {
                if let Some(reply) = lifecycle(&message) {
                    return reply;
                }
                match message["params"]["name"].as_str() {
                    // A JSON-RPC error body that never ends.
                    Some("error") => endless(500, JSON, FILLER),
                    // A JSON result body that never ends.
                    _ => endless(200, JSON, FILLER),
                }
            })
        };
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &options))
            .await
            .unwrap();
        for tool in ["json", "error"] {
            let error = bounded(client.call_tool(tool, Map::new()))
                .await
                .unwrap_err();
            assert_eq!(error.code(), "HTTP_BODY_TOO_LARGE", "{tool}: {error}");
            assert!(!error.retryable(), "{tool}");
        }

        let options = McpRemoteOptions {
            max_response_bytes: 64 * 1024,
            ..legacy_sse()
        };
        let client = bounded(McpHttpClient::connect_legacy_sse(
            "http://127.0.0.1:9/sse",
            &options,
        ))
        .await
        .unwrap();
        let error = bounded(client.call_tool("huge", Map::new()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "HTTP_BODY_TOO_LARGE", "legacy: {error}");
    }

    #[tokio::test]
    async fn a_hung_request_times_out_and_releases_the_legacy_event_stream() {
        let timeout = Some(Duration::from_millis(200));
        let options = McpRemoteOptions {
            request_timeout: timeout,
            ..scripted("2025-06-18", |_, message| {
                lifecycle(&message).unwrap_or_else(|| endless(200, EVENT_STREAM, KEEPALIVE))
            })
        };
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &options))
            .await
            .unwrap();
        let error = bounded(client.call_tool("hang", Map::new()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "MCP_TIMEOUT", "{error}");
        assert!(error.retryable());

        let options = McpRemoteOptions {
            request_timeout: timeout,
            ..legacy_sse()
        };
        let client = bounded(McpHttpClient::connect_legacy_sse(
            "http://127.0.0.1:9/sse",
            &options,
        ))
        .await
        .unwrap();
        let (hung, answered) = bounded(async {
            tokio::join!(client.call_tool("hang", Map::new()), async {
                // Start once the hung call holds the event stream.
                tokio::time::sleep(Duration::from_millis(50)).await;
                client.call_tool("echo", Map::new()).await
            })
        })
        .await;
        let hung = hung.unwrap_err();
        assert_eq!(hung.code(), "MCP_TIMEOUT", "{hung}");
        assert!(hung.retryable());
        assert_eq!(answered.unwrap()["content"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn ever_advancing_tool_listings_stop_at_the_page_and_tool_caps() {
        let paging = scripted("2025-06-18", |_, message| {
            if let Some(reply) = lifecycle(&message) {
                return reply;
            }
            let page = message["params"]["cursor"]
                .as_str()
                .and_then(|cursor| cursor.parse::<u64>().ok())
                .unwrap_or(0);
            json_reply(
                &message,
                json!({ "tools": [tool(&format!("t{page}"))], "nextCursor": (page + 1).to_string() }),
            )
        });
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &paging))
            .await
            .unwrap();
        let error = bounded(client.list_tools()).await.unwrap_err();
        assert_eq!(error.code(), "MCP_TOOL_LIMIT_EXCEEDED", "{error}");
        assert!(!error.retryable());

        let crowded = scripted("2025-06-18", |_, message| {
            if let Some(reply) = lifecycle(&message) {
                return reply;
            }
            let tools = (0..=crate::mcp::MAX_REMOTE_TOOLS)
                .map(|index| tool(&format!("t{index}")))
                .collect::<Vec<_>>();
            json_reply(&message, json!({ "tools": tools }))
        });
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &crowded))
            .await
            .unwrap();
        let error = bounded(client.list_tools()).await.unwrap_err();
        assert_eq!(error.code(), "MCP_TOOL_LIMIT_EXCEEDED", "{error}");

        // A progressive catalog whose offset always advances.
        let catalog = scripted("2025-06-18", |_, message| {
            if let Some(reply) = lifecycle(&message) {
                return reply;
            }
            let structured = |value: Value| json!({ "content": [], "structuredContent": value });
            match (
                message["method"].as_str(),
                message["params"]["name"].as_str(),
            ) {
                // The four progressive catalog tools.
                (Some("tools/list"), _) => json_reply(
                    &message,
                    json!({ "tools": [
                        tool("search_tools"),
                        tool("get_tool_details"),
                        tool("call_read_tool"),
                        tool("call_write_tool"),
                    ] }),
                ),
                // One tool per page, and always another page.
                (_, Some("search_tools")) => {
                    let offset = message["params"]["arguments"]["offset"]
                        .as_u64()
                        .unwrap_or(0);
                    json_reply(
                        &message,
                        structured(json!({
                            "tools": [{ "name": format!("x{offset}") }],
                            "nextOffset": offset + 1
                        })),
                    )
                }
                // Details for any tool.
                _ => {
                    let name = message["params"]["arguments"]["name"]
                        .as_str()
                        .unwrap_or("x");
                    json_reply(&message, structured(tool(name)))
                }
            }
        });
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &catalog))
            .await
            .unwrap();
        match bounded(crate::mcp::remote_commands_from_client(client)).await {
            // The catalog stopped at the page cap.
            Err(crate::errors::Error::Incur(error)) => {
                assert_eq!(error.code, "MCP_TOOL_LIMIT_EXCEEDED", "{}", error.message);
                assert!(!error.retryable);
            }
            // Anything else is a failure.
            Err(other) => panic!("expected a coded error, got {other}"),
            Ok(commands) => panic!("expected a failure, got {} commands", commands.len()),
        }
    }

    #[tokio::test]
    async fn empty_advancing_pages_stop_at_an_independent_256_page_boundary() {
        for progressive in [false, true] {
            let observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = std::sync::Arc::clone(&observed);
            let options = scripted("2025-06-18", move |_, message| {
                if let Some(reply) = lifecycle(&message) {
                    return reply;
                }
                if progressive && message["method"] == "tools/list" {
                    return json_reply(
                        &message,
                        json!({ "tools": [
                        tool("search_tools"), tool("get_tool_details"),
                        tool("call_read_tool"), tool("call_write_tool")
                    ] }),
                    );
                }
                let page = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                assert!(page <= 256, "requested a 257th empty catalog page");
                if progressive {
                    assert_eq!(message["params"]["name"], "search_tools");
                    json_reply(
                        &message,
                        json!({ "content": [], "structuredContent": {
                        "tools": [], "nextOffset": page
                    } }),
                    )
                } else {
                    json_reply(
                        &message,
                        json!({ "tools": [], "nextCursor": page.to_string() }),
                    )
                }
            });
            let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &options))
                .await
                .unwrap();
            if progressive {
                match bounded(crate::mcp::remote_commands_from_client(client)).await {
                    Err(crate::errors::Error::Incur(error)) => {
                        assert_eq!(error.code, "MCP_TOOL_LIMIT_EXCEEDED")
                    }
                    Err(other) => panic!("expected a coded limit, got {other}"),
                    Ok(_) => panic!("expected a catalog limit"),
                }
            } else {
                assert_eq!(
                    bounded(client.list_tools()).await.unwrap_err().code(),
                    "MCP_TOOL_LIMIT_EXCEEDED"
                );
            }
            assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 256);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_legacy_request_deadline_includes_waiting_for_the_stream_lock() {
        let options = McpRemoteOptions {
            request_timeout: Some(Duration::from_millis(200)),
            ..legacy_sse()
        };
        let client = McpHttpClient::connect_legacy_sse("http://127.0.0.1:9/sse", &options)
            .await
            .unwrap();
        let Wire::LegacySse(wire) = &client.wire else {
            panic!("legacy client expected");
        };
        let held = wire.events.lock().await;
        let error = bounded(client.call_tool("echo", Map::new()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "MCP_TIMEOUT");
        assert!(error.retryable());
        drop(held);
        assert_eq!(
            client.call_tool("echo", Map::new()).await.unwrap()["content"][0]["text"],
            "ok"
        );
    }

    struct SlowCatalog {
        inner: SharedHttpClient,
    }

    #[async_trait::async_trait]
    impl crate::outbound::HttpClient for SlowCatalog {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
            let message: Value =
                serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
            if message["method"] == "tools/list" || message["params"]["name"] == "search_tools" {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            self.inner.send(request).await
        }
        fn sleep(&self, duration: Duration) -> Option<crate::outbound::Sleep> {
            self.inner.sleep(duration)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn whole_catalog_deadlines_do_not_restart_for_each_answered_page() {
        for progressive in [false, true] {
            let mut options = scripted("2025-06-18", move |_, message| {
                if let Some(reply) = lifecycle(&message) {
                    return reply;
                }
                if progressive && message["method"] == "tools/list" {
                    return json_reply(
                        &message,
                        json!({ "tools": [
                        tool("search_tools"), tool("get_tool_details"),
                        tool("call_read_tool"), tool("call_write_tool")
                    ] }),
                    );
                }
                if progressive {
                    let offset = message["params"]["arguments"]["offset"]
                        .as_u64()
                        .unwrap_or(0);
                    json_reply(
                        &message,
                        json!({ "content": [], "structuredContent": {
                        "tools": [], "nextOffset": offset + 1
                    } }),
                    )
                } else {
                    let page = message["params"]["cursor"]
                        .as_str()
                        .unwrap_or("0")
                        .parse::<u64>()
                        .unwrap();
                    json_reply(
                        &message,
                        json!({ "tools": [], "nextCursor": (page + 1).to_string() }),
                    )
                }
            });
            options.request_timeout = Some(Duration::from_millis(50));
            options.http_client = Some(std::sync::Arc::new(SlowCatalog {
                inner: options.http_client.take().unwrap(),
            }));
            let client = McpHttpClient::connect("http://hostile.test/mcp", &options)
                .await
                .unwrap();
            if progressive {
                match bounded(crate::mcp::remote_commands_from_client(client)).await {
                    Err(crate::errors::Error::Incur(error)) => {
                        assert_eq!(error.code, "MCP_TIMEOUT")
                    }
                    Err(other) => panic!("expected a coded timeout, got {other}"),
                    Ok(_) => panic!("expected a whole catalog deadline"),
                }
            } else {
                assert_eq!(
                    bounded(client.list_tools()).await.unwrap_err().code(),
                    "MCP_TIMEOUT"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_rejected_tool_call_keeps_its_code_and_is_not_retryable() {
        let options = scripted("2025-06-18", |_, message| {
            if let Some(reply) = lifecycle(&message) {
                return reply;
            }
            match message["method"].as_str() {
                // One tool.
                Some("tools/list") => json_reply(&message, json!({ "tools": [tool("echo")] })),
                // Every call is rejected.
                _ => HttpResponse::from_bytes(
                    401,
                    vec![("www-authenticate".to_string(), "Bearer".to_string())],
                    Vec::new(),
                ),
            }
        });
        let client = bounded(McpHttpClient::connect("http://hostile.test/mcp", &options))
            .await
            .unwrap();
        let commands = bounded(crate::mcp::remote_commands_from_client(client))
            .await
            .unwrap();
        let outcome = bounded(run_command(&commands["echo"], json!({}))).await;
        assert_eq!(outcome["error"], "MCP_UNAUTHORIZED", "{outcome}");
        assert_eq!(outcome["retryable"], false, "{outcome}");
        assert_eq!(outcome["exit_code"], 1, "{outcome}");
        assert_eq!(
            outcome["message"],
            "MCP server rejected the credentials with HTTP 401"
        );
    }

    #[tokio::test]
    async fn a_configured_protocol_version_header_is_rejected_like_other_reserved_headers() {
        let error = McpHttpClient::connect(
            "http://127.0.0.1:1/mcp",
            &McpRemoteOptions {
                headers: vec![("Mcp-Protocol-Version".to_string(), "2025-06-18".to_string())],
                ..McpRemoteOptions::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "INVALID_HEADER", "{error}");
        assert!(
            error.to_string().contains("mcp-protocol-version"),
            "{error}"
        );
    }
}
