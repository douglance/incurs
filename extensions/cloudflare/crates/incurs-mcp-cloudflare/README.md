# incurs-mcp-cloudflare

`incurs-mcp-cloudflare` exposes any [`incurs::tool::ToolCatalog`] as a
stateless Streamable HTTP MCP endpoint suitable for a Cloudflare Worker.

The crate deliberately owns no Worker routing, authentication, or storage.
The Worker converts its request into `McpHttpRequest`, applies its own auth,
calls `handle_mcp_request`, and converts the returned `McpHttpResponse` back
into a Worker response.

## Outbound HTTP

incurs sends outbound HTTP through `incurs::outbound::HttpClient`. Native
builds default to a `reqwest` client; wasm32 builds have no default, and a
call made without a client fails with the coded `HTTP_CLIENT_REQUIRED` error.

`WorkersHttpClient` (wasm32 only) implements that contract with
`worker::Fetch` and streams each response body, so Streamable HTTP and legacy
SSE responses are read incrementally. Pass it wherever incurs calls out:

```rust,ignore
use std::sync::Arc;

use incurs::agent_plugin_runtime::AgentPluginRuntimeOptions;
use incurs::mcp::McpRemoteOptions;
use incurs::openapi::OpenApiSource;
use incurs_mcp_cloudflare::WorkersHttpClient;

let client: incurs::outbound::SharedHttpClient = Arc::new(WorkersHttpClient::new());
let remote = McpRemoteOptions { http_client: Some(client.clone()), ..Default::default() };
let plugins = AgentPluginRuntimeOptions { http_client: Some(client.clone()), ..Default::default() };
let spec = OpenApiSource::Url { url: "https://example.com/openapi.json".into(), client };
```

`WorkersHttpClient::new()` follows redirects, the `fetch` default.
`WorkersHttpClient::manual_redirects()` returns redirect responses unchanged,
so configured credentials are never replayed to a redirect target.

Using this client keeps `reqwest` out of the Worker, which matters: `reqwest`
and `worker` link different `wasm-streams` versions that export the same
symbols, and the Worker fails to link when both are present.

## Protocol coverage

This adapter implements the **legacy MCP lifecycle only**: `initialize`, `ping`,
`tools/list`, and `tools/call`. `supported_versions()` returns exactly the
revisions it serves, newest first, and `MCP_PROTOCOL_VERSION` is the newest of
them.

It does not advertise the modern family. That family replaces `initialize` with
stateless `server/discover`, requires per-request client metadata, adds a result
type discriminator, and uses a different wire codec — none of which this
transport implements. A client asking for a modern revision during `initialize`
is answered with the newest legacy revision instead, and a modern
`MCP-Protocol-Version` header on a later request is refused with `-32022` rather
than being served under rules the handler does not honour.

Implementing the modern family here is real work, not a version bump.

## Transport requirements

A request must be a `POST` with `Content-Type: application/json` and an `Accept`
containing both `application/json` and `text/event-stream`. Bodies are capped by
`McpHttpOptions::max_body_bytes` (1 MiB by default).

The host must supply an explicit origin allowlist and an authentication boundary
before exposing this remotely. `McpHttpOptions::allowed_origins` is empty by
default, which rejects any request carrying an `Origin` header.
