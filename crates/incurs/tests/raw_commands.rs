//! Raw commands, default subcommands, and hidden commands.
//!
//! A raw command wraps another program: it must see argv exactly as typed,
//! framework flags included, and the framework must not print around it.

use std::collections::BTreeMap;

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use incurs::tool::{ToolCallOptions, ToolCallOutcome};
use serde_json::{Value, json};

/// Returns the argv the framework handed over, so tests can compare it with
/// what was typed.
struct EchoArgv;

#[async_trait::async_trait]
impl CommandHandler for EchoArgv {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: ctx.args,
            cta: None,
            exit_code: None,
        }
    }
}

/// Stands in for a handler that already wrote to the terminal: no data, and
/// the wrapped program's exit code.
struct AlreadyRendered(i32);

#[async_trait::async_trait]
impl CommandHandler for AlreadyRendered {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: Value::Null,
            cta: None,
            exit_code: Some(self.0),
        }
    }
}

fn raw(name: &str) -> CommandDef {
    CommandDef::build(name, EchoArgv)
        .description(format!("Raw {name}"))
        .raw()
        .done()
}

fn strings(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| token.to_string()).collect()
}

async fn run(cli: &Cli, argv: &[&str]) -> (String, Option<i32>) {
    let mut output = Vec::new();
    let code = cli
        .serve_to(strings(argv), &mut output, false)
        .await
        .expect("serve_to");
    (String::from_utf8(output).expect("utf8"), code)
}

async fn run_json(cli: &Cli, argv: &[&str]) -> Value {
    let (output, code) = run(cli, argv).await;
    assert_eq!(code, None, "unexpected exit code, output: {output}");
    serde_json::from_str(&output).unwrap_or_else(|_| panic!("not JSON: {output}"))
}

#[tokio::test]
async fn raw_command_receives_framework_flags_unchanged() {
    let cli = Cli::create("app").command("gen", raw("gen"));
    let argv = [
        "gen",
        "--json",
        "--help",
        "--format",
        "dot",
        "-x",
        "--",
        "tail",
    ];
    assert_eq!(run_json(&cli, &argv).await, json!({ "argv": argv }));
}

#[tokio::test]
async fn raw_command_with_null_result_prints_nothing_and_passes_the_exit_code() {
    let command = CommandDef::build("build", AlreadyRendered(64))
        .raw()
        .done();
    let cli = Cli::create("app").command("build", command);
    assert_eq!(
        run(&cli, &["build", "--bogus"]).await,
        (String::new(), Some(64))
    );
}

#[tokio::test]
async fn default_subcommand_takes_unmatched_tokens_without_consuming_them() {
    let test = Cli::create("test")
        .description("Tests")
        .default_command("run")
        .command("run", raw("run"))
        .command("list", raw("list"));
    let cli = Cli::create("app").group(test);

    assert_eq!(
        run_json(&cli, &["test", "Foo", "--retry", "2"]).await,
        json!({ "argv": ["test", "Foo", "--retry", "2"] })
    );
    assert_eq!(
        run_json(&cli, &["test"]).await,
        json!({ "argv": ["test"] })
    );
    assert_eq!(
        run_json(&cli, &["test", "list", "--json"]).await,
        json!({ "argv": ["test", "list", "--json"] })
    );
}

#[tokio::test]
async fn raw_root_takes_unknown_commands_and_unknown_leading_flags() {
    let cli = Cli::create("app")
        .root(raw("app"))
        .command("gen", raw("gen"));

    assert_eq!(
        run_json(&cli, &["plugin-task", "a"]).await,
        json!({ "argv": ["plugin-task", "a"] })
    );
    assert_eq!(run_json(&cli, &[]).await, json!({ "argv": [] }));
    assert_eq!(
        run_json(&cli, &["--verbose", "gen"]).await,
        json!({ "argv": ["--verbose", "gen"] })
    );
    assert_eq!(
        run_json(&cli, &["--help"]).await,
        json!({ "argv": ["--help"] })
    );
    assert_eq!(
        run_json(&cli, &["--version"]).await,
        json!({ "argv": ["--version"] })
    );
}

