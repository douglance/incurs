//! `incurs` — the command-line tool for building incurs command graphs.
//!
//! This CLI is defined with incurs itself. Every command below is a
//! [`incurs::command::CommandDef`], so `--help`, `--schema`, `--llms`,
//! `--llms-full`, `--config-schema`, `skills`, shell completions, MCP, and the
//! structured output envelope are all derived from one definition rather than
//! implemented here.
//!
//! That is not decoration. A framework whose claim is "define a command once
//! and expose it everywhere" should not ship a tool that defines its commands
//! by hand and exposes them exactly once, and the duplicate `kebab` helper this
//! file used to carry — which left `_` untouched and so generated `--dry_run`
//! for any underscored key — is what that costs.

use std::sync::Arc;

use incurs::cli::{Cli, ConfigOptions};
use incurs::command::{CommandDef, Example, McpAnnotations, McpCommandOptions};
use incurs::middleware::{BoxFuture, MiddlewareContext, MiddlewareFn, MiddlewareNext};
use incurs::output::Format;

mod explain;
mod generate;
mod plugin;
mod plugin_install;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `--tui` is recognised only as the first argument, so `incurs gen --tui`
    // stays an ordinary unknown-option error rather than silently opening a
    // full-screen application.
    #[cfg(feature = "tui")]
    if std::env::args().nth(1).as_deref() == Some("--tui") {
        return Ok(incurs_app_ratatui::TerminalApp::from_cli(&build_cli())?.run()?);
    }

    // Deliberately not `#[tokio::main]`. The terminal surface builds its own
    // Tokio runtime, and Tokio panics when a runtime is dropped inside another.
    tokio::runtime::Runtime::new()?.block_on(build_cli().serve())
}

/// Builds the `incurs` command graph.
///
/// Exposed so tests can drive the same definitions the binary serves, through
/// [`incurs::cli::Cli::serve_to`], without spawning a process.
pub fn build_cli() -> Cli {
    Cli::create("incurs")
        .version(env!("CARGO_PKG_VERSION"))
        .description("Build, generate, and package incurs command graphs")
        // Config support is what makes `--config-schema` meaningful: a project
        // can pin `incurs gen` defaults beside its source instead of repeating
        // flags in every script.
        .config(ConfigOptions {
            flag: "config".to_string(),
            files: vec![
                "incurs.config.json".to_string(),
                ".incursrc.json".to_string(),
            ],
        })
        .use_middleware(tracing_middleware())
        .command("gen", gen_command())
        .command("explain", explain_command())
        .group(plugin_group())
}

/// Reports which command ran, for a person watching a slow generation.
///
/// Writes only when the consumer is not an agent, so structured output stays
/// exactly what the caller parses.
fn tracing_middleware() -> MiddlewareFn {
    Arc::new(
        |ctx: MiddlewareContext, next: MiddlewareNext| -> BoxFuture<()> {
            Box::pin(async move {
                if !ctx.agent {
                    eprintln!("[incurs] running `{}`", ctx.command);
                }
                next().await;
            })
        },
    )
}

/// `incurs gen` — generate Rust command types and JSON manifests.
fn gen_command() -> CommandDef {
    CommandDef::typed::<(), generate::GenOptions, (), generate::GenOutput, _, _>(
        "gen",
        generate::run,
    )
    .description("Generate Rust command types and JSON manifests")
    .hint("Run this from the Cargo project whose CLI you want typed helpers for.")
    .examples(vec![
        Example {
            command: "--dir ./my-cli".to_string(),
            description: Some("Generate into a project directory".to_string()),
        },
        Example {
            command: "--entry target/debug/my-cli".to_string(),
            description: Some("Use a built executable instead of cargo run".to_string()),
        },
        Example {
            command: "--config-schema".to_string(),
            description: Some("Also write config.schema.json".to_string()),
        },
    ])
    // Generation output is consumed by scripts and build steps, so it stays
    // JSON by default rather than the human-facing default format.
    .format(Format::Json)
    .done()
}

/// `incurs explain` — the Rust authoring reference.
fn explain_command() -> CommandDef {
    CommandDef::typed::<explain::ExplainArgs, (), (), explain::ExplainOutput, _, _>(
        "explain",
        explain::run,
    )
    .description("Explain how to build an incurs CLI in Rust")
    .hint("Run without a topic to list every topic.")
    .examples(vec![
        Example {
            command: String::new(),
            description: Some("List every topic".to_string()),
        },
        Example {
            command: "typed-commands".to_string(),
            description: Some("Read one topic".to_string()),
        },
    ])
    .done()
}

