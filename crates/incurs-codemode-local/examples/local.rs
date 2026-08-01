use std::sync::Arc;

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::CommandResult;
use incurs_codemode::{CodeMode, IncurConnector, MemoryStore};
use incurs_codemode_local::LocalExecutor;
use serde::Deserialize;

struct Sum;

#[async_trait::async_trait]
impl CommandHandler for Sum {
    async fn run(&self, context: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: serde_json::json!({
                "sum": context.options["left"].as_i64().unwrap_or_default()
                    + context.options["right"].as_i64().unwrap_or_default()
            }),
            cta: None,
            exit_code: None,
        }
    }
}

#[derive(incurs::Options, Deserialize)]
#[allow(dead_code)]
struct SumOptions {
    /// Left operand.
    left: i64,
    /// Right operand.
    right: i64,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let catalog = Cli::create("math")
        .command(
            "sum",
            CommandDef::build("sum", Sum)
                .description("Add two integers.")
                .options::<SumOptions>()
                .mcp(McpCommandOptions {
                    annotations: Some(McpAnnotations {
                        read_only_hint: Some(true),
                        ..McpAnnotations::default()
                    }),
                    ..McpCommandOptions::default()
                })
                .done(),
        )
        .tool_catalog();
    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        LocalExecutor::default(),
        vec![Arc::new(IncurConnector::new(catalog))],
    );
    let code = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "math.sum({ left: 2, right: 3 })".to_string());
    let execution = codemode.execute(&code).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&execution).map_err(|error| error.to_string())?
    );
    Ok(())
}
