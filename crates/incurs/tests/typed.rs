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
