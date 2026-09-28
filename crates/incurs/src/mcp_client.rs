//! Portable MCP client over HTTP.
//!
//! [`McpHttpClient`] speaks MCP to a remote server through one
//! [`crate::outbound::HttpClient`] and `futures`. It spawns no task and reads
//! no clock, thread, or async runtime, so it runs unchanged on
//! `wasm32-unknown-unknown` hosts such as a Cloudflare Worker, where the host
//! supplies the client in [`McpRemoteOptions::http_client`]. Native builds
//! default to [`crate::outbound::ReqwestHttpClient`], which follows no
//! redirect.
//!
//! Two transports are supported:
//!
//! - Streamable HTTP ([`McpHttpClient::connect`]) for every official standard.
//!   Legacy standards use `initialize` and `notifications/initialized`, echo
//!   an `Mcp-Session-Id` when the server issues one, and send
//!   `MCP-Protocol-Version` on every later request. The 2026-07-28 standard
//!   uses `server/discover`, per-request `_meta` from the modern wire codec,
//!   and the `Mcp-Method` and `Mcp-Name` headers. A response body may be
//!   `application/json` or `text/event-stream`.
//! - Legacy HTTP+SSE ([`McpHttpClient::connect_legacy_sse`]): a long-lived GET
//!   event stream announces a POST endpoint, and responses to POSTed messages
//!   arrive on that stream. The stream is read in place while a request waits,
//!   so no background reader is needed.
//!
//! Standard selection goes through [`incurs_mcp_protocol::negotiate`]. A client
//! that enables 2026-07-28 probes with `server/discover` and falls back to a
//! legacy `initialize` only on protocol evidence, the peer answering
//! `-32601 Method not found`. An authentication, server, or transport failure
//! fails closed with a coded [`McpClientError`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use http::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use incurs_mcp_protocol::{
    McpClientMetadata, McpLifecycleFamily, McpNegotiationDecision, McpNegotiationError,
    McpNegotiationEvidence, McpStandardSet, McpVersion, McpWireCodecKind,
};
use serde_json::{Map, Value};

use crate::mcp::McpRemoteOptions;
use crate::outbound::{
    HttpBody, HttpClientError, HttpRequest, HttpResponse, Redirects, SharedHttpClient,
};

const HEADER_SESSION_ID: &str = "mcp-session-id";
const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const HEADER_METHOD: &str = "mcp-method";
const HEADER_NAME: &str = "mcp-name";
const HEADER_PARAM_PREFIX: &str = "mcp-param-";
const EVENT_STREAM: &str = "text/event-stream";
const JSON: &str = "application/json";
const METHOD_NOT_FOUND: i64 = -32601;
/// Largest SSE event accepted before the stream is treated as hostile.
const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure while connecting to or calling a remote MCP server.
///
/// Every variant has a stable machine-readable [`code`](Self::code), carried
/// into [`crate::errors::IncurError`] by the `From` conversion.
#[derive(Debug)]
#[non_exhaustive]
pub enum McpClientError {
    /// A configured header name or value is not valid, or is reserved by the
    /// transport.
    InvalidHeader(String),
    /// The server URL is not an absolute HTTP or HTTPS URL.
    InvalidUrl(String),
    /// The HTTP exchange failed before a response arrived, or no HTTP client
    /// is available on this target.
    Transport(HttpClientError),
    /// The server rejected the credentials with HTTP 401 or 403.
    Unauthorized {
        /// HTTP status code.
        status: u16,
        /// The `WWW-Authenticate` challenge, when the server sent one.
        www_authenticate: Option<String>,
    },
    /// The server discarded the session this client was using (HTTP 404).
    SessionExpired,
    /// The server answered with an unexpected HTTP status.
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body, for diagnosis.
        body: String,
    },
    /// The server answered with a JSON-RPC error.
    JsonRpc {
        /// JSON-RPC error code.
        code: i64,
        /// JSON-RPC error message.
        message: String,
        /// JSON-RPC error data, when present.
        data: Option<Value>,
    },
    /// The server violated the transport or protocol contract.
    Protocol(String),
    /// Client and server share no enabled standard.
    Negotiation(McpNegotiationError),
}

