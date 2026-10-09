//! HTTP transport for the incurs framework.
//!
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

use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::StreamExt;
use serde_json::Value;

use crate::cli::{Cli, CommandEntry};
use crate::command::{self, CommandDef, ExecuteOptions, InternalResult, ParseMode};
use crate::fetch::FetchHandler;
use crate::middleware::MiddlewareFn;
use crate::output::{Format, StreamRecord};
use crate::schema::FieldMeta;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Starts an HTTP server that exposes all registered commands as routes.
///
/// Binds a TCP socket, so it exists only off wasm32. A Worker passes each
/// request to [`build_cli_router`] instead.
#[cfg(not(target_arch = "wasm32"))]
pub async fn serve_http(cli: &Cli, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_cli_router(cli)?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Builds an Axum router with command routes and stateless MCP-over-HTTP.
///
/// Native builds serve `/mcp` with the rmcp-based server. wasm32 builds, which
/// have no async runtime for it, serve the runtime-free
/// [`crate::mcp::McpHttpServer`] with its default configuration. A wasm32 host
/// has no process environment, so env-backed fields read as absent here; a
/// Worker passes its environment through [`build_cli_router_with`].
pub fn build_cli_router(cli: &Cli) -> Result<Router, crate::errors::Error> {
    let router = build_router(build_app_state(cli));
    #[cfg(not(target_arch = "wasm32"))]
    let router = router.nest_service("/mcp", crate::mcp::http_service(cli)?);
    #[cfg(target_arch = "wasm32")]
    let router = router.merge(mcp_router(crate::mcp::McpHttpServer::from_cli(
        cli,
        crate::mcp::McpHttpConfig::default(),
    )?));
    Ok(router)
}

/// Host-supplied inputs for [`build_cli_router_with`].
#[derive(Default)]
pub struct RouterOptions {
    /// Environment for command routes and MCP tool calls. `None` reads the
    /// process environment, which is empty on wasm32.
    pub env: Option<HashMap<String, String>>,
    /// Allowed hosts, origins, and body limit for `/mcp`. Its `environment` is
    /// replaced by [`Self::env`] when that is set.
    pub mcp: crate::mcp::McpHttpConfig,
    /// Admission check run before every route, `/mcp` included. `None` admits
    /// every request, so a host reachable from outside must set one.
    pub guard: Option<RequestGuard>,
}

/// Decides whether a request may reach any route.
///
/// A refused request is answered `401 Unauthorized` with an `UNAUTHORIZED`
/// error before any command or MCP handler runs.
#[derive(Clone)]
pub struct RequestGuard(Arc<dyn Fn(&axum::http::request::Parts) -> bool + Send + Sync>);

impl RequestGuard {
    /// Admits a request whose `Authorization` header is `Bearer <token>`.
    ///
    /// The comparison takes the same time for every token of the same length,
    /// so response timing does not reveal how much of a guess was right.
    pub fn bearer(token: impl Into<String>) -> Self {
        let expected = format!("Bearer {}", token.into()).into_bytes();
        Self::custom(move |parts| {
            parts
                .headers
                .get(header::AUTHORIZATION)
                .is_some_and(|value| constant_time_eq(value.as_bytes(), &expected))
        })
    }

    /// Admits a request when `check` returns `true`.
    pub fn custom(
        check: impl Fn(&axum::http::request::Parts) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(check))
    }

    /// Runs this guard before every route of `router`, including routes a
    /// host adds beside the incurs ones.
    pub fn protect(self, router: Router) -> Router {
        router.layer(axum::middleware::from_fn_with_state(self, enforce_guard))
    }

    fn admits(&self, parts: &axum::http::request::Parts) -> bool {
        (self.0)(parts)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

async fn enforce_guard(
    State(guard): State<RequestGuard>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let (parts, body) = request.into_parts();
    if !guard.admits(&parts) {
        let body = serde_json::json!({
            "ok": false,
            "error": {
                "code": "UNAUTHORIZED",
                "message": "This request is not authorized",
                "retryable": false,
            },
        });
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            axum::Json(body),
        )
            .into_response();
    }
    next.run(axum::extract::Request::from_parts(parts, body))
        .await
}

/// Builds the command routes and `/mcp` from host-supplied inputs.
///
/// Every target serves `/mcp` with the runtime-free
/// [`crate::mcp::McpHttpServer`], so a Cloudflare Worker gets the same routes
/// as a native host that passes the same options.
pub fn build_cli_router_with(
    cli: &Cli,
    options: RouterOptions,
) -> Result<Router, crate::errors::Error> {
    let RouterOptions {
        env,
        mut mcp,
        guard,
    } = options;
    let mut state = build_app_state(cli);
    if let Some(env) = env {
        mcp.environment = crate::tool::EnvironmentSource::Values(env.clone());
        state = state.with_env(env);
    }
    let server = crate::mcp::McpHttpServer::from_cli(cli, mcp)?;
    let router = build_router(state).merge(mcp_router(server));
    Ok(match guard {
        Some(guard) => guard.protect(router),
        None => router,
    })
}

/// Trusted session ownership for bidirectional MCP requests over HTTP.
///
/// Authentication middleware inserts this value into the request extensions after
/// verifying the caller. Use the same session identifier for the initiating call
/// and its peer responses. Never derive it from an unverified request header or
/// share one identifier across unrelated callers.
#[derive(Clone, Debug)]
pub struct McpPeerSession(
    /// Session identifier supplied by the authenticated host.
    pub String,
);

/// Serves a runtime-free MCP server at `/mcp`.
///
/// Converts each request into a [`crate::mcp::McpHttpRequest`] and streams
/// event-stream responses as they are produced; dropping the response body
/// cancels the tool call. Hosts enable server-initiated peer requests by inserting
/// [`McpPeerSession`] into each authenticated request before dispatch.
pub fn mcp_router(server: crate::mcp::McpHttpServer) -> Router {
    Router::new()
        .route("/mcp", axum::routing::any(handle_mcp))
        .with_state(server)
}

/// Reads up to `cap` bytes of a body and drops the rest unread.
async fn read_bounded(body: Body, cap: usize) -> Result<Vec<u8>, axum::Error> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while bytes.len() < cap {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk?;
        let take = chunk.len().min(cap - bytes.len());
        bytes.extend_from_slice(&chunk[..take]);
    }
    Ok(bytes)
}

