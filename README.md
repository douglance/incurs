# incurs

The CLI framework for humans and agents.

Define a command once. incurs derives the argument parsing, help, validation, output
formatting, JSON Schemas, and every transport from that single definition — so a
command you wrote for a terminal is already an MCP tool, an HTTP route, an OpenAPI
operation, a skill file an agent can read, a shell completion, and a window.

```toml
[dependencies]
incurs = "0.6"
schemars = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

Requires Rust 1.88 or newer.

## Quick start

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
{
  "message": "Hello, Ada!"
}
```

Doc comments become descriptions everywhere. `CommandDef::typed` derives the input
schemas from `GreetArgs` and `GreetOptions`, and the output schema from `GreetOutput`.
The handler receives validated values whether the call arrived from a terminal, HTTP,
or an agent.

## What that one definition already gives you

Nothing below needs extra code.

| Run this | You get |
| --- | --- |
| `greet --help` | Help for the CLI and every command |
| `greet greet --schema` | The command's JSON Schema |
| `greet --llms` / `--llms-full` | A manifest written for an agent to read |
| `greet --format json\|yaml\|toon\|jsonl\|md` | The same result in any output format |
| `greet --mcp` | An MCP server over stdio |
| `greet mcp add` | Registration with detected MCP clients |
| `greet skills add` | Agent Skills installed for detected agents |
| `greet completions bash\|zsh\|fish` | A shell completion script |
| `greet plugin build --output ./dist` | A portable Agent Plugins 1.0 package |
| `greet --config-schema` | A schema for the config file format |

## Output

A command returns `TypedResult::ok(value)`, and the serialized value *is* the output.
Pass `--full-output` for the `{ok, data, meta}` envelope when a caller wants timing and
command metadata. `TypedResult::error(code, message)` produces the structured error
envelope and a non-zero exit; `ok_with_exit_code` passes a wrapped subprocess's status
through without turning success into a failure; `ok_with_cta` attaches follow-up
commands.

