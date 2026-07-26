# Cloudflare Code Mode extension

This standalone Cargo workspace adapts the platform-neutral
`incurs-codemode` runtime to Cloudflare Workers. Nothing in the root Cargo
workspace depends on it.

The extension contains:

- `crates/incurs-codemode-cloudflare`: WorkerLoader execution, a Workers clock,
  and Durable Object SQLite persistence.
- `examples/codemode-worker`: an executable lifecycle example with execute,
  approval, rejection, rollback, pending-action, and expiry routes.

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
