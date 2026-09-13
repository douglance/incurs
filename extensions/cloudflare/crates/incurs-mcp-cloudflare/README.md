# incurs-mcp-cloudflare

`incurs-mcp-cloudflare` exposes any [`incurs::tool::ToolCatalog`] as a
stateless Streamable HTTP MCP endpoint suitable for a Cloudflare Worker.

The crate deliberately owns no Worker routing, authentication, or storage.
The Worker converts its request into `McpHttpRequest`, applies its own auth,
calls `handle_mcp_request`, and converts the returned `McpHttpResponse` back
into a Worker response.

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

Tool annotations use MCP camelCase hints and omit unspecified values. Output
schemas and structured results follow the shared `incurs-mcp-protocol::structured`
projection: object outputs remain direct; other outputs use a marked `data`
envelope. JSON text content retains the original value. Incurs clients restore
the original contract when importing these tools; ordinary MCP clients consume
the advertised object schema.

## Transport requirements

A request must be a `POST` with `Content-Type: application/json` and an `Accept`
containing both `application/json` and `text/event-stream`. Bodies are capped by
`McpHttpOptions::max_body_bytes` (1 MiB by default).

The host must supply an explicit origin allowlist and an authentication boundary
before exposing this remotely. `McpHttpOptions::allowed_origins` is empty by
default, which rejects any request carrying an `Origin` header.
