//! A todo-list CLI built with incurs.
//!
//! Demonstrates: commands, args, options, streaming, middleware, groups,
//! and all built-in flags (--help, --version, --json, --format, --filter-output).
//!
//! Usage:
//!   cargo run -p incurs --example todoapp -- --help
//!   cargo run -p incurs --example todoapp -- add "Buy groceries" --priority high
//!   cargo run -p incurs --example todoapp -- list
//!   cargo run -p incurs --example todoapp -- list --status done
//!   cargo run -p incurs --example todoapp -- get 1
//!   cargo run -p incurs --example todoapp -- complete 1
//!   cargo run -p incurs --example todoapp -- stats
//!   cargo run -p incurs --example todoapp -- stream
//!   cargo run -p incurs --example todoapp -- list --json
//!   cargo run -p incurs --example todoapp -- list --format yaml
//!   cargo run -p incurs --example todoapp -- --version

use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler, Example};
use incurs::middleware::{BoxFuture, MiddlewareContext, MiddlewareFn, MiddlewareNext};
use incurs::output::{CommandResult, CtaBlock, CtaEntry};
use incurs::schema::{FieldMeta, FieldType};
use serde_json::{json, Value};
use std::pin::Pin;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for `add <title> [--priority <low|medium|high>]`
struct AddHandler;

#[async_trait::async_trait]
impl CommandHandler for AddHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let title = ctx.args.get("title").and_then(|v| v.as_str()).unwrap_or("untitled");
        let priority = ctx
            .options
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");

        CommandResult::Ok {
            data: json!({
                "id": 42,
                "title": title,
                "priority": priority,
                "status": "pending"
            }),
            cta: Some(CtaBlock {
                commands: vec![
                    CtaEntry::Simple("list".to_string()),
                    CtaEntry::Detailed {
                        command: "get 42".to_string(),
                        description: Some("View the new todo".to_string()),
                    },
                ],
                description: Some("Next steps:".to_string()),
            }),
        }
    }
}

/// Handler for `list [--status <pending|done|all>]`
struct ListHandler;

#[async_trait::async_trait]
impl CommandHandler for ListHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let status = ctx
            .options
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        let todos = vec![
            json!({"id": 1, "title": "Buy groceries", "priority": "high", "status": "pending"}),
            json!({"id": 2, "title": "Write docs", "priority": "medium", "status": "done"}),
            json!({"id": 3, "title": "Fix bug #123", "priority": "high", "status": "pending"}),
            json!({"id": 4, "title": "Review PR", "priority": "low", "status": "done"}),
            json!({"id": 5, "title": "Deploy v2", "priority": "medium", "status": "pending"}),
        ];

        let filtered: Vec<Value> = if status == "all" {
            todos
        } else {
            todos
                .into_iter()
                .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some(status))
                .collect()
        };

        CommandResult::Ok {
            data: Value::Array(filtered),
            cta: None,
        }
    }
}

/// Handler for `get <id>`
struct GetHandler;

#[async_trait::async_trait]
impl CommandHandler for GetHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let id = ctx
            .args
            .get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if id == 0 || id > 5 {
            return CommandResult::Error {
                code: "NOT_FOUND".to_string(),
                message: format!("Todo #{} not found", id),
                retryable: false,
                exit_code: Some(1),
                cta: Some(CtaBlock {
                    commands: vec![CtaEntry::Simple("list".to_string())],
                    description: Some("Try listing all todos:".to_string()),
                }),
            };
        }

        CommandResult::Ok {
            data: json!({
                "id": id,
                "title": format!("Todo #{}", id),
                "priority": "medium",
                "status": "pending",
                "created_at": "2026-03-21T12:00:00Z"
            }),
            cta: None,
        }
    }
}

/// Handler for `complete <id>`
struct CompleteHandler;

#[async_trait::async_trait]
impl CommandHandler for CompleteHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let id = ctx
            .args
            .get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        CommandResult::Ok {
            data: json!({
                "id": id,
                "status": "done",
                "completed_at": "2026-03-21T15:30:00Z"
            }),
            cta: None,
        }
    }
}

/// Handler for `stats` — returns aggregate statistics
struct StatsHandler;

#[async_trait::async_trait]
impl CommandHandler for StatsHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({
                "total": 5,
                "pending": 3,
                "done": 2,
                "by_priority": {
                    "high": 2,
                    "medium": 2,
                    "low": 1
                }
            }),
            cta: None,
        }
    }
}

/// Handler for `stream` — demonstrates async streaming output
struct StreamHandler;