impl McpClientError {
    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            // A header supplied by the caller cannot be sent.
            Self::InvalidHeader(_) => "INVALID_HEADER",
            // The configured URL cannot be used.
            Self::InvalidUrl(_) => "MCP_INVALID_URL",
            // The request never produced a response.
            Self::Transport(HttpClientError::Transport { .. }) => "MCP_TRANSPORT_ERROR",
            // No client, or a request the client cannot express, keeps its code.
            Self::Transport(error) => error.code(),
            // Credentials were rejected.
            Self::Unauthorized { .. } => "MCP_UNAUTHORIZED",
            // The session is gone.
            Self::SessionExpired => "MCP_SESSION_EXPIRED",
            // An unexpected HTTP status.
            Self::Http { .. } => "MCP_HTTP_ERROR",
            // A JSON-RPC error response.
            Self::JsonRpc { .. } => "MCP_JSONRPC_ERROR",
            // A malformed or contradictory response.
            Self::Protocol(_) => "MCP_PROTOCOL_ERROR",
            // No common standard.
            Self::Negotiation(_) => "MCP_NEGOTIATION_FAILED",
        }
    }

    /// Whether repeating the operation may succeed.
    pub fn retryable(&self) -> bool {
        match self {
            // Network failures are transient; a missing client is not.
            Self::Transport(error) => error.retryable(),
            // Lost sessions are transient.
            Self::SessionExpired => true,
            // Server-side failures may be transient; client errors are not.
            Self::Http { status, .. } => *status >= 500,
            // Everything else needs a configuration or server change.
            _ => false,
        }
    }

    fn hint(&self) -> Option<String> {
        match self {
            // Point at the credential inputs.
            Self::Unauthorized { .. } => {
                Some("Check the bearer token or the configured Authorization header".to_string())
            }
            // Point at the standard set.
            Self::Negotiation(_) => Some(
                "Enable an MCP standard the server supports in McpRemoteOptions::standards"
                    .to_string(),
            ),
            // Point at the option that takes a client.
            Self::Transport(error) => error.hint(),
            // Other failures carry their detail in the message.
            _ => None,
        }
    }
}

impl std::fmt::Display for McpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Caller configuration.
            Self::InvalidHeader(message) | Self::InvalidUrl(message) => f.write_str(message),
            // Network failure.
            Self::Transport(error) => write!(f, "MCP request failed: {error}"),
            // Credentials rejected.
            Self::Unauthorized { status, .. } => {
                write!(f, "MCP server rejected the credentials with HTTP {status}")
            }
            // Session lost.
            Self::SessionExpired => f.write_str("MCP server discarded the session"),
            // Unexpected status.
            Self::Http { status, body } => write!(f, "HTTP {status}: {body}"),
            // Same text a JSON-RPC error has through the native rmcp client.
            Self::JsonRpc {
                code,
                message,
                data,
            } => {
                write!(f, "Mcp error: {code}: {message}")?;
                if let Some(data) = data {
                    write!(f, "({data})")?;
                }
                Ok(())
            }
            // Contract violation.
            Self::Protocol(message) => write!(f, "MCP protocol error: {message}"),
            // No common standard.
            Self::Negotiation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for McpClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // The underlying HTTP failure.
            Self::Transport(error) => Some(error),
            // The underlying negotiation failure.
            Self::Negotiation(error) => Some(error),
            // Everything else is self-describing.
            _ => None,
        }
    }
}

impl From<McpClientError> for crate::errors::Error {
    fn from(error: McpClientError) -> Self {
        crate::errors::Error::Incur(crate::errors::IncurError {
            message: error.to_string(),
            code: error.code().to_string(),
            hint: error.hint(),
            retryable: error.retryable(),
            exit_code: None,
            cause: Some(Box::new(error)),
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP exchange
// ---------------------------------------------------------------------------

/// The client every exchange goes through: the supplied one, or the native
/// default that follows no redirect, so configured credentials are never
/// replayed to a redirect target.
fn http_client(options: &McpRemoteOptions) -> Result<SharedHttpClient, McpClientError> {
    crate::outbound::resolve(
        options.http_client.as_ref(),
        "McpRemoteOptions::http_client",
        Redirects::Refuse,
    )
    .map_err(McpClientError::Transport)
}

/// Converts a composed header map into the contract's `(name, value)` pairs.
pub(crate) fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// Sends one request through `http`.
async fn send(
    http: &SharedHttpClient,
    method: &str,
    url: &url::Url,
    headers: &HeaderMap,
    body: Option<Vec<u8>>,
) -> Result<HttpResponse, McpClientError> {
    let request = HttpRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers: header_pairs(headers),
        body,
    };
    http.send(request).await.map_err(McpClientError::Transport)
}

// ---------------------------------------------------------------------------
// Server-sent events
// ---------------------------------------------------------------------------

/// One dispatched server-sent event.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// Event type, when the event named one.
    pub(crate) event: Option<String>,
    /// Data lines joined with `\n`.
    pub(crate) data: String,
}

/// Incremental `text/event-stream` parser.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// Appends received bytes.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), McpClientError> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_EVENT_BYTES {
            return Err(McpClientError::Protocol(
                "server-sent event exceeds the size limit".to_string(),
            ));
        }
        Ok(())
    }

    /// Returns the next complete event, if the buffer holds one.
    pub(crate) fn next_event(&mut self) -> Option<SseEvent> {
        loop {
            let end = self
                .buffer
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\r')?;
            let mut consumed = end + 1;
            if self.buffer[end] == b'\r' {
                match self.buffer.get(end + 1) {
                    // CRLF line ending.
                    Some(b'\n') => consumed += 1,
                    // A lone CR may still be followed by LF in the next chunk.
                    None => return None,
                    // A lone CR line ending.
                    Some(_) => {}
                }
            }
            let line = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
            self.buffer.drain(..consumed);
            if line.is_empty() {
                if self.data.is_empty() {
                    self.event = None;
                    continue;
                }
                let event = SseEvent {
                    event: self.event.take(),
                    data: self.data.join("\n"),
                };
                self.data.clear();
                return Some(event);
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                // `field: value`, with one optional leading space removed.
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                // A bare field name has an empty value.
                None => (line.as_str(), ""),
            };
            match field {
                // Event type for the pending event.
                "event" => self.event = Some(value.to_string()),
                // One data line.
                "data" => self.data.push(value.to_string()),
                // `id`, `retry`, and unknown fields do not affect MCP messages.
                _ => {}
            }
        }
    }
}

