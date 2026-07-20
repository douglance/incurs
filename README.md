# incurs

A Rust implementation of [wevm/incur](https://github.com/wevm/incur), the CLI framework for humans and agents.

Define a command once and expose the same validated behavior through CLI, HTTP, MCP, OpenAPI, skill files, and shell completions. The vendored TypeScript 0.4.17 implementation is the behavioral oracle; Rust-only extensions are opt-in.

## Status

Version 0.3.0 establishes an executable parity gate and a typed Rust authoring path.

| Surface | 0.3 status |
| --- | --- |
| CLI parsing, help, validation, aliases, output and streaming | Parity-gated |
| HTTP, nested routes, middleware and fetch gateways | Implemented and tested |
| MCP 2025-11-25, progressive/direct discovery and calls | Implemented with `rmcp` 2.2 |
| OpenAPI, skills and shell completions | Generated from the shared command graph |
| Typed args, options, env and output | `CommandDef::typed` plus derive macros |
| Rust and JSON generation | `incurs gen` |
| Rust-only table and CSV formats | Explicit `incurs-extras` opt-in |

The parity inventory classifies all 1,062 tests in the vendored TypeScript oracle. `cargo xtask parity` also runs shared CLI observations against both implementations and compares structured JSON/JSONL or normalized text.

## Quick start

```toml
[dependencies]
incurs = "0.3"
schemars = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, incurs::Args)]
struct GreetArgs {
    /// Name to greet.
    name: String,
}

#[derive(Deserialize, incurs::Options)]
struct GreetOptions {
    /// Add an exclamation mark.
    excited: bool,
}

#[derive(JsonSchema, Serialize)]
struct GreetOutput {
    message: String,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let greet = CommandDef::typed::<GreetArgs, GreetOptions, (), GreetOutput, _, _>(
        "greet",
        |ctx: TypedContext<GreetArgs, GreetOptions, ()>| async move {
            TypedResult::ok(GreetOutput {
                message: format!(
                    "Hello, {}{}",
                    ctx.args.name,
                    if ctx.options.excited { "!" } else { "." },
                ),
            })
        },
    )
    .description("Greet someone")
    .done();

    Cli::create("greet")
        .version("1.0.0")
        .command("greet", greet)
        .serve()
        .await
}
```

```console
$ greet greet Ada --excited --json
{"message":"Hello, Ada!"}
```

`CommandDef::typed` derives the transport schemas from the input types and the output JSON Schema from `GreetOutput`. The handler receives validated values regardless of whether it was called by CLI, HTTP, or MCP.

## Code generation

Install or run the workspace CLI, then point it at a Cargo project whose binary exports `--llms-full --format json`:

```bash
cargo run -p incurs-cli -- gen --dir ./my-cli --entry my-cli --config-schema
```

The command writes deterministic artifacts:

- `src/incurs_generated.rs`: typed command modules, argument/option types, CTA renderers, and the embedded manifest
- `incurs.manifest.json`: canonical shared command manifest
- `config.schema.json`: optional configuration schema

Use `--output` and `--json-output` to override the first two paths. `--entry` accepts a Cargo binary name or an executable path.

## Rust-only extensions

Parity-default help and parsing expose only upstream formats. Table and CSV remain available through the separate extension crate:

```toml
[dependencies]
incurs-extras = "0.3"
```

```rust
use incurs_extras::{CliExtras, ExtraFormat};

let cli = cli.default_extra_format(ExtraFormat::Table);
```

## Runtime and transports

`Cli::run_to` is the injectable execution boundary. `serve` and `serve_with` are process adapters over it, while `serve_to` is the stable buffered test surface. This keeps parsing, discovery, middleware, command execution, formatting, CTAs, and exit behavior on one path.

The optional transport features are:

```toml
incurs = { version = "0.3", features = ["http", "mcp", "openapi"] }
```

HTTP exposes root and arbitrarily nested commands, OpenAPI documents, well-known skill files, and fetch gateways. MCP supports current protocol initialization, filtered progressive discovery, direct discovery, and invocation through the same command graph.

## Examples

[`crates/incurs/examples/todoapp.rs`](crates/incurs/examples/todoapp.rs) exercises commands, streaming, middleware, CTAs, discovery, and output formats.

```bash
cargo run -p incurs --example todoapp -- --help
cargo run -p incurs --example todoapp -- add "Buy groceries" --priority high
cargo run -p incurs --example todoapp -- list --json
cargo run -p incurs --example todoapp -- stream
```

## Verification

```bash
# TypeScript oracle inventory plus executable cross-language cases
cargo xtask parity

# Rust contracts across every feature
cargo test --workspace --all-features

# Public documentation
cargo doc --workspace --all-features --no-deps
```

See [MIGRATION.md](MIGRATION.md) for the 0.2 to 0.3 transition.

## Architecture

```text
typed command definitions
          |
          v
shared command graph + schemas
  |       |       |       |
 CLI     HTTP     MCP   generated artifacts
                           |-- OpenAPI
                           |-- skills
                           |-- completions
                           `-- Rust/JSON codegen
```

## License

MIT, matching upstream [wevm/incur](https://github.com/wevm/incur).
