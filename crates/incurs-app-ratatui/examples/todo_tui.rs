//! A todo list, as a terminal application.
//!
//! ```sh
//! cargo run -p incurs-app-ratatui --example todo_tui
//! ```
//!
//! The command graph is chosen for variety rather than realism. It carries a
//! required argument, a closed choice, a switch, a list, a repeat counter, a
//! streaming command, a destructive one, and one that suggests a follow-up —
//! because those are the shapes that expose a wrong control, and a fixture that
//! only holds strings proves nothing about a form.
//!
//! The same binary is still a CLI. Pass any argument and it serves one.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, Example, McpAnnotations, McpCommandOptions,
};
use incurs::output::{CommandResult, CtaBlock, CtaEntry};
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_ratatui::TerminalApp;
use serde_json::{Value, json};

/// One todo.
#[derive(Clone)]
struct Todo {
    id: u64,
    title: String,
    priority: String,
    tags: Vec<String>,
    done: bool,
}

impl Todo {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "priority": self.priority,
            "tags": self.tags,
            "done": self.done,
        })
    }
}

/// The in-memory store.
fn store() -> &'static Mutex<Vec<Todo>> {
    static STORE: OnceLock<Mutex<Vec<Todo>>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(vec![
            Todo {
                id: 1,
                title: "Buy groceries".to_string(),
                priority: "high".to_string(),
                tags: vec!["home".to_string()],
                done: false,
            },
            Todo {
                id: 2,
                title: "Write the report".to_string(),
                priority: "medium".to_string(),
                tags: vec!["work".to_string(), "urgent".to_string()],
                done: true,
            },
        ])
    })
}

/// Builds one field description.
fn field(
    name: &'static str,
    field_type: FieldType,
    required: bool,
    description: &'static str,
) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.replace('_', "-"),
        description: Some(description),
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

/// Assembles one command from its parts.
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

/// `add` — a required argument, a choice, a list, and a switch.
struct AddHandler;

#[async_trait::async_trait]
impl CommandHandler for AddHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let title = ctx
            .args
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() {
            return CommandResult::Error {
                code: "EMPTY_TITLE".to_string(),
                message: "Give it a title.".to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        let mut todos = store().lock().expect("store");
        let todo = Todo {
            id: todos.len() as u64 + 1,
            title,
            priority: ctx
                .options
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("medium")
                .to_string(),
            tags: ctx
                .options
                .get("tags")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            done: ctx
                .options
                .get("done")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let value = todo.to_value();
        let id = todo.id;
        todos.push(todo);

        CommandResult::Ok {
            data: value,
            cta: Some(CtaBlock {
                description: Some("Next".to_string()),
                commands: vec![CtaEntry::Simple(format!("complete {id}"))],
            }),
            exit_code: None,
        }
    }
}

/// `list` — a choice and a repeat counter.
struct ListHandler;

#[async_trait::async_trait]
impl CommandHandler for ListHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let status = ctx
            .options
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("all");
        let verbose = ctx
            .options
            .get("verbose")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let todos = store().lock().expect("store");
        let matching: Vec<Value> = todos
            .iter()
            .filter(|todo| match status {
                "done" => todo.done,
                "pending" => !todo.done,
                _ => true,
            })
            .map(|todo| {
                if verbose > 0 {
                    todo.to_value()
                } else {
                    json!({ "id": todo.id, "title": todo.title })
                }
            })
            .collect();

        CommandResult::Ok {
            data: json!({ "count": matching.len(), "todos": matching }),
            cta: None,
            exit_code: None,
        }
    }
}

/// `complete` — a numeric argument, and a failure a person can trigger.
struct CompleteHandler;