#[async_trait::async_trait]
impl CommandHandler for StreamHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let s: Pin<Box<dyn Stream<Item = Value> + Send>> = Box::pin(stream! {
            for i in 1..=5 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                yield json!({
                    "event": "progress",
                    "step": i,
                    "total": 5,
                    "message": format!("Processing batch {}...", i)
                });
            }
            yield json!({
                "event": "complete",
                "message": "All batches processed successfully"
            });
        });
        CommandResult::Stream(s)
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Logging middleware that prints request info (for humans) before each command.
fn logging_middleware() -> MiddlewareFn {
    Arc::new(
        |ctx: MiddlewareContext, next: MiddlewareNext| -> BoxFuture<()> {
            Box::pin(async move {
                if !ctx.agent {
                    eprintln!("[todoapp] running `{}`", ctx.command);
                }
                next().await;
            })
        },
    )
}

// ---------------------------------------------------------------------------
// CLI construction
// ---------------------------------------------------------------------------

fn build_cli() -> Cli {
    Cli::create("todoapp")
        .description("A simple todo list manager")
        .version("0.1.0")
        .use_middleware(logging_middleware())
        // --- add command ---
        .command(
            "add",
            CommandDef {
                name: "add".to_string(),
                description: Some("Add a new todo item".to_string()),
                args_fields: vec![FieldMeta {
                    name: "title",
                    cli_name: "title".to_string(),
                    description: Some("The todo title"),
                    field_type: FieldType::String,
                    required: true,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                options_fields: vec![FieldMeta {
                    name: "priority",
                    cli_name: "priority".to_string(),
                    description: Some("Priority level"),
                    field_type: FieldType::Enum(vec![
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                    ]),
                    required: false,
                    default: Some(json!("medium")),
                    alias: Some('p'),
                    deprecated: false,
                    env_name: None,
                }],
                env_fields: vec![],
                aliases: HashMap::from([("priority".to_string(), 'p')]),
                examples: vec![
                    Example {
                        command: "\"Buy groceries\"".to_string(),
                        description: Some("Add with default priority".to_string()),
                    },
                    Example {
                        command: "\"Fix bug\" --priority high".to_string(),
                        description: Some("Add with high priority".to_string()),
                    },
                    Example {
                        command: "\"Read book\" -p low".to_string(),
                        description: Some("Add with short alias".to_string()),
                    },
                ],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(AddHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        // --- list command ---
        .command(
            "list",
            CommandDef {
                name: "list".to_string(),
                description: Some("List todo items".to_string()),
                args_fields: vec![],
                options_fields: vec![
                    FieldMeta {
                        name: "status",
                        cli_name: "status".to_string(),
                        description: Some("Filter by status"),
                        field_type: FieldType::Enum(vec![
                            "all".to_string(),
                            "pending".to_string(),
                            "done".to_string(),
                        ]),
                        required: false,
                        default: Some(json!("all")),
                        alias: Some('s'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "limit",
                        cli_name: "limit".to_string(),
                        description: Some("Maximum number of results"),
                        field_type: FieldType::Number,
                        required: false,
                        default: Some(json!(50)),
                        alias: Some('n'),
                        deprecated: false,
                        env_name: None,
                    },
                ],
                env_fields: vec![],
                aliases: HashMap::from([
                    ("status".to_string(), 's'),
                    ("limit".to_string(), 'n'),
                ]),
                examples: vec![
                    Example {
                        command: "".to_string(),
                        description: Some("List all todos".to_string()),
                    },
                    Example {
                        command: "--status pending".to_string(),
                        description: Some("List only pending".to_string()),
                    },
                    Example {
                        command: "--json".to_string(),
                        description: Some("Output as JSON".to_string()),
                    },
                ],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ListHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        // --- get command ---
        .command(
            "get",
            CommandDef {
                name: "get".to_string(),
                description: Some("Get a todo by ID".to_string()),
                args_fields: vec![FieldMeta {
                    name: "id",
                    cli_name: "id".to_string(),
                    description: Some("The todo ID"),
                    field_type: FieldType::Number,
                    required: true,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![Example {
                    command: "1".to_string(),
                    description: Some("Get todo #1".to_string()),
                }],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(GetHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        // --- complete command ---
        .command(
            "complete",
            CommandDef {
                name: "complete".to_string(),
                description: Some("Mark a todo as done".to_string()),
                args_fields: vec![FieldMeta {
                    name: "id",
                    cli_name: "id".to_string(),
                    description: Some("The todo ID to complete"),
                    field_type: FieldType::Number,
                    required: true,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![Example {
                    command: "1".to_string(),
                    description: Some("Complete todo #1".to_string()),
                }],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(CompleteHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        // --- stats command ---
        .command(
            "stats",
            CommandDef {
                name: "stats".to_string(),
                description: Some("Show todo statistics".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StatsHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        // --- stream command ---
        .command(
            "stream",
            CommandDef {
                name: "stream".to_string(),
                description: Some("Stream progress updates (demo)".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: Some("Streams 5 progress events with 300ms delays.".to_string()),
                format: None,
                output_policy: None,
                handler: Box::new(StreamHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
}

#[tokio::main]
async fn main() {
    let cli = build_cli();
    if let Err(e) = cli.serve().await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
