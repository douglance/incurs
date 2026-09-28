//! Outbound HTTP for incurs through the Workers `fetch` API.

use futures::StreamExt;
use incurs::outbound::{HttpClient, HttpClientError, HttpRequest, HttpResponse};
use worker::send::{SendFuture, SendWrapper};

/// An [`HttpClient`] that sends every request with [`worker::Fetch`] and
/// streams the response body.
///
/// incurs has no default HTTP client on wasm32. Pass this client wherever
/// incurs makes outbound requests from a Worker:
/// `McpRemoteOptions::http_client`, `AgentPluginRuntimeOptions::http_client`,
/// and `OpenApiSource::Url`.
///
/// Workers run on one thread, so the `fetch` futures and body streams, which
/// hold JavaScript values, are wrapped to satisfy the contract's `Send` bound.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkersHttpClient {
    redirect: worker::RequestRedirect,
}

impl WorkersHttpClient {
    /// A client that follows redirects, the `fetch` default.
    pub fn new() -> Self {
        Self::default()
    }

    /// A client that returns redirect responses as they are, so configured
    /// credentials are never replayed to a redirect target.
    pub fn manual_redirects() -> Self {
        Self {
            redirect: worker::RequestRedirect::Manual,
        }
    }
}

fn transport(error: worker::Error) -> HttpClientError {
    HttpClientError::transport_message(error.to_string())
}

async fn exchange(
    redirect: worker::RequestRedirect,
    request: HttpRequest,
) -> Result<HttpResponse, HttpClientError> {
    let headers = worker::Headers::new();
    for (name, value) in &request.headers {
        headers.append(name, value).map_err(|_| {
            HttpClientError::InvalidRequest(format!("header `{name}` cannot be sent"))
        })?;
    }
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::from(request.method))
        .with_headers(headers)
        .with_redirect(redirect);
    if let Some(body) = request.body {
        init.with_body(Some(
            worker::js_sys::Uint8Array::from(body.as_slice()).into(),
        ));
    }
    let outbound = worker::Request::new_with_init(&request.url, &init)
        .map_err(|error| HttpClientError::InvalidRequest(error.to_string()))?;
    let mut response = worker::Fetch::Request(outbound)
        .send()
        .await
        .map_err(transport)?;
    let status = response.status_code();
    let headers = response.headers().entries().collect();
    let body = match response.stream() {
        // A fetched body is a JavaScript stream, read chunk by chunk.
        Ok(stream) => {
            let chunks =
                futures::stream::unfold(SendWrapper::new(stream), |mut stream| async move {
                    let chunk = SendFuture::new(stream.next()).await?;
                    Some((chunk.map_err(transport), stream))
                });
            Box::pin(chunks) as incurs::outbound::HttpBody
        }
        // A body that is not a stream, including no body, is read at once.
        Err(_) => {
            let bytes = response.bytes().await.unwrap_or_default();
            Box::pin(futures::stream::once(async move { Ok(bytes) }))
        }
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

#[async_trait::async_trait]
impl HttpClient for WorkersHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        SendFuture::new(exchange(self.redirect, request)).await
    }
}
