//! Host-supplied outbound HTTP.
//!
//! incurs sends HTTP requests from three places: the portable MCP client
//! ([`crate::mcp_client`]), Agent Plugin MCP servers, and OpenAPI documents
//! loaded by URL. Every one of those requests is a single [`HttpClient`]
//! exchange, so the host decides how requests leave the process.
//!
//! Native builds default to [`ReqwestHttpClient`] when no client is supplied.
//! wasm32 builds have no default: a request made without a client fails with
//! the coded `HTTP_CLIENT_REQUIRED` error, whose hint names the option that
//! takes one. A Cloudflare Worker supplies `WorkersHttpClient` from the
//! `incurs-mcp-cloudflare` crate.
//!
//! Futures and body streams are `Send`. A wasm32 host whose fetch values are
//! not `Send` wraps them, which is sound on a single-threaded host.
//!
//! Buffered body reads stop at a size cap ([`DEFAULT_MAX_RESPONSE_BYTES`]
//! unless the caller passes its own) and fail with the coded
//! `HTTP_BODY_TOO_LARGE` error. A client may also supply a timer through
//! [`HttpClient::sleep`]; incurs uses it to put deadlines on requests, since
//! wasm32 has no async runtime of its own to read a clock from.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};

/// One outbound HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP method, such as `GET` or `POST`.
    pub method: String,
    /// Absolute request URL.
    pub url: String,
    /// Request headers as `(name, value)` pairs, in send order. A name may
    /// repeat.
    pub headers: Vec<(String, String)>,
    /// Request body, when the request has one.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// A request with no headers and no body.
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Appends one header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the body.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// Largest response body [`HttpResponse::bytes`] and [`HttpResponse::text`]
/// buffer: 16 MiB. A longer body fails with `HTTP_BODY_TOO_LARGE`; use
/// [`HttpResponse::bytes_limited`] for a different cap.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// A future that completes once a delay has elapsed, returned by
/// [`HttpClient::sleep`].
pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A response body, read incrementally as byte chunks.
pub type HttpBody = Pin<Box<dyn Stream<Item = Result<Vec<u8>, HttpClientError>> + Send>>;

