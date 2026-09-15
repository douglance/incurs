# incurs

The CLI framework for humans and agents.

Define a command once and expose the same validated behavior through CLI, HTTP, MCP, OpenAPI, Agent Plugin packages, skill files, shell completions, and a native desktop window.

incurs began as a Rust port of [wevm/incur](https://github.com/wevm/incur) and keeps its command model. The two are now independent: incurs is the implementation, not a translation of one, and its observable surface is defined by its own tests.

## Status

Version 0.6.0 builds the `incurs` CLI with incurs itself. Doing so surfaced
three defects no existing test could see: a builtin command silently shadowed a
user-defined one of the same name, a boolean option could not be omitted, and
one behavioral fact was carried by two independent fields. All three are fixed.
`SKILL.md` is now generated from the command graph rather than hand-written, and
`incurs explain` carries the Rust authoring reference as command data. Version
0.6 requires Rust 1.88 or newer. See [MIGRATION.md](MIGRATION.md).

| Surface | 0.6 status |
| --- | --- |
| CLI parsing, help, validation, aliases, output and streaming | Golden-tested |
| HTTP, nested routes, middleware and fetch gateways | Implemented and tested |
| MCP 2024-11-05 through 2026-07-28, progressive/direct discovery and calls | Exact standard profiles with `rmcp` 3 |
| OpenAPI, skills and shell completions | Generated from the shared command graph |
| Agent Plugins 1.0 | Portable `plugin.json`, Agent Skills, and optional `mcp.json` output |
| Native desktop application | GPUI window over the shared tool catalog, in `extensions/gpui` |
| Durable Code Mode | Platform-neutral Rust lifecycle with local sandbox execution |
| Typed args, options, env and output | `CommandDef::typed` plus derive macros |
| Rust and JSON generation | `incurs gen` |
| Authoring reference | `incurs explain`, compiled into `SKILL.md` |
| Rust-only table and CSV formats | Explicit `incurs-extras` opt-in |

`crates/incurs/tests/cli_surface.rs` pins the observable CLI surface — exit code and stdout together — with golden files covering every documented output format, error envelope, and streaming mode.

## Quick start

```toml
[dependencies]
incurs = "0.6"
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

### Agent Plugin packages

Build an [Agent Plugins 1.0](https://agent-plugins.org/specification) directory from the same complete command graph:

```bash
my-cli plugin build --bundle-cli --output ./dist/my-cli-plugin
```

Or add it to the normal code-generation pass:

```bash
cargo run -p incurs-cli -- gen \
  --dir ./my-cli \
  --entry my-cli \
  --plugin-output ./dist/my-cli-plugin \
  --plugin-bundle-cli
```

The publisher keeps each layer explicit: `skills/<name>/SKILL.md` files are Prompt Artifacts, root `mcp.json` is the Tool Binding, and `bin/my-cli` is the target-specific Tool Runtime. The root `plugin.json` declares all three. Use `--plugin-no-mcp` for a skills-only package. Regeneration refuses to replace existing plugin artifacts unless you pass `--plugin-force`; the target command uses the equivalent `--force` option.

Install the CLI, MCP server, and skills from that one directory:

```bash
incurs plugin install ./dist/my-cli-plugin
```

The installer validates the full package, checks its operating system and architecture, copies it into the user data directory, and installs its command into the user executable directory. It reports when that directory is not on `PATH` but never edits a shell profile. Native agent clients still control how they discover Agent Plugin directories; Incurs does not rewrite legacy agent configuration files.

Remove the managed package and command while preserving its persistent data, or explicitly purge the data:

```bash
incurs plugin uninstall my-cli
incurs plugin uninstall my-cli --purge
```

The standalone `incurs` binary also acts as an Agent Plugins 1.0 client. Validation is offline and reports fatal manifest failures separately from skipped skills and MCP servers:

```bash
incurs plugin validate ./dist/my-cli-plugin \
  --data-dir "$HOME/.local/share/my-cli-plugin"
```

Connect valid stdio, Streamable HTTP, and legacy HTTP+SSE servers and inspect the resulting namespaced `ToolCatalog`:

```bash
incurs plugin tools ./dist/my-cli-plugin \
  --data-dir "$HOME/.local/share/my-cli-plugin"
```

Call any discovered namespaced tool with a flat JSON object:

```bash
incurs plugin call ./dist/my-cli-plugin my-server_my-tool \
  --arguments '{"name":"Ada"}' \
  --data-dir "$HOME/.local/share/my-cli-plugin"
```

Each MCP server connects independently, so one connection, authentication, or handshake failure does not hide tools from other servers. The explicit data directory is created before launch, persists across runs, and is never removed by the runtime. Configured HTTP headers are visible configuration rather than a secret store. The stdio runtime launches exact argv without a shell and does not claim to sandbox the subprocess.

Library consumers can enable `agent-plugins` for offline loading or `agent-plugins-mcp` for loading plus all three MCP transports. See [Agent Plugins compatibility](docs/agent-plugins.md) for the complete behavior and failure-boundary matrix.

## Rust-only extensions

Built-in help and parsing expose the core output formats only. Table and CSV remain available through the separate extension crate:

```toml
[dependencies]
incurs-extras = "0.6"
```

```rust
use incurs_extras::{CliExtras, ExtraFormat};

let cli = cli.default_extra_format(ExtraFormat::Table);
```

## Runtime and transports

`Cli::run_to` is the injectable execution boundary. `serve` and `serve_with` are process adapters over it, while `serve_to` is the stable buffered test surface. This keeps parsing, discovery, middleware, command execution, formatting, CTAs, and exit behavior on one path.

The optional transport features are:

```toml
incurs = { version = "0.6", features = ["http", "mcp", "openapi"] }
```

HTTP exposes root and arbitrarily nested commands, OpenAPI documents, well-known
skill files, and fetch gateways. MCP supports all five official standards at
once. Modern clients negotiate with `server/discover`; legacy clients continue
to initialize with their exact standard. HTTP clients fall back only on
protocol-specific evidence, never on authentication, transport, or server
failures.

`incurs-mcp-protocol` owns the exact standard registry, lifecycle families,
wire-era codecs, feature changes, and negotiation policy. Each published
standard has its own module and embeds its pinned official JSON Schema. Run
`cargo xtask mcp-schema-sync --check` to verify schema provenance and generated
method registries.

## Code Mode

`Cli::tool_catalog()` exposes every MCP-visible leaf command as a
transport-neutral Rust API. Calls use the same schemas, middleware, declared
environment fields, CLI global defaults, command config sections, request
metadata, streaming results, and structured errors as the shared command
runtime. `Cli::try_tool_catalog()` reports exposed-name collisions instead of
silently replacing a command.

The `incurs-codemode` crate owns the generic `CodeMode` lifecycle and executor
contract. It provides typed connector discovery, search and describe, resolved
approval policy, immutable capability snapshots, deterministic replay,
cancellation, ordered events, artifact-backed large values, rollback hooks,
snippets, and bounded durable history. It can wrap an incurs catalog, a remote
MCP client, or an authenticated OpenAPI client.

Local incurs annotations are authoritative. A read-only local tool skips
approval only when it is neither destructive nor open-world. Remote MCP and
OpenAPI tools require approval and use logged replay unless the host installs an
explicit `ToolPolicyResolver`.

Code Mode programs use JavaScript for low startup latency and direct access to
JSON-shaped tool inputs and outputs. `incurs-codemode-local` runs them in a
resource-limited QuickJS runtime. Remote and provider-specific executors
implement the same Rust `CodeExecutor` contract in standalone workspaces under
`extensions/`.

Run the native example with:

```sh
cargo run -p incurs-codemode-local --example local
```

The lifecycle, connector policy, dispatch, harness generation, and persistence
contracts are Rust. The generated JavaScript harness is shared by local and
remote executors.

`incurs-codemode-mcp` provides a reusable `rmcp::ServerHandler` and stdio
adapter for the stable provider-neutral lifecycle surface:

| Tool | Purpose |
| --- | --- |
| `codemode_search` | Search current tools and snippets with declarations needed to call each match. |
| `codemode_execute` | Start a JavaScript execution. |
| `codemode_execution` | Read execution state or an owned artifact. |
| `codemode_decide` | Approve or reject one pending action. |
| `codemode_cancel` | Cancel a running or paused execution. |

`codemode_execute` returns a durable running state before the actor drives the
non-`Send` QuickJS pass. MCP cancellation remains responsive and propagates
through connector calls. The same handler can be served over stdio or an HTTP
transport. HTTP method, path, and headers flow into incurs request context when
the transport provides them. Oversized values remain artifact references in MCP
execution snapshots and can be fetched with `codemode_execution.artifact_id`.

## Examples

[`crates/incurs/examples/todoapp.rs`](crates/incurs/examples/todoapp.rs) exercises commands, streaming, middleware, CTAs, discovery, and output formats.

```bash
cargo run -p incurs --example todoapp -- --help
cargo run -p incurs --example todoapp -- add "Buy groceries" --priority high
cargo run -p incurs --example todoapp -- list --json
cargo run -p incurs --example todoapp -- stream
```

## Native desktop applications

`extensions/gpui` ships the same command graph as a double-clickable
application, for people who will never open a terminal. Commands are listed in
a window, each command's inputs are collected from its Tool Contract schema,
and every call goes through `ToolCatalog`, so validation, middleware, config
defaults, streaming, and cancellation behave as they do on the CLI.

```rust
use incurs_app_gpui::DesktopApp;

DesktopApp::from_cli(&cli)?.title("Todo").run()
```

`bundle::MacBundle` wraps the built executable in a macOS `.app` so it can be
installed by dragging it. GPUI is a large platform-specific dependency, so the
extension is a standalone workspace and the root workspace does not depend on
it. See [extensions/gpui/README.md](extensions/gpui/README.md).

## Verification

```bash
# Rust contracts across every feature, including the CLI surface goldens
cargo test --workspace --all-features

# Public documentation
cargo doc --workspace --all-features --no-deps
```

See [MIGRATION.md](MIGRATION.md) for release migration notes.

## Architecture

```text
typed command definitions
          |
          v
shared command graph + schemas
  |       |       |       |                  |
 CLI     HTTP     MCP   tool catalog    generated artifacts
                           |              |-- OpenAPI
                           |              |-- skills
                           |              |-- completions
                           |              `-- Rust/JSON codegen
                           |
                           |-- generic Code Mode
                           |     |           |
                           |  local QuickJS  remote executors
                           |
                           `-- native desktop window (extensions/gpui)
```

## License

MIT, matching upstream [wevm/incur](https://github.com/wevm/incur).