/// A response body read as server-sent events.
struct EventReader {
    stream: HttpBody,
    parser: SseParser,
}

impl EventReader {
    fn new(response: HttpResponse) -> Self {
        Self {
            stream: response.body,
            parser: SseParser::default(),
        }
    }

    async fn next(&mut self) -> Result<Option<SseEvent>, McpClientError> {
        loop {
            if let Some(event) = self.parser.next_event() {
                return Ok(Some(event));
            }
            match self.stream.next().await {
                // More bytes for the parser.
                Some(Ok(bytes)) => self.parser.push(&bytes)?,
                // The body failed mid-stream.
                Some(Err(error)) => return Err(McpClientError::Transport(error)),
                // The body ended; an unterminated event is discarded.
                None => return Ok(None),
            }
        }
    }

    /// Reads events until the JSON-RPC response for `id` arrives, skipping
    /// notifications, server requests, and non-message events.
    async fn response(&mut self, id: u64) -> Result<Value, McpClientError> {
        loop {
            let Some(event) = self.next().await? else {
                return Err(McpClientError::Protocol(
                    "event stream ended before the response arrived".to_string(),
                ));
            };
            if event.event.as_deref().is_some_and(|name| name != "message")
                || event.data.trim().is_empty()
            {
                continue;
            }
            let message: Value = serde_json::from_str(&event.data).map_err(|error| {
                McpClientError::Protocol(format!("event data is not JSON: {error}"))
            })?;
            if let Some(outcome) = find_response(&message, id) {
                return outcome;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC
// ---------------------------------------------------------------------------

fn id_matches(value: Option<&Value>, id: u64) -> bool {
    match value {
        // Numeric ids are echoed as numbers.
        Some(Value::Number(number)) => number.as_u64() == Some(id),
        // Some servers stringify numeric ids.
        Some(Value::String(text)) => *text == id.to_string(),
        // Anything else is not this request's response.
        _ => false,
    }
}

fn jsonrpc_error(error: &Value) -> McpClientError {
    McpClientError::JsonRpc {
        code: error
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        data: error.get("data").cloned(),
    }
}

/// Finds the response for `id` in one message or a batch.
fn find_response(message: &Value, id: u64) -> Option<Result<Value, McpClientError>> {
    if let Some(batch) = message.as_array() {
        return batch.iter().find_map(|message| find_response(message, id));
    }
    if message.get("method").is_some() || !id_matches(message.get("id"), id) {
        return None;
    }
    if let Some(error) = message.get("error") {
        return Some(Err(jsonrpc_error(error)));
    }
    Some(match message.get("result") {
        // A successful response.
        Some(result) => Ok(result.clone()),
        // Neither result nor error.
        None => Err(McpClientError::Protocol(
            "response has neither result nor error".to_string(),
        )),
    })
}

/// Reply to one POSTed message.
struct Reply {
    result: Option<Value>,
    session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

struct Streamable {
    http: SharedHttpClient,
    url: url::Url,
    headers: HeaderMap,
    session_id: Option<HeaderValue>,
}

impl Streamable {
    async fn exchange(
        &self,
        message: &Value,
        id: Option<u64>,
        protocol_headers: HeaderMap,
    ) -> Result<Reply, McpClientError> {
        let body = serde_json::to_vec(message)
            .map_err(|error| McpClientError::Protocol(error.to_string()))?;
        let mut headers = self.headers.clone();
        headers.extend(protocol_headers);
        headers.append(
            ACCEPT,
            HeaderValue::from_static("text/event-stream, application/json"),
        );
        headers.append(CONTENT_TYPE, HeaderValue::from_static(JSON));
        if let Some(session_id) = &self.session_id {
            headers.append(HEADER_SESSION_ID, session_id.clone());
        }
        let response = send(&self.http, "POST", &self.url, &headers, Some(body)).await?;
        let status = response.status;
        rejected_credentials(&response)?;
        if matches!(status, 202 | 204) {
            return accepted(id, None);
        }
        if status == 404 && self.session_id.is_some() {
            return Err(McpClientError::SessionExpired);
        }
        let content_type = header_text(&response, CONTENT_TYPE.as_str());
        let session_id = header_text(&response, HEADER_SESSION_ID);
        let is_json = content_type
            .as_deref()
            .is_some_and(|value| value.starts_with(JSON));
        if !response.is_success() {
            let body = response.text().await.unwrap_or_default();
            if is_json
                && let Ok(message) = serde_json::from_str::<Value>(&body)
                && let Some(error) = message.get("error")
            {
                return Err(jsonrpc_error(error));
            }
            return Err(McpClientError::Http { status, body });
        }
        if content_type
            .as_deref()
            .is_some_and(|value| value.starts_with(EVENT_STREAM))
        {
            let Some(id) = id else {
                return Ok(Reply {
                    result: None,
                    session_id,
                });
            };
            let result = EventReader::new(response).response(id).await?;
            return Ok(Reply {
                result: Some(result),
                session_id,
            });
        }
        if !is_json {
            return Err(McpClientError::Protocol(format!(
                "unexpected response content type {}",
                content_type.as_deref().unwrap_or("<none>")
            )));
        }
        let bytes = response.bytes().await.map_err(McpClientError::Transport)?;
        let (Some(id), Ok(message)) = (id, serde_json::from_slice::<Value>(&bytes)) else {
            // A notification's body, or an unparseable body, is an acceptance.
            return accepted(id, session_id);
        };
        match find_response(&message, id) {
            // The matching response.
            Some(outcome) => outcome.map(|result| Reply {
                result: Some(result),
                session_id,
            }),
            // An error that could not be attributed to a request id.
            None if message.get("id").is_none_or(Value::is_null) => Err(message
                .get("error")
                .map(jsonrpc_error)
                .unwrap_or_else(|| McpClientError::Protocol("response has no id".to_string()))),
            // Some other request's response.
            None => Err(McpClientError::Protocol(
                "response id does not match the request".to_string(),
            )),
        }
    }
}

fn accepted(id: Option<u64>, session_id: Option<String>) -> Result<Reply, McpClientError> {
    if id.is_some() {
        return Err(McpClientError::Protocol(
            "server accepted a request without answering it".to_string(),
        ));
    }
    Ok(Reply {
        result: None,
        session_id,
    })
}

fn rejected_credentials(response: &HttpResponse) -> Result<(), McpClientError> {
    let status = response.status;
    if matches!(status, 401 | 403) {
        return Err(McpClientError::Unauthorized {
            status,
            www_authenticate: header_text(response, "www-authenticate"),
        });
    }
    Ok(())
}

fn header_text(response: &HttpResponse, name: &str) -> Option<String> {
    response.header(name).map(ToString::to_string)
}

struct LegacySse {
    http: SharedHttpClient,
    endpoint: url::Url,
    headers: HeaderMap,
    events: futures::lock::Mutex<EventReader>,
}

impl LegacySse {
    async fn exchange(
        &self,
        message: &Value,
        id: Option<u64>,
        protocol_headers: HeaderMap,
    ) -> Result<Reply, McpClientError> {
        // Holding the stream across the POST keeps one request's response from
        // being consumed by another.
        let mut events = self.events.lock().await;
        let body = serde_json::to_vec(message)
            .map_err(|error| McpClientError::Protocol(error.to_string()))?;
        let mut headers = self.headers.clone();
        headers.extend(protocol_headers);
        headers.append(ACCEPT, HeaderValue::from_static(JSON));
        headers.append(CONTENT_TYPE, HeaderValue::from_static(JSON));
        let response = send(&self.http, "POST", &self.endpoint, &headers, Some(body)).await?;
        rejected_credentials(&response)?;
        if !response.is_success() {
            return Err(McpClientError::Http {
                status: response.status,
                body: response.text().await.unwrap_or_default(),
            });
        }
        drop(response);
        let result = match id {
            // Requests are answered on the event stream.
            Some(id) => Some(events.response(id).await?),
            // Notifications are not answered.
            None => None,
        };
        Ok(Reply {
            result,
            session_id: None,
        })
    }
}

enum Wire {
    Streamable(Streamable),
    LegacySse(LegacySse),
}

/// Resolves a legacy SSE `endpoint` event against the configured URL, and
/// rejects endpoints that could leak data: non-HTTP schemes, user
/// information, fragments, and plain HTTP off loopback.
pub(crate) fn resolve_endpoint(
    configured: &url::Url,
    value: &str,
) -> Result<url::Url, McpClientError> {
    let endpoint = configured
        .join(value)
        .map_err(|error| McpClientError::Protocol(format!("endpoint event is invalid: {error}")))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(McpClientError::Protocol(
            "endpoint event must contain an HTTP or HTTPS URL".to_string(),
        ));
    }
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(McpClientError::Protocol(
            "endpoint event URL must not contain user information or a fragment".to_string(),
        ));
    }
    if endpoint.scheme() == "http" && !is_loopback(&endpoint) {
        return Err(McpClientError::Protocol(
            "non-loopback endpoint event URLs must use HTTPS".to_string(),
        ));
    }
    Ok(endpoint)
}

fn is_loopback(url: &url::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Whether an endpoint shares the configured URL's origin, so configured
/// headers may follow it.
pub(crate) fn same_origin(configured: &url::Url, endpoint: &url::Url) -> bool {
    configured.scheme() == endpoint.scheme()
        && configured.host_str() == endpoint.host_str()
        && configured.port_or_known_default() == endpoint.port_or_known_default()
}

/// Removes headers the legacy SSE transport sets itself.
pub(crate) fn client_safe_headers(mut headers: HeaderMap) -> HeaderMap {
    headers.remove(ACCEPT);
    headers.remove(CONTENT_TYPE);
    headers
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A connected MCP client that needs no async runtime.
///
/// Connect with [`connect`](Self::connect) for Streamable HTTP or
/// [`connect_legacy_sse`](Self::connect_legacy_sse) for the 2024-11-05
/// HTTP+SSE transport, then list and call tools. Project the tools as incurs
/// commands with [`crate::mcp::remote_commands_from_client`].
///
/// The client is `Send` and `Sync`, and its futures are `Send` on every
/// target: every exchange goes through a `Send` [`crate::outbound::HttpClient`].
pub struct McpHttpClient {
    wire: Wire,
    standard: McpVersion,
    modern: bool,
    metadata: McpClientMetadata,
    next_id: AtomicU64,
    param_headers: std::sync::Mutex<HashMap<String, Vec<(String, String)>>>,
}

impl std::fmt::Debug for McpHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHttpClient")
            .field("standard", &self.standard)
            .field("session_id", &self.session_id())
            .finish_non_exhaustive()
    }
}

impl McpHttpClient {
    /// Connects to a Streamable HTTP MCP endpoint and negotiates a standard
    /// from `options.standards`.
    ///
    /// Sends `options.auth_token` as a bearer token and `options.headers` on
    /// every request. `Accept`, `Content-Type`, and `Mcp-Session-Id` are set
    /// by the transport and are rejected as configured headers. Requests go
    /// through `options.http_client`; on wasm32, where there is no default,
    /// a missing client fails with `HTTP_CLIENT_REQUIRED`.
    pub async fn connect(url: &str, options: &McpRemoteOptions) -> Result<Self, McpClientError> {
        let url = parse_url(url)?;
        let mut headers = configured_headers(options)?;
        for reserved in [
            ACCEPT,
            CONTENT_TYPE,
            HeaderName::from_static(HEADER_SESSION_ID),
        ] {
            if headers.contains_key(&reserved) {
                return Err(McpClientError::InvalidHeader(format!(
                    "`{reserved}` is set by the MCP transport and cannot be configured"
                )));
            }
        }
        headers.remove(HEADER_PROTOCOL_VERSION);
        let wire = Wire::Streamable(Streamable {
            http: http_client(options)?,
            url,
            headers,
            session_id: None,
        });
        Self::negotiate(wire, &options.standards).await
    }

    /// Connects to a legacy HTTP+SSE MCP endpoint: opens the event stream,
    /// waits for its `endpoint` event, and negotiates a standard from
    /// `options.standards` over it.
    ///
    /// Configured headers follow the POST endpoint only when it shares the
    /// event stream's origin.
    pub async fn connect_legacy_sse(
        url: &str,
        options: &McpRemoteOptions,
    ) -> Result<Self, McpClientError> {
        let configured = parse_url(url)?;
        let headers = client_safe_headers(configured_headers(options)?);
        let http = http_client(options)?;
        let mut request_headers = headers.clone();
        request_headers.append(ACCEPT, HeaderValue::from_static(EVENT_STREAM));
        let response = send(&http, "GET", &configured, &request_headers, None).await?;
        rejected_credentials(&response)?;
        if !response.is_success() {
            return Err(McpClientError::Http {
                status: response.status,
                body: response.text().await.unwrap_or_default(),
            });
        }
        let content_type = header_text(&response, CONTENT_TYPE.as_str());
        if !content_type
            .as_deref()
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case(EVENT_STREAM))
        {
            return Err(McpClientError::Protocol(
                "SSE endpoint did not return text/event-stream".to_string(),
            ));
        }
        let mut events = EventReader::new(response);
        let endpoint = loop {
            let Some(event) = events.next().await? else {
                return Err(McpClientError::Protocol(
                    "SSE endpoint closed before its endpoint event".to_string(),
                ));
            };
            if event.event.as_deref() == Some("endpoint") {
                break resolve_endpoint(&configured, event.data.trim())?;
            }
        };
        let headers = if same_origin(&configured, &endpoint) {
            headers
        } else {
            HeaderMap::new()
        };
        let wire = Wire::LegacySse(LegacySse {
            http,
            endpoint,
            headers,
            events: futures::lock::Mutex::new(events),
        });
        Self::negotiate(wire, &options.standards).await
    }

    /// The exact standard negotiated with the server.
    pub fn standard(&self) -> &McpVersion {
        &self.standard
    }

    /// The `Mcp-Session-Id` the server issued, when it issued one.
    pub fn session_id(&self) -> Option<&str> {
        match &self.wire {
            // Only legacy Streamable HTTP sessions carry an id.
            Wire::Streamable(wire) => wire
                .session_id
                .as_ref()
                .and_then(|value| value.to_str().ok()),
            // The legacy SSE transport identifies the session by its stream.
            Wire::LegacySse(_) => None,
        }
    }

    /// Sends one JSON-RPC request and returns its `result`.
    ///
    /// A JSON-RPC error response is returned as [`McpClientError::JsonRpc`].
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = self.envelope(method, Some(params), Some(id));
        let headers = self.protocol_headers(&message, true);
        let reply = self.exchange(&message, Some(id), headers).await?;
        let result = reply
            .result
            .ok_or_else(|| McpClientError::Protocol("request received no response".to_string()))?;
        if self.modern
            && let Some(kind) = result.get("resultType").and_then(Value::as_str)
            && kind != "complete"
        {
            return Err(McpClientError::Protocol(format!(
                "result type `{kind}` is not supported by this client"
            )));
        }
        Ok(result)
    }

    /// Lists every tool, following `nextCursor` pagination, as raw MCP tool
    /// objects.
    ///
    /// Under 2026-07-28, a tool with invalid `x-mcp-header` annotations is
    /// dropped, and valid annotations are remembered so later calls send the
    /// matching `Mcp-Param-*` headers.
    pub async fn list_tools(&self) -> Result<Vec<Value>, McpClientError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                // Follow-up page.
                Some(cursor) => serde_json::json!({ "cursor": cursor }),
                // First page.
                None => Value::Object(Map::new()),
            };
            let result = self.request("tools/list", params).await?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    McpClientError::Protocol("tools/list result has no tools array".to_string())
                })?;
            for tool in page {
                if self.modern {
                    let Some(annotations) = param_header_annotations(tool) else {
                        continue;
                    };
                    if let (Some(name), Ok(mut cache)) = (
                        tool.get("name").and_then(Value::as_str),
                        self.param_headers.lock(),
                    ) {
                        cache.insert(name.to_string(), annotations);
                    }
                }
                tools.push(tool.clone());
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                // The server repeated a cursor; paging would never end.
                Some(next) if cursor.as_deref() == Some(next) => {
                    return Err(McpClientError::Protocol(
                        "tools/list returned a non-advancing cursor".to_string(),
                    ));
                }
                // Another page.
                Some(next) => cursor = Some(next.to_string()),
                // Last page.
                None => return Ok(tools),
            }
        }
    }

    /// Calls one tool and returns the raw MCP `CallToolResult` object.
    ///
    /// A tool failure is a successful call whose result has `isError: true`;
    /// an unknown tool or invalid request is [`McpClientError::JsonRpc`].
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Result<Value, McpClientError> {
        self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    async fn negotiate(wire: Wire, standards: &McpStandardSet) -> Result<Self, McpClientError> {
        let lifecycle = |version: &McpVersion| {
            incurs_mcp_protocol::standards::known_standard(version)
                .map(|standard| standard.lifecycle())
        };
        let versions = standards.versions();
        let first_modern = versions
            .iter()
            .find(|version| lifecycle(version) == Some(McpLifecycleFamily::Modern))
            .cloned();
        let mut client = Self {
            wire,
            standard: versions[0].clone(),
            modern: false,
            metadata: McpClientMetadata::new(
                "incurs",
                env!("CARGO_PKG_VERSION"),
                Value::Object(Map::new()),
            ),
            next_id: AtomicU64::new(0),
            param_headers: std::sync::Mutex::new(HashMap::new()),
        };
        let Some(mut candidate) = first_modern else {
            let legacy = versions[0].clone();
            client.initialize(legacy, standards).await?;
            return Ok(client);
        };
        let mut attempted: Vec<McpVersion> = Vec::new();
        loop {
            attempted.push(candidate.clone());
            client.standard = candidate.clone();
            client.modern = true;
            let outcome = client
                .request("server/discover", Value::Object(Map::new()))
                .await;
            let (evidence, failure) = match outcome {
                // The peer speaks the modern lifecycle.
                Ok(result) => (discovered(&result), None),
                // Classify the failure as negotiation evidence.
                Err(error) => (evidence_for(&error), Some(error)),
            };
            match incurs_mcp_protocol::negotiate(standards, &evidence) {
                // A mutually supported modern standard.
                Ok(McpNegotiationDecision::SelectModern(version)) => {
                    client.standard = version;
                    return Ok(client);
                }
                // Retry discovery once per standard the peer said it supports.
                Ok(McpNegotiationDecision::RetryModern(version)) => {
                    if attempted.iter().filter(|tried| **tried == version).count() >= 2 {
                        return Err(McpClientError::Negotiation(
                            McpNegotiationError::NoCommonStandard,
                        ));
                    }
                    candidate = version;
                }
                // Protocol evidence that the peer is legacy.
                Ok(McpNegotiationDecision::FallBackLegacy(version)) => {
                    client.modern = false;
                    client.initialize(version, standards).await?;
                    return Ok(client);
                }
                // Fail closed with the failure that produced the evidence.
                Err(McpNegotiationError::FailedClosed(reason)) => {
                    return Err(failure.unwrap_or_else(|| {
                        McpClientError::Protocol(format!("server/discover failed: {reason}"))
                    }));
                }
                // No common standard.
                Err(error) => return Err(McpClientError::Negotiation(error)),
            }
        }
    }

    async fn initialize(
        &mut self,
        version: McpVersion,
        standards: &McpStandardSet,
    ) -> Result<(), McpClientError> {
        self.modern = false;
        self.standard = version.clone();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = self.envelope(
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": version,
                "capabilities": self.metadata.capabilities,
                "clientInfo": {
                    "name": self.metadata.name,
                    "version": self.metadata.version,
                },
            })),
            Some(id),
        );
        let headers = self.protocol_headers(&message, false);
        let reply = self.exchange(&message, Some(id), headers).await?;
        let result = reply.result.unwrap_or(Value::Null);
        let negotiated = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(McpVersion::from)
            .ok_or_else(|| {
                McpClientError::Protocol("initialize result has no protocolVersion".to_string())
            })?;
        let legacy = incurs_mcp_protocol::standards::known_standard(&negotiated)
            .is_some_and(|standard| standard.lifecycle() == McpLifecycleFamily::Legacy);
        if !legacy || !standards.versions().contains(&negotiated) {
            return Err(McpClientError::Negotiation(
                McpNegotiationError::NoCommonStandard,
            ));
        }
        self.standard = negotiated;
        if let (Wire::Streamable(wire), Some(session_id)) = (&mut self.wire, reply.session_id) {
            wire.session_id = Some(HeaderValue::from_str(&session_id).map_err(|_| {
                McpClientError::Protocol("Mcp-Session-Id is not a valid header value".to_string())
            })?);
        }
        let notification = self.envelope("notifications/initialized", None, None);
        let headers = self.protocol_headers(&notification, true);
        self.exchange(&notification, None, headers).await?;
        Ok(())
    }

    async fn exchange(
        &self,
        message: &Value,
        id: Option<u64>,
        headers: HeaderMap,
    ) -> Result<Reply, McpClientError> {
        match &self.wire {
            // Streamable HTTP POST.
            Wire::Streamable(wire) => wire.exchange(message, id, headers).await,
            // Legacy POST answered on the event stream.
            Wire::LegacySse(wire) => wire.exchange(message, id, headers).await,
        }
    }

    fn envelope(&self, method: &str, params: Option<Value>, id: Option<u64>) -> Value {
        let mut message = Map::new();
        message.insert("jsonrpc".to_string(), Value::from("2.0"));
        if let Some(id) = id {
            message.insert("id".to_string(), Value::from(id));
        }
        message.insert("method".to_string(), Value::from(method));
        if let Some(params) = params {
            message.insert("params".to_string(), params);
        }
        let message = Value::Object(message);
        if self.modern && id.is_some() {
            incurs_mcp_protocol::codec(McpWireCodecKind::Modern).project_outbound_request(
                &self.standard,
                &self.metadata,
                message,
            )
        } else {
            message
        }
    }

    /// Headers that depend on the negotiated standard and the message.
    fn protocol_headers(&self, message: &Value, negotiated: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let mut push = |name: &str, value: &str| {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        };
        if negotiated {
            push(HEADER_PROTOCOL_VERSION, self.standard.as_str());
        }
        if !self.modern {
            return headers;
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return headers;
        };
        push(HEADER_METHOD, method);
        let params = message.get("params");
        let name_key = match method {
            // Named primitives.
            "tools/call" | "prompts/get" => Some("name"),
            // Resources are named by URI.
            "resources/read" | "resources/subscribe" | "resources/unsubscribe" => Some("uri"),
            // Tasks are named by id.
            "tasks/get" | "tasks/update" | "tasks/cancel" => Some("taskId"),
            // No name header.
            _ => None,
        };
        let name = name_key
            .and_then(|key| params.and_then(|params| params.get(key)))
            .and_then(Value::as_str);
        if let Some(name) = name {
            push(HEADER_NAME, &encode_header_value(name));
        }
        if method == "tools/call"
            && let (Some(name), Some(arguments)) =
                (name, params.and_then(|params| params.get("arguments")))
            && let Ok(cache) = self.param_headers.lock()
            && let Some(annotations) = cache.get(name)
        {
            for (property, header) in annotations {
                let value = match arguments.get(property) {
                    // Primitive arguments are promoted to headers.
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Bool(flag)) => flag.to_string(),
                    Some(Value::Number(number)) => number.to_string(),
                    // Absent or structured arguments are not.
                    _ => continue,
                };
                push(
                    &format!("{HEADER_PARAM_PREFIX}{header}"),
                    &encode_header_value(&value),
                );
            }
        }
        headers
    }
}

