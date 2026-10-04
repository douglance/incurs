//! Legacy HTTP+SSE transport for Agent Plugins MCP servers.
//!
//! This is the native `rmcp` transport. Its HTTP exchanges go through an
//! [`crate::outbound::HttpClient`]. wasm32 builds connect through
//! [`crate::mcp_client::McpHttpClient::connect_legacy_sse`], which applies
//! the same endpoint rules.
#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;

use futures::StreamExt;
use rmcp::RoleClient;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

use crate::mcp_client::{
    SseParser, client_safe_headers, header_pairs, resolve_endpoint, same_origin,
};
use crate::outbound::{HttpBody, HttpClientError, HttpRequest, SharedHttpClient};

/// Failure while connecting to or sending through a legacy MCP SSE server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LegacySseError {
    /// The HTTP request failed.
    #[error("legacy MCP SSE request failed: {0}")]
    Http(#[from] HttpClientError),
    /// The server answered with a non-success status.
    #[error("legacy MCP SSE request failed: HTTP status {0}")]
    Status(u16),
    /// The endpoint event contained an invalid or unsafe URL, or the event
    /// stream was invalid.
    #[error("legacy MCP SSE endpoint is invalid: {0}")]
    Endpoint(#[from] crate::mcp_client::McpClientError),
    /// A JSON-RPC message could not be encoded.
    #[error("legacy MCP SSE message is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The server violated the legacy transport contract.
    #[error("legacy MCP SSE protocol error: {0}")]
    Protocol(&'static str),
}

/// rmcp transport for the MCP 2024-11-05 HTTP+SSE protocol.
pub(crate) struct LegacySseTransport {
    client: SharedHttpClient,
    endpoint: url::Url,
    headers: http::HeaderMap,
    incoming: tokio::sync::mpsc::Receiver<RxJsonRpcMessage<RoleClient>>,
    reader: tokio::task::JoinHandle<()>,
}

/// Reads the next complete event from an SSE body.
async fn next_event(
    body: &mut HttpBody,
    parser: &mut SseParser,
) -> Result<Option<crate::mcp_client::SseEvent>, LegacySseError> {
    loop {
        if let Some(event) = parser.next_event() {
            return Ok(Some(event));
        }
        match body.next().await {
            // More bytes for the parser.
            Some(Ok(bytes)) => parser.push(&bytes)?,
            // The body failed mid-stream.
            Some(Err(error)) => return Err(error.into()),
            // The body ended.
            None => return Ok(None),
        }
    }
}

impl LegacySseTransport {
    /// Connects to the configured SSE endpoint through `client` and waits for
    /// its POST endpoint event.
    pub(crate) async fn connect(
        configured: url::Url,
        headers: http::HeaderMap,
        client: SharedHttpClient,
    ) -> Result<Self, LegacySseError> {
        let headers = client_safe_headers(headers);
        let mut request_headers = headers.clone();
        request_headers.append(
            http::header::ACCEPT,
            http::HeaderValue::from_static("text/event-stream"),
        );
        let response = client
            .send(HttpRequest {
                method: "GET".to_string(),
                url: configured.to_string(),
                headers: header_pairs(&request_headers),
                body: None,
            })
            .await?;
        if !response.is_success() {
            return Err(LegacySseError::Status(response.status));
        }
        let content_type = response.header("content-type").unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(LegacySseError::Protocol(
                "SSE endpoint did not return text/event-stream",
            ));
        }

        let mut body = response.body;
        let mut parser = SseParser::default();
        let endpoint = loop {
            let event =
                next_event(&mut body, &mut parser)
                    .await?
                    .ok_or(LegacySseError::Protocol(
                        "SSE endpoint closed before its endpoint event",
                    ))?;
            if event.event.as_deref() != Some("endpoint") {
                continue;
            }
            break resolve_endpoint(&configured, event.data.trim())?;
        };
        let headers = if same_origin(&configured, &endpoint) {
            headers
        } else {
            http::HeaderMap::new()
        };
        let (sender, incoming) = tokio::sync::mpsc::channel(32);
        let reader = tokio::spawn(async move {
            while let Ok(Some(event)) = next_event(&mut body, &mut parser).await {
                if event.event.as_deref().is_some_and(|name| name != "message") {
                    continue;
                }
                let Ok(message) = serde_json::from_str(&event.data) else {
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
            let mut headers = headers;
            headers.append(
                http::header::ACCEPT,
                http::HeaderValue::from_static("application/json"),
            );
            headers.append(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            let response = client
                .send(HttpRequest {
                    method: "POST".to_string(),
                    url: endpoint.to_string(),
                    headers: header_pairs(&headers),
                    body: Some(body),
                })
                .await?;
            if !response.is_success() {
                return Err(LegacySseError::Status(response.status));
            }
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
