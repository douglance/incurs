//! The README quick start, as an example the build compiles.
//!
//! Keeping it here rather than only in Markdown means the first code a reader
//! sees cannot silently stop compiling. `cargo build --examples` covers it.
//!
//! ```sh
//! cargo run -p incurs --example greet -- greet Ada --excited --json
//! ```

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
        .version(env!("CARGO_PKG_VERSION"))
        .description("A greeting CLI")
        .command("greet", greet)
        .serve()
        .await
}
