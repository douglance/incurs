# incurs-codemode-cloudflare

This crate supplies Cloudflare-specific execution, storage, lifecycle, and
Streamable HTTP adapters for the provider-neutral `incurs-codemode` runtime.

Use the workspace example for a complete Worker, Durable Object, authentication,
Origin allowlist, and Dynamic Worker configuration:
[`../../examples/codemode-worker`](../../examples/codemode-worker).

The remote endpoint fails closed until `MCP_AUTH_TOKEN` is configured. Set
`MCP_ALLOWED_ORIGINS` to a comma-separated list of additional browser origins.
