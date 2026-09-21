//! A todo list shipped as a native desktop application.
//!
//! The command graph below is an ordinary incurs CLI. The only desktop-aware
//! line is in `main`, which hands the same graph to [`DesktopApp`] instead of
//! `Cli::serve`. Run it as a window with no arguments, or as a normal CLI by
//! passing any:
//!
//! ```sh
//! cargo run -p todo-desktop              # opens the window
//! cargo run -p todo-desktop -- list --json
//! ```

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_stream::stream;
use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::{CommandResult, CtaBlock, CtaEntry};
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_gpui::DesktopApp;
use serde_json::{Value, json};

/// One stored todo.
#[derive(Clone)]
struct Todo {
    id: u64,
    title: String,
    priority: String,
    done: bool,
}

impl Todo {
    /// Renders the todo as the structured result shape.
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "priority": self.priority,
            "done": self.done,
        })
    }
}

/// The in-memory store backing every command.
fn store() -> &'static Mutex<HashMap<u64, Todo>> {
    static STORE: OnceLock<Mutex<HashMap<u64, Todo>>> = OnceLock::new();
    STORE.get_or_init(|| {
        let mut todos = HashMap::new();
        for (id, title, priority, done) in [
            (1, "Buy groceries", "high", false),
            (2, "Renew passport", "medium", false),
            (3, "Water the plants", "low", true),
        ] {
            todos.insert(
                id,
                Todo {
                    id,
                    title: title.to_string(),
                    priority: priority.to_string(),
                    done,
                },
            );
        }
        Mutex::new(todos)
    })
}

/// Builds one optional field description.
fn field(name: &'static str, description: &'static str, field_type: FieldType) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.to_string(),
        description: Some(description),
        field_type,
        required: false,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

/// Builds one command with this example's conventions.
fn command(
    name: &str,
    description: &str,
    args_fields: Vec<FieldMeta>,
    options_fields: Vec<FieldMeta>,
    handler: Box<dyn CommandHandler>,
) -> CommandDef {
    CommandDef {
        name: name.to_string(),
        description: Some(description.to_string()),
        args_fields,
        options_fields,
        env_fields: Vec::new(),
        aliases: HashMap::new(),
        command_aliases: Vec::new(),
        examples: Vec::new(),
        hint: None,
        format: None,
        output_policy: None,
        handler,
        middleware: Vec::new(),
        output_schema: None,
        raw: false,
        hidden: false,
    }
}

/// Adds one todo.
struct AddHandler;

#[async_trait::async_trait]
impl CommandHandler for AddHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let title = ctx
            .args
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let priority = ctx
            .options
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("medium")
            .to_string();

        let mut todos = store().lock().expect("store is not poisoned");
        let id = todos.keys().copied().max().unwrap_or(0) + 1;
        let todo = Todo {
            id,
            title,
            priority,
            done: false,
        };
        todos.insert(id, todo.clone());

        CommandResult::Ok {
            data: todo.to_value(),
            cta: Some(CtaBlock {
                commands: vec![CtaEntry::Detailed {
                    command: format!("complete {id}"),
                    description: Some("Mark this todo as done".to_string()),
                }],
                description: Some("What you can do next".to_string()),
            }),
            exit_code: None,
        }
    }
}

/// Lists todos, optionally filtered.
struct ListHandler;

#[async_trait::async_trait]
impl CommandHandler for ListHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let status = ctx.options.get("status").and_then(Value::as_str);
        let limit = ctx
            .options
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);

        let todos = store().lock().expect("store is not poisoned");
        let mut items: Vec<Todo> = todos
            .values()
            .filter(|todo| match status {
                Some("done") => todo.done,
                Some("pending") => !todo.done,
                _ => true,
            })
            .cloned()
            .collect();
        items.sort_by_key(|todo| todo.id);

        CommandResult::Ok {
            data: json!({
                "count": items.len(),
                "todos": items
                    .iter()
                    .take(usize::try_from(limit).unwrap_or(usize::MAX))
                    .map(Todo::to_value)
                    .collect::<Vec<_>>(),
            }),
            cta: None,
            exit_code: None,
        }
    }
}

/// Marks one todo as done.
struct CompleteHandler;

#[async_trait::async_trait]
impl CommandHandler for CompleteHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let Some(id) = ctx.args.get("id").and_then(Value::as_u64) else {
            return CommandResult::Error {
                code: "INVALID_ID".to_string(),
                message: "That todo number is not valid.".to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        };

