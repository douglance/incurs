# Migrating from incurs 0.4 to 0.5

Version 0.5 adds exact multi-era Model Context Protocol (MCP) negotiation,
standard `--` end-of-options parsing, and explicit process exit codes for
successful commands. It also updates the MCP transport integration from
`rmcp` 2.2 to the pinned `rmcp` 3.0.0 beta used by this release.

## Prerequisites

incurs 0.5 requires Rust 1.88 or newer. Update the toolchain before changing
dependency versions:

```bash
rustup update stable
rustc --version
```

The reported compiler version must be 1.88.0 or newer.

## Package versions

Update packages that share the core incurs API to their 0.5 release line:

```toml
[dependencies]
incurs = "0.5"
incurs-extras = "0.5" # only when using Rust-only formats
```

The derive crate remains on its compatible published version:

```toml
incurs-macros = "0.4"
```

Update Code Mode packages together because their public APIs exchange incurs
and Code Mode types:

```toml
[dependencies]
incurs-codemode = "0.2"
incurs-codemode-local = "0.2"
incurs-codemode-mcp = "0.2"
```

Cloudflare integrations use `incurs-codemode-cloudflare = "0.2"`. The new
`incurs-mcp-protocol = "0.1"` crate is available for applications that need
the provider-neutral standard registry, codecs, validation, or negotiation
policy directly.

## Update successful command results

`TypedResult::Ok` and the lower-level successful command result now include an
optional `exit_code`. Constructors remain the preferred API:

```rust
TypedResult::ok(output)
```

Use `TypedResult::ok_with_exit_code` when a command successfully wraps a
subprocess and must pass through its process status:

```rust
TypedResult::ok_with_exit_code(output, status.code().unwrap_or(1))
```

If an application constructs a successful result variant directly, add
`exit_code: None` to preserve the normal success status:

```rust
TypedResult::Ok {
    data: output,
    cta: None,
    exit_code: None,
}
```

The CLI process adapter reports an explicit successful-result exit code.
Buffered and embedded runtimes continue to return the structured result to the
caller.

## Preserve arguments after `--`

The parser now treats `--` as the end-of-options marker. Tokens after it are
positional or passthrough arguments even when they begin with `-`:

```console
my-cli run --image local -- printf --image --unknown
```

The command receives `printf`, `--image`, and `--unknown` as passthrough
arguments. Audit commands that previously treated `--` as an ordinary token.

## Update MCP integrations

The MCP feature now uses exact standard profiles for `2024-11-05`,
`2025-03-26`, `2025-06-18`, `2025-11-25`, and `2026-07-28`.
`incurs-mcp-protocol` owns the profile registry, lifecycle families, wire-era
codecs, validation, and negotiation policy.

Applications that expose `rmcp` types through their own public APIs must update
for `rmcp` 3.0.0-beta.5. In particular, tool handlers now return the rmcp 3
response types, and servers can declare exact supported protocol versions.

`incurs-codemode-mcp::CodeModeMcpServer::new` enables all supported standards.
Use `CodeModeMcpServer::with_standards` with an `McpStandardSet` when a server
must restrict its advertised versions:

```rust
use incurs_codemode_mcp::CodeModeMcpServer;
use incurs_mcp_protocol::McpStandardSet;

let server = CodeModeMcpServer::with_standards(
    service,
    McpStandardSet::legacy_only(),
);
```

Modern clients use discovery. Legacy clients continue to initialize with an
exact standard. Authentication, transport, and server failures do not trigger
protocol fallback.

## Validate the migration

Run the checks that match the features used by the application:

```bash
cargo check --all-features
cargo test --all-features
cargo doc --all-features --no-deps
```

For an HTTP or MCP integration, also test its real transport against every
protocol version that it advertises.
