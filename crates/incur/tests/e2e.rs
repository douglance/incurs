//! End-to-end tests for the incur CLI framework.
//!
//! Ported from `src/e2e.test.ts`. These tests exercise the full CLI lifecycle
//! through `Cli::serve_to`, which writes output to a buffer instead of stdout
//! and returns exit codes instead of calling `process::exit`.
//!
//! Each test follows the same pattern:
//! 1. Build a `Cli` with commands registered
//! 2. Call `serve_to(argv, &mut buf, human)` with `human = false` (agent mode)
//! 3. Assert on the output string and exit code

use std::collections::HashMap;

use incur::cli::Cli;
use incur::command::{CommandContext, CommandDef, CommandHandler, Example};
use incur::output::*;
use incur::schema::{FieldMeta, FieldType};

// ---------------------------------------------------------------------------
// Test helper
// ---------------------------------------------------------------------------

/// Result of running a CLI command in tests.
struct ServeResult {
    output: String,
    exit_code: Option<i32>,
}

/// Runs the CLI with the given argv in agent mode (non-TTY) and captures output.
async fn serve(cli: &Cli, argv: &[&str]) -> ServeResult {
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut buf = Vec::new();
    let exit_code = cli
        .serve_to(argv, &mut buf, false)
        .await
        .expect("serve_to should not return Err");
    let raw = String::from_utf8(buf).expect("output should be valid UTF-8");
    let output = strip_durations(&raw);
    ServeResult { output, exit_code }
}

/// Runs the CLI in human/TTY mode.
async fn serve_human(cli: &Cli, argv: &[&str]) -> ServeResult {
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut buf = Vec::new();
    let exit_code = cli
        .serve_to(argv, &mut buf, true)
        .await
        .expect("serve_to should not return Err");
    let raw = String::from_utf8(buf).expect("output should be valid UTF-8");
    let output = strip_durations(&raw);
    ServeResult { output, exit_code }
}

/// Strips duration values from output for deterministic snapshot comparison.
fn strip_durations(s: &str) -> String {
    // Handle both TOON format `duration: 42ms` and JSON `"duration": "42ms"`
    let re_toon = regex_lite::Regex::new(r#"duration: \d+ms"#).unwrap();
    let re_json = regex_lite::Regex::new(r#""duration": "\d+ms""#).unwrap();
    let s = re_toon.replace_all(s, "duration: <stripped>");
    let s = re_json.replace_all(&s, r#""duration": "<stripped>""#);
    s.to_string()
}

/// Parses a JSON string, panicking with the raw string on failure.
fn json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw.trim()).unwrap_or_else(|e| {
        panic!("Failed to parse JSON: {e}\nRaw output:\n{raw}");
    })
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// A handler that returns a static JSON value.
struct StaticHandler(serde_json::Value);

#[async_trait::async_trait]
impl CommandHandler for StaticHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: self.0.clone(),
            cta: None,
        }
    }
}

/// A handler that returns nothing (void/null).
struct VoidHandler;

#[async_trait::async_trait]
impl CommandHandler for VoidHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: serde_json::Value::Null,
            cta: None,
        }
    }
}

/// A handler that always throws an error.
struct ErrorHandler {
    message: String,
}

#[async_trait::async_trait]
impl CommandHandler for ErrorHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Error {
            code: "UNKNOWN".to_string(),
            message: self.message.clone(),
            retryable: false,
            exit_code: None,
            cta: None,
        }
    }
}

/// A handler that returns an IncurError with specific code and retryable flag.
struct IncurErrorHandler {
    code: String,
    message: String,
    retryable: bool,
}

#[async_trait::async_trait]
impl CommandHandler for IncurErrorHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            retryable: self.retryable,
            exit_code: None,
            cta: None,
        }
    }
}

/// Echo handler: reads args and options, returns formatted result.
struct EchoHandler;

#[async_trait::async_trait]
impl CommandHandler for EchoHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let message = ctx.args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let upper = ctx.options.get("upper").and_then(|v| v.as_bool()).unwrap_or(false);
        let prefix = ctx.options.get("prefix").and_then(|v| v.as_str()).unwrap_or("");
        let repeat = ctx.args.get("repeat").and_then(|v| v.as_u64()).unwrap_or(1);

        let mut msg = if prefix.is_empty() {
            message.to_string()
        } else {
            format!("{prefix} {message}")
        };
        if upper {
            msg = msg.to_uppercase();
        }

        let result: Vec<serde_json::Value> = (0..repeat)
            .map(|_| serde_json::Value::String(msg.clone()))
            .collect();

        CommandResult::Ok {
            data: serde_json::json!({ "result": result }),
            cta: None,
        }
    }
}

/// Project list handler with filtering.
struct ProjectListHandler;