fn parse_url(url: &str) -> Result<url::Url, McpClientError> {
    let parsed = url::Url::parse(url).map_err(|error| {
        McpClientError::InvalidUrl(format!("`{url}` is not a valid URL: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(McpClientError::InvalidUrl(format!(
            "`{url}` is not an HTTP or HTTPS URL"
        )));
    }
    Ok(parsed)
}

fn configured_headers(options: &McpRemoteOptions) -> Result<HeaderMap, McpClientError> {
    let mut headers = HeaderMap::with_capacity(options.headers.len() + 1);
    for (name, value) in &options.headers {
        let header = HeaderName::try_from(name.as_str()).map_err(|_| {
            McpClientError::InvalidHeader(format!("`{name}` is not a valid header name"))
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            McpClientError::InvalidHeader(format!("the value for header `{name}` is not valid"))
        })?;
        headers.insert(header, value);
    }
    if let Some(token) = &options.auth_token {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            McpClientError::InvalidHeader(
                "the bearer token is not a valid header value".to_string(),
            )
        })?;
        value.set_sensitive(true);
        headers.insert(http::header::AUTHORIZATION, value);
    }
    Ok(headers)
}

fn discovered(result: &Value) -> McpNegotiationEvidence {
    let supported = result
        .get("supportedVersions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .filter_map(Value::as_str)
                .map(McpVersion::from)
                .collect::<Vec<_>>()
        });
    match supported {
        // A well-formed discovery result.
        Some(supported) => McpNegotiationEvidence::Discovered { supported },
        // A discovery result without supported versions.
        None => McpNegotiationEvidence::InvalidModernResponse,
    }
}

