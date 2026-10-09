# OpenAI MCP extensions for incurs

This workspace implements the OpenAI MCP extension contracts in Rust. The independently pinned reference is [openai/mcp-extensions at 7e1be49](https://github.com/openai/mcp-extensions/tree/7e1be49daea03d7ec46ed2472f410099db2743d6).

The crates have separate responsibilities:

| Crate | Responsibility |
| --- | --- |
| `incurs-openai-mcp-protocol` | Typed extension payloads, schema normalization, and form validation |
| `incurs-openai-mcp` | Server settings, mentions, elicitation, metadata, and Incurs integration |
| `incurs-openai-mcp-app` | OpenAI app helpers for files, messages, context, resources, and deep links |
| `incurs-mcp-apps` in the root workspace | Provider-neutral app lifecycle and browser JSON-RPC transport |

```text
┌──────────────────────┐
│ OpenAI server helpers│
└──────────┬───────────┘
           │ metadata and validated requests
           ▼
┌──────────────────────┐
│ Incurs ToolCatalog   │
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐
│ MCP transports       │
└──────────────────────┘

┌──────────────────────┐
│ OpenAI app helpers   │
└──────────┬───────────┘
           │ extension requests and notifications
           ▼
┌──────────────────────┐
│ Neutral app runtime  │
└──────────┬───────────┘
           │ browser postMessage
           ▼
┌──────────────────────┐
│ MCP app host         │
└──────────────────────┘
```

Run the reference and Rust checks from the repository root:

```sh
npm ci --ignore-scripts --prefix extensions/openai-mcp/parity
python3 extensions/openai-mcp/parity/check.py
python3 extensions/openai-mcp/parity/differential.py
python3 extensions/openai-mcp/parity/form-differential.py
python3 extensions/openai-mcp/parity/helper-differential.py
(cd extensions/openai-mcp/parity && node --import tsx server-oracle.mjs && node --import tsx app-oracle.mjs)
python3 extensions/openai-mcp/crates/incurs-openai-mcp/tests/native_stdio_probe.py
cargo test --manifest-path extensions/openai-mcp/Cargo.toml --workspace --locked
cargo check --manifest-path extensions/openai-mcp/Cargo.toml --workspace --target wasm32-unknown-unknown --locked
```

The oracle checks the copied upstream source hashes and exported surface. The differential corpus compares independently computed upstream acceptance and normalized values against the Rust implementation, with a positive control for every schema. Server behavior controls exercise registration, strict settings updates, mentions replacement, and form elicitation.

The browser harness exercises actual compiled WASM in Chromium against an independent JavaScript parent-window host. The host checks literal method names and plain JSON-RPC objects, resource notification delivery, numeric write results, cancellation, timeouts, disposal, and source filtering. It is a local MCP host fixture; it does not certify availability in every ChatGPT or Codex host. Capabilities negotiated by the connected host determine which app helpers are available.

The app browser check requires Node, the Rust wasm32 target, and wasm-bindgen CLI 0.2.126:

```sh
node extensions/openai-mcp/crates/incurs-openai-mcp-app/examples/browser-smoke-gate.mjs
```

The gate installs its pinned npm dependencies in a disposable directory. Set `CHROME_PATH` to an installed Chrome executable to use it; otherwise it installs Chromium's headless shell. Linux runners also need Playwright's browser system dependencies.

The server runtime check runs the actual facade through `Cli`, `ToolCatalog`, and `McpHttpServer` in WASM. Its `WASM_BINDGEN` executable must match the wasm-bindgen version resolved in this workspace's lockfile:

```sh
cargo build --manifest-path extensions/openai-mcp/Cargo.toml -p incurs-openai-mcp --example openai_mcp_wasm_runtime --target wasm32-unknown-unknown --no-default-features --locked
node extensions/openai-mcp/crates/incurs-openai-mcp/tests/wasm_runtime_probe.mjs
```

Legacy MCP connections send OpenAI form requests through the connected peer. Modern MCP uses `server/discover` and input-required results with validated input-response replay. Both require the OpenAI form capability. Settings validate declared scalar constraints before calling persistence handlers; storage and authorization remain host responsibilities.

The reference source remains under `parity/upstream` with its original Apache-2.0 license. Runtime Rust packages do not execute that reference source. The reference revision and exported surface are pinned so updates can be compared against executable controls.