async fn handle_mcp(
    State(server): State<crate::mcp::McpHttpServer>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    // Read at most one byte past the limit, so an oversized body is never
    // buffered whole; the server still answers it `413` in its usual order.
    let Ok(body) = read_bounded(body, server.max_request_body_bytes().saturating_add(1)).await
    else {
        return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response();
    };
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let request = crate::mcp::McpHttpRequest {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        headers,
        body: body.to_vec(),
    };
    let response = match parts.extensions.get::<McpPeerSession>() {
        Some(session) => server.handle_for_peer(request, &session.0).await,
        None => server.handle(request).await,
    };
    let mut builder = Response::builder().status(response.status);
    for (name, value) in &response.headers {
        builder = builder.header(name, value);
    }
    let body = match response.body {
        crate::mcp::McpHttpBody::Empty => Body::empty(),
        crate::mcp::McpHttpBody::Full(bytes) => Body::from(bytes),
        crate::mcp::McpHttpBody::EventStream(events) => {
            Body::from_stream(events.map(Ok::<_, std::convert::Infallible>))
        }
    };
    builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Builds an Axum router from the CLI. Useful for testing without binding
/// to a socket.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/.well-known/openapi.json", get(openapi_json))
        .route("/openapi.yml", get(openapi_yaml))
        .route("/openapi.yaml", get(openapi_yaml))
        .route(
            "/.well-known/skills/index.json",
            get(well_known_skills_index),
        )
        .route("/.well-known/skills/{name}/SKILL.md", get(well_known_skill))
        .route("/", get(handle_root).post(handle_root))
        .route("/{*path}", get(handle_path).post(handle_path))
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
    /// OpenAPI 3.2 document generated from the command tree.
    pub openapi: Arc<Value>,
    /// Flattened command lookup.
    pub commands: Arc<BTreeMap<String, Arc<CommandDef>>>,
    /// Root-level middleware.
    pub middleware: Arc<Vec<MiddlewareFn>>,
    /// Group-level middleware keyed by group name.
    pub group_middleware: Arc<BTreeMap<String, Vec<MiddlewareFn>>>,
    /// CLI-level env fields.
    pub env_fields: Arc<Vec<FieldMeta>>,
    /// CLI-level global option fields.
    pub globals_fields: Arc<Vec<FieldMeta>>,
    /// Middleware vars fields.
    pub vars_fields: Arc<Vec<FieldMeta>>,
    /// Generated skill discovery index.
    pub skill_index: Arc<Value>,
    /// Generated skill files keyed by their public directory name.
    pub skills: Arc<BTreeMap<String, String>>,
    /// Fetch gateways keyed by their mounted command path.
    pub gateways: Arc<BTreeMap<String, HttpGateway>>,
    /// Environment read by env-backed command fields. `None` reads the process
    /// environment on every request; a host without one, such as a Cloudflare
    /// Worker, supplies its bindings here through [`AppState::with_env`].
    pub env: Option<Arc<HashMap<String, String>>>,
}

