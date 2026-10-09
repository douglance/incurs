//! Runtime-free MCP Streamable HTTP server.
//!
//! [`McpHttpServer`] serves a CLI's commands as MCP tools over the stateless
//! Streamable HTTP transport without sockets, threads, timers, or a tokio
//! runtime, so a wasm32 host such as a Cloudflare Worker can drive it. The host
//! converts its native request into an [`McpHttpRequest`], awaits
//! [`McpHttpServer::handle`], and writes the returned [`McpHttpResponse`].
//!
//! The baseline request and tool wire behavior is compared with the native
//! stateless server by the sibling `parity_tests` module. This server also
//! validates `Origin` (see [`McpHttpConfig::allowed_origins`]). Bidirectional
//! peer requests use [`McpHttpServer::handle_for_peer`] with a trusted session
//! supplied by the host; anonymous requests cannot resolve peer responses.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, Stream, StreamExt};
use serde_json::{Map, Value, json};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::McpServeOptions;
use super::shared::{CallContext, ResourceContext, ServerSource, ToolServer, progress_message};
use crate::command::{McpPeer, McpPeerError, McpPeerNotification, McpPeerRequest};
use crate::tool::{EnvironmentSource, ToolCallControl, ToolEvent, ToolEventSink};

/// Default maximum request body size: 4 MiB.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";
const MODERN: &str = "2026-07-28";
const KNOWN_VERSIONS: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const MISSING_REQUIRED_CLIENT_CAPABILITY: i64 = -32021;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// HTTP configuration for [`McpHttpServer`].
#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    /// Allowed hostnames or `host:port` authorities for the inbound `Host`
    /// header. An entry without a port matches any port. An empty list allows
    /// every host.
    ///
    /// Defaults to `localhost`, `127.0.0.1`, and `::1`, which rejects DNS
    /// rebinding against a locally served endpoint. A public deployment lists
    /// its own hostnames.
    pub allowed_hosts: Vec<String>,
    /// Allowed browser origins for the inbound `Origin` header, as
    /// `scheme://host[:port]` entries or `null`. An entry without a port
    /// matches any port; an empty list allows every origin.
    ///
    /// `None`, the default, allows an origin whose host passes
    /// [`Self::allowed_hosts`]. A request without `Origin` always passes. A
    /// present `Origin` that is not allowed is rejected with `403`, and one
    /// that is not an origin with `400`. This check is the one deliberate
    /// difference from the native server, which does not inspect `Origin`.
    pub allowed_origins: Option<Vec<String>>,
    /// Environment values for command environment fields.
    ///
    /// Defaults to [`EnvironmentSource::DeclaredHost`], which reads the
    /// process environment exactly as the native server does. A wasm32 host
    /// has no process environment, so it supplies
    /// [`EnvironmentSource::Values`] from its own bindings.
    pub environment: EnvironmentSource,
    /// Maximum request body size in bytes; larger bodies receive `413`.
    pub max_request_body_bytes: usize,
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            allowed_origins: None,
            environment: EnvironmentSource::DeclaredHost,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        }
    }
}

/// One inbound HTTP request, independent of any HTTP library.
#[derive(Debug, Clone, Default)]
pub struct McpHttpRequest {
    /// HTTP method, such as `POST`.
    pub method: String,
    /// Request path, passed to commands as request metadata.
    pub path: String,
    /// Header names and values in arrival order. Names match
    /// case-insensitively; the first value of a repeated header is used.
    pub headers: Vec<(String, String)>,
    /// Complete request body.
    pub body: Vec<u8>,
}

/// The body of an [`McpHttpResponse`].
pub enum McpHttpBody {
    /// No body.
    Empty,
    /// A complete body.
    Full(Vec<u8>),
    /// Server-sent events, each item one complete event including its
    /// terminating blank line. Dropping the stream cancels the tool call that
    /// produces it.
    EventStream(BoxStream<'static, String>),
}

impl std::fmt::Debug for McpHttpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Full(bytes) => f.debug_tuple("Full").field(&bytes.len()).finish(),
            Self::EventStream(_) => f.write_str("EventStream"),
        }
    }
}

/// One outbound HTTP response.
#[derive(Debug)]
pub struct McpHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response header names and values.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: McpHttpBody,
}

impl McpHttpResponse {
    fn text(status: u16, text: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: McpHttpBody::Full(text.into().into_bytes()),
        }
    }

    fn bad_request(text: &str) -> Self {
        Self {
            headers: vec![(
                "content-type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            ..Self::text(400, text)
        }
    }

    fn json(status: u16, message: &Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: McpHttpBody::Full(message.to_string().into_bytes()),
        }
    }

    fn accepted() -> Self {
        Self {
            status: 202,
            headers: Vec::new(),
            body: McpHttpBody::Empty,
        }
    }

    fn event_stream(stream: impl Stream<Item = Value> + Send + 'static) -> Self {
        Self {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "text/event-stream".to_string()),
                ("cache-control".to_string(), "no-cache".to_string()),
                ("x-accel-buffering".to_string(), "no".to_string()),
            ],
            body: McpHttpBody::EventStream(
                stream.map(|message| format!("data: {message}\n\n")).boxed(),
            ),
        }
    }
}

type PendingPeerRequests =
    Arc<Mutex<HashMap<(String, String), oneshot::Sender<Result<Value, McpPeerError>>>>>;

struct PendingPeerRequestGuard {
    pending: PendingPeerRequests,
    id: Option<(String, String)>,
}

impl PendingPeerRequestGuard {
    fn new(pending: PendingPeerRequests, id: (String, String)) -> Self {
        Self {
            pending,
            id: Some(id),
        }
    }
}

impl Drop for PendingPeerRequestGuard {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }
}

/// A transport-neutral MCP Streamable HTTP server for one CLI.
///
/// Cloning is cheap; clones share the resolved tool catalog.
#[derive(Clone)]
pub struct McpHttpServer {
    tools: ToolServer,
    config: Arc<McpHttpConfig>,
    peer_ids: Arc<AtomicU64>,
    pending_peer: PendingPeerRequests,
}

impl McpHttpServer {
    /// Builds a server for a CLI using its configured MCP options.
    pub fn from_cli(
        cli: &crate::cli::Cli,
        config: McpHttpConfig,
    ) -> Result<Self, crate::errors::Error> {
        Self::new(cli, &cli.mcp_options, config)
    }

