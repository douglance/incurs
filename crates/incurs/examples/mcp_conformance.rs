//! Minimal HTTP server used by the upstream MCP conformance suite.

use std::net::SocketAddr;

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use serde_json::json;

struct Echo;

#[async_trait::async_trait]
impl CommandHandler for Echo {
    async fn run(&self, context: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({
                "args": context.args,
                "options": context.options,
            }),
            cta: None,
            exit_code: None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("INCURS_MCP_CONFORMANCE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3030".to_string())
        .parse::<SocketAddr>()?;
    let cli = Cli::create("incurs-conformance").command(
        "echo",
        CommandDef::build("echo", Echo)
            .description("Echo validated arguments and options")
            .done(),
    );
    incurs::http::serve_http(&cli, address).await
}
