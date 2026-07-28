# Cloudflare Code Mode extension

This standalone Cargo workspace adapts the platform-neutral
`incurs-codemode` runtime to Cloudflare Workers. Nothing in the root Cargo
workspace depends on it.

The extension contains:

- `crates/incurs-codemode-cloudflare`: WorkerLoader execution, a Workers clock,
  Durable Object SQLite persistence, and a validated stateless Streamable HTTP
  MCP adapter.
- `examples/codemode-worker`: an executable lifecycle example with execute,
  approval, rejection, rollback, pending-action, expiry, and MCP routes.

The MCP adapter supports protocol revisions `2025-03-26`, `2025-06-18`, and
`2025-11-25`. Its host must provide an explicit origin allowlist and an
authentication boundary before exposing it remotely. The example disables
remote access until `MCP_AUTH_TOKEN` is configured.

Run its validation from this directory:

```sh
cargo test --workspace --all-features
cargo check --workspace --target wasm32-unknown-unknown
```

Build and exercise the example from `examples/codemode-worker`:

```sh
worker-build --release
npx wrangler dev
```

Local Worker Loader support requires Wrangler 4.114.0 or newer.
