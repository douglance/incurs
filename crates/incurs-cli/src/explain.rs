//! The Rust authoring reference, carried as command data.
//!
//! This is the guidance an agent or a person needs to write an incurs CLI. It
//! lives in the command graph rather than in a side file, so it reaches every
//! surface the framework already serves — `--help`, `--llms-full`, MCP, the
//! generated `SKILL.md` — from one definition. A second copy in Markdown would
//! be a second thing to keep true.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which topic to explain.
#[derive(Deserialize, incurs::Args)]
pub struct ExplainArgs {
    /// Topic name. Omit to list every topic.
    pub topic: Option<String>,
}

/// One reference topic.
#[derive(Debug, JsonSchema, Serialize)]
pub struct Topic {
    /// Stable topic name, as passed to `incurs explain`.
    pub name: String,
    /// Human-readable title.
    pub title: String,
    /// One-line description.
    pub summary: String,
    /// Full reference text. Empty when listing topics.
    pub body: String,
}

/// Topics matching the request.
#[derive(Debug, JsonSchema, Serialize)]
pub struct ExplainOutput {
    /// One topic when named, every topic summary otherwise.
    pub topics: Vec<Topic>,
}

/// Returns one topic, or every topic's summary.
pub async fn run(ctx: TypedContext<ExplainArgs, (), ()>) -> TypedResult<ExplainOutput> {
    match ctx.args.topic.as_deref() {
        None => TypedResult::ok(ExplainOutput {
            topics: TOPICS
                .iter()
                .map(|topic| Topic {
                    name: topic.0.to_string(),
                    title: topic.1.to_string(),
                    summary: topic.2.to_string(),
                    body: String::new(),
                })
                .collect(),
        }),
        Some(name) => match TOPICS.iter().find(|topic| topic.0 == name) {
            Some(topic) => TypedResult::ok(ExplainOutput {
                topics: vec![Topic {
                    name: topic.0.to_string(),
                    title: topic.1.to_string(),
                    summary: topic.2.to_string(),
                    body: topic.3.trim().to_string(),
                }],
            }),
            None => TypedResult::error(
                "UNKNOWN_TOPIC",
                format!(
                    "unknown topic `{name}`. Available: {}",
                    TOPICS
                        .iter()
                        .map(|topic| topic.0)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        },
    }
}

/// `(name, title, summary, body)` for every reference topic.
type TopicEntry = (&'static str, &'static str, &'static str, &'static str);

const TOPICS: &[TopicEntry] = &[
    (
        "quick-start",
        "Quick start",
        "The smallest incurs CLI that builds and runs.",
        r#"
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
    /// Who to greet.
    name: String,
}

#[derive(JsonSchema, Serialize)]
struct Greeting {
    message: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let greet = CommandDef::typed::<GreetArgs, (), (), Greeting, _, _>(
        "greet",
        |ctx: TypedContext<GreetArgs, (), ()>| async move {
            TypedResult::ok(Greeting {
                message: format!("Hello, {}.", ctx.args.name),
            })
        },
    )
    .description("Greet someone")
    .done();

    Cli::create("hello")
        .version(env!("CARGO_PKG_VERSION"))
        .description("A greeting CLI")
        .command("greet", greet)
        .serve()
        .await
}
```

That one definition already serves `hello greet Ada`, `--help`, `--schema`,
`--llms`, `--llms-full`, `--format json|yaml|toon|jsonl|md`, shell completions,
an MCP server over stdio, and skill files.
"#,
    ),
    (
        "typed-commands",
        "Typed commands",
        "CommandDef::typed, the three derive macros, and what each generates.",
        r#"
`CommandDef::typed::<Args, Options, Env, Output, _, _>(name, handler)` is the
authoring path. Use `()` for any of the first three when a command has none.

- `Args` derives `incurs::Args` — positional, in declaration order.
- `Options` derives `incurs::Options` — named flags.
- `Env` derives `incurs::Env` — environment bindings.
- `Output` derives `schemars::JsonSchema` and `serde::Serialize`. Its schema is
  published automatically as the command's output contract.

Doc comments become descriptions on every surface, so write them.

```rust
#[derive(Deserialize, incurs::Options)]
struct ListOptions {
    /// Maximum number of results.
    #[incurs(alias = "n", default = 10)]
    limit: u32,
    /// Include archived items.
    #[incurs(alias = "a")]
    archived: bool,
    /// Filter by tag. Repeatable.
    tag: Vec<String>,
    /// Verbosity. Repeat to increase.
    #[incurs(count)]
    verbose: u8,
}

#[derive(Deserialize, incurs::Env)]
struct AppEnv {
    /// API token.
    #[incurs(env = "API_TOKEN")]
    api_token: String,
}
```

Required means: not `Option<T>`, not `Vec<T>`, not `bool`, not `count`, and no
`default`. A `bool` is always optional and defaults to `false`, so an absent
flag parses rather than failing.
"#,
    ),
    (
        "wrapping-programs",
        "Wrapping another program",
        "Raw commands, default subcommands, and hidden commands.",
        r#"
A raw command hands its argv to the handler unchanged, so an incurs CLI can
front an existing program without re-declaring its flags.

```rust
let build = CommandDef::build("build", Forward).description("Build").raw().done();
```

- Once argv names a raw command, built-in flags (`--help`, `--json`,
  `--format`, ...) and option validation are skipped. The handler owns them.
- The handler reads `ctx.args["argv"]`. From the CLI it is every token after
  the program name, as typed. From a tool call it is the command path followed
  by the caller's `arguments` array, the only option a raw command declares.
- A handler that already wrote to the terminal returns `null` data with the
  wrapped program's exit code: `CommandResult::Ok { data: Value::Null, cta:
  None, exit_code: Some(code) }`. Nothing else is printed.
- `Cli::root(def)` with a raw `def` receives any argv the command tree cannot
  run: empty argv, an unknown command at any depth, a group named without a
  subcommand, and leading flags such as `--help` and `--version`. The
  machine-facing flags (`--llms`, `--mcp`, `--schema`, `--format`, ...) and
  builtin commands such as `completions` still reach the framework.

`Cli::default_command("run")` on a CLI mounted with `.group(...)` runs `run`
when the next token names no subcommand, without consuming that token:
`app test Foo` runs `app test run` and passes `Foo` along.

`.hidden()` keeps a command out of help, completions, skills, `--llms`, and
tool catalogs. It still runs when invoked by name.
"#,
    ),
    (
        "output",
        "Output and errors",
        "TypedResult, the output envelope, formats, exit codes, and CTAs.",
        r#"
Return `TypedResult::ok(value)` for success. The command's stdout is the
serialized value — not an envelope. `--full-output` adds `{ok, data, meta}`
when a caller wants the metadata.

- `TypedResult::ok_with_cta(value, cta)` suggests follow-up commands.
- `TypedResult::ok_with_exit_code(value, code)` passes through a wrapped
  subprocess's status without turning success into an error.
- `TypedResult::error(code, message)` produces the structured error envelope
  and a non-zero exit.

Formats are `toon` (default), `json`, `yaml`, `md`, and `jsonl`. `--json` is
shorthand for `--format json`. Set a different default per command with
`.format(Format::Json)` when the output is meant for scripts. `table` and `csv`
are opt-in through the separate `incurs-extras` crate.

Streaming commands return `CommandResult::Stream` or `RecordStream`; `--format
jsonl` emits one record per line, and other formats buffer.
"#,
    ),
    (
        "surfaces",
        "Surfaces",
        "Everything one command graph exposes, and how to reach each.",
        r#"
| Surface | How |
| --- | --- |
| CLI | `cli.serve()` |
| Buffered CLI, for tests | `cli.serve_to(argv, &mut buf, human)` |
| JSON Schema per command | `<cli> <command> --schema` |
| Agent manifest | `<cli> --llms`, `<cli> --llms-full [--format json]` |
| Config file schema | `<cli> --config-schema`, after `.config(ConfigOptions { .. })` |
| Shell completions | `<cli> completions bash\|zsh\|fish` |
| Agent Skills | `<cli> skills add`, `<cli> skills list` |
| MCP over stdio | `<cli> --mcp`, or `<cli> mcp add` to register |
| Agent Plugin package | `<cli> plugin build --output <dir>` |
| Non-CLI invocation | `cli.tool_catalog()` — `definitions()` and `call()` |
| HTTP | the `http` feature |
| OpenAPI | the `openapi` feature |
| Native desktop window | `incurs-app-gpui` in `extensions/gpui` |

`ToolCatalog` is the single non-CLI invocation boundary. MCP, Code Mode, and
the desktop surface all route through it, so validation, middleware, config
defaults, streaming and cancellation behave identically everywhere.

A command you define wins over a builtin of the same name.
"#,
    ),
    (
        "structure",
        "Groups, middleware, and config",
        "Composing a larger CLI.",
        r#"
```rust
let plugin = Cli::create("plugin")
    .description("Plugin commands")
    .command("build", build_command());

Cli::create("app")
    .group(plugin)                       // app plugin build
    .config(ConfigOptions {
        flag: "config".to_string(),
        files: vec!["app.config.json".to_string()],
    })
    .use_middleware(tracing_middleware())
    .globals::<Globals>()                // flags valid before the command
    .command("run", run_command())
```

Middleware wraps execution onion-style: code before `next().await` runs before
the handler, code after runs after. Check `ctx.agent` before writing anything
for a person, so structured output stays exactly what a caller parses.

Config files supply option defaults per command, mirroring the command tree.
`--config-schema` publishes the shape so editors can validate it.
"#,
    ),
    (
        "testing",
        "Testing",
        "How to test a command graph without spawning a process.",
        r#"
`serve_to` is the stable buffered surface. It takes argv, a writer, and a
`human` flag, and returns the exit code — the same path `serve()` uses, so a
test cannot drift from the process.

```rust
let mut output = Vec::new();
let exit = build_cli()
    .serve_to(vec!["greet".into(), "Ada".into(), "--json".into()], &mut output, false)
    .await?;

assert_eq!(exit, None);
assert_eq!(
    serde_json::from_slice::<serde_json::Value>(&output)?,
    serde_json::json!({ "message": "Hello, Ada." })
);
```

Assert the whole observation — exit code and stdout together — rather than
selected fields, and normalize measured values such as durations instead of
asserting around them.

Test the non-CLI boundary too: `cli.tool_catalog()` gives the definitions MCP
and Code Mode see, which is where a missing description or output schema shows
up.
"#,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listing_returns_every_topic_without_bodies() {
        let output = run(context(None)).await;
        let TypedResult::Ok { data, .. } = output else {
            panic!("listing topics should succeed");
        };

        assert_eq!(data.topics.len(), TOPICS.len());
        assert!(
            data.topics.iter().all(|topic| topic.body.is_empty()),
            "a listing carries summaries, not full bodies"
        );
    }

    #[tokio::test]
    async fn a_named_topic_returns_its_body() {
        let output = run(context(Some("typed-commands"))).await;
        let TypedResult::Ok { data, .. } = output else {
            panic!("a known topic should succeed");
        };

        assert_eq!(data.topics.len(), 1);
        assert!(data.topics[0].body.contains("CommandDef::typed"));
    }

    #[tokio::test]
    async fn an_unknown_topic_names_the_available_ones() {
        let output = run(context(Some("nope"))).await;
        let TypedResult::Error { message, .. } = output else {
            panic!("an unknown topic should fail");
        };

        assert!(message.contains("quick-start"), "got: {message}");
    }

    /// Every topic has content, so a listed topic is never empty when asked for.
    #[test]
    fn every_topic_has_a_summary_and_a_body() {
        for (name, title, summary, body) in TOPICS {
            assert!(!title.trim().is_empty(), "`{name}` needs a title");
            assert!(!summary.trim().is_empty(), "`{name}` needs a summary");
            assert!(
                body.trim().len() > 200,
                "`{name}` needs a substantive body, got {} bytes",
                body.trim().len()
            );
        }
    }

    fn context(topic: Option<&str>) -> TypedContext<ExplainArgs, (), ()> {
        TypedContext {
            agent: true,
            args: ExplainArgs {
                topic: topic.map(ToString::to_string),
            },
            display_name: "incurs".to_string(),
            env: (),
            globals: serde_json::Value::Null,
            options: (),
            request: None,
            format: incurs::output::Format::Json,
            format_explicit: true,
            name: "incurs".to_string(),
            vars: serde_json::Value::Null,
            version: None,
        }
    }
}