    /// Builds a server for a CLI with explicit MCP options.
    pub fn new(
        cli: &crate::cli::Cli,
        options: &McpServeOptions,
        config: McpHttpConfig,
    ) -> Result<Self, crate::errors::Error> {
        Ok(Self {
            tools: ToolServer::new(&ServerSource::from_cli(cli), options)?,
            config: Arc::new(config),
            peer_ids: Arc::new(AtomicU64::new(1)),
            pending_peer: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Answers one HTTP request.
    ///
    /// Validation, notifications, and most requests resolve immediately. A
    /// request that negotiates its protocol version inline resolves once the
    /// first JSON-RPC message is ready, because its HTTP status depends on it.
    /// Dropping the returned future or an event-stream body cancels the tool
    /// call behind it.
    /// Largest request body this server accepts. A transport should stop
    /// reading one byte past it: the server answers `413` for any longer body.
    pub fn max_request_body_bytes(&self) -> usize {
        self.config.max_request_body_bytes
    }

    /// Answers a stateless request without a trusted peer session.
    /// Server-initiated requests require [`Self::handle_for_peer`].
    pub async fn handle(&self, request: McpHttpRequest) -> McpHttpResponse {
        self.handle_inner(request, None).await
    }

    /// Answers a request owned by a trusted authenticated client session.
    ///
    /// The host must derive `session_id` from verified authentication and session
    /// ownership, never from an unverified HTTP header or JSON-RPC parameter.
    /// Supply the same ID on the client's response POSTs. Different sessions
    /// cannot resolve each other's requests, even when a response ID is known.
    /// A local fixture may explicitly supply its isolated fixture session ID.
    pub async fn handle_for_peer(
        &self,
        request: McpHttpRequest,
        session_id: &str,
    ) -> McpHttpResponse {
        if session_id.is_empty() {
            return McpHttpResponse::bad_request("Trusted peer session must not be empty");
        }
        self.handle_inner(request, Some(session_id.to_string()))
            .await
    }

    async fn handle_inner(
        &self,
        request: McpHttpRequest,
        peer_session: Option<String>,
    ) -> McpHttpResponse {
        let headers = Headers(&request.headers);
        if let Err(response) = self.validate_host_and_origin(&headers) {
            return response;
        }
        if request.method != "POST" {
            let mut response = McpHttpResponse::text(405, "Method Not Allowed");
            response
                .headers
                .push(("allow".to_string(), "POST".to_string()));
            return response;
        }
        match self.handle_post(&request, &headers, peer_session).await {
            Ok(response) | Err(response) => response,
        }
    }

    fn validate_host_and_origin(&self, headers: &Headers<'_>) -> Result<(), McpHttpResponse> {
        let host = match headers.get("host") {
            HeaderValue::Absent => {
                return Err(McpHttpResponse::bad_request(
                    "Bad Request: missing Host header",
                ));
            }
            HeaderValue::Opaque => {
                return Err(McpHttpResponse::bad_request(
                    "Bad Request: Invalid Host header encoding",
                ));
            }
            HeaderValue::Text(host) => parse_authority(host)
                .ok_or_else(|| McpHttpResponse::bad_request("Bad Request: Invalid Host header"))?,
        };
        if !host_is_allowed(&host, &self.config.allowed_hosts) {
            return Err(McpHttpResponse::text(
                403,
                "Forbidden: Host header is not allowed",
            ));
        }
        let origin = match headers.get("origin") {
            HeaderValue::Absent => return Ok(()),
            HeaderValue::Opaque => {
                return Err(McpHttpResponse::bad_request(
                    "Bad Request: Invalid Origin header encoding",
                ));
            }
            HeaderValue::Text(origin) => parse_origin(origin).ok_or_else(|| {
                McpHttpResponse::bad_request("Bad Request: Invalid Origin header")
            })?,
        };
        let allowed = match &self.config.allowed_origins {
            Some(allowed) => origin_is_allowed(&origin, allowed),
            None => match &origin {
                Origin::Null => self.config.allowed_hosts.is_empty(),
                Origin::Tuple { host, port, .. } => host_is_allowed(
                    &Authority {
                        host: host.clone(),
                        port: *port,
                    },
                    &self.config.allowed_hosts,
                ),
            },
        };
        if allowed {
            Ok(())
        } else {
            Err(McpHttpResponse::text(
                403,
                "Forbidden: Origin header is not allowed",
            ))
        }
    }

    async fn handle_post(
        &self,
        request: &McpHttpRequest,
        headers: &Headers<'_>,
        peer_session: Option<String>,
    ) -> Result<McpHttpResponse, McpHttpResponse> {
        let accepts = headers.text("accept").is_some_and(|accept| {
            accept.contains("application/json") && accept.contains("text/event-stream")
        });
        if !accepts {
            return Err(McpHttpResponse::text(
                406,
                "Not Acceptable: Client must accept both application/json and text/event-stream",
            ));
        }
        if !headers
            .text("content-type")
            .is_some_and(|value| value.starts_with("application/json"))
        {
            return Err(McpHttpResponse::text(
                415,
                "Unsupported Media Type: Content-Type must be application/json",
            ));
        }
        let limit = self.config.max_request_body_bytes;
        if request.body.len() > limit {
            return Err(McpHttpResponse::text(
                413,
                format!("Payload Too Large: request body exceeds {limit} bytes"),
            ));
        }
        let message = parse_message(&request.body).map_err(|error| {
            McpHttpResponse::text(415, format!("fail to deserialize request body {error}"))
        })?;

        let per_request_version = match &message {
            Message::Request(request) => request.meta_protocol_version().is_some(),
            _ => false,
        };
        match &message {
            Message::Request(request) => match &request.kind {
                Kind::Initialize { protocol_version } => {
                    header_matches_init_body(headers, protocol_version, &request.id)?
                }
                _ => validate_protocol_version_header(headers, per_request_version)?,
            },
            _ => validate_protocol_version_header(headers, per_request_version)?,
        }
        validate_standard_headers(headers, &message)?;
        if let Message::Request(request) = &message {
            validate_request_protocol_version_meta(headers, request)?;
        }

        let Message::Request(client_request) = message else {
            if let Message::Response { id, outcome } = message {
                self.complete_peer_response(peer_session.as_deref(), id, outcome);
            }
            return Ok(McpHttpResponse::accepted());
        };
        let negotiates = per_request_version || matches!(client_request.kind, Kind::Discover);
        let peer_version = match &client_request.kind {
            Kind::Initialize { protocol_version } => protocol_version.clone(),
            _ => headers
                .text(PROTOCOL_VERSION_HEADER)
                .unwrap_or("2025-03-26")
                .to_string(),
        };
        let transport = crate::command::RequestContext {
            headers: request
                .headers
                .iter()
                .filter(|(_, value)| is_header_text(value))
                .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
                .collect(),
            method: request.method.clone(),
            path: request.path.clone(),
        };
        if matches!(client_request.kind, Kind::SubscriptionsListen { .. }) {
            return Ok(self
                .subscription_response(*client_request, peer_version, Some(transport))
                .await);
        }
        let mut stream = MessageStream::spawn(
            self.clone(),
            *client_request,
            peer_version,
            Some(transport),
            peer_session,
        );
        if !negotiates {
            return Ok(McpHttpResponse::event_stream(stream));
        }
        let Some(first) = stream.next().await else {
            return Err(McpHttpResponse::text(
                500,
                "Encounter an error when empty response: no response message received from handler",
            ));
        };
        let terminal = first.get("method").is_none();
        let status = http_status(&first);
        if terminal && status != 200 {
            drop(stream);
            return Ok(McpHttpResponse::json(status, &first));
        }
        Ok(McpHttpResponse::event_stream(
            futures::stream::once(async move { first }).chain(stream),
        ))
    }

    fn connected_peer(
        &self,
        sender: mpsc::UnboundedSender<Value>,
        peer_session: Option<String>,
    ) -> McpPeer {
        let request_pending = Arc::clone(&self.pending_peer);
        let request_ids = Arc::clone(&self.peer_ids);
        let request_sender = sender.clone();
        let notify_sender = sender;
        McpPeer::with_notify(
            move |request: McpPeerRequest| {
                let peer_session = peer_session.clone();
                let pending = Arc::clone(&request_pending);
                let sender = request_sender.clone();
                let id = format!("incurs-peer-{}", request_ids.fetch_add(1, Ordering::SeqCst));
                async move {
                    let Some(session) = peer_session else {
                        return Err(McpPeerError {
                            code: "MCP_PEER_SESSION_REQUIRED".to_string(),
                            message: "Server-initiated requests require a trusted peer session"
                                .to_string(),
                            data: None,
                        });
                    };
                    let key = (session, id.clone());
                    let (tx, rx) = oneshot::channel();
                    if let Ok(mut pending) = pending.lock() {
                        pending.insert(key.clone(), tx);
                    } else {
                        return Err(McpPeerError {
                            code: "MCP_PEER_STATE_UNAVAILABLE".to_string(),
                            message: "MCP peer response state is unavailable".to_string(),
                            data: None,
                        });
                    }
                    let _pending_guard =
                        PendingPeerRequestGuard::new(Arc::clone(&pending), key.clone());
                    let message = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": request.method,
                        "params": params_with_meta(request.params, request.meta),
                    });
                    if sender.unbounded_send(message).is_err() {
                        if let Ok(mut pending) = pending.lock() {
                            pending.remove(&key);
                        }
                        return Err(McpPeerError {
                            code: "MCP_PEER_STREAM_CLOSED".to_string(),
                            message: "MCP peer event stream closed before the request was sent"
                                .to_string(),
                            data: None,
                        });
                    }
                    rx.await.map_err(|_| McpPeerError {
                        code: "MCP_PEER_RESPONSE_DROPPED".to_string(),
                        message: "MCP peer response was dropped before completion".to_string(),
                        data: None,
                    })?
                }
            },
            move |notification: McpPeerNotification| {
                let sender = notify_sender.clone();
                async move {
                    sender
                        .unbounded_send(json!({
                            "jsonrpc": "2.0",
                            "method": notification.method,
                            "params": params_with_meta(notification.params, notification.meta),
                        }))
                        .map_err(|_| McpPeerError {
                            code: "MCP_PEER_STREAM_CLOSED".to_string(),
                            message:
                                "MCP peer event stream closed before the notification was sent"
                                    .to_string(),
                            data: None,
                        })
                }
            },
        )
    }

    fn complete_peer_response(
        &self,
        peer_session: Option<&str>,
        id: String,
        outcome: Result<Value, McpPeerError>,
    ) {
        let Some(session) = peer_session else {
            return;
        };
        if let Ok(mut pending) = self.pending_peer.lock()
            && let Some(sender) = pending.remove(&(session.to_string(), id))
        {
            let _ = sender.send(outcome);
        }
    }

    /// Runs one request through MCP dispatch and returns its final message.
    async fn dispatch(
        &self,
        request: ClientRequest,
        peer_version: String,
        transport: Option<crate::command::RequestContext>,
        events: Arc<ProgressSink>,
        cancellation: CancellationToken,
        peer_session: Option<String>,
    ) -> Value {
        let id = request.id.clone();
        match self
            .dispatch_result(
                request,
                peer_version,
                transport,
                events,
                cancellation,
                peer_session,
            )
            .await
        {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        }
    }

    fn supported_versions(&self) -> Vec<String> {
        self.tools
            .standards
            .versions()
            .iter()
            .map(|version| version.as_str().to_string())
            .collect()
    }

    fn initialize_versions(&self) -> Vec<String> {
        self.supported_versions()
            .into_iter()
            .filter(|version| version.as_str() != MODERN)
            .collect()
    }

    fn server_info(&self) -> Value {
        json!({ "name": self.tools.server_name, "version": self.tools.server_version })
    }

    fn capabilities(&self) -> Value {
        let mut capabilities = self.tools.capabilities();
        if self.tools.subscription_listen_supported()
            && let Some(object) = capabilities.as_object_mut()
        {
            object.insert("subscriptions".to_string(), json!({ "listen": true }));
        }
        capabilities
    }

    fn initialize_result(&self, protocol_version: &str) -> Value {
        let mut result = json!({
            "protocolVersion": protocol_version,
            "capabilities": self.capabilities(),
            "serverInfo": self.server_info(),
        });
        if let Some(instructions) = &self.tools.instructions {
            result["instructions"] = json!(instructions);
        }
        result
    }

    async fn subscription_response(
        &self,
        request: ClientRequest,
        peer_version: String,
        transport: Option<crate::command::RequestContext>,
    ) -> McpHttpResponse {
        let id = request.id.clone();
        let requested = request.meta_protocol_version();
        let protocol_version = requested.unwrap_or(peer_version);
        let context = resource_context(&request, &protocol_version, None);
        let Kind::SubscriptionsListen {
            notifications,
            resource_uris,
            cursor,
        } = request.kind
        else {
            return McpHttpResponse::json(
                500,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": error(INVALID_REQUEST, "request was not subscriptions/listen"),
                }),
            );
        };
        let _ = transport;
        match self
            .tools
            .listen_subscriptions(notifications, resource_uris, cursor, context)
            .await
        {
            Ok(stream) => McpHttpResponse::event_stream(stream),
            Err(subscription_error) => {
                let message = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": resource_error(subscription_error),
                });
                McpHttpResponse::json(http_status(&message), &message)
            }
        }
    }

    async fn dispatch_result(
        &self,
        request: ClientRequest,
        peer_version: String,
        transport: Option<crate::command::RequestContext>,
        events: Arc<ProgressSink>,
        cancellation: CancellationToken,
        peer_session: Option<String>,
    ) -> Result<Value, Value> {
        let requested = request.meta_protocol_version();
        let protocol_version = requested.clone().unwrap_or(peer_version);
        let modern_result = protocol_version.as_str() >= MODERN;
        let connected_peer = self.connected_peer(events.sender.clone(), peer_session);
        let inline = !matches!(request.kind, Kind::Initialize { .. });
        let supported = self.supported_versions();
        if inline
            && let Some(requested) = &requested
            && !supported.contains(requested)
        {
            return Err(unsupported_protocol_version(requested, &supported));
        }
        let requires_metadata = inline
            && (matches!(request.kind, Kind::Discover)
                || requested
                    .as_deref()
                    .is_some_and(|version| version >= MODERN));
        if requires_metadata {
            let mut missing = Vec::new();
            if requested.is_none() {
                missing.push(META_PROTOCOL_VERSION);
            }
            if !request
                .meta
                .get(META_CLIENT_CAPABILITIES)
                .is_some_and(Value::is_object)
            {
                missing.push(META_CLIENT_CAPABILITIES);
            }
            if !missing.is_empty() {
                return Err(error(
                    INVALID_PARAMS,
                    format!(
                        "request _meta is missing or has malformed required fields: {}",
                        missing.join(", ")
                    ),
                ));
            }
        }
        let legacy = !requires_metadata && protocol_version.as_str() < MODERN;
        let exactly_modern = protocol_version == MODERN;
        let cached = |mut result: Value| {
            if exactly_modern {
                result["ttlMs"] = json!(0);
                result["cacheScope"] = json!("private");
            }
            result
        };

        let mut result = match request.kind {
            Kind::Initialize { protocol_version } => {
                let initialize_supported = self.initialize_versions();
                let selected = initialize_supported
                    .iter()
                    .find(|version| **version == protocol_version)
                    .cloned();
                let selected =
                    if selected.is_none() && KNOWN_VERSIONS.contains(&protocol_version.as_str()) {
                        None
                    } else {
                        selected.or_else(|| initialize_supported.first().cloned())
                    };
                match selected {
                    Some(selected) => self.initialize_result(&selected),
                    None => {
                        return Err(unsupported_protocol_version(
                            &protocol_version,
                            &initialize_supported,
                        ));
                    }
                }
            }
            Kind::Discover => {
                let mut result = json!({
                    "resultType": "complete",
                    "supportedVersions": supported,
                    "capabilities": self.capabilities(),
                    "ttlMs": 0,
                    "cacheScope": "private",
                    "_meta": { "io.modelcontextprotocol/serverInfo": self.server_info() },
                });
                if let Some(instructions) = &self.tools.instructions {
                    result["instructions"] = json!(instructions);
                }
                result
            }
            Kind::Ping if legacy => json!({}),
            Kind::Complete => json!({ "resultType": "complete", "completion": { "values": [] } }),
            Kind::ListTools => cached(json!({
                "resultType": "complete",
                "tools": *self.tools.tool_list,
            })),
            Kind::ListPrompts => cached(json!({ "resultType": "complete", "prompts": [] })),
            Kind::ListResources => cached(self.tools.list_resources()),
            Kind::ListResourceTemplates => cached(self.tools.list_resource_templates()),
            Kind::ReadResource { ref uri } => cached(
                self.tools
                    .read_resource(
                        uri.clone(),
                        resource_context(&request, &protocol_version, Some(connected_peer.clone())),
                    )
                    .await
                    .map_err(resource_error)?,
            ),
            Kind::Subscribe { ref uri } => cached(
                self.tools
                    .set_resource_subscription(
                        uri.clone(),
                        true,
                        resource_context(&request, &protocol_version, Some(connected_peer.clone())),
                    )
                    .await
                    .map_err(resource_error)?,
            ),
            Kind::Unsubscribe { ref uri } => cached(
                self.tools
                    .set_resource_subscription(
                        uri.clone(),
                        false,
                        resource_context(&request, &protocol_version, Some(connected_peer.clone())),
                    )
                    .await
                    .map_err(resource_error)?,
            ),
            Kind::CallTool { name, arguments } => {
                let progress = request
                    .meta
                    .get("progressToken")
                    .filter(|token| is_request_id(token));
                events.set_token(progress.cloned());
                self.tools
                    .call_tool(
                        name,
                        arguments,
                        CallContext {
                            protocol_version: protocol_version.clone(),
                            request: transport,
                            request_meta: Some(Value::Object(request.meta.clone())),
                            client_capabilities: request
                                .meta
                                .get(META_CLIENT_CAPABILITIES)
                                .cloned(),
                            input_responses: request.input_responses.clone(),
                            request_state: request.request_state.clone(),
                            peer: Some(connected_peer),
                            control: ToolCallControl {
                                cancellation,
                                events: Some(events),
                            },
                            environment: self.config.environment.clone(),
                        },
                    )
                    .await
                    .map_err(|message| error(INVALID_PARAMS, message))?
            }
            Kind::Ping
            | Kind::SetLevel
            | Kind::GetPrompt
            | Kind::SubscriptionsListen { .. }
            | Kind::Task
            | Kind::Custom => {
                return Err(error(METHOD_NOT_FOUND, request.method));
            }
        };
        if !modern_result && result.get("resultType") == Some(&json!("input_required")) {
            return Err(error(
                INVALID_REQUEST,
                "InputRequiredResult requires negotiated protocol version 2026-07-28 or newer",
            ));
        }
        if !modern_result
            && let Some(object) = result.as_object_mut()
            && object.get("resultType") == Some(&json!("complete"))
            && !object.contains_key("supportedVersions")
        {
            object.remove("resultType");
        }
        Ok(result)
    }
}