#[async_trait::async_trait]
impl CommandHandler for ProjectListHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let archived = ctx.options.get("archived").and_then(|v| v.as_bool()).unwrap_or(false);
        let limit = ctx.options.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        let all_items = vec![
            serde_json::json!({"id": "p1", "name": "Alpha", "archived": false}),
            serde_json::json!({"id": "p2", "name": "Beta", "archived": true}),
        ];
        let items: Vec<_> = all_items
            .into_iter()
            .filter(|p| archived || !p["archived"].as_bool().unwrap_or(false))
            .take(limit)
            .collect();
        let total = items.len();

        let cta_commands: Vec<CtaEntry> = items
            .iter()
            .map(|p| CtaEntry::Detailed {
                command: format!("project get {}", p["id"].as_str().unwrap()),
                description: Some(format!("View \"{}\"", p["name"].as_str().unwrap())),
            })
            .collect();

        CommandResult::Ok {
            data: serde_json::json!({
                "items": items,
                "total": total,
                "cta": {
                    "commands": cta_commands.iter().map(|c| match c {
                        CtaEntry::Detailed { command, description } => serde_json::json!({
                            "command": format!("app project get {}", command.strip_prefix("project get ").unwrap_or(command)),
                            "description": description,
                        }),
                        _ => serde_json::Value::Null,
                    }).collect::<Vec<_>>(),
                    "description": "Suggested commands:",
                },
            }),
            cta: Some(CtaBlock {
                commands: cta_commands,
                description: None,
            }),
        }
    }
}

/// Project get handler.
struct ProjectGetHandler;

#[async_trait::async_trait]
impl CommandHandler for ProjectGetHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let id = ctx.args.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        CommandResult::Ok {
            data: serde_json::json!({
                "id": id,
                "name": "Alpha",
                "description": "Main project",
                "members": [{"userId": "u1", "role": "admin"}],
            }),
            cta: None,
        }
    }
}

/// Project create handler.
struct ProjectCreateHandler;

#[async_trait::async_trait]
impl CommandHandler for ProjectCreateHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let name = ctx.args.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
        CommandResult::Ok {
            data: serde_json::json!({
                "id": "p-new",
                "url": "https://example.com/projects/p-new",
            }),
            cta: Some(CtaBlock {
                commands: vec![
                    CtaEntry::Detailed {
                        command: "project get p-new".to_string(),
                        description: Some(format!("View \"{name}\"")),
                    },
                    CtaEntry::Simple("project list".to_string()),
                ],
                description: None,
            }),
        }
    }
}

/// Project delete handler (requires --force).
struct ProjectDeleteHandler;

#[async_trait::async_trait]
impl CommandHandler for ProjectDeleteHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let id = ctx.args.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let force = ctx.options.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        if !force {
            return CommandResult::Error {
                code: "CONFIRMATION_REQUIRED".to_string(),
                message: format!("Use --force to delete project {id}"),
                retryable: true,
                exit_code: None,
                cta: None,
            };
        }
        CommandResult::Ok {
            data: serde_json::json!({"deleted": true, "id": id}),
            cta: None,
        }
    }
}

/// Deploy status handler.
struct DeployStatusHandler;

#[async_trait::async_trait]
impl CommandHandler for DeployStatusHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let deploy_id = ctx.args.get("deploy_id").and_then(|v| v.as_str()).unwrap_or("unknown");
        CommandResult::Ok {
            data: serde_json::json!({
                "deployId": deploy_id,
                "status": "running",
                "progress": 75,
            }),
            cta: None,
        }
    }
}

/// Deploy create handler.
struct DeployCreateHandler;

#[async_trait::async_trait]
impl CommandHandler for DeployCreateHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let env = ctx.args.get("env").and_then(|v| v.as_str()).unwrap_or("unknown");
        let dry_run = ctx.options.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
        CommandResult::Ok {
            data: serde_json::json!({
                "deployId": "d-123",
                "url": format!("https://{env}.example.com"),
                "status": if dry_run { "dry-run" } else { "pending" },
            }),
            cta: None,
        }
    }
}

/// Deploy rollback handler.
struct DeployRollbackHandler;

#[async_trait::async_trait]
impl CommandHandler for DeployRollbackHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let deploy_id = ctx.args.get("deploy_id").and_then(|v| v.as_str()).unwrap_or("unknown");
        CommandResult::Ok {
            data: serde_json::json!({"rolledBack": true, "deployId": deploy_id}),
            cta: None,
        }
    }
}

/// Config handler (with optional key arg).
struct ConfigHandler;

#[async_trait::async_trait]
impl CommandHandler for ConfigHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let key = ctx.args.get("key").and_then(|v| v.as_str());
        let data = if let Some(k) = key {
            serde_json::json!({"key": k, "value": "some-value"})
        } else {
            serde_json::json!({
                "apiUrl": "https://api.example.com",
                "timeout": 30,
                "debug": false,
            })
        };
        CommandResult::Ok { data, cta: None }
    }
}

/// Auth login handler.
struct AuthLoginHandler;

#[async_trait::async_trait]
impl CommandHandler for AuthLoginHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let hostname = ctx
            .options
            .get("hostname")
            .and_then(|v| v.as_str())
            .unwrap_or("api.example.com");
        let scopes = ctx
            .options
            .get("scopes")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        CommandResult::Ok {
            data: serde_json::json!({
                "hostname": hostname,
                "scopes": scopes,
            }),
            cta: Some(CtaBlock {
                commands: vec![CtaEntry::Simple("auth status".to_string())],
                description: Some("Verify your session:".to_string()),
            }),
        }
    }
}

/// Auth status handler (always returns error).
struct AuthStatusHandler;

#[async_trait::async_trait]
impl CommandHandler for AuthStatusHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        CommandResult::Error {
            code: "NOT_AUTHENTICATED".to_string(),
            message: "Not logged in".to_string(),
            retryable: false,
            exit_code: None,
            cta: Some(CtaBlock {
                commands: vec![CtaEntry::Simple("auth login".to_string())],
                description: None,
            }),
        }
    }
}

/// Async handler (simulates delay).
struct SlowHandler;

