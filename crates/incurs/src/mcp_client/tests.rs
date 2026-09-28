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

    #[tokio::test]
    async fn portable_client_matches_rmcp_for_every_standard_and_discovery_mode() {
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
                let client = within(McpHttpClient::connect(&url, &options))
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