/// `incurs plugin ...` — Agent Plugins 1.0 client commands.
fn plugin_group() -> Cli {
    Cli::create("plugin")
        .description("Install, validate, connect, or call an Agent Plugins 1.0 directory")
        .command(
            "install",
            CommandDef::typed::<
                plugin::InstallArgs,
                plugin::InstallOptions,
                (),
                plugin_install::InstallResult,
                _,
                _,
            >("install", plugin::install)
            .description("Install a portable plugin and its declared shell command")
            .examples(vec![Example {
                command: "./demo-plugin".to_string(),
                description: Some("Install into the managed command directory".to_string()),
            }])
            .format(Format::Json)
            .done(),
        )
        .command(
            "uninstall",
            CommandDef::typed::<
                plugin::UninstallArgs,
                plugin::UninstallOptions,
                (),
                plugin_install::UninstallResult,
                _,
                _,
            >("uninstall", plugin::uninstall)
            .description("Remove installer-owned plugin and command artifacts")
            .examples(vec![Example {
                command: "demo-tools --purge".to_string(),
                description: Some("Remove a plugin and its persistent data".to_string()),
            }])
            // Uninstall deletes installed artifacts, so every surface must be
            // able to warn before running it. Two fields carry that one fact
            // today: `destructive` gates skill confirmation, and the MCP
            // `destructive_hint` annotation is what MCP clients read. Setting
            // only one leaves the other surface unwarned.
            .mcp(McpCommandOptions {
                destructive: true,
                annotations: Some(McpAnnotations {
                    title: Some("Uninstall Agent Plugin".to_string()),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                ..Default::default()
            })
            .format(Format::Json)
            .done(),
        )
        .command(
            "validate",
            CommandDef::typed::<
                plugin::PluginArgs,
                plugin::LoadOptions,
                (),
                plugin::ValidateOutput,
                _,
                _,
            >("validate", plugin::validate)
            .description("Load all fixed components and print diagnostics")
            .examples(vec![Example {
                command: "./demo-plugin --data-dir ./plugin-data".to_string(),
                description: Some("Validate without connecting any server".to_string()),
            }])
            .format(Format::Json)
            .done(),
        )
        .command(
            "tools",
            CommandDef::typed::<
                plugin::PluginArgs,
                plugin::LoadOptions,
                (),
                plugin::ToolsOutput,
                _,
                _,
            >("tools", plugin::tools)
            .description("Connect every valid MCP server and print namespaced tools")
            .examples(vec![Example {
                command: "./demo-plugin --data-dir ./plugin-data".to_string(),
                description: Some("List every tool the plugin exposes".to_string()),
            }])
            .format(Format::Json)
            .done(),
        )
        .command(
            "call",
            CommandDef::typed::<plugin::CallArgs, plugin::CallOptions, (), serde_json::Value, _, _>(
                "call",
                plugin::call,
            )
            .description("Invoke one namespaced tool with flat JSON arguments")
            .examples(vec![Example {
                command: "./demo-plugin server_ping --data-dir ./plugin-data".to_string(),
                description: Some("Call a tool on a loaded plugin".to_string()),
            }])
            .format(Format::Json)
            .done(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one argv against the real command graph and returns stdout.
    async fn observe(argv: &[&str]) -> (Option<i32>, String) {
        let cli = build_cli();
        let argv = argv.iter().map(|token| token.to_string()).collect();
        let mut output = Vec::new();
        let exit = cli
            .serve_to(argv, &mut output, false)
            .await
            .expect("serve_to should not return Err");
        (exit, String::from_utf8(output).expect("valid UTF-8"))
    }

    /// Every command this CLI declares is reachable and documented.
    ///
    /// This replaces the hand-rolled `print_help` string. Help is now derived
    /// from the definitions, so a command that exists but is undocumented — or
    /// documented but removed — fails here.
    #[tokio::test]
    async fn help_lists_every_command() {
        let (_, output) = observe(&["--help"]).await;

        for command in ["gen", "plugin"] {
            assert!(output.contains(command), "help must list `{command}`");
        }
    }

    /// The plugin group documents each of its five commands.
    #[tokio::test]
    async fn plugin_help_lists_every_action() {
        let (_, output) = observe(&["plugin", "--help"]).await;

        for command in ["install", "uninstall", "validate", "tools", "call"] {
            assert!(
                output.contains(command),
                "plugin help must list `{command}`"
            );
        }
    }

    /// `gen` still accepts every Agent Plugin option the hand-rolled parser did.
    ///
    /// The old `parse_gen_accepts_agent_plugin_options` asserted this against a
    /// parser that no longer exists. Asserting it through `--schema` checks the
    /// same contract against the definition the CLI actually serves.
    #[tokio::test]
    async fn gen_declares_every_agent_plugin_option() {
        let (_, output) = observe(&["gen", "--schema", "--json"]).await;
        let schema: serde_json::Value = serde_json::from_str(&output).expect("schema is JSON");
        let properties = schema["options"]["properties"]
            .as_object()
            .expect("gen declares options");

        for option in [
            "dir",
            "entry",
            "output",
            "json-output",
            "config-schema",
            "plugin-output",
            "plugin-skill-depth",
            "plugin-no-mcp",
            "plugin-bundle-cli",
            "plugin-force",
        ] {
            assert!(
                properties.contains_key(option),
                "gen must declare `--{option}`, got {:?}",
                properties.keys().collect::<Vec<_>>()
            );
        }
    }

    /// An option the tool never had is still rejected.
    ///
    /// Replaces `parse_gen_rejects_removed_agent_plugin_metadata_options`. The
    /// message now comes from the shared parser rather than a local `format!`,
    /// so this asserts the flag is refused, not the exact wording.
    #[tokio::test]
    async fn gen_rejects_an_unknown_option() {
        let (exit, output) = observe(&["gen", "--plugin-name", "demo", "--json"]).await;

        assert!(
            exit.is_some(),
            "an unknown option must not exit successfully, got: {output}"
        );
        assert!(
            output.contains("plugin-name"),
            "the rejection must name the offending option, got: {output}"
        );
    }

    /// Loading a plugin requires a dedicated data directory.
    ///
    /// Replaces `parse_plugin_requires_a_dedicated_data_directory`.
    #[tokio::test]
    async fn plugin_validate_requires_a_data_directory() {
        let (exit, output) = observe(&["plugin", "validate", "./demo", "--json"]).await;

        assert!(
            exit.is_some(),
            "a missing required option must fail, got: {output}"
        );
        assert!(
            output.contains("data-dir") || output.contains("data_dir"),
            "the failure must name the missing option, got: {output}"
        );
    }

    /// `plugin call` declares both positional arguments and its options.
    ///
    /// Replaces `parse_plugin_accepts_call_arguments`.
    #[tokio::test]
    async fn plugin_call_declares_its_arguments() {
        let (_, output) = observe(&["plugin", "call", "--schema", "--json"]).await;
        let schema: serde_json::Value = serde_json::from_str(&output).expect("schema is JSON");

        assert!(schema["args"]["properties"]["path"].is_object());
        assert!(schema["args"]["properties"]["tool"].is_object());
        assert!(schema["options"]["properties"]["arguments"].is_object());
        assert!(schema["options"]["properties"]["data-dir"].is_object());
    }

    /// Uninstall is declared destructive, so every surface can warn first.
    #[tokio::test]
    async fn uninstall_is_declared_destructive() {
        let catalog = build_cli().tool_catalog();
        let uninstall = catalog
            .get("plugin_uninstall")
            .expect("plugin uninstall is exposed as a tool");

        assert_eq!(
            uninstall
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.destructive_hint),
            Some(true),
            "MCP clients read destructive_hint"
        );
    }

    /// The whole graph is reachable as tools, not only as a CLI.
    ///
    /// This is the property the rebuild exists to establish: the same
    /// definitions serve MCP and Code Mode with no additional work.
    #[tokio::test]
    async fn every_command_is_exposed_as_a_tool() {
        let catalog = build_cli().tool_catalog();
        let names: Vec<String> = catalog
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();

        for expected in [
            "gen",
            "plugin_install",
            "plugin_uninstall",
            "plugin_validate",
            "plugin_tools",
            "plugin_call",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "`{expected}` must be callable as a tool, got {names:?}"
            );
        }
    }

    /// `--tui` is a first-argument flag, not a global option.
    ///
    /// Anywhere else it must stay an unknown option, or a typo after a command
    /// would silently open a full-screen application instead of failing.
    #[tokio::test]
    async fn tui_is_not_accepted_after_a_command() {
        let (exit, output) = observe(&["gen", "--tui", "--json"]).await;

        assert!(exit.is_some(), "got: {output}");
        assert!(
            output.contains("tui"),
            "the rejection names it, got: {output}"
        );
    }

    /// Every command carries a description and an output schema.
    ///
    /// A typed command gets its output schema from `schemars`, so this fails
    /// only if a command was added without going through `CommandDef::typed`.
    #[tokio::test]
    async fn every_tool_declares_a_description_and_an_output_schema() {
        for definition in build_cli().tool_catalog().definitions() {
            assert!(
                !definition.description.trim().is_empty(),
                "`{}` needs a description",
                definition.name
            );
            assert!(
                definition.output_schema.is_some(),
                "`{}` needs an output schema",
                definition.name
            );
        }
    }
}