/// One HTTP response. The body has not been read yet.
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers as `(name, value)` pairs. A name may repeat.
    pub headers: Vec<(String, String)>,
    /// The body as a stream of byte chunks.
    pub body: HttpBody,
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    /// A response whose body is `body`, delivered as one chunk.
    pub fn from_bytes(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body: Box::pin(futures::stream::once(async move { Ok(body) })),
        }
    }

    /// The first value of the named header, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the status is in the 2xx range.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Reads the whole body, up to [`DEFAULT_MAX_RESPONSE_BYTES`].
    pub async fn bytes(self) -> Result<Vec<u8>, HttpClientError> {
        self.bytes_limited(DEFAULT_MAX_RESPONSE_BYTES).await
    }

    /// Reads the whole body as text, up to [`DEFAULT_MAX_RESPONSE_BYTES`],
    /// replacing invalid UTF-8.
    pub async fn text(self) -> Result<String, HttpClientError> {
        self.text_limited(DEFAULT_MAX_RESPONSE_BYTES).await
    }

    /// Reads the whole body, failing with [`HttpClientError::BodyTooLarge`]
    /// as soon as it would exceed `limit` bytes. Reading stops there; the
    /// rest of the body is never pulled from the stream.
    pub async fn bytes_limited(mut self, limit: usize) -> Result<Vec<u8>, HttpClientError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.body.next().await {
            let chunk = chunk?;
            if chunk.len() > limit - bytes.len() {
                return Err(HttpClientError::BodyTooLarge { limit });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Reads the whole body as text, like [`bytes_limited`](Self::bytes_limited),
    /// replacing invalid UTF-8.
    pub async fn text_limited(self, limit: usize) -> Result<String, HttpClientError> {
        let bytes = self.bytes_limited(limit).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Sends one HTTP request and returns its response.
///
/// Implementations do not interpret the status: a 4xx or 5xx response is a
/// successful exchange. An error means no response arrived, or the body
/// failed while it was read.
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    /// Performs one exchange.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError>;

    /// A future that completes after `duration`, from the host's timer.
    ///
    /// incurs races it against a request to enforce a deadline, such as
    /// `McpRemoteOptions::request_timeout`. The default returns `None`: the
    /// client has no timer, and requests through it wait without a deadline.
    fn sleep(&self, duration: Duration) -> Option<Sleep> {
        let _ = duration;
        None
    }
}

/// A shareable host HTTP client.
pub type SharedHttpClient = Arc<dyn HttpClient>;

/// Failure of one outbound HTTP exchange.
///
/// Every variant has a stable machine-readable [`code`](Self::code), carried
/// into [`crate::errors::IncurError`] by the `From` conversion.
#[derive(Debug)]
#[non_exhaustive]
pub enum HttpClientError {
    /// No client was supplied and this target has no default.
    Required {
        /// The option that takes a client, such as `McpRemoteOptions::http_client`.
        option: &'static str,
    },
    /// The request cannot be expressed, such as an invalid method or header.
    InvalidRequest(String),
    /// No response arrived, or the body failed while it was read.
    Transport {
        /// Description of the failure.
        message: String,
        /// The underlying failure, when there is one.
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// The response body exceeded the size cap, and reading stopped there.
    BodyTooLarge {
        /// The cap, in bytes.
        limit: usize,
    },
}

impl HttpClientError {
    /// The error for a request made without a client on a target with no
    /// default. `option` names where the caller supplies one.
    pub fn required(option: &'static str) -> Self {
        Self::Required { option }
    }

    /// A transport failure caused by `error`.
    pub fn transport(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Transport {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    /// A transport failure described only by `message`.
    pub fn transport_message(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
            source: None,
        }
    }

    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            // The caller must supply a client.
            Self::Required { .. } => "HTTP_CLIENT_REQUIRED",
            // The request itself is malformed.
            Self::InvalidRequest(_) => "HTTP_INVALID_REQUEST",
            // The network or host failed.
            Self::Transport { .. } => "HTTP_TRANSPORT_ERROR",
            // The peer sent more than the caller accepts.
            Self::BodyTooLarge { .. } => "HTTP_BODY_TOO_LARGE",
        }
    }

    /// Whether repeating the exchange may succeed.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }

    /// Actionable guidance, when there is any.
    pub fn hint(&self) -> Option<String> {
        match self {
            // Name the option that takes a client.
            Self::Required { option } => Some(format!(
                "Pass an incurs::outbound::HttpClient in {option}; this target has no default HTTP client"
            )),
            // Name the cap that stopped the read.
            Self::BodyTooLarge { limit } => Some(format!(
                "The response body exceeded {limit} bytes; raise the caller's body limit if the peer is trusted"
            )),
            // Other failures carry their detail in the message.
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // No client.
            Self::Required { option } => {
                write!(f, "an HTTP client is required on this target: set {option}")
            }
            // Malformed request.
            Self::InvalidRequest(message) => write!(f, "invalid HTTP request: {message}"),
            // Network failure, described exactly as the host reported it.
            Self::Transport { message, .. } => f.write_str(message),
            // Oversized body.
            Self::BodyTooLarge { limit } => {
                write!(f, "response body exceeds the {limit}-byte limit")
            }
        }
    }
}

impl std::error::Error for HttpClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // The host's own failure.
            Self::Transport {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            // Everything else is self-describing.
            _ => None,
        }
    }
}

impl From<HttpClientError> for crate::errors::Error {
    fn from(error: HttpClientError) -> Self {
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

/// Whether the default client follows redirects.
#[cfg(any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Redirects {
    /// Follow redirects with the default policy.
    #[cfg_attr(not(feature = "openapi"), allow(dead_code))]
    Follow,
    /// Return a redirect response as it is, so credentials are never replayed
    /// to another URL.
    #[cfg_attr(
        not(any(feature = "http", feature = "agent-plugins-mcp")),
        allow(dead_code)
    )]
    Refuse,
}

/// Returns the supplied client, or this target's default.
///
/// Native builds default to [`ReqwestHttpClient`] with the given redirect
/// policy. wasm32 builds have no default and return
/// [`HttpClientError::Required`] naming `option`.
#[cfg(any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"))]
pub(crate) fn resolve(
    client: Option<&SharedHttpClient>,
    option: &'static str,
    redirects: Redirects,
) -> Result<SharedHttpClient, HttpClientError> {
    match client {
        // The host chose a client.
        Some(client) => Ok(Arc::clone(client)),
        // Fall back to the target default.
        None => default_client(option, redirects),
    }
}

#[cfg(all(
    any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
fn default_client(
    _option: &'static str,
    redirects: Redirects,
) -> Result<SharedHttpClient, HttpClientError> {
    let client = match redirects {
        // The reqwest default policy.
        Redirects::Follow => ReqwestHttpClient::new()?,
        // No redirect is followed.
        Redirects::Refuse => ReqwestHttpClient::without_redirects()?,
    };
    Ok(Arc::new(client))
}