impl AppState {
    /// Replaces the process environment with an explicit one for every route.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(Arc::new(env));
        self
    }
}

/// Fetch gateway projected into the HTTP router.
#[derive(Clone)]
pub struct HttpGateway {
    /// Optional path prepended before forwarding.
    pub base_path: Option<String>,
    /// Gateway handler.
    pub handler: Arc<dyn FetchHandler>,
}

/// Builds the application state from a Cli instance.
pub fn build_app_state(cli: &Cli) -> AppState {
    let mut commands = BTreeMap::new();
    let mut group_middleware = BTreeMap::new();
    let mut gateways = BTreeMap::new();

    flatten_commands(
        &cli.commands,
        "",
        &mut commands,
        &mut group_middleware,
        &mut gateways,
    );
    if let Some(root) = &cli.root_command {
        commands.insert(String::new(), Arc::clone(root));
    }
    let skills = cli
        .skill_files(1)
        .into_iter()
        .map(|file| (file.dir, file.content))
        .collect::<BTreeMap<_, _>>();
    let skill_index = serde_json::json!({
        "skills": skills
            .iter()
            .map(|(name, content)| serde_json::json!({
                "name": name,
                "description": skill_description(content),
                "files": ["SKILL.md"],
            }))
            .collect::<Vec<_>>()
    });

    AppState {
        name: cli.name.clone(),
        version: cli.version.clone(),
        openapi: Arc::new(crate::openapi::from_cli(
            cli,
            &crate::openapi::DocumentOptions::default(),
        )),
        commands: Arc::new(commands),
        middleware: Arc::new(cli.middleware.clone()),
        group_middleware: Arc::new(group_middleware),
        env_fields: Arc::new(cli.env_fields.clone()),
        globals_fields: Arc::new(cli.globals_fields.clone()),
        vars_fields: Arc::new(cli.vars_fields.clone()),
        skill_index: Arc::new(skill_index),
        skills: Arc::new(skills),
        gateways: Arc::new(gateways),
        env: None,
    }
}

