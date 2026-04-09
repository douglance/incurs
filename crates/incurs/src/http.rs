//! HTTP transport for the incurs framework.
//!
//! Ported from the `fetchImpl()` function in `src/Cli.ts` (lines ~1450-1700).
//! Exposes incur CLI commands over HTTP using Axum. Each registered command
//! becomes a route: `GET/POST /{command}` for top-level commands and
//! `GET/POST /{group}/{command}` for grouped commands.
//!
//! # Feature gate
//!
//! This module requires the `http` feature flag.
//!
//! # Streaming
//!
//! Commands that return a stream produce NDJSON (`application/x-ndjson`)
//! responses, where each line is a JSON object with `type: "chunk"` or
//! `type: "done"`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use serde_json::Value;

use crate::cli::{Cli, CommandEntry};
use crate::command::{self, CommandDef, ExecuteOptions, InternalResult, ParseMode};
use crate::middleware::MiddlewareFn;
use crate::output::Format;
use crate::schema::FieldMeta;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Starts an HTTP server that exposes all registered commands as routes.
pub async fn serve_http(
    cli: &Cli,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = build_app_state(cli);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Builds an Axum router from the CLI. Useful for testing without binding
/// to a socket.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/{command}", get(handle_command).post(handle_command))
        .route(
            "/{group}/{command}",
            get(handle_group_command).post(handle_group_command),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Shared application state for the HTTP server.
#[derive(Clone)]
pub struct AppState {
    /// The CLI name.
    pub name: String,
    /// The CLI version.
    pub version: Option<String>,
    /// Flattened command lookup.
    pub commands: Arc<BTreeMap<String, Arc<CommandDef>>>,
    /// Root-level middleware.
    pub middleware: Arc<Vec<MiddlewareFn>>,
    /// Group-level middleware keyed by group name.
    pub group_middleware: Arc<BTreeMap<String, Vec<MiddlewareFn>>>,
    /// CLI-level env fields.
    pub env_fields: Arc<Vec<FieldMeta>>,
    /// Middleware vars fields.
    pub vars_fields: Arc<Vec<FieldMeta>>,
}

/// Builds the application state from a Cli instance.
pub fn build_app_state(cli: &Cli) -> AppState {
    let mut commands = BTreeMap::new();
    let mut group_middleware = BTreeMap::new();

    flatten_commands(&cli.commands, "", &mut commands, &mut group_middleware);

    AppState {
        name: cli.name.clone(),
        version: cli.version.clone(),
        commands: Arc::new(commands),
        middleware: Arc::new(cli.middleware.clone()),
        group_middleware: Arc::new(group_middleware),
        env_fields: Arc::new(cli.env_fields.clone()),
        vars_fields: Arc::new(cli.vars_fields.clone()),
    }
}

fn flatten_commands(
    entries: &BTreeMap<String, CommandEntry>,
    prefix: &str,
    commands: &mut BTreeMap<String, Arc<CommandDef>>,
    group_mw: &mut BTreeMap<String, Vec<MiddlewareFn>>,
) {
    for (name, entry) in entries {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", prefix, name)
        };

        match entry {
            CommandEntry::Leaf(def) => {
                commands.insert(key, Arc::clone(def));
            }
            CommandEntry::Group {
                commands: sub_commands,
                middleware,
                ..
            } => {
                if !middleware.is_empty() {
                    group_mw.insert(key.clone(), middleware.clone());
                }
                flatten_commands(sub_commands, &key, commands, group_mw);
            }
            CommandEntry::FetchGateway { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn handle_command(
    State(state): State<AppState>,
    Path(command): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    execute_http_command(&state, &command, &[], query, body).await
}

async fn handle_group_command(
    State(state): State<AppState>,
    Path((group, command)): Path<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    let key = format!("{}/{}", group, command);
    execute_http_command(&state, &key, &[], query, body).await
}

async fn execute_http_command(
    state: &AppState,
    command_key: &str,
    args: &[String],
    query: BTreeMap<String, String>,
    body: axum::body::Bytes,
) -> Response {
    let start = std::time::Instant::now();
    let path = command_key.replace('/', " ");

    let command = match state.commands.get(command_key) {
        Some(cmd) => Arc::clone(cmd),
        None => {
            return json_response(
                StatusCode::NOT_FOUND,
                &serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "COMMAND_NOT_FOUND",
                        "message": format!("'{}' is not a command for '{}'.", command_key, state.name),
                    },
                    "meta": {
                        "command": path,
                        "duration": format_duration(start),
                    }
                }),
            );
        }
    };

    let mut input_options: BTreeMap<String, Value> = query
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    if !body.is_empty() {
        if let Ok(body_str) = std::str::from_utf8(&body) {
            if let Ok(Value::Object(body_map)) = serde_json::from_str::<Value>(body_str) {
                for (k, v) in body_map {
                    input_options.insert(k, v);
                }
            }
        }
    }

    let mut all_middleware: Vec<MiddlewareFn> = state.middleware.as_ref().clone();

    if let Some(slash_pos) = command_key.find('/') {
        let group = &command_key[..slash_pos];
        if let Some(group_mw) = state.group_middleware.get(group) {
            all_middleware.extend(group_mw.iter().cloned());
        }
    }

    all_middleware.extend(command.middleware.iter().cloned());

    let env_source: std::collections::HashMap<String, String> = std::env::vars().collect();

    let result = command::execute(
        command,
        ExecuteOptions {
            agent: true,
            argv: args.to_vec(),
            defaults: None,
            env_fields: state.env_fields.as_ref().clone(),
            env_source,
            format: Format::Json,
            format_explicit: true,
            input_options,
            middlewares: all_middleware,
            name: state.name.clone(),
            parse_mode: ParseMode::Split,
            path: path.clone(),
            vars_fields: state.vars_fields.as_ref().clone(),
            version: state.version.clone(),
        },
    )
    .await;

    let duration = format_duration(start);

    match result {
        InternalResult::Ok { data, cta } => {
            let mut response = serde_json::json!({
                "ok": true,
                "data": data,
                "meta": {
                    "command": path,
                    "duration": duration,
                }
            });
            if let Some(cta) = cta {
                if let Some(meta) = response.get_mut("meta").and_then(|m| m.as_object_mut()) {
                    meta.insert(
                        "cta".to_string(),
                        serde_json::to_value(cta).unwrap_or(Value::Null),
                    );
                }
            }
            json_response(StatusCode::OK, &response)
        }
        InternalResult::Error {
            code,
            message,
            retryable,
            cta,
            ..
        } => {
            let status = if code == "VALIDATION_ERROR" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };

            let mut error_obj = serde_json::json!({ "code": code, "message": message });
            if let Some(r) = retryable {
                error_obj
                    .as_object_mut()
                    .unwrap()
                    .insert("retryable".to_string(), Value::Bool(r));
            }

            let mut response = serde_json::json!({
                "ok": false,
                "error": error_obj,
                "meta": { "command": path, "duration": duration }
            });
            if let Some(cta) = cta {
                if let Some(meta) = response.get_mut("meta").and_then(|m| m.as_object_mut()) {
                    meta.insert(
                        "cta".to_string(),
                        serde_json::to_value(cta).unwrap_or(Value::Null),
                    );
                }
            }
            json_response(status, &response)
        }
        InternalResult::Stream(stream) => ndjson_stream_response(stream, &path),
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

fn json_response(status: StatusCode, body: &Value) -> Response {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "null".to_string());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body_str))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        })
}