The default format is [TOON](https://crates.io/crates/toon-format). Table and CSV are
deliberately opt-in, through a separate crate:

```toml
incurs-extras = "0.6"
```

```rust
use incurs_extras::{CliExtras, ExtraFormat};

let cli = cli.default_extra_format(ExtraFormat::Table);
```

## Feature flags

```toml
incurs = { version = "0.6", features = ["http", "mcp", "openapi"] }
```

| Feature | Adds |
| --- | --- |
| `cli` *(default)* | Process adapters, signals, and terminal output |
| `toon`, `tokens` *(default)* | TOON output and `--token-count` / `--token-limit` |
| `http` | Axum routes for root and nested commands, plus fetch gateways |
| `mcp` | MCP server, all five published standards at once |
| `openapi` | Import an OpenAPI 3.x document as commands, and emit one |
| `yaml` | YAML output |
| `agent-plugins`, `agent-plugins-mcp` | Load Agent Plugin packages, with or without MCP transports |

MCP clients that support discovery negotiate with `server/discover`; older clients
initialize with their exact standard. Fallback happens only on protocol evidence, never
on an authentication, transport, or server failure.

## The `incurs` command-line tool

```bash
cargo install incurs-cli     # installs the `incurs` binary
```

```bash
incurs explain               # the Rust authoring reference, by topic
incurs explain typed-commands
```

### Code generation

Point it at a project whose binary exposes `--llms-full`, and it writes typed helpers
for calling that CLI from Rust:

```bash
incurs gen --dir ./my-cli --entry my-cli --config-schema
```

- `src/incurs_generated.rs` — typed command modules, argument and option types, CTA renderers, and the embedded manifest
- `incurs.manifest.json` — the canonical command manifest
- `config.schema.json` — the config file schema, with `--config-schema`

`--output` and `--json-output` override the first two paths. `--entry` takes a Cargo
binary name or a path to an executable.

### Agent Plugin packages

Any incurs CLI can package itself:

```bash
my-cli plugin build --bundle-cli --output ./dist/my-cli-plugin
```

The package keeps its layers separate: `skills/<name>/SKILL.md` for agents to read,
`mcp.json` declaring the tool surface, and `bin/my-cli` as the executable, all declared
by a root `plugin.json`. Use `--plugin-no-mcp` for a skills-only package. Regeneration
will not overwrite existing plugin artifacts without `--force`.

The `incurs` binary is also a client for those packages:

```bash
incurs plugin install ./dist/my-cli-plugin
incurs plugin validate ./dist/my-cli-plugin --data-dir ~/.local/share/my-cli-plugin
incurs plugin tools    ./dist/my-cli-plugin --data-dir ~/.local/share/my-cli-plugin
incurs plugin call     ./dist/my-cli-plugin my-server_my-tool \
  --arguments '{"name":"Ada"}' --data-dir ~/.local/share/my-cli-plugin
incurs plugin uninstall my-cli [--purge]
```

Installation validates the package, checks its operating system and architecture, and
installs the command into your executable directory. It tells you when that directory
is not on `PATH`, and never edits a shell profile. Each MCP server connects
independently, so one failure does not hide the tools from the others. The data
directory you name is created before launch, persists across runs, and is never removed
for you.

See [Agent Plugins compatibility](docs/agent-plugins.md) for the full behavior and
failure-boundary matrix.

## Calling commands without a CLI

`Cli::tool_catalog()` exposes every MCP-visible command as a transport-neutral Rust
API. Calls made through it use the same schemas, middleware, environment fields, config
defaults, streaming results, and structured errors as the CLI — it is the boundary MCP,
Code Mode, and the desktop application all go through. `try_tool_catalog()` reports
name collisions instead of silently replacing a command.

## Code Mode

`incurs-codemode` lets an agent write a small JavaScript program that calls your tools,
instead of making one tool call per step. It provides connector discovery, approval
policy, immutable capability snapshots, deterministic replay, cancellation, ordered
events, artifact-backed large values, rollback hooks, and bounded durable history. It
can wrap an incurs catalog, a remote MCP client, or an authenticated OpenAPI client.

Your command's own annotations decide policy: a read-only tool skips approval only when
it is neither destructive nor open-world. Remote tools require approval unless the host
installs its own policy resolver.

`incurs-codemode-local` runs those programs in a resource-limited QuickJS runtime, and
`incurs-codemode-mcp` exposes the lifecycle to MCP clients:

| Tool | Purpose |
| --- | --- |
| `codemode_search` | Search tools and snippets, with the declarations needed to call each match |
| `codemode_execute` | Start a JavaScript execution |
| `codemode_execution` | Read execution state or an owned artifact |
| `codemode_decide` | Approve or reject one pending action |
| `codemode_cancel` | Cancel a running or paused execution |

## Terminal applications

`incurs-app-ratatui` runs a command graph full-screen in a terminal: commands on
the left, the selected command's inputs collected from its schema, results below.

```rust
use incurs_app_ratatui::TerminalApp;

TerminalApp::from_cli(&cli)?.title("Todo").run()
```

The installed `incurs` tool does this for its own commands:

```bash
incurs --tui
```

See [its README](crates/incurs-app-ratatui/README.md) for the key map and the
controls each schema type gets.

## Native desktop applications

`incurs-app-gpui` ships the same command graph as a double-clickable application, for
people who will never open a terminal. Commands are listed in a window, each command's
inputs are collected from its schema, and every call goes through the tool catalog — so
validation, middleware, config defaults, streaming, and cancellation behave exactly as
they do on the CLI.

```rust
use incurs_app_gpui::DesktopApp;

DesktopApp::from_cli(&cli)?.title("Todo").run()
```

`MacBundle` wraps the built executable in a macOS `.app` that installs by dragging.
See [its README](extensions/gpui/README.md) for the full guide and current limits.

## How it fits together

```text
              typed command definitions
                         |
                         v
            shared command graph + schemas
      ______________|____________________________
     |        |        |            |            |
    CLI     HTTP      MCP     tool catalog   generated artifacts
                                   |          |-- OpenAPI
                                   |          |-- Agent Skills
                                   |          |-- shell completions
                                   |          `-- Rust and JSON codegen
                                   |
                                   |-- Code Mode  (local QuickJS, remote executors)
                                   |
                                   `-- native desktop window
```

## Learn more

- [`incurs explain`](crates/incurs-cli) — the authoring reference, also compiled into [`SKILL.md`](SKILL.md) for agents
- [A worked example](crates/incurs/examples/todoapp.rs) covering commands, streaming, middleware, CTAs, discovery, and output formats
- [MIGRATION.md](MIGRATION.md) — upgrading between releases
- [CHANGELOG.md](CHANGELOG.md) — what changed
- [CONTRIBUTING.md](CONTRIBUTING.md) — working on incurs itself

## License

MIT. incurs began as a Rust port of [wevm/incur](https://github.com/wevm/incur) and
keeps its command model; the two are now independent.