fn flatten_commands(
    entries: &BTreeMap<String, CommandEntry>,
    prefix: &str,
    commands: &mut BTreeMap<String, Arc<CommandDef>>,
    group_mw: &mut BTreeMap<String, Vec<MiddlewareFn>>,
    gateways: &mut BTreeMap<String, HttpGateway>,
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
                flatten_commands(sub_commands, &key, commands, group_mw, gateways);
            }
            CommandEntry::FetchGateway {
                base_path, handler, ..
            } => {
                gateways.insert(
                    key,
                    HttpGateway {
                        base_path: base_path.clone(),
                        handler: Arc::clone(handler),
                    },
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn openapi_json(State(state): State<AppState>) -> Response {
    json_response(StatusCode::OK, &state.openapi)
}

async fn openapi_yaml(State(state): State<AppState>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(crate::formatter::format(
            &state.openapi,
            Format::Yaml,
        )))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn well_known_skills_index(State(state): State<AppState>) -> Response {
    cached_response(
        "application/json",
        serde_json::to_string(&*state.skill_index).unwrap_or_default(),
    )
}

async fn well_known_skill(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match state.skills.get(&name) {
        Some(content) => cached_response("text/markdown", content.clone()),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle_root(
    State(state): State<AppState>,
    Query(query): Query<BTreeMap<String, String>>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    execute_http_command(&state, "", &[], query, method, headers, body).await
}

async fn handle_path(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let resolved = (1..=segments.len()).rev().find_map(|length| {
        let key = segments[..length].join("/");
        (state.commands.contains_key(&key) || state.gateways.contains_key(&key))
            .then_some((key, length))
    });
    let Some((key, length)) = resolved else {
        return command_not_found(&state, &path);
    };
    let args = segments[length..]
        .iter()
        .map(|segment| (*segment).to_string())
        .collect::<Vec<_>>();
    if let Some(gateway) = state.gateways.get(&key) {
        return execute_http_gateway(gateway, &path, query, method, headers, body).await;
    }
    execute_http_command(&state, &key, &args, query, method, headers, body).await
}

async fn execute_http_gateway(
    gateway: &HttpGateway,
    path: &str,
    query: BTreeMap<String, String>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let path = match &gateway.base_path {
        Some(base) => format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        ),
        None => format!("/{}", path.trim_start_matches('/')),
    };
    let output = gateway
        .handler
        .handle(crate::fetch::FetchInput {
            path,
            method: method.to_string(),
            headers: headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.to_string(), value.to_string()))
                })
                .collect(),
            body: (!body.is_empty()).then(|| String::from_utf8_lossy(&body).into_owned()),
            query: query.into_iter().collect(),
        })
        .await;
    let mut response = Response::builder().status(output.status);
    for (name, value) in output.headers {
        response = response.header(name, value);
    }
    let body = match output.data {
        Value::String(value) => value,
        value => serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
    };
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn execute_http_command(
    state: &AppState,
    command_key: &str,
    args: &[String],
    query: BTreeMap<String, String>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let start = web_time::Instant::now();
    let path = command_key.replace('/', " ");

    let command = match state.commands.get(command_key) {
        Some(cmd) => Arc::clone(cmd),
        None => return command_not_found(state, command_key),
    };

    let mut input_options: BTreeMap<String, Value> = query
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    if !body.is_empty()
        && let Ok(body_str) = std::str::from_utf8(&body)
        && let Ok(Value::Object(body_map)) = serde_json::from_str::<Value>(body_str)
    {
        for (k, v) in body_map {
            input_options.insert(k, v);
        }
    }

    let (globals, input_options) =
        match crate::parser::parse_global_input(input_options, &state.globals_fields) {
            Ok(parsed) => parsed,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &serde_json::json!({
                        "ok": false,
                        "error": { "code": "VALIDATION_ERROR", "message": error.to_string() },
                        "meta": { "command": path, "duration": format_duration(start) },
                    }),
                );
            }
        };
    let request_headers = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();

    let mut all_middleware: Vec<MiddlewareFn> = state.middleware.as_ref().clone();

    let segments = command_key.split('/').collect::<Vec<_>>();
    for length in 1..segments.len() {
        let group = segments[..length].join("/");
        if let Some(group_mw) = state.group_middleware.get(&group) {
            all_middleware.extend(group_mw.iter().cloned());
        }
    }

    all_middleware.extend(command.middleware.iter().cloned());

    let env_source: HashMap<String, String> = match &state.env {
        Some(env) => env.as_ref().clone(),
        None => crate::process_env(),
    };

    let result = command::execute(
        command,
        ExecuteOptions {
            agent: true,
            argv: args.to_vec(),
            defaults: None,
            display_name: state.name.clone(),
            env_fields: state.env_fields.as_ref().clone(),
            env_source,
            format: Format::Json,
            format_explicit: true,
            globals,
            input_options,
            middlewares: all_middleware,
            name: state.name.clone(),
            parse_mode: ParseMode::Split,
            path: path.clone(),
            request: Some(crate::command::RequestContext {
                headers: request_headers,
                method: method.to_string(),
                path: format!(
                    "/{}",
                    std::iter::once(command_key)
                        .chain(args.iter().map(String::as_str))
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join("/")
                ),
            }),
            mcp: None,
            vars_fields: state.vars_fields.as_ref().clone(),
            version: state.version.clone(),
        },
    )
    .await;

    let duration = format_duration(start);

    match result {
        // HTTP has no process to exit; a wrapped exit code stays in the data.
        InternalResult::Ok {
            data,
            cta,
            exit_code: _,
        } => {
            let mut response = serde_json::json!({
                "ok": true,
                "data": data,
                "meta": {
                    "command": path,
                    "duration": duration,
                }
            });
            if let Some(cta) = cta
                && let Some(meta) = response.get_mut("meta").and_then(|m| m.as_object_mut())
            {
                meta.insert(
                    "cta".to_string(),
                    serde_json::to_value(cta).unwrap_or(Value::Null),
                );
            }
            json_response(StatusCode::OK, &response)
        }
        InternalResult::Error {
            code,
            message,
            retryable,
            field_errors,
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
            if let Some(field_errors) = field_errors {
                error_obj.as_object_mut().unwrap().insert(
                    "fieldErrors".to_string(),
                    Value::Array(
                        field_errors
                            .into_iter()
                            .map(|error| {
                                serde_json::json!({
                                    "path": error.path,
                                    "expected": error.expected,
                                    "received": error.received,
                                    "message": error.message,
                                })
                            })
                            .collect(),
                    ),
                );
            }

            let mut response = serde_json::json!({
                "ok": false,
                "error": error_obj,
                "meta": { "command": path, "duration": duration }
            });
            if let Some(cta) = cta
                && let Some(meta) = response.get_mut("meta").and_then(|m| m.as_object_mut())
            {
                meta.insert(
                    "cta".to_string(),
                    serde_json::to_value(cta).unwrap_or(Value::Null),
                );
            }
            json_response(status, &response)
        }
        InternalResult::InputRequired { .. } => {
            let response = serde_json::json!({
                "ok": false,
                "error": {
                    "code": "MCP_INPUT_REQUIRED",
                    "message": "This command requires an MCP client input round"
                },
                "meta": { "command": path, "duration": duration }
            });
            json_response(StatusCode::CONFLICT, &response)
        }
        InternalResult::Stream(stream) => ndjson_stream_response(stream, &path, start),
        InternalResult::RecordStream(stream) => record_stream_response(stream, &path, start),
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

fn cached_response(content_type: &'static str, body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=300")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn command_not_found(state: &AppState, path: &str) -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        &serde_json::json!({
            "ok": false,
            "error": {
                "code": "COMMAND_NOT_FOUND",
                "message": format!("'{}' is not a command for '{}'.", path, state.name),
            },
            "meta": {
                "command": path.replace('/', " "),
                "duration": "0ms",
            }
        }),
    )
}