fn error(code: i64, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn resource_error(resource_error: super::McpResourceError) -> Value {
    let mut value = error(i64::from(resource_error.code), resource_error.message);
    if let Some(data) = resource_error.data {
        value["data"] = data;
    }
    value
}

fn resource_context(
    request: &ClientRequest,
    protocol_version: &str,
    peer: Option<McpPeer>,
) -> ResourceContext {
    ResourceContext {
        protocol_version: Some(protocol_version.to_string()),
        request_meta: Some(Value::Object(request.meta.clone())),
        client_capabilities: request.meta.get(META_CLIENT_CAPABILITIES).cloned(),
        input_responses: request.input_responses.clone(),
        request_state: request.request_state.clone(),
        peer,
    }
}

fn params_with_meta(params: Option<Value>, meta: Option<Value>) -> Option<Value> {
    let Some(meta) = meta else {
        return params;
    };
    match params {
        Some(Value::Object(mut object)) => {
            object.insert("_meta".to_string(), meta);
            Some(Value::Object(object))
        }
        Some(value) => Some(json!({ "value": value, "_meta": meta })),
        None => Some(json!({ "_meta": meta })),
    }
}

fn unsupported_protocol_version(requested: &str, supported: &[String]) -> Value {
    json!({
        "code": UNSUPPORTED_PROTOCOL_VERSION,
        "message": "Unsupported protocol version",
        "data": { "requested": requested, "supported": supported },
    })
}

fn http_status(message: &Value) -> u16 {
    match message
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
    {
        Some(
            UNSUPPORTED_PROTOCOL_VERSION | MISSING_REQUIRED_CLIENT_CAPABILITY | INVALID_PARAMS,
        ) => 400,
        Some(METHOD_NOT_FOUND) => 404,
        _ => 200,
    }
}

// ---------------------------------------------------------------------------
// Response streaming
// ---------------------------------------------------------------------------

/// Delivers tool events for one request as MCP progress notifications.
struct ProgressSink {
    sender: mpsc::UnboundedSender<Value>,
    token: std::sync::Mutex<Option<Value>>,
    count: AtomicU64,
}

impl ProgressSink {
    fn set_token(&self, token: Option<Value>) {
        if let Ok(mut slot) = self.token.lock() {
            *slot = token;
        }
    }
}

#[async_trait::async_trait]
impl ToolEventSink for ProgressSink {
    async fn emit(&self, event: ToolEvent) {
        let Some(token) = self.token.lock().ok().and_then(|token| token.clone()) else {
            return;
        };
        let count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.sender.unbounded_send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": token,
                "progress": count as f64,
                "message": progress_message(event),
            },
        }));
    }
}