#[async_trait::async_trait]
impl CommandHandler for CompleteHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let id = ctx.args.get("id").and_then(Value::as_u64).unwrap_or(0);
        let mut todos = store().lock().expect("store");
        match todos.iter_mut().find(|todo| todo.id == id) {
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
                message: format!("There is no todo {id}."),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

/// `watch` — streams, so progress and cancellation are reachable.
struct WatchHandler;

#[async_trait::async_trait]
impl CommandHandler for WatchHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Stream(Box::pin(futures::stream::unfold(
            0u64,
            |index| async move {
                if index >= 12 {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(600)).await;
                Some((
                    json!({ "tick": index, "message": format!("step {index}") }),
                    index + 1,
                ))
            },
        )))
    }
}

/// `clear` — destructive, so the warning marker is reachable.
struct ClearHandler;

#[async_trait::async_trait]
impl CommandHandler for ClearHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let mut todos = store().lock().expect("store");
        let before = todos.len();
        todos.retain(|todo| !todo.done);
        CommandResult::Ok {
            data: json!({ "removed": before - todos.len(), "remaining": todos.len() }),
            cta: None,
            exit_code: None,
        }
    }
}

/// Builds the todo command graph.
fn build_cli() -> Cli {
    let add = CommandDef {
        examples: vec![Example {
            command: "\"Buy milk\" --priority high".to_string(),
            description: Some("Add an urgent item".to_string()),
        }],
        hint: Some("Press ^R to add it.".to_string()),
        ..command(
            "add",
            "Add a new todo item",
            vec![field("title", FieldType::String, true, "What needs doing")],
            vec![
                FieldMeta {
                    default: Some(json!("medium")),
                    ..field(
                        "priority",
                        FieldType::Enum(vec![
                            "low".to_string(),
                            "medium".to_string(),
                            "high".to_string(),
                        ]),
                        false,
                        "How urgent it is",
                    )
                },
                field(
                    "tags",
                    FieldType::Array(Box::new(FieldType::String)),
                    false,
                    "Labels to file it under",
                ),
                field(
                    "done",
                    FieldType::Boolean,
                    false,
                    "Mark it done immediately",
                ),
            ],
            Box::new(AddHandler),
        )
    };

    let list = command(
        "list",
        "Show your todos",
        Vec::new(),
        vec![
            FieldMeta {
                default: Some(json!("all")),
                ..field(
                    "status",
                    FieldType::Enum(vec![
                        "all".to_string(),
                        "pending".to_string(),
                        "done".to_string(),
                    ]),
                    false,
                    "Which ones to show",
                )
            },
            field("verbose", FieldType::Count, false, "Repeat for more detail"),
        ],
        Box::new(ListHandler),
    );

    let complete = command(
        "complete",
        "Mark a todo as done",
        vec![field(
            "id",
            FieldType::Number,
            true,
            "Which todo to complete",
        )],
        Vec::new(),
        Box::new(CompleteHandler),
    );

    let watch = command(
        "watch",
        "Stream progress updates (demo)",
        Vec::new(),
        Vec::new(),
        Box::new(WatchHandler),
    );

    Cli::create("todo")
        .version(env!("CARGO_PKG_VERSION"))
        .description("Keep track of what needs doing")
        .command("add", add)
        .command("list", list)
        .command("complete", complete)
        .command("watch", watch)
        .command(
            "clear",
            CommandDef::build("clear", ClearHandler)
                .description("Remove every completed todo")
                .mcp(McpCommandOptions {
                    destructive: true,
                    annotations: Some(McpAnnotations {
                        title: Some("Clear completed".to_string()),
                        read_only_hint: Some(false),
                        destructive_hint: Some(true),
                        idempotent_hint: Some(true),
                        open_world_hint: Some(false),
                    }),
                    ..Default::default()
                })
                .done(),
        )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = build_cli();

    // Arguments still reach the CLI, so one binary serves both audiences.
    if std::env::args().nth(1).is_some() {
        return tokio::runtime::Runtime::new()?.block_on(cli.serve());
    }

    // Deliberately not `#[tokio::main]`: the runner owns a Tokio runtime, and
    // Tokio panics when a runtime is dropped inside another one.
    TerminalApp::from_cli(&cli)?.title("Todo").run()?;
    Ok(())
}