fn skill_description(content: &str) -> String {
    content
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or_default()
        .to_string()
}

fn ndjson_stream_response(
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Value> + Send>>,
    path: &str,
    start: web_time::Instant,
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
            "meta": { "command": path, "duration": format_duration(start) }
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

fn record_stream_response(
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = StreamRecord> + Send>>,
    path: &str,
    start: web_time::Instant,
) -> Response {
    let path = path.to_string();
    let output = async_stream::stream! {
        let mut inner = stream;
        let mut terminal = None;
        while let Some(record) = inner.next().await {
            match record {
                StreamRecord::Chunk(value) => {
                    let mut line = serde_json::to_string(&serde_json::json!({
                        "type": "chunk", "data": value,
                    })).unwrap_or_default();
                    line.push('\n');
                    yield Ok::<_, std::io::Error>(line);
                }
                record => {
                    terminal = Some(record);
                    break;
                }
            }
        }

        let duration = format_duration(start);
        let value = match terminal {
            Some(StreamRecord::Error { code, message, retryable, cta, .. }) => {
                let mut value = serde_json::json!({
                    "type": "error", "ok": false,
                    "error": { "code": code, "message": message },
                    "meta": { "command": path, "duration": duration },
                });
                if retryable {
                    value["error"]["retryable"] = Value::Bool(true);
                }
                if let Some(cta) = cta {
                    value["meta"]["cta"] = serde_json::to_value(cta).unwrap_or(Value::Null);
                }
                value
            }
            terminal => {
                let mut value = serde_json::json!({
                    "type": "done", "ok": true,
                    "meta": { "command": path, "duration": duration },
                });
                if let Some(StreamRecord::Ok { cta: Some(cta) }) = terminal {
                    value["meta"]["cta"] = serde_json::to_value(cta).unwrap_or(Value::Null);
                }
                value
            }
        };
        let mut line = serde_json::to_string(&value).unwrap_or_default();
        line.push('\n');
        yield Ok::<_, std::io::Error>(line);
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(output))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        })
}

