//! The upstream MCP conformance fixture served by the runtime-free
//! [`incurs::mcp::McpHttpServer`] through [`incurs::http::mcp_router`], the
//! same route a Cloudflare Worker serves.
//!
//! Serves the same `echo` command as `mcp_conformance`, so both examples run
//! against the same conformance baseline.

use std::net::SocketAddr;

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::mcp::{McpHttpConfig, McpHttpServer};
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
        .unwrap_or_else(|_| "127.0.0.1:3032".to_string())
        .parse::<SocketAddr>()?;
    let cli = Cli::create("incurs-conformance").command(
        "echo",
        CommandDef::build("echo", Echo)
            .description("Echo validated arguments and options")
            .done(),
    );
    let router = incurs::http::mcp_router(McpHttpServer::from_cli(&cli, McpHttpConfig::default())?);
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
