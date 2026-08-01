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