fn format_duration(start: web_time::Instant) -> String {
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

    async fn mcp_request(cli: &Cli, body: Value) -> (StatusCode, Value) {
        let response = build_cli_router(cli)
            .unwrap()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let json = if let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) {
            serde_json::from_str(data)
                .unwrap_or_else(|error| panic!("invalid SSE JSON ({error}): {text:?}"))
        } else {
            serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("invalid MCP JSON ({error}): {text:?}"))
        };
        (status, json)
    }

    struct HttpPeerHandler;

    #[async_trait::async_trait]
    impl CommandHandler for HttpPeerHandler {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let data = ctx
                .mcp
                .unwrap()
                .peer
                .unwrap()
                .request(crate::command::McpPeerRequest {
                    method: "sampling/createMessage".to_string(),
                    params: Some(serde_json::json!({"messages":[]})),
                    meta: None,
                })
                .await
                .expect("authenticated peer response");
            CommandResult::Ok {
                data,
                cta: None,
                exit_code: None,
            }
        }
    }

    fn peer_http_request(message: Value, session: Option<&str>) -> axum::http::Request<Body> {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "localhost")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-03-26")
            .header("mcp-session-id", "owner")
            .body(Body::from(serde_json::to_vec(&message).unwrap()))
            .unwrap();
        if let Some(session) = session {
            request
                .extensions_mut()
                .insert(McpPeerSession(session.to_string()));
        }
        request
    }

    fn peer_http_event(bytes: &[u8]) -> Value {
        let text = std::str::from_utf8(bytes).unwrap();
        let payload = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        serde_json::from_str(payload).unwrap()
    }

    #[tokio::test]
    async fn mcp_http_peer_response_requires_trusted_owning_session() {
        use futures::FutureExt;
        let cli = Cli::create("test")
            .mcp(crate::mcp::McpServeOptions {
                tools: crate::mcp::McpToolFilter {
                    discovery: crate::mcp::McpDiscovery::Direct,
                    ..Default::default()
                },
                ..Default::default()
            })
            .command("peer", CommandDef::build("peer", HttpPeerHandler).done());
        let server = crate::mcp::McpHttpServer::from_cli(&cli, Default::default()).unwrap();
        let router = mcp_router(server);
        let response = router
            .clone()
            .oneshot(peer_http_request(
                serde_json::json!({
                    "jsonrpc":"2.0", "id":"call", "method":"tools/call",
                    "params":{"name":"peer", "arguments":{}}
                }),
                Some("owner"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let peer = peer_http_event(&stream.next().await.unwrap().unwrap());
        assert_eq!(peer["method"], "sampling/createMessage");
        for scope in [None, Some("other-caller")] {
            let response = router
                .clone()
                .oneshot(peer_http_request(
                    serde_json::json!({
                        "jsonrpc":"2.0", "id":peer["id"], "result":{"accepted":false}
                    }),
                    scope,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            assert!(
                stream.next().now_or_never().is_none(),
                "cross-session response completed the call"
            );
        }
        let response = router
            .oneshot(peer_http_request(
                serde_json::json!({
                    "jsonrpc":"2.0", "id":peer["id"], "result":{"accepted":true}
                }),
                Some("owner"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let result = peer_http_event(&stream.next().await.unwrap().unwrap());
        let content: Value =
            serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(content["accepted"], true);
    }

    struct EchoHandler;

    #[async_trait::async_trait]
    impl CommandHandler for EchoHandler {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let mut data = serde_json::Map::new();
            data.insert("args".to_string(), ctx.args);
            data.insert("globals".to_string(), ctx.globals);
            data.insert("options".to_string(), ctx.options);
            data.insert("env".to_string(), ctx.env);
            data.insert(
                "request".to_string(),
                ctx.request
                    .map(|request| {
                        serde_json::json!({
                            "method": request.method,
                            "path": request.path,
                            "headers": request.headers,
                        })
                    })
                    .unwrap_or(Value::Null),
            );
            CommandResult::Ok {
                data: Value::Object(data),
                cta: None,
                exit_code: None,
            }
        }
    }

    struct StreamHandler;

    struct GatewayHandler;

    #[async_trait::async_trait]
    impl crate::fetch::FetchHandler for GatewayHandler {
        async fn handle(&self, request: crate::fetch::FetchInput) -> crate::fetch::FetchOutput {
            crate::fetch::FetchOutput {
                ok: true,
                status: 200,
                data: serde_json::json!({
                    "path": request.path,
                    "method": request.method,
                    "query": request.query,
                }),
                headers: vec![("content-type".to_string(), "application/json".to_string())],
            }
        }
    }

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
            command_aliases: Vec::new(),
            examples: Vec::new(),
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(EchoHandler),
            middleware: Vec::new(),
            output_schema: None,
            raw: false,
            hidden: false,
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
            command_aliases: Vec::new(),
            examples: Vec::new(),
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(StreamHandler),
            middleware: Vec::new(),
            output_schema: None,
            raw: false,
            hidden: false,
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
        let mut typed = make_echo_command("typed");
        typed.options_fields = vec![FieldMeta {
            name: "limit",
            cli_name: "limit".to_string(),
            description: None,
            field_type: crate::schema::FieldType::Number,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }];
        commands.insert("typed".to_string(), Arc::new(typed));

        AppState {
            name: "test-app".to_string(),
            version: Some("1.0.0".to_string()),
            openapi: Arc::new(serde_json::json!({
                "openapi": "3.2.0",
                "info": { "title": "test-app", "version": "0.0.0" },
                "paths": {},
            })),
            commands: Arc::new(commands),
            middleware: Arc::new(Vec::new()),
            group_middleware: Arc::new(BTreeMap::new()),
            env_fields: Arc::new(Vec::new()),
            globals_fields: Arc::new(Vec::new()),
            vars_fields: Arc::new(Vec::new()),
            skill_index: Arc::new(serde_json::json!({ "skills": [] })),
            skills: Arc::new(BTreeMap::new()),
            gateways: Arc::new(BTreeMap::new()),
            env: None,
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
    async fn test_openapi_document_routes() {
        let response = build_router(make_test_state())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["openapi"],
            "3.2.0"
        );

        let response = build_router(make_test_state())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/openapi.yaml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/yaml");
    }

    #[tokio::test]
    async fn test_stateless_mcp_http_rejects_get() {
        let cli = Cli::create("test").command("echo", make_echo_command("echo"));
        let response = build_cli_router(&cli)
            .unwrap()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/mcp")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_mcp_http_rejects_non_loopback_host() {
        let cli = Cli::create("test").command("echo", make_echo_command("echo"));
        let response = build_cli_router(&cli)
            .unwrap()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::HOST, "evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_mcp_initialize_and_progressive_tools() {
        let cli = Cli::create("test")
            .version("1.0.0")
            .command("echo", make_echo_command("echo"));
        let (status, initialized) = mcp_request(
            &cli,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1.0.0" }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(initialized["result"]["serverInfo"]["name"], "test");
        assert_eq!(initialized["result"]["serverInfo"]["version"], "1.0.0");
        assert!(initialized["result"]["capabilities"]["tools"].is_object());

        let (_, listed) = mcp_request(
            &cli,
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
        let names = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "search_tools",
                "get_tool_details",
                "call_read_tool",
                "call_write_tool"
            ]
        );
    }

    #[tokio::test]
    async fn test_mcp_direct_tool_call_executes_shared_command() {
        let cli = Cli::create("test")
            .mcp(crate::mcp::McpServeOptions {
                tools: crate::mcp::McpToolFilter {
                    discovery: crate::mcp::McpDiscovery::Direct,
                    ..Default::default()
                },
                ..Default::default()
            })
            .command("echo", make_echo_command("echo"));
        let (_, listed) = mcp_request(
            &cli,
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
        assert_eq!(listed["result"]["tools"][0]["name"], "echo");

        let (_, called) = mcp_request(
            &cli,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "echo", "arguments": {} }
            }),
        )
        .await;
        assert_eq!(called["result"]["isError"], false);
        let content: Value =
            serde_json::from_str(called["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(content["options"], serde_json::json!({}));
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
    async fn test_structured_input_coercion_and_field_errors() {
        let response = build_router(make_test_state())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/typed?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["options"]["limit"], 10);

        let response = build_router(make_test_state())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/typed?limit=nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["fieldErrors"][0]["path"], "limit");
        assert_eq!(json["error"]["fieldErrors"][0]["expected"], "number");
    }

    fn guarded_router(guard: Option<RequestGuard>) -> Router {
        let cli =
            Cli::create("guarded").command("echo", CommandDef::build("echo", EchoHandler).done());
        build_cli_router_with(
            &cli,
            RouterOptions {
                guard,
                ..RouterOptions::default()
            },
        )
        .unwrap()
    }

    fn guarded_requests(authorization: Option<&str>) -> [axum::http::Request<Body>; 2] {
        let command = axum::http::Request::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json");
        let mcp = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        let (command, mcp) = match authorization {
            Some(value) => (
                command.header("authorization", value),
                mcp.header("authorization", value),
            ),
            None => (command, mcp),
        };
        [
            command.body(Body::from("{}")).unwrap(),
            mcp.body(Body::from(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            ))
            .unwrap(),
        ]
    }

    #[tokio::test]
    async fn a_bearer_guard_covers_command_routes_and_mcp() {
        let guard = || Some(RequestGuard::bearer("s3cret"));
        for (authorization, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some("Bearer wrong"), StatusCode::UNAUTHORIZED),
            // Same length as the real token, so only the content check refuses it.
            (Some("Bearer s3creT"), StatusCode::UNAUTHORIZED),
            (Some("Bearer s3cret-and-more"), StatusCode::UNAUTHORIZED),
            (Some("s3cret"), StatusCode::UNAUTHORIZED),
            (Some("Bearer s3cret"), StatusCode::OK),
        ] {
            for request in guarded_requests(authorization) {
                let path = request.uri().path().to_string();
                let response = guarded_router(guard()).oneshot(request).await.unwrap();
                assert_eq!(response.status(), expected, "{path} with {authorization:?}");
                if expected == StatusCode::UNAUTHORIZED {
                    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                    let json: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(json["error"]["code"], "UNAUTHORIZED");
                }
            }
        }
    }

    #[tokio::test]
    async fn an_oversized_mcp_body_is_refused_without_reading_it_all() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        const CHUNK: usize = 512;
        const CHUNKS: usize = 10_000;
        let pulled = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&pulled);
        let body = Body::from_stream(futures::stream::iter(0..CHUNKS).map(move |_| {
            counter.fetch_add(CHUNK, Ordering::SeqCst);
            Ok::<_, std::convert::Infallible>(vec![b' '; CHUNK])
        }));
        let cli =
            Cli::create("bounded").command("echo", CommandDef::build("echo", EchoHandler).done());
        let router = build_cli_router_with(
            &cli,
            RouterOptions {
                mcp: crate::mcp::McpHttpConfig {
                    max_request_body_bytes: 1024,
                    ..crate::mcp::McpHttpConfig::default()
                },
                ..RouterOptions::default()
            },
        )
        .unwrap();
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body)
            .unwrap();

        let response = router.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let pulled = pulled.load(Ordering::SeqCst);
        assert!(
            pulled <= 1024 + 2 * CHUNK,
            "read {pulled} of {} bytes",
            CHUNK * CHUNKS
        );
    }

    #[tokio::test]
    async fn without_a_guard_every_request_is_admitted() {
        for request in guarded_requests(None) {
            let path = request.uri().path().to_string();
            let response = guarded_router(None).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn a_host_supplied_environment_reaches_env_fields() {
        // The name is unique to this test, so the process environment cannot
        // supply it: only the explicit environment can.
        let name = "INCURS_HTTP_TEST_HOST_ONLY_TOKEN";
        assert!(std::env::var(name).is_err());
        let mut state = make_test_state();
        let mut echo = make_echo_command("echo");
        echo.env_fields = vec![FieldMeta {
            name: "token",
            cli_name: "token".to_string(),
            description: None,
            field_type: crate::schema::FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: Some(name),
        }];
        let mut commands = state.commands.as_ref().clone();
        commands.insert("echo".to_string(), Arc::new(echo));
        state.commands = Arc::new(commands);
        let request = || {
            axum::http::Request::builder()
                .method("POST")
                .uri("/echo")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap()
        };

        let supplied = build_router(
            state
                .clone()
                .with_env(HashMap::from([(name.to_string(), "from-host".to_string())])),
        )
        .oneshot(request())
        .await
        .unwrap();
        let body = to_bytes(supplied.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["env"]["token"], "from-host", "{json}");

        let missing = build_router(state).oneshot(request()).await.unwrap();
        let body = to_bytes(missing.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["env"]["token"], Value::Null, "{json}");
    }

    #[tokio::test]
    async fn test_http_context_splits_globals_and_preserves_request_metadata() {
        let mut state = make_test_state();
        state.globals_fields = Arc::new(vec![FieldMeta {
            name: "profile",
            cli_name: "profile".to_string(),
            description: None,
            field_type: crate::schema::FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }]);
        let app = build_router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "application/json")
                    .header("x-trace-id", "trace-1")
                    .body(Body::from(r#"{"profile":"work","name":"bob"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["globals"]["profile"], "work");
        assert_eq!(json["data"]["options"]["name"], "bob");
        assert_eq!(json["data"]["request"]["method"], "POST");
        assert_eq!(json["data"]["request"]["headers"]["x-trace-id"], "trace-1");
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
    async fn test_arbitrarily_nested_command_with_trailing_args() {
        let mut state = make_test_state();
        let mut command = make_echo_command("show");
        command.args_fields = vec![FieldMeta {
            name: "id",
            cli_name: "id".to_string(),
            description: None,
            field_type: crate::schema::FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }];
        Arc::make_mut(&mut state.commands)
            .insert("admin/users/show".to_string(), Arc::new(command));
        let response = build_router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/users/show/user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"]["args"]["id"], "user-1");
        assert_eq!(value["data"]["request"]["path"], "/admin/users/show/user-1");
    }

    #[tokio::test]
    async fn test_root_command() {
        let mut state = make_test_state();
        Arc::make_mut(&mut state.commands)
            .insert(String::new(), Arc::new(make_echo_command("root")));
        let response = build_router(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/?name=Ada")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_well_known_skills() {
        let cli = Cli::create("test").command("echo", make_echo_command("echo"));
        let app = build_cli_router(&cli).unwrap();
        let index = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/.well-known/skills/index.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(
            index.headers()[header::CACHE_CONTROL],
            "public, max-age=300"
        );
        let body = to_bytes(index.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["skills"][0]["name"], "echo");

        let skill = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/.well-known/skills/echo/SKILL.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(skill.status(), StatusCode::OK);
        assert_eq!(skill.headers()[header::CONTENT_TYPE], "text/markdown");
    }

    #[tokio::test]
    async fn test_fetch_gateway_forwards_http_request() {
        let cli = Cli::create("test").fetch_gateway(
            "api",
            GatewayHandler,
            crate::fetch::FetchGatewayOptions {
                description: None,
                base_path: None,
                output_policy: None,
            },
        );
        let response = build_cli_router(&cli)
            .unwrap()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/users?limit=2")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["path"], "/api/users");
        assert_eq!(value["method"], "POST");
        assert_eq!(value["query"][0], serde_json::json!(["limit", "2"]));
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
                default_command: None,
            },
        );

        let mut commands = BTreeMap::new();
        let mut group_mw = BTreeMap::new();
        let mut gateways = BTreeMap::new();
        flatten_commands(&entries, "", &mut commands, &mut group_mw, &mut gateways);

        assert!(commands.contains_key("hello"));
        assert!(commands.contains_key("users/list"));
        assert!(commands.contains_key("users/get"));
        assert!(!commands.contains_key("users"));
    }
}
