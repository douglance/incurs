# Migrating from incurs 0.2 to 0.3

Version 0.3 makes TypeScript incur 0.4.17 behavior the default contract and adds typed command authoring without removing the lower-level command API.

## Package versions

Update all incurs workspace crates together:

```toml
[dependencies]
incurs = "0.3"
incurs-extras = "0.3" # only when using Rust-only formats
```

The `incurs`, `incurs-macros`, `incurs-cli`, and `incurs-extras` packages share the 0.3 release line.

## Prefer typed commands

Existing `CommandDef` values and `CommandHandler` implementations continue to work. New commands should use `CommandDef::typed` with `incurs::Args`, `incurs::Options`, or `incurs::Env` derives and a `JsonSchema + Serialize` output type.

```rust
let command = CommandDef::typed::<Args, Options, Env, Output, _, _>(
    "name",
    |ctx: TypedContext<Args, Options, Env>| async move {
        TypedResult::ok(Output::from(ctx))
    },
)
.done();
```

The compiler now checks that the handler returns the declared output type. The same generated schemas drive CLI, HTTP, MCP, OpenAPI, skills, and code generation.

## Use one runtime path

Custom launchers and tests should call `Cli::run_to(argv, writer, runtime)` when they need injected display name, environment, or human/agent mode. Existing buffered tests may continue using `serve_to`; process applications may continue using `serve` or `serve_with`.

## Opt into table and CSV

`--table`, `--csv`, and `--format table|csv` are no longer part of the parity-default CLI. If a Rust application intentionally relies on these formats, add `incurs-extras` and explicitly configure or render `ExtraFormat::Table` or `ExtraFormat::Csv`.

```rust
use incurs_extras::{CliExtras, ExtraFormat};

let cli = cli.default_extra_format(ExtraFormat::Csv);
```

This is the only expected user-visible default-surface change in 0.3.

## Generate interoperable artifacts

`incurs gen` replaces the former CLI placeholder:

```bash
incurs gen --dir . --entry my-cli --config-schema
```

Check generated Rust and JSON artifacts into source control only if that matches the consuming repository's policy. Generation is deterministic so CI can regenerate and diff them.

## Validate the migration

```bash
cargo test --workspace --all-features
cargo xtask parity
```

If you maintain an integration over HTTP or MCP, also smoke-test its actual transport. Both now resolve through the same command graph and MCP uses `rmcp` 2.2 with protocol 2025-11-25 support.
