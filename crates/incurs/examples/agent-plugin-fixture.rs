use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(JsonSchema, Serialize)]
struct PingOutput {
    message: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ping = CommandDef::typed::<(), (), (), PingOutput, _, _>(
        "ping",
        |_ctx: TypedContext<(), (), ()>| async move {
            TypedResult::ok(PingOutput {
                message: "pong".to_string(),
            })
        },
    )
    .description("Return a deterministic pong response")
    .done();

    Cli::create("agent-plugin-fixture")
        .version("0.1.0")
        .description("Agent Plugins stdio conformance fixture")
        .command("ping", ping)
        .serve()
        .await
}