/// The JSON-RPC messages for one request, driven by polling the stream.
///
/// The request's dispatch future runs inside `poll_next`, so no executor is
/// needed. Dropping the stream before it ends cancels the request's
/// [`ToolCallControl::cancellation`] token.
struct MessageStream {
    driver: Option<BoxFuture<'static, ()>>,
    receiver: mpsc::UnboundedReceiver<Value>,
    cancel_on_drop: Option<DropGuard>,
}

impl MessageStream {
    fn spawn(
        server: McpHttpServer,
        request: ClientRequest,
        peer_version: String,
        transport: Option<crate::command::RequestContext>,
        peer_session: Option<String>,
    ) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        let cancellation = CancellationToken::new();
        let events = Arc::new(ProgressSink {
            sender: sender.clone(),
            token: std::sync::Mutex::new(None),
            count: AtomicU64::new(0),
        });
        let token = cancellation.clone();
        let driver = async move {
            let message = server
                .dispatch(
                    request,
                    peer_version,
                    transport,
                    events,
                    token,
                    peer_session,
                )
                .await;
            let _ = sender.unbounded_send(message);
            sender.close_channel();
        }
        .boxed();
        Self {
            driver: Some(driver),
            receiver,
            cancel_on_drop: Some(cancellation.drop_guard()),
        }
    }
}

impl Stream for MessageStream {
    type Item = Value;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Value>> {
        if let Some(driver) = self.driver.as_mut()
            && driver.poll_unpin(cx).is_ready()
        {
            self.driver = None;
        }
        let polled = self.receiver.poll_next_unpin(cx);
        if let Poll::Ready(None) = polled
            && let Some(guard) = self.cancel_on_drop.take()
        {
            // The request completed; there is nothing left to cancel.
            guard.disarm();
        }
        polled
    }
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

enum HeaderValue<'a> {
    Absent,
    /// Present, but not representable as visible ASCII text.
    Opaque,
    Text(&'a str),
}

/// Whether an HTTP header value is visible ASCII (plus tab).
fn is_header_text(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || (32..127).contains(&byte))
}

struct Headers<'a>(&'a [(String, String)]);

impl<'a> Headers<'a> {
    fn get(&self, name: &str) -> HeaderValue<'a> {
        match self
            .0
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            None => HeaderValue::Absent,
            Some((_, value)) if is_header_text(value) => HeaderValue::Text(value),
            Some(_) => HeaderValue::Opaque,
        }
    }

    fn text(&self, name: &str) -> Option<&'a str> {
        match self.get(name) {
            HeaderValue::Text(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Authority {
    host: String,
    port: Option<u16>,
}

fn normalize_host(host: &str) -> String {
    host.trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase()
}

fn is_authority_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
                | ':'
                | '['
                | ']'
                | '@'
        )
}

/// Parses an RFC 3986 authority (`[userinfo@]host[:port]`).
fn parse_authority(value: &str) -> Option<Authority> {
    if value.is_empty() || !value.chars().all(is_authority_char) {
        return None;
    }
    let host_port = value.rsplit_once('@').map_or(value, |(_, rest)| rest);
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if inside.is_empty() || inside.contains('[') {
            return None;
        }
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':')?),
        };
        (&host_port[..inside.len() + 2], port)
    } else {
        if host_port.contains('[') || host_port.contains(']') {
            return None;
        }
        match host_port.split_once(':') {
            Some((host, port)) => {
                if port.contains(':') {
                    return None;
                }
                (host, Some(port))
            }
            None => (host_port, None),
        }
    };
    let port = match port {
        None | Some("") => None,
        Some(port) => {
            if !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some(port.parse::<u16>().ok()?)
        }
    };
    Some(Authority {
        host: normalize_host(host),
        port,
    })
}