        let mut todos = store().lock().expect("store is not poisoned");
        match todos.get_mut(&id) {
            Some(todo) => {
                todo.done = true;
                CommandResult::Ok {
                    data: todo.to_value(),
                    cta: None,
                    exit_code: None,
                }
            }
            None => CommandResult::Error {
                code: "NOT_FOUND".to_string(),
                message: format!("No todo has the number {id}."),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

/// Deletes every completed todo.
struct ClearHandler;

impl ClearHandler {
    /// Declares the command as destructive so every surface can warn first.
    fn options() -> &'static McpCommandOptions {
        static OPTIONS: OnceLock<McpCommandOptions> = OnceLock::new();
        OPTIONS.get_or_init(|| McpCommandOptions {
            annotations: Some(McpAnnotations {
                title: Some("Clear completed".to_string()),
                destructive_hint: Some(true),
                read_only_hint: Some(false),
                idempotent_hint: Some(true),
                open_world_hint: Some(false),
            }),
            destructive: true,
            ..McpCommandOptions::default()
        })
    }
}

#[async_trait::async_trait]
impl CommandHandler for ClearHandler {
    fn mcp_options(&self) -> Option<&McpCommandOptions> {
        Some(Self::options())
    }

    async fn run(&self, _: CommandContext) -> CommandResult {
        let mut todos = store().lock().expect("store is not poisoned");
        let before = todos.len();
        todos.retain(|_, todo| !todo.done);

        CommandResult::Ok {
            data: json!({ "removed": before - todos.len(), "remaining": todos.len() }),
            cta: None,
            exit_code: None,
        }
    }
}

/// Streams a slow check so progress and streamed results are visible.
struct CheckHandler;

#[async_trait::async_trait]
impl CommandHandler for CheckHandler {
    async fn run(&self, _: CommandContext) -> CommandResult {
        let items: Vec<Todo> = {
            let todos = store().lock().expect("store is not poisoned");
            let mut items: Vec<Todo> = todos.values().cloned().collect();
            items.sort_by_key(|todo| todo.id);
            items
        };

        CommandResult::Stream(Box::pin(stream! {
            for todo in items {
                tokio::time::sleep(Duration::from_millis(600)).await;
                yield json!({
                    "id": todo.id,
                    "title": todo.title,
                    "checked": true,
                });
            }
        }))
    }
}

/// Builds the shared command graph.
fn build_cli() -> Cli {
    let add = command(
        "add",
        "Add a todo",
        vec![FieldMeta {
            required: true,
            ..field("title", "What needs doing.", FieldType::String)
        }],
        vec![FieldMeta {
            default: Some(json!("medium")),
            ..field(
                "priority",
                "How urgent it is.",
                FieldType::Enum(vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                ]),
            )
        }],
        Box::new(AddHandler),
    );

    let list = command(
        "list",
        "Show your todos",
        Vec::new(),
        vec![
            field(
                "status",
                "Show only todos with this status.",
                FieldType::Enum(vec!["pending".to_string(), "done".to_string()]),
            ),
            field("limit", "Show at most this many todos.", FieldType::Number),
        ],
        Box::new(ListHandler),
    );

    let complete = command(
        "complete",
        "Mark a todo as done",
        vec![FieldMeta {
            required: true,
            ..field("id", "The todo number.", FieldType::Number)
        }],
        Vec::new(),
        Box::new(CompleteHandler),
    );

    let clear = command(
        "clear",
        "Delete every completed todo",
        Vec::new(),
        Vec::new(),
        Box::new(ClearHandler),
    );

    let check = command(
        "check",
        "Check every todo, one at a time",
        Vec::new(),
        Vec::new(),
        Box::new(CheckHandler),
    );

    Cli::create("todo")
        .version("1.0.0")
        .description("Keep track of what needs doing")
        .command("add", add)
        .command("list", list)
        .command("complete", complete)
        .command("clear", clear)
        .command("check", check)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = build_cli();

    // Arguments still reach the CLI, so the same binary serves both audiences.
    if std::env::args().nth(1).is_some() {
        return tokio::runtime::Runtime::new()?.block_on(cli.serve());
    }

    DesktopApp::from_cli(&cli)
        .expect("command names are unique")
        .title("Todo")
        .run()?;
    Ok(())
}