fn evidence_for(error: &McpClientError) -> McpNegotiationEvidence {
    match error {
        // The peer recognized discovery but not the requested standard.
        McpClientError::JsonRpc { code, data, .. }
            if *code == i64::from(incurs_mcp_protocol::standards::UNSUPPORTED_PROTOCOL_VERSION) =>
        {
            McpNegotiationEvidence::UnsupportedVersion {
                supported: data
                    .as_ref()
                    .and_then(|data| data.get("supported"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(McpVersion::from)
                    .collect(),
            }
        }
        // The peer does not know `server/discover`: it is a legacy server.
        McpClientError::JsonRpc { code, .. } if *code == METHOD_NOT_FOUND => {
            McpNegotiationEvidence::HttpBadRequestUnrecognized
        }
        // Credentials were rejected.
        McpClientError::Unauthorized { .. } => McpNegotiationEvidence::AuthenticationFailure,
        // The server failed.
        McpClientError::Http { status, .. } if *status >= 500 => {
            McpNegotiationEvidence::ServerFailure
        }
        // No response arrived.
        McpClientError::Transport(_) => McpNegotiationEvidence::TransportFailure,
        // Anything else is not evidence of a legacy peer.
        _ => McpNegotiationEvidence::InvalidModernResponse,
    }
}

/// Returns a tool's valid `x-mcp-header` annotations, or `None` when any
/// annotation is invalid and the tool must be dropped.
fn param_header_annotations(tool: &Value) -> Option<Vec<(String, String)>> {
    let Some(properties) = tool
        .pointer("/inputSchema/properties")
        .and_then(Value::as_object)
    else {
        return Some(Vec::new());
    };
    let mut annotations = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (property, schema) in properties {
        if has_nested_annotation(schema) {
            return None;
        }
        let Some(header) = schema.get("x-mcp-header") else {
            continue;
        };
        let header = header.as_str()?;
        let token = !header.is_empty()
            && header
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c));
        let primitive = matches!(
            schema.get("type").and_then(Value::as_str),
            Some("string" | "integer" | "boolean")
        );
        if !token || !primitive || !seen.insert(header.to_ascii_lowercase()) {
            return None;
        }
        annotations.push((property.clone(), header.to_string()));
    }
    Some(annotations)
}

fn has_nested_annotation(schema: &Value) -> bool {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|nested| {
            nested
                .values()
                .any(|value| value.get("x-mcp-header").is_some() || has_nested_annotation(value))
        })
}

/// Encodes a header value, wrapping it as `=?base64?...?=` when it cannot be
/// sent verbatim.
fn encode_header_value(value: &str) -> String {
    let needs_encoding = !value.is_empty()
        && (value.starts_with([' ', '\t'])
            || value.ends_with([' ', '\t'])
            || value.chars().any(|c| !(' '..='~').contains(&c))
            || (value.starts_with("=?base64?") && value.ends_with("?=")));
    if needs_encoding {
        format!("=?base64?{}?=", base64(value.as_bytes()))
    } else {
        value.to_string()
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(triple >> (18 - 6 * index)) as usize & 63],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
