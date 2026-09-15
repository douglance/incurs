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
    /// Render an excited greeting.
    excited: bool,
}

#[derive(JsonSchema, Serialize)]
struct GreetOutput {
    message: String,
}

#[tokio::test]
async fn typed_command_parses_and_serializes_across_shared_execution() {
    let command = CommandDef::typed::<GreetArgs, GreetOptions, (), GreetOutput, _, _>(
        "greet",
        |ctx: TypedContext<GreetArgs, GreetOptions, ()>| async move {
            TypedResult::ok(GreetOutput {
                message: format!(
                    "Hello, {}{}",
                    ctx.args.name,
                    if ctx.options.excited { "!" } else { "." }
                ),
            })
        },
    )
    .description("Greet someone")
    .done();
    assert_eq!(command.output_schema.as_ref().unwrap()["type"], "object");

    let cli = Cli::create("greet").command("greet", command);
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec![
                "greet".to_string(),
                "Ada".to_string(),
                "--excited".to_string(),
                "--json".to_string(),
            ],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, None);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({ "message": "Hello, Ada!" })
    );
}

#[derive(JsonSchema, Serialize)]
struct WrappedOutput {
    exit_code: i32,
}

/// A command that wraps a subprocess reports its status without being an error.
#[tokio::test]
async fn typed_ok_can_report_a_wrapped_process_exit_code() {
    let command = CommandDef::typed::<(), (), (), WrappedOutput, _, _>("wrap", |_| async move {
        TypedResult::ok_with_exit_code(WrappedOutput { exit_code: 37 }, 37)
    })
    .description("Run a wrapped process")
    .done();

    let cli = Cli::create("wrap").command("wrap", command);
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["wrap".to_string(), "--json".to_string()],
            &mut output,
            false,
        )
        .await
        .unwrap();

    // The exit status reaches the caller, and the payload is still success
    // shaped rather than an error envelope.
    assert_eq!(exit, Some(37));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({ "exit_code": 37 })
    );
}

/// The default success constructor must not change the exit status.
#[tokio::test]
async fn typed_ok_without_exit_code_leaves_status_untouched() {
    let command = CommandDef::typed::<(), (), (), WrappedOutput, _, _>("wrap", |_| async move {
        TypedResult::ok(WrappedOutput { exit_code: 0 })
    })
    .description("Run a wrapped process")
    .done();

    let cli = Cli::create("wrap").command("wrap", command);
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["wrap".to_string(), "--json".to_string()],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, None);
}

/// A boolean option must parse when its flag is absent.
///
/// A `bool` field is not required, so omitting the flag is ordinary usage. It
/// nevertheless failed to deserialize, because the derive declared no default
/// and `bool` has no serde default of its own. Every command with an optional
/// flag was therefore unusable without passing it.
#[tokio::test]
async fn an_absent_boolean_flag_parses_as_false() {
    let command = CommandDef::typed::<GreetArgs, GreetOptions, (), GreetOutput, _, _>(
        "greet",
        |ctx: TypedContext<GreetArgs, GreetOptions, ()>| async move {
            TypedResult::ok(GreetOutput {
                message: format!(
                    "Hello, {}{}",
                    ctx.args.name,
                    if ctx.options.excited { "!" } else { "." }
                ),
            })
        },
    )
    .description("Greet someone")
    .done();

    let cli = Cli::create("greet").command("greet", command);
    let mut output = Vec::new();
    let exit = cli
        .serve_to(
            vec!["greet".to_string(), "Ada".to_string(), "--json".to_string()],
            &mut output,
            false,
        )
        .await
        .unwrap();

    assert_eq!(exit, None, "omitting an optional flag is not an error");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({ "message": "Hello, Ada." })
    );
}

/// The declared default reaches the published schema, not only the parser.
///
/// Consumers that never run the parser — MCP clients, config-file authors,
/// generated code, form controls — learn that a flag is off by default only
/// from the schema.
#[tokio::test]
async fn a_boolean_option_publishes_its_default() {
    let command = CommandDef::typed::<GreetArgs, GreetOptions, (), GreetOutput, _, _>(
        "greet",
        |_: TypedContext<GreetArgs, GreetOptions, ()>| async move {
            TypedResult::ok(GreetOutput {
                message: String::new(),
            })
        },
    )
    .description("Greet someone")
    .done();

    let cli = Cli::create("greet").command("greet", command);
    let mut output = Vec::new();
    cli.serve_to(
        vec![
            "greet".to_string(),
            "--schema".to_string(),
            "--json".to_string(),
        ],
        &mut output,
        false,
    )
    .await
    .unwrap();

    let schema: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(schema["options"]["properties"]["excited"]["default"], false);
}
