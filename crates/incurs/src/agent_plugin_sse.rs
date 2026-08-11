//! Legacy HTTP+SSE transport for Agent Plugins MCP servers.

use std::future::Future;

use futures::StreamExt;
use rmcp::RoleClient;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

/// Failure while connecting to or sending through a legacy MCP SSE server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LegacySseError {
    /// The HTTP request failed.
    #[error("legacy MCP SSE request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The SSE stream was invalid.
    #[error("legacy MCP SSE stream failed: {0}")]
    Sse(#[from] sse_stream::Error),
    /// The endpoint event contained an invalid URL.
    #[error("legacy MCP SSE endpoint is invalid: {0}")]
    Url(#[from] url::ParseError),
    /// A JSON-RPC message could not be encoded.
    #[error("legacy MCP SSE message is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The server violated the legacy transport contract.
    #[error("legacy MCP SSE protocol error: {0}")]
    Protocol(&'static str),
}

/// rmcp transport for the MCP 2024-11-05 HTTP+SSE protocol.
pub(crate) struct LegacySseTransport {
    client: reqwest::Client,
    endpoint: url::Url,
    headers: http::HeaderMap,
    incoming: tokio::sync::mpsc::Receiver<RxJsonRpcMessage<RoleClient>>,
    reader: tokio::task::JoinHandle<()>,
}

impl LegacySseTransport {
    /// Connects to the configured SSE endpoint and waits for its POST endpoint event.
    pub(crate) async fn connect(
        configured: url::Url,
        headers: http::HeaderMap,
    ) -> Result<Self, LegacySseError> {
        let headers = client_safe_headers(headers);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let response = client
            .get(configured.clone())
            .headers(headers.clone())
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await?
            .error_for_status()?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(LegacySseError::Protocol(
                "SSE endpoint did not return text/event-stream",
            ));
        }

        let mut events = Box::pin(sse_stream::SseStream::from_bytes_stream(
            response.bytes_stream(),
        ));
        let endpoint = loop {
            let event = events.next().await.ok_or(LegacySseError::Protocol(
                "SSE endpoint closed before its endpoint event",
            ))??;
            if event.event.as_deref() != Some("endpoint") {
                continue;
            }
            let value = event
                .data
                .ok_or(LegacySseError::Protocol("SSE endpoint event has no data"))?;
            break resolve_endpoint(&configured, &value)?;
        };
        let headers = if same_origin(&configured, &endpoint) {
            headers
        } else {
            http::HeaderMap::new()
        };
        let (sender, incoming) = tokio::sync::mpsc::channel(32);
        let reader = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let Ok(event) = event else {
                    break;
                };
                if event.event.as_deref().is_some_and(|name| name != "message") {
                    continue;
                }
                let Some(data) = event.data else {
                    continue;
                };
                let Ok(message) = serde_json::from_str(&data) else {
                    break;
                };
                if sender.send(message).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            client,
            endpoint,
            headers,
            incoming,
            reader,
        })
    }
}

impl Transport<RoleClient> for LegacySseTransport {
    type Error = LegacySseError;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let headers = self.headers.clone();
        async move {
            let body = serde_json::to_vec(&item)?;
            client
                .post(endpoint)
                .headers(headers)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        }
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        self.incoming.recv()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.reader.abort();
        async { Ok(()) }
    }
}

fn resolve_endpoint(configured: &url::Url, value: &str) -> Result<url::Url, LegacySseError> {
    let endpoint = configured.join(value)?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(LegacySseError::Protocol(
            "endpoint event must contain an HTTP or HTTPS URL",
        ));
    }
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(LegacySseError::Protocol(
            "endpoint event URL must not contain user information or a fragment",
        ));
    }
    if endpoint.scheme() == "http" && !is_loopback(&endpoint) {
        return Err(LegacySseError::Protocol(
            "non-loopback endpoint event URLs must use HTTPS",
        ));
    }
    Ok(endpoint)
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    }
}

fn same_origin(configured: &url::Url, endpoint: &url::Url) -> bool {
    configured.scheme() == endpoint.scheme()
        && configured.host_str() == endpoint.host_str()
        && configured.port_or_known_default() == endpoint.port_or_known_default()
}

fn client_safe_headers(mut headers: http::HeaderMap) -> http::HeaderMap {
    headers.remove(http::header::ACCEPT);
    headers.remove(http::header::CONTENT_TYPE);
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_headers_follow_only_same_origin_endpoint_events() {
        let configured = url::Url::parse("https://example.com/mcp/sse").unwrap();

        assert!(same_origin(
            &configured,
            &url::Url::parse("https://example.com/mcp/messages").unwrap()
        ));
        assert!(!same_origin(
            &configured,
            &url::Url::parse("https://other.example/mcp/messages").unwrap()
        ));
        assert!(!same_origin(
            &configured,
            &url::Url::parse("http://example.com/mcp/messages").unwrap()
        ));
    }

    #[test]
    fn endpoint_events_resolve_relative_urls_and_reject_unsafe_urls() {
        let configured = url::Url::parse("http://localhost:3000/mcp/sse").unwrap();

        assert_eq!(
            resolve_endpoint(&configured, "../messages")
                .unwrap()
                .as_str(),
            "http://localhost:3000/messages"
        );
        assert!(resolve_endpoint(&configured, "file:///tmp/messages").is_err());
        assert!(resolve_endpoint(&configured, "http://example.com/messages").is_err());
        assert!(resolve_endpoint(&configured, "https://user@example.com/messages").is_err());
        assert!(resolve_endpoint(&configured, "https://example.com/messages#secret").is_err());
    }

    #[test]
    fn configured_headers_cannot_override_client_protocol_headers() {
        let headers = http::HeaderMap::from_iter([
            (
                http::header::ACCEPT,
                http::HeaderValue::from_static("text/plain"),
            ),
            (
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/plain"),
            ),
            (
                http::HeaderName::from_static("x-plugin"),
                http::HeaderValue::from_static("visible"),
            ),
        ]);

        let headers = client_safe_headers(headers);

        assert!(!headers.contains_key(http::header::ACCEPT));
        assert!(!headers.contains_key(http::header::CONTENT_TYPE));
        assert_eq!(headers["x-plugin"], "visible");
    }
}