#[cfg(all(
    any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"),
    target_arch = "wasm32"
))]
fn default_client(
    option: &'static str,
    _redirects: Redirects,
) -> Result<SharedHttpClient, HttpClientError> {
    Err(HttpClientError::required(option))
}

#[cfg(all(
    any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
pub use reqwest_client::ReqwestHttpClient;

#[cfg(all(
    any(feature = "http", feature = "openapi", feature = "agent-plugins-mcp"),
    not(target_arch = "wasm32")
))]
mod reqwest_client {
    use futures::StreamExt;

    use super::{HttpClient, HttpClientError, HttpRequest, HttpResponse, Sleep};

    /// The native default [`HttpClient`], backed by `reqwest`.
    #[derive(Debug, Clone)]
    pub struct ReqwestHttpClient {
        client: reqwest::Client,
    }

    impl ReqwestHttpClient {
        /// A client with reqwest's default policy, which follows redirects.
        pub fn new() -> Result<Self, HttpClientError> {
            reqwest::Client::builder()
                .build()
                .map(Self::from_client)
                .map_err(HttpClientError::transport)
        }

        /// A client that follows no redirect, so configured credentials are
        /// never replayed to a redirect target.
        pub fn without_redirects() -> Result<Self, HttpClientError> {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map(Self::from_client)
                .map_err(HttpClientError::transport)
        }

        /// Wraps a configured `reqwest` client, such as one with a proxy.
        pub fn from_client(client: reqwest::Client) -> Self {
            Self { client }
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for ReqwestHttpClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
            let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
                HttpClientError::InvalidRequest(format!(
                    "`{}` is not a valid method",
                    request.method
                ))
            })?;
            let mut headers = reqwest::header::HeaderMap::with_capacity(request.headers.len());
            for (name, value) in &request.headers {
                let header =
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                        HttpClientError::InvalidRequest(format!(
                            "`{name}` is not a valid header name"
                        ))
                    })?;
                let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                    HttpClientError::InvalidRequest(format!(
                        "the value for header `{name}` is not valid"
                    ))
                })?;
                headers.append(header, value);
            }
            let mut builder = self.client.request(method, &request.url).headers(headers);
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder.send().await.map_err(HttpClientError::transport)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect();
            let body = response.bytes_stream().map(|chunk| {
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

        /// A tokio timer. reqwest already requires a tokio runtime.
        fn sleep(&self, duration: std::time::Duration) -> Option<Sleep> {
            Some(Box::pin(tokio::time::sleep(duration)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_client_maps_to_a_coded_error_naming_the_option() {
        let error =
            crate::errors::Error::from(HttpClientError::required("McpRemoteOptions::http_client"));
        let crate::errors::Error::Incur(error) = error else {
            panic!("expected a coded error");
        };
        assert_eq!(error.code, "HTTP_CLIENT_REQUIRED");
        assert!(!error.retryable);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("McpRemoteOptions::http_client")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn response_headers_are_matched_case_insensitively() {
        let response = HttpResponse::from_bytes(
            200,
            vec![("Content-Type".to_string(), "text/plain".to_string())],
            b"ok".to_vec(),
        );
        assert_eq!(response.header("content-type"), Some("text/plain"));
        assert!(response.is_success());
        let text = futures::executor::block_on(response.text()).unwrap();
        assert_eq!(text, "ok");
    }

    /// A body far longer than the cap stops there with a coded,
    /// non-retryable error, after pulling only the chunks that fit plus the
    /// one that overflowed. The stream is finite so that a missing cap fails
    /// the test instead of exhausting memory.
    #[test]
    fn a_long_body_stops_at_the_cap_with_a_coded_error() {
        let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&pulled);
        let endless = futures::stream::repeat_with(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![b'x'; 1024])
        })
        .take(1000);
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Box::pin(endless),
        };
        let error = futures::executor::block_on(response.bytes_limited(10 * 1024)).unwrap_err();
        assert_eq!(error.code(), "HTTP_BODY_TOO_LARGE");
        assert!(!error.retryable());
        assert_eq!(pulled.load(std::sync::atomic::Ordering::SeqCst), 11);
        let crate::errors::Error::Incur(error) = crate::errors::Error::from(error) else {
            panic!("expected a coded error");
        };
        assert_eq!(error.code, "HTTP_BODY_TOO_LARGE");
        let exact = HttpResponse::from_bytes(200, Vec::new(), vec![b'y'; 8]);
        assert_eq!(
            futures::executor::block_on(exact.bytes_limited(8)).unwrap(),
            vec![b'y'; 8]
        );
    }
}