#[async_trait::async_trait]
impl CommandHandler for SlowHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        CommandResult::Ok {
            data: serde_json::json!({"done": true}),
            cta: None,
        }
    }
}

/// Stream handler that yields multiple chunks.
struct StreamHandler;

#[async_trait::async_trait]
impl CommandHandler for StreamHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let stream = async_stream::stream! {
            yield serde_json::json!({"content": "hello"});
            yield serde_json::json!({"content": "world"});
        };
        CommandResult::Stream(Box::pin(stream))
    }
}

/// Stream handler that yields plain text strings.
struct StreamTextHandler;

#[async_trait::async_trait]
impl CommandHandler for StreamTextHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let stream = async_stream::stream! {
            yield serde_json::json!("hello");
            yield serde_json::json!("world");
        };
        CommandResult::Stream(Box::pin(stream))
    }
}

/// Stream handler with ok() return and CTA.
struct StreamOkHandler;

#[async_trait::async_trait]
impl CommandHandler for StreamOkHandler {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        let stream = async_stream::stream! {
            yield serde_json::json!({"n": 1});
            yield serde_json::json!({"n": 2});
        };
        // Note: In the TS version, the generator returns ok() with a CTA.
        // In Rust, streaming with CTA isn't supported the same way via this trait.
        // For now we just return the stream.
        CommandResult::Stream(Box::pin(stream))
    }
}

// NOTE: StreamErrorHandler and StreamThrowHandler from the TS tests are not
// ported because CommandResult::Stream doesn't support mid-stream errors.
// That would require encoding errors as stream items, which is a different
// pattern than the TS async generator approach.

// ---------------------------------------------------------------------------
// App builder
// ---------------------------------------------------------------------------

fn make_field(name: &'static str, desc: &'static str, ft: FieldType, required: bool) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: incur::schema::to_kebab(name),
        description: Some(desc),
        field_type: ft,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

fn make_field_with_default(
    name: &'static str,
    desc: &'static str,
    ft: FieldType,
    default: serde_json::Value,
) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: incur::schema::to_kebab(name),
        description: Some(desc),
        field_type: ft,
        required: false,
        default: Some(default),
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