fn ndjson_stream_response(
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Value> + Send>>,
    path: &str,
) -> Response {
    let path = path.to_string();

    let ndjson_stream = async_stream::stream! {
        let mut inner = stream;

        while let Some(value) = inner.next().await {
            let chunk = serde_json::json!({ "type": "chunk", "data": value });
            let mut line = serde_json::to_string(&chunk).unwrap_or_default();
            line.push('\n');
            yield Ok::<_, std::io::Error>(line);
        }

        let done = serde_json::json!({
            "type": "done",
            "ok": true,
            "meta": { "command": path }
        });
        let mut done_line = serde_json::to_string(&done).unwrap_or_default();
        done_line.push('\n');
        yield Ok::<_, std::io::Error>(done_line);
    };

    let body = Body::from_stream(ndjson_stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(body)
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        })
}

fn format_duration(start: std::time::Instant) -> String {
    format!("{}ms", start.elapsed().as_millis())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandContext, CommandHandler};
    use crate::output::CommandResult;
    use axum::body::to_bytes;
    use std::collections::HashMap;
    use tower::ServiceExt;

    struct EchoHandler;

    #[async_trait::async_trait]
    impl CommandHandler for EchoHandler {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let mut data = serde_json::Map::new();
            data.insert("args".to_string(), ctx.args);
            data.insert("options".to_string(), ctx.options);
            CommandResult::Ok {
                data: Value::Object(data),
                cta: None,
            }
        }
    }

    struct StreamHandler;

    #[async_trait::async_trait]
    impl CommandHandler for StreamHandler {
        async fn run(&self, _ctx: CommandContext) -> CommandResult {
            let stream =
                futures::stream::iter(vec![Value::from(1), Value::from(2), Value::from(3)]);
            CommandResult::Stream(Box::pin(stream))
        }
    }

    fn make_echo_command(name: &str) -> CommandDef {
        CommandDef {
            name: name.to_string(),
            description: Some(format!("Test command: {}", name)),
            args_fields: Vec::new(),
            options_fields: Vec::new(),
            env_fields: Vec::new(),
            aliases: HashMap::new(),
            examples: Vec::new(),
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(EchoHandler),
            middleware: Vec::new(),
            output_schema: None,
        }
    }

    fn make_stream_command(name: &str) -> CommandDef {
        CommandDef {
            name: name.to_string(),
            description: Some(format!("Streaming command: {}", name)),
            args_fields: Vec::new(),
            options_fields: Vec::new(),
            env_fields: Vec::new(),
            aliases: HashMap::new(),
            examples: Vec::new(),
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(StreamHandler),
            middleware: Vec::new(),
            output_schema: None,
        }
    }

    fn make_test_state() -> AppState {
        let mut commands = BTreeMap::new();
        commands.insert("echo".to_string(), Arc::new(make_echo_command("echo")));
        commands.insert(
            "stream".to_string(),
            Arc::new(make_stream_command("stream")),
        );
        commands.insert(
            "users/list".to_string(),
            Arc::new(make_echo_command("list")),
        );

        AppState {
            name: "test-app".to_string(),
            version: Some("1.0.0".to_string()),
            commands: Arc::new(commands),
            middleware: Arc::new(Vec::new()),
            group_middleware: Arc::new(BTreeMap::new()),
            env_fields: Arc::new(Vec::new()),
            vars_fields: Arc::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn test_command_not_found() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "COMMAND_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_get_command_with_query_params() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/echo?name=alice&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert!(json["data"]["options"].is_object());
        assert_eq!(json["meta"]["command"], "echo");
    }

    #[tokio::test]
    async fn test_post_command_with_json_body() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": "bob", "age": 30}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_grouped_command() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/users/list?limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["meta"]["command"], "users list");
    }

    #[tokio::test]
    async fn test_streaming_command() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/x-ndjson"
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        let lines: Vec<&str> = body_str.trim().split('\n').collect();

        assert_eq!(lines.len(), 4);

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], "chunk");
        assert_eq!(first["data"], 1);

        let last: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(last["type"], "done");
        assert_eq!(last["ok"], true);
    }

    #[tokio::test]
    async fn test_response_has_duration_meta() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["meta"]["duration"].as_str().unwrap().ends_with("ms"));
    }

    #[test]
    fn test_flatten_commands() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "hello".to_string(),
            CommandEntry::Leaf(Arc::new(make_echo_command("hello"))),
        );

        let mut sub_commands = BTreeMap::new();
        sub_commands.insert(
            "list".to_string(),
            CommandEntry::Leaf(Arc::new(make_echo_command("list"))),
        );
        sub_commands.insert(
            "get".to_string(),
            CommandEntry::Leaf(Arc::new(make_echo_command("get"))),
        );

        entries.insert(
            "users".to_string(),
            CommandEntry::Group {
                description: Some("User commands".to_string()),
                commands: sub_commands,
                middleware: Vec::new(),
                output_policy: None,
            },
        );

        let mut commands = BTreeMap::new();
        let mut group_mw = BTreeMap::new();
        flatten_commands(&entries, "", &mut commands, &mut group_mw);

        assert!(commands.contains_key("hello"));
        assert!(commands.contains_key("users/list"));
        assert!(commands.contains_key("users/get"));
        assert!(!commands.contains_key("users"));
    }
}