fn host_is_allowed(host: &Authority, allowed_hosts: &[String]) -> bool {
    if allowed_hosts.is_empty() {
        return true;
    }
    allowed_hosts
        .iter()
        .filter_map(|allowed| {
            let allowed = allowed.trim();
            if allowed.is_empty() {
                return None;
            }
            parse_authority(allowed).or_else(|| {
                Some(Authority {
                    host: normalize_host(allowed),
                    port: None,
                })
            })
        })
        .any(|allowed| {
            allowed.host == host.host && allowed.port.is_none_or(|port| host.port == Some(port))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Null,
    Tuple {
        scheme: String,
        host: String,
        port: Option<u16>,
    },
}

fn parse_origin(value: &str) -> Option<Origin> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("null") {
        return Some(Origin::Null);
    }
    let (scheme, rest) = value.split_once("://")?;
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = parse_authority(authority)?;
    Some(Origin::Tuple {
        scheme: scheme.to_ascii_lowercase(),
        host: authority.host,
        port: authority.port,
    })
}

fn origin_is_allowed(origin: &Origin, allowed_origins: &[String]) -> bool {
    if allowed_origins.is_empty() {
        return true;
    }
    allowed_origins
        .iter()
        .filter_map(|raw| parse_origin(raw))
        .any(|allowed| match (&allowed, origin) {
            (Origin::Null, Origin::Null) => true,
            (
                Origin::Tuple {
                    scheme: a_scheme,
                    host: a_host,
                    port: a_port,
                },
                Origin::Tuple {
                    scheme: o_scheme,
                    host: o_host,
                    port: o_port,
                },
            ) => a_scheme == o_scheme && a_host == o_host && (a_port.is_none() || a_port == o_port),
            _ => false,
        })
}

fn jsonrpc_error(status: u16, id: Option<&Value>, code: i64, message: String) -> McpHttpResponse {
    let mut body = json!({ "jsonrpc": "2.0", "error": error(code, message) });
    if let Some(id) = id {
        body["id"] = id.clone();
    }
    McpHttpResponse::json(status, &body)
}

fn validate_protocol_version_header(
    headers: &Headers<'_>,
    allow_unknown: bool,
) -> Result<(), McpHttpResponse> {
    match headers.get(PROTOCOL_VERSION_HEADER) {
        HeaderValue::Absent => Ok(()),
        HeaderValue::Opaque => Err(McpHttpResponse::text(
            400,
            "Bad Request: Invalid MCP-Protocol-Version header encoding",
        )),
        HeaderValue::Text(version) => {
            if !allow_unknown && !KNOWN_VERSIONS.contains(&version) {
                Err(McpHttpResponse::text(
                    400,
                    format!("Bad Request: Unsupported MCP-Protocol-Version: {version}"),
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn header_matches_init_body(
    headers: &Headers<'_>,
    body_version: &str,
    id: &Value,
) -> Result<(), McpHttpResponse> {
    match headers.get(PROTOCOL_VERSION_HEADER) {
        HeaderValue::Absent => Ok(()),
        HeaderValue::Opaque => Err(jsonrpc_error(
            400,
            Some(id),
            INVALID_REQUEST,
            "Invalid Request: MCP-Protocol-Version header is not valid UTF-8".to_string(),
        )),
        HeaderValue::Text(header) if header != body_version => Err(jsonrpc_error(
            400,
            Some(id),
            INVALID_REQUEST,
            format!(
                "Invalid Request: MCP-Protocol-Version header ({header}) does not match initialize params.protocolVersion ({body_version})"
            ),
        )),
        HeaderValue::Text(_) => Ok(()),
    }
}

fn validate_request_protocol_version_meta(
    headers: &Headers<'_>,
    request: &ClientRequest,
) -> Result<(), McpHttpResponse> {
    if matches!(request.kind, Kind::Initialize { .. }) {
        return Ok(());
    }
    let Some(meta_version) = request.meta_protocol_version() else {
        if matches!(request.kind, Kind::Discover) {
            return Err(jsonrpc_error(
                400,
                Some(&request.id),
                INVALID_PARAMS,
                "Invalid params: server/discover requires protocolVersion in request _meta"
                    .to_string(),
            ));
        }
        return Ok(());
    };
    let Some(header_version) = headers.text(PROTOCOL_VERSION_HEADER) else {
        return Err(jsonrpc_error(
            400,
            Some(&request.id),
            INVALID_REQUEST,
            "Invalid Request: request _meta protocolVersion requires MCP-Protocol-Version header"
                .to_string(),
        ));
    };
    if header_version != meta_version {
        return Err(jsonrpc_error(
            400,
            Some(&request.id),
            HEADER_MISMATCH,
            format!(
                "MCP-Protocol-Version header ({header_version}) does not match request _meta protocolVersion ({meta_version})"
            ),
        ));
    }
    Ok(())
}

/// Validates the `Mcp-Method`, `Mcp-Name`, and `Mcp-Param-*` headers that
/// protocol versions from 2026-07-28 require.
///
/// `Mcp-Param-*` headers are checked against a tool's `x-mcp-header`
/// annotations only when a tool schema is known to the transport. As on the
/// native server, none is, so those headers are not validated.
fn validate_standard_headers(
    headers: &Headers<'_>,
    message: &Message,
) -> Result<(), McpHttpResponse> {
    let requires_headers = headers
        .text(PROTOCOL_VERSION_HEADER)
        .is_some_and(|version| version >= MODERN);
    if !requires_headers {
        return Ok(());
    }
    let (id, method, params) = match message {
        Message::Request(request) => {
            if matches!(request.kind, Kind::Initialize { .. }) {
                return Ok(());
            }
            (Some(&request.id), &request.method, request.params.as_ref())
        }
        Message::Notification { method, params } => (None, method, params.as_ref()),
        Message::Response { .. } => return Ok(()),
    };
    let mismatch = |reason: String| jsonrpc_error(400, id, HEADER_MISMATCH, reason);
    match headers.text("mcp-method") {
        None => return Err(mismatch("missing required Mcp-Method header".to_string())),
        Some(value) if value != method => {
            return Err(mismatch(format!(
                "Mcp-Method header `{value}` does not match body method `{method}`"
            )));
        }
        Some(_) => {}
    }
    let key = match method.as_str() {
        "tools/call" | "prompts/get" => Some("name"),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => Some("uri"),
        "tasks/get" | "tasks/update" | "tasks/cancel" => Some("taskId"),
        _ => None,
    };
    let expected = key.and_then(|key| params?.get(key)?.as_str());
    if let Some(expected) = expected {
        match headers.text("mcp-name") {
            None => {
                return Err(mismatch(format!(
                    "missing required Mcp-Name header for `{method}`"
                )));
            }
            Some(raw) => {
                let decoded = decode_header_value(raw)
                    .ok_or_else(|| mismatch("Mcp-Name header is not valid Base64".to_string()))?;
                if decoded != expected {
                    return Err(mismatch(format!(
                        "Mcp-Name header `{decoded}` does not match body value `{expected}`"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Reverses the `=?base64?<b64>?=` header encoding.
fn decode_header_value(value: &str) -> Option<String> {
    match value
        .strip_prefix("=?base64?")
        .and_then(|inner| inner.strip_suffix("?="))
    {
        Some(inner) => String::from_utf8(decode_base64(inner)?).ok(),
        None => Some(value.to_owned()),
    }
}

/// Strict RFC 4648 base64 with padding, rejecting non-canonical trailing bits.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != chunks) {
            return None;
        }
        let mut value = 0u32;
        for byte in &chunk[..4 - padding] {
            value = (value << 6) | sextet(*byte)?;
        }
        value <<= 6 * padding as u32;
        let decoded = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        let kept = 3 - padding;
        if decoded[kept..].iter().any(|byte| *byte != 0) {
            return None;
        }
        out.extend_from_slice(&decoded[..kept]);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// JSON-RPC message classification
// ---------------------------------------------------------------------------

/// The typed request a message deserializes to, or `Custom` when its method
/// is unknown or its params do not fit the method's schema.
enum Kind {
    Initialize {
        protocol_version: String,
    },
    Discover,
    Ping,
    Complete,
    SetLevel,
    GetPrompt,
    ListPrompts,
    ListResources,
    ListResourceTemplates,
    ReadResource {
        uri: String,
    },
    SubscriptionsListen {
        notifications: Value,
        resource_uris: Vec<String>,
        cursor: Option<String>,
    },
    Subscribe {
        uri: String,
    },
    Unsubscribe {
        uri: String,
    },
    CallTool {
        name: String,
        arguments: Option<Map<String, Value>>,
    },
    ListTools,
    Task,
    Custom,
}

struct ClientRequest {
    id: Value,
    method: String,
    params: Option<Value>,
    meta: Map<String, Value>,
    input_responses: Option<Value>,
    request_state: Option<String>,
    kind: Kind,
}

impl ClientRequest {
    fn meta_protocol_version(&self) -> Option<String> {
        self.meta
            .get(META_PROTOCOL_VERSION)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }
}

enum Message {
    Request(Box<ClientRequest>),
    Notification {
        method: String,
        params: Option<Value>,
    },
    /// A client response or error; neither is answered.
    Response {
        id: String,
        outcome: Result<Value, McpPeerError>,
    },
}

/// Whether a value is a JSON-RPC request id: a string or an `i64` integer.
fn is_request_id(value: &Value) -> bool {
    value.is_string() || value.is_i64() || value.as_u64().is_some_and(|n| n <= i64::MAX as u64)
}

fn request_id_key(value: &Value) -> Option<String> {
    if let Some(id) = value.as_str() {
        return Some(id.to_string());
    }
    if value.is_i64() || value.as_u64().is_some_and(|n| n <= i64::MAX as u64) {
        return Some(value.to_string());
    }
    None
}

/// Splits `params` into its `_meta` map, rejecting shapes no message accepts.
fn params_and_meta(params: Option<&Value>) -> Option<(Option<Value>, Map<String, Value>)> {
    match params {
        None | Some(Value::Null) => Some((None, Map::new())),
        Some(Value::Object(object)) => {
            let meta = match object.get("_meta") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(meta)) => meta.clone(),
                Some(_) => return None,
            };
            let mut params = object.clone();
            params.remove("_meta");
            Some((Some(Value::Object(params)), meta))
        }
        Some(_) => None,
    }
}

fn parse_message(body: &[u8]) -> Result<Message, String> {
    let value: Value = serde_json::from_slice(body).map_err(|error| error.to_string())?;
    let untagged = || "data did not match any variant of untagged enum JsonRpcMessage".to_string();
    let Value::Object(object) = value else {
        return Err(untagged());
    };
    if object.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(untagged());
    }
    let method = object.get("method").and_then(Value::as_str);
    let params = params_and_meta(object.get("params"));
    let id = object.get("id").filter(|id| is_request_id(id));
    if let (Some(id), Some(method), Some((params, meta))) = (id, method, params.clone()) {
        let kind = classify(method, params.as_ref());
        let input_responses = params
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|params| params.get("inputResponses").cloned());
        let request_state = params
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|params| params.get("requestState"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        return Ok(Message::Request(Box::new(ClientRequest {
            id: id.clone(),
            method: method.to_string(),
            params,
            meta,
            input_responses,
            request_state,
            kind,
        })));
    }
    if let Some(id) = id.and_then(request_id_key)
        && object.contains_key("result")
    {
        return Ok(Message::Response {
            id,
            outcome: Ok(object.get("result").cloned().unwrap_or(Value::Null)),
        });
    }
    if let (Some(method), Some((params, _))) = (method, params) {
        return Ok(Message::Notification {
            method: method.to_string(),
            params,
        });
    }
    let error_id_ok = object
        .get("id")
        .is_none_or(|id| id.is_null() || is_request_id(id));
    let error_ok = object.get("error").is_some_and(|error| {
        error
            .get("code")
            .and_then(Value::as_i64)
            .is_some_and(|code| i32::try_from(code).is_ok())
            && error.get("message").is_some_and(Value::is_string)
    });
    if error_id_ok && error_ok {
        if let Some(id) = object.get("id").and_then(request_id_key) {
            let error = object.get("error").cloned().unwrap_or(Value::Null);
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .map(|code| code.to_string())
                .unwrap_or_else(|| "MCP_PEER_ERROR".to_string());
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MCP peer returned an error")
                .to_string();
            let data = error.get("data").cloned();
            return Ok(Message::Response {
                id: id.to_string(),
                outcome: Err(McpPeerError {
                    code,
                    message,
                    data,
                }),
            });
        }
        return Ok(Message::Response {
            id: String::new(),
            outcome: Err(McpPeerError {
                code: "MCP_PEER_ERROR".to_string(),
                message: "MCP peer returned an error without a string id".to_string(),
                data: object.get("error").cloned(),
            }),
        });
    }
    Err(untagged())
}

fn is_str(params: &Value, key: &str) -> bool {
    params.get(key).is_some_and(Value::is_string)
}

fn is_optional(params: &Value, key: &str, check: fn(&Value) -> bool) -> bool {
    params
        .get(key)
        .is_none_or(|value| value.is_null() || check(value))
}

fn string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

fn is_implementation(value: &Value) -> bool {
    is_str(value, "name")
        && is_str(value, "version")
        && ["title", "description", "websiteUrl"]
            .iter()
            .all(|key| is_optional(value, key, Value::is_string))
        && is_optional(value, "icons", Value::is_array)
}

fn classify(method: &str, params: Option<&Value>) -> Kind {
    let object = params.filter(|params| params.is_object());
    let with = |check: &dyn Fn(&Value) -> bool, kind: Kind| match object {
        Some(params) if check(params) => kind,
        _ => Kind::Custom,
    };
    let input_state = |params: &Value| {
        is_optional(params, "requestState", Value::is_string)
            && is_optional(params, "inputResponses", Value::is_object)
    };
    match method {
        "ping" => Kind::Ping,
        "initialize" => match object {
            Some(params)
                if params.get("capabilities").is_some_and(Value::is_object)
                    && params.get("clientInfo").is_some_and(is_implementation) =>
            {
                match params.get("protocolVersion").and_then(Value::as_str) {
                    Some(version) => Kind::Initialize {
                        protocol_version: version.to_string(),
                    },
                    None => Kind::Custom,
                }
            }
            _ => Kind::Custom,
        },
        "server/discover" => with(&|_| true, Kind::Discover),
        "completion/complete" => with(
            &|params| {
                let reference = params.get("ref");
                let reference_ok = match reference
                    .and_then(|reference| reference.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("ref/prompt") => reference.is_some_and(|r| is_str(r, "name")),
                    Some("ref/resource") => reference.is_some_and(|r| is_str(r, "uri")),
                    _ => false,
                };
                reference_ok
                    && params.get("argument").is_some_and(|argument| {
                        is_str(argument, "name") && is_str(argument, "value")
                    })
                    && is_optional(params, "context", Value::is_object)
            },
            Kind::Complete,
        ),
        "logging/setLevel" => with(
            &|params| {
                matches!(
                    params.get("level").and_then(Value::as_str),
                    Some(
                        "debug"
                            | "info"
                            | "notice"
                            | "warning"
                            | "error"
                            | "critical"
                            | "alert"
                            | "emergency"
                    )
                )
            },
            Kind::SetLevel,
        ),
        "prompts/get" => with(
            &|params| {
                is_str(params, "name")
                    && is_optional(params, "arguments", Value::is_object)
                    && input_state(params)
            },
            Kind::GetPrompt,
        ),
        // Paginated list params are optional and flattened: params that do
        // not fit are dropped rather than rejected.
        "prompts/list" => Kind::ListPrompts,
        "resources/list" => Kind::ListResources,
        "resources/templates/list" => Kind::ListResourceTemplates,
        "tools/list" => Kind::ListTools,
        "resources/read" => match object {
            Some(params) if is_str(params, "uri") && input_state(params) => Kind::ReadResource {
                uri: params["uri"].as_str().unwrap_or_default().to_string(),
            },
            _ => Kind::Custom,
        },
        "subscriptions/listen" => match object {
            Some(params)
                if params.get("notifications").is_some_and(Value::is_object)
                    && is_optional(params, "resourceUris", string_array)
                    && is_optional(params, "cursor", Value::is_string) =>
            {
                Kind::SubscriptionsListen {
                    notifications: params["notifications"].clone(),
                    resource_uris: params
                        .get("resourceUris")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(ToString::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                    cursor: params
                        .get("cursor")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                }
            }
            _ => Kind::Custom,
        },
        "resources/subscribe" => match object {
            Some(params) if is_str(params, "uri") => Kind::Subscribe {
                uri: params["uri"].as_str().unwrap_or_default().to_string(),
            },
            _ => Kind::Custom,
        },
        "resources/unsubscribe" => match object {
            Some(params) if is_str(params, "uri") => Kind::Unsubscribe {
                uri: params["uri"].as_str().unwrap_or_default().to_string(),
            },
            _ => Kind::Custom,
        },
        "tasks/get" | "tasks/cancel" => with(&|params| is_str(params, "taskId"), Kind::Task),
        "tasks/update" => with(
            &|params| {
                is_str(params, "taskId")
                    && params.get("inputResponses").is_some_and(Value::is_object)
            },
            Kind::Task,
        ),
        "tools/call" => match object {
            Some(params)
                if is_str(params, "name")
                    && is_optional(params, "arguments", Value::is_object)
                    && input_state(params) =>
            {
                Kind::CallTool {
                    name: params["name"].as_str().unwrap_or_default().to_string(),
                    arguments: params.get("arguments").and_then(Value::as_object).cloned(),
                }
            }
            _ => Kind::Custom,
        },
        _ => Kind::Custom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use crate::command::{
        CommandContext, CommandDef, CommandHandler, McpCommandOptions, McpPeerRequest,
    };
    use crate::mcp::{
        McpDiscovery, McpResourceRegistry, McpResultMapper, McpResultMapping, McpServeOptions,
        McpSubscriptionListenHandler, McpToolFilter,
    };
    use crate::output::CommandResult;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn base64_is_strict() {
        assert_eq!(decode_base64("cHJvZmlsZQ==").unwrap(), b"profile");
        assert_eq!(decode_base64("").unwrap(), b"");
        assert!(decode_base64("cHJvZmlsZQ").is_none());
        assert!(decode_base64("cHJvZmlsZR==").is_none());
        assert!(decode_base64("***=").is_none());
    }

    #[test]
    fn authorities_parse_like_http_hosts() {
        assert_eq!(
            parse_authority("[::1]:3000"),
            Some(Authority {
                host: "::1".to_string(),
                port: Some(3000)
            })
        );
        assert_eq!(
            parse_authority("Example.COM"),
            Some(Authority {
                host: "example.com".to_string(),
                port: None
            })
        );
        assert!(parse_authority("local host").is_none());
        assert!(parse_authority("host:99999").is_none());
    }

    fn null_origin_ping(config: McpHttpConfig) -> u16 {
        let cli = crate::cli::Cli::create("origin");
        let server = McpHttpServer::from_cli(&cli, config).unwrap();
        let request = McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: vec![
                ("host".to_string(), "localhost".to_string()),
                ("origin".to_string(), "null".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "accept".to_string(),
                    "application/json, text/event-stream".to_string(),
                ),
            ],
            body: br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec(),
        };
        futures::executor::block_on(server.handle(request)).status
    }

    #[test]
    fn a_null_origin_is_refused_unless_explicitly_allowed() {
        // Sandboxed frames and file pages send `Origin: null`. With the default
        // host allowlist nothing vouches for them, so they are refused; a host
        // that wants them lists "null".
        assert_eq!(null_origin_ping(McpHttpConfig::default()), 403);
        assert_eq!(
            null_origin_ping(McpHttpConfig {
                allowed_origins: Some(vec!["null".to_string()]),
                ..McpHttpConfig::default()
            }),
            200
        );
    }

    #[test]
    fn default_origin_policy_follows_allowed_hosts() {
        let origin = parse_origin("http://localhost:5173").unwrap();
        let Origin::Tuple { host, port, .. } = origin else {
            panic!("tuple origin");
        };
        assert!(host_is_allowed(
            &Authority { host, port },
            &McpHttpConfig::default().allowed_hosts
        ));
        assert!(parse_origin("not an origin").is_none());
    }

    struct NeedsInput;

    #[async_trait::async_trait]
    impl CommandHandler for NeedsInput {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let mcp = ctx.mcp.expect("MCP context");
            if let Some(responses) = mcp.input_responses {
                CommandResult::Ok {
                    data: json!({ "responses": responses, "state": mcp.request_state }),
                    cta: None,
                    exit_code: None,
                }
            } else {
                CommandResult::InputRequired {
                    input_requests: std::collections::BTreeMap::from([(
                        "roots".to_string(),
                        json!({ "method": "roots/list", "params": {} }),
                    )]),
                    request_state: Some("state-1".to_string()),
                    meta: std::collections::BTreeMap::new(),
                }
            }
        }
    }

    struct UsesPeer;

    #[async_trait::async_trait]
    impl CommandHandler for UsesPeer {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let peer = ctx.mcp.expect("MCP context").peer.expect("connected peer");
            let response = peer
                .request(McpPeerRequest {
                    method: "sampling/createMessage".to_string(),
                    params: Some(json!({ "messages": [] })),
                    meta: None,
                })
                .await
                .expect("peer response");
            CommandResult::Ok {
                data: response,
                cta: None,
                exit_code: None,
            }
        }
    }

    struct EchoOptions;

    #[async_trait::async_trait]
    impl CommandHandler for EchoOptions {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: json!({ "options": ctx.options }),
                cta: None,
                exit_code: None,
            }
        }
    }

    fn tool_server(mut command: CommandDef) -> McpHttpServer {
        command.output_schema = Some(json!({ "type": "object" }));
        let name = command.name.clone();
        let cli = Cli::create("portable-peer").command(name, command);
        McpHttpServer::new(
            &cli,
            &McpServeOptions {
                tools: McpToolFilter {
                    discovery: McpDiscovery::Direct,
                    ..Default::default()
                },
                ..Default::default()
            },
            McpHttpConfig::default(),
        )
        .unwrap()
    }

    fn tool_server_with_options(
        mut command: CommandDef,
        options: McpServeOptions,
    ) -> McpHttpServer {
        command.output_schema = Some(json!({ "type": "object" }));
        let name = command.name.clone();
        let cli = Cli::create("portable-peer").command(name, command);
        McpHttpServer::new(&cli, &options, McpHttpConfig::default()).unwrap()
    }

    fn post(body: Value, version: &str, method: &str, name: &str) -> McpHttpRequest {
        McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: vec![
                ("host".to_string(), "localhost".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "accept".to_string(),
                    "application/json, text/event-stream".to_string(),
                ),
                (PROTOCOL_VERSION_HEADER.to_string(), version.to_string()),
                ("mcp-method".to_string(), method.to_string()),
                ("mcp-name".to_string(), name.to_string()),
            ],
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    async fn collect_event_stream(response: McpHttpResponse) -> Vec<Value> {
        let McpHttpBody::EventStream(mut stream) = response.body else {
            panic!("event stream response")
        };
        let mut messages = Vec::new();
        while let Some(event) = stream.next().await {
            let payload = event
                .strip_prefix("data: ")
                .and_then(|event| event.strip_suffix("\n\n"))
                .expect("SSE data event");
            messages.push(serde_json::from_str(payload).unwrap());
        }
        messages
    }

    #[test]
    fn progressive_direct_tools_are_callable_by_advertised_name() {
        futures::executor::block_on(async {
            let server = tool_server_with_options(
                CommandDef::build("direct", EchoOptions)
                    .description("Direct tool")
                    .mcp(McpCommandOptions {
                        name: Some("direct.tool".to_string()),
                        direct: true,
                        ..McpCommandOptions::default()
                    })
                    .done(),
                McpServeOptions {
                    tools: McpToolFilter {
                        discovery: McpDiscovery::Progressive,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            let response = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "direct",
                        "method": "tools/call",
                        "params": { "name": "direct.tool", "arguments": { "value": 42 } }
                    }),
                    "2025-03-26",
                    "tools/call",
                    "direct.tool",
                ))
                .await;
            let messages = collect_event_stream(response).await;
            assert_eq!(
                messages[0]["result"]["structuredContent"]["options"]["value"],
                42
            );
        });
    }

    #[test]
    fn result_mapper_can_replace_content_and_add_dynamic_meta() {
        futures::executor::block_on(async {
            let server = tool_server_with_options(
                CommandDef::build("mapped", EchoOptions)
                    .description("Mapped tool")
                    .done(),
                McpServeOptions {
                    tools: McpToolFilter {
                        discovery: McpDiscovery::Direct,
                        ..Default::default()
                    },
                    result_mapper: Some(McpResultMapper::new(|_| McpResultMapping {
                        is_error: Some(false),
                        structured_content: Some(json!({ "mapped": true })),
                        content: Some(Vec::new()),
                        meta: BTreeMap::from([("example/meta".to_string(), json!(true))]),
                    })),
                    ..Default::default()
                },
            );
            let response = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "mapped",
                        "method": "tools/call",
                        "params": { "name": "mapped", "arguments": { "value": 7 } }
                    }),
                    "2025-03-26",
                    "tools/call",
                    "mapped",
                ))
                .await;
            let messages = collect_event_stream(response).await;
            assert_eq!(
                messages[0]["result"]["content"],
                json!([]),
                "{:#?}",
                messages[0]
            );
            assert_eq!(messages[0]["result"]["structuredContent"]["mapped"], true);
            assert_eq!(messages[0]["result"]["_meta"]["example/meta"], true);
        });
    }

    #[test]
    fn modern_mrtr_input_required_replays_with_input_responses() {
        futures::executor::block_on(async {
            let server = tool_server(
                CommandDef::build("mrtr", NeedsInput)
                    .description("Needs input")
                    .done(),
            );
            let first = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "first",
                        "method": "tools/call",
                        "params": {
                            "name": "mrtr",
                            "arguments": {},
                            "_meta": {
                                META_PROTOCOL_VERSION: MODERN,
                                META_CLIENT_CAPABILITIES: {}
                            }
                        }
                    }),
                    MODERN,
                    "tools/call",
                    "mrtr",
                ))
                .await;
            let messages = collect_event_stream(first).await;
            assert_eq!(messages[0]["result"]["resultType"], "input_required");
            assert!(
                messages[0]["result"].get("content").is_none(),
                "intermediate MRTR results must not acquire a tool content field"
            );
            assert_eq!(messages[0]["result"]["requestState"], "state-1");
            assert_eq!(
                messages[0]["result"]["inputRequests"]["roots"]["method"],
                "roots/list"
            );

            let second = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "second",
                        "method": "tools/call",
                        "params": {
                            "name": "mrtr",
                            "arguments": {},
                            "inputResponses": { "roots": { "roots": [] } },
                            "requestState": "state-1",
                            "_meta": {
                                META_PROTOCOL_VERSION: MODERN,
                                META_CLIENT_CAPABILITIES: {}
                            }
                        }
                    }),
                    MODERN,
                    "tools/call",
                    "mrtr",
                ))
                .await;
            let messages = collect_event_stream(second).await;
            assert_eq!(messages[0]["result"]["resultType"], "complete");
            assert_eq!(
                messages[0]["result"]["structuredContent"]["responses"]["roots"]["roots"],
                json!([])
            );
            assert_eq!(
                messages[0]["result"]["structuredContent"]["state"],
                "state-1"
            );
        });
    }

    #[test]
    fn portable_peer_request_emits_literal_method_and_consumes_response() {
        futures::executor::block_on(async {
            let server = tool_server(
                CommandDef::build("peer", UsesPeer)
                    .description("Uses peer")
                    .done(),
            );
            let response = server
                .handle_for_peer(
                    post(
                        json!({
                            "jsonrpc": "2.0",
                            "id": "call",
                            "method": "tools/call",
                            "params": { "name": "peer", "arguments": {} }
                        }),
                        "2025-03-26",
                        "tools/call",
                        "peer",
                    ),
                    "local-peer-fixture",
                )
                .await;
            let McpHttpBody::EventStream(mut stream) = response.body else {
                panic!("event stream response")
            };
            let event = stream.next().await.expect("peer request event");
            let payload = event
                .strip_prefix("data: ")
                .and_then(|event| event.strip_suffix("\n\n"))
                .expect("SSE data event");
            let peer_request: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(peer_request["method"], "sampling/createMessage");
            let response = server
                .handle_for_peer(
                    post(
                        json!({
                            "jsonrpc": "2.0",
                            "id": peer_request["id"].clone(),
                            "result": { "accepted": true }
                        }),
                        "2025-03-26",
                        "tools/call",
                        "peer",
                    ),
                    "local-peer-fixture",
                )
                .await;
            assert_eq!(response.status, 202);
            let event = stream.next().await.expect("tool result event");
            let payload = event
                .strip_prefix("data: ")
                .and_then(|event| event.strip_suffix("\n\n"))
                .expect("SSE data event");
            let result: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(result["result"]["structuredContent"]["accepted"], true);
        });
    }

    #[test]
    fn dropping_peer_request_stream_clears_pending_response() {
        futures::executor::block_on(async {
            let server = tool_server(
                CommandDef::build("peer", UsesPeer)
                    .description("Uses peer")
                    .done(),
            );
            let response = server
                .handle_for_peer(
                    post(
                        json!({
                            "jsonrpc": "2.0",
                            "id": "call",
                            "method": "tools/call",
                            "params": { "name": "peer", "arguments": {} }
                        }),
                        "2025-03-26",
                        "tools/call",
                        "peer",
                    ),
                    "local-peer-fixture",
                )
                .await;
            let McpHttpBody::EventStream(mut stream) = response.body else {
                panic!("event stream response")
            };
            let event = stream.next().await.expect("peer request event");
            let payload = event
                .strip_prefix("data: ")
                .and_then(|event| event.strip_suffix("\n\n"))
                .expect("SSE data event");
            let peer_request: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(peer_request["method"], "sampling/createMessage");
            assert_eq!(server.pending_peer.lock().unwrap().len(), 1);

            drop(stream);

            assert_eq!(server.pending_peer.lock().unwrap().len(), 0);
        });
    }
    #[test]
    fn release_progressive_direct_names_cannot_shadow_discovery() {
        for name in [
            "search_tools",
            "get_tool_details",
            "call_read_tool",
            "call_write_tool",
        ] {
            let cli = Cli::create("collision").command(
                "host",
                CommandDef::build("host", EchoOptions)
                    .mcp(McpCommandOptions {
                        name: Some(name.to_string()),
                        direct: true,
                        ..Default::default()
                    })
                    .done(),
            );
            let error = match McpHttpServer::from_cli(&cli, McpHttpConfig::default()) {
                Err(error) => error,
                Ok(_) => panic!("reserved tool accepted: {name}"),
            };
            assert!(
                error
                    .to_string()
                    .contains("reserved for progressive discovery"),
                "{error}"
            );
        }
    }

    #[test]
    fn release_peer_responses_require_trusted_session_ownership() {
        futures::executor::block_on(async {
            let server = tool_server(CommandDef::build("peer", UsesPeer).done());
            let response = server
                .handle_for_peer(
                    post(
                        json!({
                            "jsonrpc": "2.0", "id": "victim", "method": "tools/call",
                            "params": { "name": "peer", "arguments": {} }
                        }),
                        "2025-03-26",
                        "tools/call",
                        "peer",
                    ),
                    "verified-victim-session",
                )
                .await;
            let McpHttpBody::EventStream(mut stream) = response.body else {
                panic!("peer stream");
            };
            let event = stream.next().await.unwrap();
            let request: Value = serde_json::from_str(
                event
                    .strip_prefix("data: ")
                    .unwrap()
                    .strip_suffix("\n\n")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(request["method"], "sampling/createMessage");
            let fake_response = json!({ "jsonrpc": "2.0", "id": request["id"], "result": { "accepted": "attacker" } });
            let _ = server
                .handle_for_peer(
                    post(fake_response.clone(), "2025-03-26", "tools/call", "peer"),
                    "verified-attacker-session",
                )
                .await;
            assert_eq!(
                server.pending_peer.lock().unwrap().len(),
                1,
                "cross-session response consumed victim request"
            );
            let _ = server
                .handle(post(fake_response, "2025-03-26", "tools/call", "peer"))
                .await;
            assert_eq!(
                server.pending_peer.lock().unwrap().len(),
                1,
                "anonymous response consumed victim request"
            );
            let _ = server
                .handle_for_peer(
                    post(
                        json!({
                            "jsonrpc": "2.0", "id": request["id"], "result": { "accepted": "owner" }
                        }),
                        "2025-03-26",
                        "tools/call",
                        "peer",
                    ),
                    "verified-victim-session",
                )
                .await;
            let event = stream.next().await.unwrap();
            let result: Value = serde_json::from_str(
                event
                    .strip_prefix("data: ")
                    .unwrap()
                    .strip_suffix("\n\n")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(result["result"]["structuredContent"]["accepted"], "owner");
            assert!(server.pending_peer.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn modern_subscription_listener_advertises_and_streams_events() {
        futures::executor::block_on(async {
            let server = tool_server_with_options(
                CommandDef::build("echo", EchoOptions).done(),
                McpServeOptions {
                    resources: McpResourceRegistry {
                        listen: Some(McpSubscriptionListenHandler::new(|request| async move {
                            assert_eq!(request.resource_uris, vec!["memory://one".to_string()]);
                            assert_eq!(request.cursor.as_deref(), Some("opaque-1"));
                            assert_eq!(request.notifications["resources"], true);
                            Ok(futures::stream::iter([json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/resources/updated",
                                "params": {
                                    "cursor": "opaque-2",
                                    "resourceUris": ["memory://one"],
                                    "kind": "updated"
                                }
                            })])
                            .boxed())
                        })),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            let discover = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "discover",
                        "method": "server/discover",
                        "params": {
                            "_meta": {
                                META_PROTOCOL_VERSION: MODERN,
                                META_CLIENT_CAPABILITIES: {}
                            }
                        }
                    }),
                    MODERN,
                    "server/discover",
                    "",
                ))
                .await;
            let messages = collect_event_stream(discover).await;
            assert_eq!(messages[0]["result"]["capabilities"]["subscriptions"]["listen"], true);

            let response = server
                .handle(post(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "listen",
                        "method": "subscriptions/listen",
                        "params": {
                            "notifications": { "resources": true },
                            "resourceUris": ["memory://one"],
                            "cursor": "opaque-1"
                        }
                    }),
                    MODERN,
                    "subscriptions/listen",
                    "",
                ))
                .await;
            let messages = collect_event_stream(response).await;
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["method"], "notifications/resources/updated");
            assert_eq!(messages[0]["params"]["cursor"], "opaque-2");
        });
    }

    #[test]
    fn release_anonymous_peer_requests_require_explicit_host_scope() {
        futures::executor::block_on(async {
            let server = tool_server(CommandDef::build("peer", UsesPeer).done());
            let (sender, _receiver) = mpsc::unbounded();
            let error = server
                .connected_peer(sender, None)
                .request(McpPeerRequest {
                    method: "sampling/createMessage".to_string(),
                    params: Some(json!({})),
                    meta: None,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, "MCP_PEER_SESSION_REQUIRED");
            assert!(server.pending_peer.lock().unwrap().is_empty());
        });
    }
}
