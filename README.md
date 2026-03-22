# incurs

A Rust port of [wevm/incur](https://github.com/wevm/incur) — the CLI framework for humans and AI agents.

Build CLIs that work for both humans and agents. Same commands serve via CLI, HTTP, and MCP. Agent discovery, token-efficient output, streaming, middleware, shell completions — all built in.

## Status

Ported from incur v0.3.6 (TypeScript). 245 tests, 10/10 parity with the TS implementation on JSON output.

| Feature | Status |
|---------|--------|
| CLI parsing (args, options, flags) | Done |
| Three transports (CLI, HTTP, MCP) | CLI done, HTTP/MCP stubbed |
| Help (`--help`, `--version`) | Done |
| Output formats (`--json`, `--format`, `--verbose`) | Done |
| Output filtering (`--filter-output`) | Done |
| Streaming (async generators) | Done |
| Middleware (onion-style) | Done |
| Shell completions (bash/zsh/fish/nushell) | Done |
| Agent discovery (21 agents) | Done |
| Skill file generation (`--llms`, `skills add`) | Done |
| MCP registration (`mcp add`) | Done |
| TOON output format | Done (via `toon-format` crate) |
| Token counting/limiting | Done (via `tiktoken-rs`) |
| Config file loading | Done |
| OpenAPI import | Stubbed |
| Derive macros (`IncurArgs`, `IncurOptions`, `IncurEnv`) | Done |

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
incur = { git = "https://github.com/douglance/incurs" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

Build a CLI:

```rust
use incur::cli::Cli;
use incur::command::{CommandDef, CommandHandler, CommandContext, Example};
use incur::output::CommandResult;
use incur::schema::{FieldMeta, FieldType};
use serde_json::json;
use std::collections::HashMap;

struct GreetHandler;

#[async_trait::async_trait]
impl CommandHandler for GreetHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let name = ctx.args.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("world");
        CommandResult::Ok {
            data: json!({ "message": format!("Hello, {}!", name) }),
            cta: None,
        }
    }
}

#[tokio::main]
async fn main() {
    Cli::create("greet")
        .description("A greeting CLI")
        .version("1.0.0")
        .command("hello", CommandDef {
            name: "hello".to_string(),
            description: Some("Say hello".to_string()),
            args_fields: vec![FieldMeta {
                name: "name",
                cli_name: "name".to_string(),
                description: Some("Who to greet"),
                field_type: FieldType::String,
                required: false,
                default: Some(json!("world")),
                alias: None,
                deprecated: false,
                env_name: None,
            }],
            options_fields: vec![],
            env_fields: vec![],
            aliases: HashMap::new(),
            examples: vec![],
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(GreetHandler),
            middleware: vec![],
            output_schema: None,
        })
        .serve()
        .await
        .unwrap();
}
```

```
$ greet hello Alice
{"message":"Hello, Alice!"}

$ greet --help
greet@1.0.0 — A greeting CLI
...
```

## Example

See [`crates/incur/examples/todoapp.rs`](crates/incur/examples/todoapp.rs) for a full example with 6 commands, streaming, middleware, CTAs, and all output formats.

```bash
cargo run -p incur --example todoapp -- --help
cargo run -p incur --example todoapp -- add "Buy groceries" --priority high
cargo run -p incur --example todoapp -- list --json
cargo run -p incur --example todoapp -- stats --verbose
cargo run -p incur --example todoapp -- stream
```

## Architecture

```
crates/
  incur/           # main library (9,400+ lines)
    src/
      cli.rs       # Cli builder, serve(), command tree
      command.rs   # unified execute() across CLI/HTTP/MCP
      parser.rs    # argv parsing (--key=val, -abc, --no-flag, counts, arrays)
      help.rs      # help text generation
      formatter.rs # TOON, JSON, YAML, Markdown output
      filter.rs    # output filtering by key paths
      errors.rs    # error hierarchy
      middleware.rs # onion-style async middleware
      agents.rs    # 21 AI agent configs with detection
      skill.rs     # SKILL.md generation
      completions.rs # bash/zsh/fish/nushell
      streaming.rs # async stream utilities
      ...          # + fetch, mcp, config, sync, openapi
  incur-macros/    # proc macros (#[derive(IncurArgs/Options/Env)])
  incur-cli/       # codegen binary (stub)
```

## Keeping in Sync with Upstream

This port tracks the TypeScript [wevm/incur](https://github.com/wevm/incur) as its spec. The sync mechanism is test parity:

1. TS tests define correct behavior
2. Rust integration tests (`tests/e2e.rs`, `tests/parser_test.rs`) port those tests
3. `tests/compare.sh` diffs JSON output between TS and Rust todoapp examples
4. When upstream changes, port the new tests first (they fail), then update the Rust code

```bash
# Run all Rust tests
cargo test --workspace

# Run the TS vs Rust comparison
bash tests/compare.sh
```

## License

MIT — same as upstream [wevm/incur](https://github.com/wevm/incur).