fn create_app() -> Cli {
    // --- auth group ---
    let auth = Cli::create("auth")
        .description("Authentication commands")
        .command(
            "login",
            CommandDef {
                name: "login".to_string(),
                description: Some("Log in to the service".to_string()),
                args_fields: vec![],
                options_fields: vec![
                    FieldMeta {
                        name: "hostname",
                        cli_name: "hostname".to_string(),
                        description: Some("API hostname"),
                        field_type: FieldType::String,
                        required: false,
                        default: Some(serde_json::json!("api.example.com")),
                        alias: Some('h'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "web",
                        cli_name: "web".to_string(),
                        description: Some("Open browser"),
                        field_type: FieldType::Boolean,
                        required: false,
                        default: Some(serde_json::json!(false)),
                        alias: Some('w'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "scopes",
                        cli_name: "scopes".to_string(),
                        description: Some("OAuth scopes"),
                        field_type: FieldType::Array(Box::new(FieldType::String)),
                        required: false,
                        default: None,
                        alias: None,
                        deprecated: false,
                        env_name: None,
                    },
                ],
                env_fields: vec![],
                aliases: {
                    let mut m = HashMap::new();
                    m.insert("hostname".to_string(), 'h');
                    m.insert("web".to_string(), 'w');
                    m
                },
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(AuthLoginHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "logout",
            CommandDef {
                name: "logout".to_string(),
                description: Some("Log out of the service".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StaticHandler(serde_json::json!({"loggedOut": true}))),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "status",
            CommandDef {
                name: "status".to_string(),
                description: Some("Show authentication status".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(AuthStatusHandler),
                middleware: vec![],
                output_schema: None,
            },
        );

    // --- deploy subgroup ---
    let deploy = Cli::create("deploy")
        .description("Deployment commands")
        .command(
            "create",
            CommandDef {
                name: "create".to_string(),
                description: Some("Create a deployment".to_string()),
                args_fields: vec![make_field("env", "Target environment", FieldType::String, true)],
                options_fields: vec![
                    FieldMeta {
                        name: "branch",
                        cli_name: "branch".to_string(),
                        description: Some("Branch to deploy"),
                        field_type: FieldType::String,
                        required: false,
                        default: Some(serde_json::json!("main")),
                        alias: Some('b'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "dry_run",
                        cli_name: "dry-run".to_string(),
                        description: Some("Dry run mode"),
                        field_type: FieldType::Boolean,
                        required: false,
                        default: Some(serde_json::json!(false)),
                        alias: None,
                        deprecated: false,
                        env_name: None,
                    },
                ],
                env_fields: vec![],
                aliases: {
                    let mut m = HashMap::new();
                    m.insert("branch".to_string(), 'b');
                    m
                },
                examples: vec![
                    Example {
                        command: "project deploy create staging".to_string(),
                        description: Some("Deploy staging from main".to_string()),
                    },
                    Example {
                        command: "project deploy create production --branch release --dryRun true"
                            .to_string(),
                        description: Some("Dry run a production deploy".to_string()),
                    },
                ],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(DeployCreateHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "status",
            CommandDef {
                name: "status".to_string(),
                description: Some("Check deployment status".to_string()),
                args_fields: vec![make_field(
                    "deploy_id",
                    "Deployment ID",
                    FieldType::String,
                    true,
                )],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(DeployStatusHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "rollback",
            CommandDef {
                name: "rollback".to_string(),
                description: Some("Rollback a deployment".to_string()),
                args_fields: vec![make_field(
                    "deploy_id",
                    "Deployment ID",
                    FieldType::String,
                    true,
                )],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(DeployRollbackHandler),
                middleware: vec![],
                output_schema: None,
            },
        );

    // --- project group ---
    let project = Cli::create("project")
        .description("Manage projects")
        .command(
            "list",
            CommandDef {
                name: "list".to_string(),
                description: Some("List projects".to_string()),
                args_fields: vec![],
                options_fields: vec![
                    FieldMeta {
                        name: "limit",
                        cli_name: "limit".to_string(),
                        description: Some("Max results"),
                        field_type: FieldType::Number,
                        required: false,
                        default: Some(serde_json::json!(20)),
                        alias: Some('l'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "sort",
                        cli_name: "sort".to_string(),
                        description: Some("Sort field"),
                        field_type: FieldType::Enum(vec![
                            "name".to_string(),
                            "created".to_string(),
                            "updated".to_string(),
                        ]),
                        required: false,
                        default: Some(serde_json::json!("name")),
                        alias: Some('s'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "archived",
                        cli_name: "archived".to_string(),
                        description: Some("Include archived"),
                        field_type: FieldType::Boolean,
                        required: false,
                        default: Some(serde_json::json!(false)),
                        alias: None,
                        deprecated: false,
                        env_name: None,
                    },
                ],
                env_fields: vec![],
                aliases: {
                    let mut m = HashMap::new();
                    m.insert("limit".to_string(), 'l');
                    m.insert("sort".to_string(), 's');
                    m
                },
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ProjectListHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "get",
            CommandDef {
                name: "get".to_string(),
                description: Some("Get a project by ID".to_string()),
                args_fields: vec![make_field("id", "Project ID", FieldType::String, true)],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ProjectGetHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "create",
            CommandDef {
                name: "create".to_string(),
                description: Some("Create a new project".to_string()),
                args_fields: vec![make_field("name", "Project name", FieldType::String, true)],
                options_fields: vec![
                    make_field_with_default(
                        "description",
                        "Project description",
                        FieldType::String,
                        serde_json::json!(""),
                    ),
                    make_field_with_default(
                        "private",
                        "Private project",
                        FieldType::Boolean,
                        serde_json::json!(false),
                    ),
                ],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ProjectCreateHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "delete",
            CommandDef {
                name: "delete".to_string(),
                description: Some("Delete a project".to_string()),
                args_fields: vec![make_field("id", "Project ID", FieldType::String, true)],
                options_fields: vec![FieldMeta {
                    name: "force",
                    cli_name: "force".to_string(),
                    description: Some("Skip confirmation"),
                    field_type: FieldType::Boolean,
                    required: false,
                    default: Some(serde_json::json!(false)),
                    alias: Some('f'),
                    deprecated: false,
                    env_name: None,
                }],
                env_fields: vec![],
                aliases: {
                    let mut m = HashMap::new();
                    m.insert("force".to_string(), 'f');
                    m
                },
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ProjectDeleteHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .group(deploy);

    // --- config leaf CLI ---
    let config = Cli::create("config")
        .description("Show current configuration")
        .root(CommandDef {
            name: "config".to_string(),
            description: Some("Show current configuration".to_string()),
            args_fields: vec![FieldMeta {
                name: "key",
                cli_name: "key".to_string(),
                description: Some("Config key to show"),
                field_type: FieldType::String,
                required: false,
                default: None,
                alias: None,
                deprecated: false,
                env_name: None,
            }],
            options_fields: vec![],
            env_fields: vec![],
            aliases: HashMap::new(),
            examples: vec![],
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(ConfigHandler),
            middleware: vec![],
            output_schema: None,
        });

    // --- top-level CLI ---
    Cli::create("app")
        .version("3.5.0")
        .description("A comprehensive CLI application for testing.")
        .command(
            "ping",
            CommandDef {
                name: "ping".to_string(),
                description: Some("Health check".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StaticHandler(serde_json::json!({"pong": true}))),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "echo",
            CommandDef {
                name: "echo".to_string(),
                description: Some("Echo back arguments".to_string()),
                args_fields: vec![
                    make_field("message", "Message to echo", FieldType::String, true),
                    make_field("repeat", "Times to repeat", FieldType::Number, false),
                ],
                options_fields: vec![
                    FieldMeta {
                        name: "upper",
                        cli_name: "upper".to_string(),
                        description: Some("Uppercase output"),
                        field_type: FieldType::Boolean,
                        required: false,
                        default: Some(serde_json::json!(false)),
                        alias: Some('u'),
                        deprecated: false,
                        env_name: None,
                    },
                    FieldMeta {
                        name: "prefix",
                        cli_name: "prefix".to_string(),
                        description: Some("Prefix string"),
                        field_type: FieldType::String,
                        required: false,
                        default: Some(serde_json::json!("")),
                        alias: Some('p'),
                        deprecated: false,
                        env_name: None,
                    },
                ],
                env_fields: vec![],
                aliases: {
                    let mut m = HashMap::new();
                    m.insert("upper".to_string(), 'u');
                    m.insert("prefix".to_string(), 'p');
                    m
                },
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(EchoHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "slow",
            CommandDef {
                name: "slow".to_string(),
                description: Some("Async command".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(SlowHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "explode",
            CommandDef {
                name: "explode".to_string(),
                description: Some("Always fails".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(ErrorHandler {
                    message: "kaboom".to_string(),
                }),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "explode-clac",
            CommandDef {
                name: "explode-clac".to_string(),
                description: Some("Fails with IncurError".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(IncurErrorHandler {
                    code: "QUOTA_EXCEEDED".to_string(),
                    message: "Rate limit exceeded".to_string(),
                    retryable: true,
                }),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "noop",
            CommandDef {
                name: "noop".to_string(),
                description: Some("Returns nothing".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(VoidHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "stream",
            CommandDef {
                name: "stream".to_string(),
                description: Some("Stream chunks".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StreamHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "stream-text",
            CommandDef {
                name: "stream-text".to_string(),
                description: Some("Stream plain text".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StreamTextHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .command(
            "stream-ok",
            CommandDef {
                name: "stream-ok".to_string(),
                description: Some("Stream with ok() return".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(StreamOkHandler),
                middleware: vec![],
                output_schema: None,
            },
        )
        .group(auth)
        .group(project)
        .group(config)
}

// ===========================================================================
// Tests
// ===========================================================================

mod routing {
    use super::*;

    #[tokio::test]
    async fn top_level_command() {
        let r = serve(&create_app(), &["ping"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn group_command() {
        let r = serve(&create_app(), &["auth", "logout"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["loggedOut"], true);
    }

    #[tokio::test]
    async fn nested_group_command_3_levels_deep() {
        let r = serve(
            &create_app(),
            &["project", "deploy", "status", "d-456"],
        )
        .await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["deployId"], "d-456");
        assert_eq!(parsed["status"], "running");
        assert_eq!(parsed["progress"], 75);
    }

    #[tokio::test]
    async fn mounted_leaf_cli_as_single_command() {
        let r = serve(&create_app(), &["config"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["apiUrl"], "https://api.example.com");
        assert_eq!(parsed["timeout"], 30);
        assert_eq!(parsed["debug"], false);
    }

    #[tokio::test]
    async fn mounted_leaf_cli_with_args() {
        let r = serve(&create_app(), &["config", "apiUrl"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["key"], "apiUrl");
        assert_eq!(parsed["value"], "some-value");
    }

    #[tokio::test]
    async fn unknown_top_level_command() {
        let r = serve(&create_app(), &["nonexistent"]).await;
        assert_eq!(r.exit_code, Some(1));
        assert!(
            r.output.contains("COMMAND_NOT_FOUND"),
            "Expected COMMAND_NOT_FOUND in output, got: {}",
            r.output
        );
        assert!(
            r.output.contains("'nonexistent' is not a command for 'app'"),
            "Expected error message in output, got: {}",
            r.output
        );
    }

    #[tokio::test]
    async fn unknown_top_level_command_shows_human_error_in_tty() {
        let r = serve_human(&create_app(), &["nonexistent"]).await;
        assert_eq!(r.exit_code, Some(1));
        assert!(
            r.output.contains("Error: 'nonexistent' is not a command for 'app'"),
            "Expected human error message, got: {}",
            r.output
        );
    }

    #[tokio::test]
    async fn unknown_subcommand_lists_available() {
        let r = serve(&create_app(), &["auth", "whoami"]).await;
        assert_eq!(r.exit_code, Some(1));
        assert!(r.output.contains("COMMAND_NOT_FOUND"));
        assert!(r.output.contains("'whoami' is not a command for 'app auth'"));
    }

    #[tokio::test]
    async fn unknown_nested_subcommand() {
        let r = serve(
            &create_app(),
            &["project", "deploy", "nope"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        assert!(r.output.contains("COMMAND_NOT_FOUND"));
        assert!(r.output.contains("'nope' is not a command for 'app project deploy'"));
    }
}

mod args_and_options {
    use super::*;

    #[tokio::test]
    async fn positional_args_in_order() {
        let r = serve(&create_app(), &["echo", "hello", "--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["result"][0], "hello");
    }

    #[tokio::test]
    async fn flag_value_form() {
        let r = serve(
            &create_app(),
            &["echo", "hello", "--upper", "--prefix", ">>", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["result"][0], ">> HELLO");
    }

    #[tokio::test]
    async fn short_alias_flag() {
        let r = serve(
            &create_app(),
            &["echo", "hello", "-u", "-p", ">>", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["result"][0], ">> HELLO");
    }

    #[tokio::test]
    async fn multiple_options_combined() {
        let r = serve(
            &create_app(),
            &["echo", "hi", "--upper", "--prefix", "!", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["result"][0], "! HI");
    }

    #[tokio::test]
    async fn number_coercion_from_argv_strings() {
        let r = serve(
            &create_app(),
            &["project", "list", "--limit", "5", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        // With limit=5, we get at most 5 items. Our fixture has 1 non-archived item.
        assert_eq!(parsed["total"], 1);
    }

    #[tokio::test]
    async fn force_option_passes_through() {
        let r = serve(
            &create_app(),
            &["project", "delete", "p1", "--force", "--format", "json"],
        )
        .await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["deleted"], true);
        assert_eq!(parsed["id"], "p1");
    }

    #[tokio::test]
    async fn missing_force_returns_error() {
        let r = serve(
            &create_app(),
            &["project", "delete", "p1", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "CONFIRMATION_REQUIRED");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("Use --force to delete project p1"));
    }
}

mod output_formats {
    use super::*;

    #[tokio::test]
    async fn default_format_is_json_pretty_in_agent_mode() {
        // Agent mode (non-TTY) currently uses JSON pretty-print as the default
        // toon format fallback.
        let r = serve(&create_app(), &["ping"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn format_json() {
        let r = serve(&create_app(), &["ping", "--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn json_shorthand() {
        let r = serve(&create_app(), &["ping", "--json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn verbose_full_envelope() {
        let r = serve(
            &create_app(),
            &["ping", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["pong"], true);
        assert_eq!(parsed["meta"]["command"], "ping");
        assert!(parsed["meta"]["duration"].is_string());
    }

    #[tokio::test]
    async fn nested_command_path_in_verbose_meta() {
        let r = serve(
            &create_app(),
            &[
                "project", "deploy", "status", "d-1", "--verbose", "--format", "json",
            ],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["deployId"], "d-1");
        assert_eq!(parsed["data"]["status"], "running");
        assert_eq!(parsed["data"]["progress"], 75);
        assert_eq!(parsed["meta"]["command"], "project deploy status");
    }

    #[tokio::test]
    async fn cli_level_default_format() {
        let cli = Cli::create("test")
            .format(Format::Json)
            .command(
                "ping",
                CommandDef {
                    name: "ping".to_string(),
                    description: Some("Health check".to_string()),
                    args_fields: vec![],
                    options_fields: vec![],
                    env_fields: vec![],
                    aliases: HashMap::new(),
                    examples: vec![],
                    hint: None,
                    format: None,
                    output_policy: None,
                    handler: Box::new(StaticHandler(serde_json::json!({"pong": true}))),
                    middleware: vec![],
                    output_schema: None,
                },
            );
        let r = serve(&cli, &["ping"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn command_level_default_format() {
        let cli = Cli::create("test").command(
            "ping",
            CommandDef {
                name: "ping".to_string(),
                description: Some("Health check".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: Some(Format::Json),
                output_policy: None,
                handler: Box::new(StaticHandler(serde_json::json!({"pong": true}))),
                middleware: vec![],
                output_schema: None,
            },
        );
        let r = serve(&cli, &["ping"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }
}

mod undefined_output {
    use super::*;

    #[tokio::test]
    async fn void_command_produces_null_output() {
        let r = serve(&create_app(), &["noop", "--format", "json"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn void_command_verbose_shows_envelope() {
        let r = serve(
            &create_app(),
            &["noop", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["meta"]["command"], "noop");
        assert!(parsed["meta"]["duration"].is_string());
    }
}

mod error_handling {
    use super::*;

    #[tokio::test]
    async fn thrown_error_shows_structured_error() {
        let r = serve(&create_app(), &["explode", "--format", "json"]).await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "UNKNOWN");
        assert_eq!(parsed["message"], "kaboom");
    }

    #[tokio::test]
    async fn thrown_error_shows_human_error_in_tty() {
        let r = serve_human(&create_app(), &["explode"]).await;
        assert_eq!(r.exit_code, Some(1));
        assert!(
            r.output.contains("Error: kaboom"),
            "Expected 'Error: kaboom', got: {}",
            r.output
        );
    }

    #[tokio::test]
    async fn incur_error_preserves_code_and_retryable() {
        let r = serve(
            &create_app(),
            &["explode-clac", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "QUOTA_EXCEEDED");
        assert_eq!(parsed["message"], "Rate limit exceeded");
        assert_eq!(parsed["retryable"], true);
    }

    #[tokio::test]
    async fn error_sentinel_returns_error_envelope() {
        let r = serve(
            &create_app(),
            &["auth", "status", "--verbose", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"]["code"], "NOT_AUTHENTICATED");
        assert_eq!(parsed["error"]["message"], "Not logged in");
        assert_eq!(parsed["meta"]["command"], "auth status");
    }

    #[tokio::test]
    async fn incur_error_in_nested_command() {
        let r = serve(
            &create_app(),
            &["project", "delete", "p1", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "CONFIRMATION_REQUIRED");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("Use --force to delete project p1"));
    }

    #[tokio::test]
    async fn command_not_found_returns_error_envelope() {
        let r = serve(
            &create_app(),
            &["nonexistent", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "COMMAND_NOT_FOUND");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("'nonexistent' is not a command for 'app'"));
    }

    #[tokio::test]
    async fn error_envelope_respects_format_json() {
        let r = serve(
            &create_app(),
            &["explode", "--format", "json"],
        )
        .await;
        assert_eq!(r.exit_code, Some(1));
        let parsed = json(&r.output);
        assert_eq!(parsed["code"], "UNKNOWN");
        assert_eq!(parsed["message"], "kaboom");
    }
}

mod cta {
    use super::*;

    #[tokio::test]
    async fn ok_with_string_ctas() {
        let r = serve(
            &create_app(),
            &["auth", "login", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        let cta = &parsed["meta"]["cta"];
        assert_eq!(cta["description"], "Verify your session:");
        assert_eq!(cta["commands"][0]["command"], "app auth status");
    }

    #[tokio::test]
    async fn ok_with_object_ctas_including_descriptions() {
        let r = serve(
            &create_app(),
            &["project", "create", "MyProject", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        let cta = &parsed["meta"]["cta"];
        assert_eq!(cta["commands"][0]["command"], "app project get p-new");
        assert_eq!(cta["commands"][0]["description"], "View \"MyProject\"");
        assert_eq!(cta["commands"][1]["command"], "app project list");
    }

    #[tokio::test]
    async fn plain_return_omits_cta() {
        let r = serve(
            &create_app(),
            &["ping", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert!(parsed["meta"]["cta"].is_null());
    }
}

mod async_tests {
    use super::*;

    #[tokio::test]
    async fn async_handler_resolves() {
        let r = serve(&create_app(), &["slow", "--format", "json"]).await;
        assert!(r.exit_code.is_none());
        let parsed = json(&r.output);
        assert_eq!(parsed["done"], true);
    }
}

mod streaming {
    use super::*;

    #[tokio::test]
    async fn format_json_buffers_all_chunks() {
        let r = serve(
            &create_app(),
            &["stream", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert!(parsed.is_array());
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["content"], "hello");
        assert_eq!(arr[1]["content"], "world");
    }

    #[tokio::test]
    async fn format_json_verbose_buffers_with_envelope() {
        let r = serve(
            &create_app(),
            &["stream", "--verbose", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], true);
        let data = parsed["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["content"], "hello");
        assert_eq!(data[1]["content"], "world");
        assert_eq!(parsed["meta"]["command"], "stream");
    }

    #[tokio::test]
    async fn format_jsonl_explicit() {
        let r = serve(
            &create_app(),
            &["stream", "--format", "jsonl"],
        )
        .await;
        let lines: Vec<serde_json::Value> = r
            .output
            .trim()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(lines.len() >= 3, "Expected at least 3 JSONL lines, got {}", lines.len());
        assert_eq!(lines[0]["type"], "chunk");
        assert_eq!(lines[0]["data"]["content"], "hello");
        assert_eq!(lines[1]["type"], "chunk");
        assert_eq!(lines[1]["data"]["content"], "world");
        assert_eq!(lines[2]["type"], "done");
    }

    #[tokio::test]
    async fn plain_text_streams_as_jsonl_chunks() {
        let r = serve(
            &create_app(),
            &["stream-text", "--format", "jsonl"],
        )
        .await;
        let lines: Vec<serde_json::Value> = r
            .output
            .trim()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(lines.len() >= 3);
        assert_eq!(lines[0]["type"], "chunk");
        assert_eq!(lines[0]["data"], "hello");
        assert_eq!(lines[1]["type"], "chunk");
        assert_eq!(lines[1]["data"], "world");
        assert_eq!(lines[2]["type"], "done");
    }
}

mod help {
    use super::*;

    #[tokio::test]
    async fn root_help_no_args() {
        let r = serve(&create_app(), &[]).await;
        assert!(r.exit_code.is_none());
        assert!(
            r.output.contains("Usage: app <command>"),
            "Expected 'Usage: app <command>' in output, got:\n{}",
            r.output
        );
    }

    #[tokio::test]
    async fn help_flag_on_root() {
        let r = serve(&create_app(), &["--help"]).await;
        assert!(r.exit_code.is_none());
        assert!(r.output.contains("Usage: app <command>"));
    }

    #[tokio::test]
    async fn group_help_no_subcommand() {
        let r = serve(&create_app(), &["auth"]).await;
        assert!(r.exit_code.is_none());
        assert!(r.output.contains("auth"));
        assert!(r.output.contains("login"));
        assert!(r.output.contains("logout"));
        assert!(r.output.contains("status"));
    }

    #[tokio::test]
    async fn nested_group_help() {
        let r = serve(&create_app(), &["project", "deploy"]).await;
        assert!(r.exit_code.is_none());
        assert!(r.output.contains("deploy"));
        assert!(r.output.contains("create"));
        assert!(r.output.contains("rollback"));
        assert!(r.output.contains("status"));
    }

    #[tokio::test]
    async fn help_flag_on_group() {
        let r = serve(&create_app(), &["project", "--help"]).await;
        assert!(r.exit_code.is_none());
        assert!(r.output.contains("project"));
        assert!(r.output.contains("deploy"));
        assert!(r.output.contains("list"));
    }

    #[tokio::test]
    async fn version() {
        let r = serve(&create_app(), &["--version"]).await;
        assert!(r.exit_code.is_none());
        assert_eq!(r.output.trim(), "3.5.0");
    }

    #[tokio::test]
    async fn help_takes_precedence_over_version() {
        let r = serve(&create_app(), &["--help", "--version"]).await;
        assert!(r.output.contains("Usage: app <command>"));
        assert!(r.output.contains("3.5.0"));
    }

    #[tokio::test]
    async fn root_help_lists_commands() {
        let r = serve(&create_app(), &[]).await;
        assert!(r.output.contains("ping"));
        assert!(r.output.contains("echo"));
        assert!(r.output.contains("auth"));
        assert!(r.output.contains("project"));
        assert!(r.output.contains("config"));
    }
}

mod composition {
    use super::*;

    #[tokio::test]
    async fn multiple_groups_on_same_parent() {
        let cli = create_app();
        let r1 = serve(&cli, &["auth", "logout", "--format", "json"]).await;
        let p1 = json(&r1.output);
        assert_eq!(p1["loggedOut"], true);

        let r2 = serve(&cli, &["project", "list", "--format", "json"]).await;
        let p2 = json(&r2.output);
        assert!(p2["items"].is_array());

        let r3 = serve(&cli, &["ping", "--format", "json"]).await;
        let p3 = json(&r3.output);
        assert_eq!(p3["pong"], true);
    }

    #[tokio::test]
    async fn deeply_nested_deploy_commands_work_alongside_siblings() {
        let cli = create_app();
        let r1 = serve(&cli, &["project", "deploy", "create", "staging", "--format", "json"]).await;
        let p1 = json(&r1.output);
        assert_eq!(p1["deployId"], "d-123");
        assert_eq!(p1["url"], "https://staging.example.com");
        assert_eq!(p1["status"], "pending");

        let r2 = serve(&cli, &["project", "list", "--format", "json"]).await;
        let p2 = json(&r2.output);
        assert!(p2["items"].is_array());
    }

    #[tokio::test]
    async fn leaf_cli_mounted_alongside_groups() {
        let cli = create_app();
        let r1 = serve(&cli, &["config", "--format", "json"]).await;
        let p1 = json(&r1.output);
        assert_eq!(p1["apiUrl"], "https://api.example.com");

        let r2 = serve(&cli, &["auth", "logout", "--format", "json"]).await;
        let p2 = json(&r2.output);
        assert_eq!(p2["loggedOut"], true);
    }
}

mod root_command_with_subcommands {
    use super::*;

    fn create_hybrid() -> Cli {
        Cli::create("tool")
            .description("A tool with a default action")
            .root(CommandDef {
                name: "tool".to_string(),
                description: Some("A tool with a default action".to_string()),
                args_fields: vec![FieldMeta {
                    name: "query",
                    cli_name: "query".to_string(),
                    description: Some("Search query"),
                    field_type: FieldType::String,
                    required: false,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                options_fields: vec![],
                env_fields: vec![],
                aliases: HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(RootHandler),
                middleware: vec![],
                output_schema: None,
            })
            .command(
                "info",
                CommandDef {
                    name: "info".to_string(),
                    description: Some("Show info".to_string()),
                    args_fields: vec![],
                    options_fields: vec![],
                    env_fields: vec![],
                    aliases: HashMap::new(),
                    examples: vec![],
                    hint: None,
                    format: None,
                    output_policy: None,
                    handler: Box::new(StaticHandler(serde_json::json!({"info": true}))),
                    middleware: vec![],
                    output_schema: None,
                },
            )
            .command(
                "version",
                CommandDef {
                    name: "version".to_string(),
                    description: Some("Show version".to_string()),
                    args_fields: vec![],
                    options_fields: vec![],
                    env_fields: vec![],
                    aliases: HashMap::new(),
                    examples: vec![],
                    hint: None,
                    format: None,
                    output_policy: None,
                    handler: Box::new(StaticHandler(serde_json::json!({"version": "1.0.0"}))),
                    middleware: vec![],
                    output_schema: None,
                },
            )
    }

    struct RootHandler;

    #[async_trait::async_trait]
    impl CommandHandler for RootHandler {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            let query = ctx
                .args
                .get("query")
                .and_then(|v| v.as_str())
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null);
            CommandResult::Ok {
                data: serde_json::json!({"default": true, "query": query}),
                cta: None,
            }
        }
    }

    #[tokio::test]
    async fn runs_root_handler_with_no_args() {
        let r = serve(&create_hybrid(), &["--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["default"], true);
        assert_eq!(parsed["query"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn subcommand_takes_precedence() {
        let r = serve(&create_hybrid(), &["info", "--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["info"], true);
    }

    #[tokio::test]
    async fn help_shows_root_usage_and_subcommands() {
        let r = serve(&create_hybrid(), &["--help"]).await;
        assert!(r.output.contains("tool"));
        assert!(r.output.contains("info"));
        assert!(r.output.contains("version"));
    }
}

mod edge_cases {
    use super::*;

    #[tokio::test]
    async fn command_with_only_options_no_args() {
        let r = serve(
            &create_app(),
            &["project", "list", "--limit", "1", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert!(parsed["items"].is_array());
        assert_eq!(parsed["total"], 1);
    }

    #[tokio::test]
    async fn command_with_only_args_no_options() {
        let r = serve(
            &create_app(),
            &["project", "get", "p1", "--format", "json"],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["id"], "p1");
        assert_eq!(parsed["name"], "Alpha");
        assert_eq!(parsed["description"], "Main project");
    }

    #[tokio::test]
    async fn command_with_no_schemas_at_all() {
        let r = serve(&create_app(), &["ping", "--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["pong"], true);
    }

    #[tokio::test]
    async fn optional_arg_can_be_omitted() {
        let r = serve(&create_app(), &["config", "--format", "json"]).await;
        let parsed = json(&r.output);
        assert_eq!(parsed["apiUrl"], "https://api.example.com");
        assert_eq!(parsed["timeout"], 30);
        assert_eq!(parsed["debug"], false);
    }

    #[tokio::test]
    async fn flag_order_does_not_matter() {
        let r = serve(
            &create_app(),
            &[
                "--format", "json", "project", "deploy", "create", "prod", "--branch", "release",
                "--verbose",
            ],
        )
        .await;
        let parsed = json(&r.output);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["deployId"], "d-123");
        assert_eq!(parsed["data"]["url"], "https://prod.example.com");
        assert_eq!(parsed["meta"]["command"], "project deploy create");
    }

    #[tokio::test]
    async fn empty_argv_on_router_shows_help() {
        let r = serve(&create_app(), &[]).await;
        assert!(r.exit_code.is_none());
        assert!(r.output.contains("Usage: app <command>"));
    }
}