#[tokio::test]
async fn raw_root_takes_groups_without_a_subcommand_and_unknown_subcommands() {
    let auth = Cli::create("auth")
        .description("Auth")
        .command("login", raw("login"));
    let cli = Cli::create("app").root(raw("app")).group(auth);

    assert_eq!(
        run_json(&cli, &["auth"]).await,
        json!({ "argv": ["auth"] })
    );
    assert_eq!(
        run_json(&cli, &["auth", "logn", "-x"]).await,
        json!({ "argv": ["auth", "logn", "-x"] })
    );
    assert_eq!(
        run_json(&cli, &["auth", "login", "-x"]).await,
        json!({ "argv": ["auth", "login", "-x"] })
    );
}

#[tokio::test]
async fn raw_root_leaves_framework_flags_and_builtin_commands_to_the_framework() {
    let cli = Cli::create("app")
        .description("An app")
        .root(raw("app"))
        .command("gen", raw("gen"));

    let (llms, code) = run(&cli, &["--llms"]).await;
    assert_eq!(code, None);
    assert!(llms.contains("gen"), "manifest should list gen: {llms}");
    assert!(!llms.contains("\"argv\""), "root must not run: {llms}");


    let (completions, _) = run(&cli, &["completions", "bash"]).await;
    assert!(
        !completions.contains("\"argv\""),
        "builtin completions must not reach the root: {completions}"
    );
}

#[tokio::test]
async fn default_subcommand_applies_to_ordinary_commands() {
    let generate = Cli::create("generate")
        .default_command("run")
        .command("run", CommandDef::build("run", EchoArgv).done());
    let cli = Cli::create("app").group(generate);
    // A non-raw default runs through the normal parser, so it sees parsed
    // args rather than argv.
    let value = run_json(&cli, &["generate", "--json"]).await;
    assert!(value.get("argv").is_none(), "{value}");
}

#[tokio::test]
async fn hidden_commands_run_but_are_not_listed() {
    let cli = Cli::create("app")
        .command("shown", raw("shown"))
        .command(
            "secret",
            CommandDef::build("secret", EchoArgv)
                .description("Internal plumbing")
                .raw()
                .hidden()
                .done(),
        );

    let (help, _) = run(&cli, &["--help"]).await;
    assert!(help.contains("shown"), "{help}");
    assert!(!help.contains("secret"), "{help}");

    let (llms, _) = run(&cli, &["--llms"]).await;
    assert!(llms.contains("shown"), "{llms}");
    assert!(!llms.contains("secret"), "{llms}");

    let names: Vec<String> = cli
        .tool_catalog()
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    assert_eq!(names, vec!["shown".to_string()]);

    assert_eq!(
        run_json(&cli, &["secret", "x"]).await,
        json!({ "argv": ["secret", "x"] })
    );
}

#[tokio::test]
async fn tool_call_prefixes_the_command_path_to_the_arguments() {
    let test = Cli::create("test")
        .default_command("run")
        .command("run", raw("run"));
    let cli = Cli::create("app").command("gen", raw("gen")).group(test);
    let catalog = cli.tool_catalog();

    let definition = catalog.get("test_run").expect("test_run tool");
    assert_eq!(
        definition.input_schema["properties"]["arguments"]["type"],
        json!("array")
    );

    let mut arguments = BTreeMap::new();
    arguments.insert("arguments".to_string(), json!(["--json", "Foo"]));
    match catalog
        .call("test_run", arguments, ToolCallOptions::isolated())
        .await
    {
        ToolCallOutcome::Ok { data, .. } => {
            assert_eq!(data, json!({ "argv": ["test", "run", "--json", "Foo"] }))
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    match catalog
        .call("gen", BTreeMap::new(), ToolCallOptions::isolated())
        .await
    {
        ToolCallOutcome::Ok { data, .. } => assert_eq!(data, json!({ "argv": ["gen"] })),
        other => panic!("unexpected outcome: {other:?}"),
    }

    let mut bad = BTreeMap::new();
    bad.insert("arguments".to_string(), json!([1, 2]));
    match catalog.call("gen", bad, ToolCallOptions::isolated()).await {
        ToolCallOutcome::Error { code, .. } => assert_eq!(code, "VALIDATION_ERROR"),
        other => panic!("unexpected outcome: {other:?}"),
    }
}
