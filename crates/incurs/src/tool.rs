//! Transport-neutral tool catalog for incurs command graphs.
//!
//! The catalog is the shared discovery and invocation boundary used by MCP
//! and Code Mode. It preserves command schemas, annotations, middleware, and
//! typed execution without converting a call back into CLI arguments.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::cli::{Cli, CommandEntry, ConfigOptions};
use crate::command::{
    self, CommandDef, ExecuteOptions, McpAnnotations, McpResultContent, ParseMode, RequestContext,
};
use crate::errors::FieldError;
use crate::middleware::MiddlewareFn;
use crate::output::{CtaBlock, FieldErrorOutput, Format, StreamRecord};
use crate::schema::FieldMeta;

/// Metadata and schemas for one callable command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Stable tool name exposed to non-CLI transports.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the flat tool input.
    pub input_schema: Value,
    /// JSON Schema for successful structured output.
    pub output_schema: Option<Value>,
    /// Behavioral annotations supplied by the command.
    pub annotations: Option<McpAnnotations>,
    /// Tool-specific instructions for agent clients.
    pub instructions: Option<String>,
    /// Usage examples copied from the command definition.
    pub examples: Vec<ToolExample>,
    /// Rich MCP content derived from a successful structured result.
    pub result_content: Vec<McpResultContent>,
}

/// One transport-neutral tool usage example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExample {
    /// The command invocation without the CLI name prefix.
    pub command: String,
    /// A short explanation of the example.
    pub description: Option<String>,
}

/// Source used for declared command and CLI environment fields.
#[derive(Debug, Clone, Default)]
pub enum EnvironmentSource {
    /// Read declared fields from the current process environment.
    #[default]
    DeclaredHost,
    /// Read declared fields from explicit values.
    Values(HashMap<String, String>),
    /// Do not provide environment values.
    Empty,
}

/// Source used for command option defaults.
#[derive(Debug, Clone, Default)]
pub enum ConfigSource {
    /// Use the CLI's configured file discovery.
    #[default]
    Auto,
    /// Load one explicit JSON config file.
    Path(String),
    /// Use an already parsed config tree.
    Values(BTreeMap<String, Value>),
    /// Disable config defaults.
    Disabled,
}

/// Incremental event emitted by a transport-neutral tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolEvent {
    /// Progress state reported by the runtime.
    Progress {
        /// Human-readable progress message.
        message: String,
        /// Optional completion fraction from zero through one.
        fraction: Option<f64>,
    },
    /// Diagnostic or user-facing log message.
    Log {
        /// Log severity.
        level: String,
        /// Log message.
        message: String,
    },
    /// One item from a streaming command.
    Chunk {
        /// Structured streamed value.
        data: Value,
    },
}

/// Consumer for ordered tool invocation events.
#[async_trait]
pub trait ToolEventSink: Send + Sync {
    /// Receives one event before the next event is emitted.
    async fn emit(&self, event: ToolEvent);
}

/// Execution-scoped cancellation and event delivery.
#[derive(Clone, Default)]
pub struct ToolCallControl {
    /// Cooperative cancellation signal.
    pub cancellation: CancellationToken,
    /// Optional ordered event consumer.
    pub events: Option<Arc<dyn ToolEventSink>>,
}

/// Options for one transport-neutral tool invocation.
#[derive(Clone, Default)]
pub struct ToolCallOptions {
    /// Environment values used to parse command environment fields.
    pub environment: EnvironmentSource,
    /// Command config defaults.
    pub config: ConfigSource,
    /// CLI-level global option overrides.
    pub globals: Option<Value>,
    /// Transport request metadata.
    pub request: Option<RequestContext>,
    /// Execution-scoped cancellation and events.
    pub control: ToolCallControl,
}

impl ToolCallOptions {
    /// Creates options that do not read process environment or filesystem config.
    pub fn isolated() -> Self {
        Self {
            environment: EnvironmentSource::Empty,
            config: ConfigSource::Disabled,
            ..Self::default()
        }
    }
}

/// Failure while resolving a reusable tool catalog.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolCatalogError {
    /// Two commands resolve to the same exposed tool name.
    #[error("Tool name \"{name}\" is used by both \"{first}\" and \"{second}\"")]
    DuplicateName {
        /// Colliding exposed name.
        name: String,
        /// First canonical command path.
        first: String,
        /// Second canonical command path.
        second: String,
    },
}

/// Result of one transport-neutral tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolCallOutcome {
    /// Successful command output.
    Ok {
        /// Structured command result.
        data: Value,
        /// Optional follow-up commands.
        cta: Option<CtaBlock>,
    },
    /// Structured command failure.
    Error {
        /// Machine-readable error code.
        code: String,
        /// Human-readable error message.
        message: String,
        /// Whether retrying may succeed.
        retryable: Option<bool>,
        /// Per-field validation failures.
        field_errors: Option<Vec<FieldErrorOutput>>,
        /// Optional follow-up commands.
        cta: Option<CtaBlock>,
        /// Optional process-style exit code.
        exit_code: Option<i32>,
    },
}

#[derive(Clone)]
pub(crate) struct ResolvedTool {
    pub(crate) definition: ToolDefinition,
    pub(crate) command: Arc<CommandDef>,
    pub(crate) middleware: Vec<MiddlewareFn>,
    path: String,
}

/// A reusable catalog of tools resolved from an incurs CLI.
#[derive(Clone)]
pub struct ToolCatalog {
    name: String,
    version: Option<String>,
    env_fields: Vec<FieldMeta>,
    globals_fields: Vec<FieldMeta>,
    config: Option<ConfigOptions>,
    root_middleware: Vec<MiddlewareFn>,
    tools: BTreeMap<String, ResolvedTool>,
}

impl ToolCatalog {
    pub(crate) fn from_parts(
        name: String,
        version: Option<String>,
        commands: &BTreeMap<String, CommandEntry>,
        root_middleware: &[MiddlewareFn],
        env_fields: &[FieldMeta],
        globals_fields: &[FieldMeta],
        config: Option<&ConfigOptions>,
    ) -> Result<Self, ToolCatalogError> {
        let mut tools = BTreeMap::new();
        collect(commands, &[], &[], &mut tools)?;
        Ok(Self {
            name,
            version,
            env_fields: env_fields.to_vec(),
            globals_fields: globals_fields.to_vec(),
            config: config.cloned(),
            root_middleware: root_middleware.to_vec(),
            tools,
        })
    }

    /// Returns the CLI name that owns this catalog.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the CLI version, when configured.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Lists tool definitions in stable name order.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    /// Returns one tool definition by its exposed name.
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name).map(|tool| &tool.definition)
    }

    /// Invokes one tool with flat JSON arguments.
    pub async fn call(
        &self,
        name: &str,
        mut arguments: BTreeMap<String, Value>,
        options: ToolCallOptions,
    ) -> ToolCallOutcome {
        let Some(tool) = self.tools.get(name) else {
            return ToolCallOutcome::Error {
                code: "TOOL_NOT_FOUND".to_string(),
                message: format!("Unknown tool: {name}"),
                retryable: Some(false),
                field_errors: None,
                cta: None,
                exit_code: Some(1),
            };
        };

        let environment = match options.environment {
            EnvironmentSource::DeclaredHost => {
                declared_environment(&self.env_fields, &tool.command.env_fields)
            }
            EnvironmentSource::Values(values) => filter_environment(
                values,
                self.env_fields.iter().chain(&tool.command.env_fields),
            ),
            EnvironmentSource::Empty => HashMap::new(),
        };
        let defaults = match self.resolve_config(&options.config, &tool.path) {
            Ok(defaults) => defaults,
            Err(message) => return tool_error("CONFIG_ERROR", message),
        };
        if let Some(defaults) = &defaults {
            for (name, value) in defaults {
                arguments
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        let globals = match resolve_globals(options.globals, &self.globals_fields) {
            Ok(globals) => globals,
            Err(message) => return tool_error("VALIDATION_ERROR", message),
        };
        if options.control.cancellation.is_cancelled() {
            return tool_error("CANCELLED", "Tool call cancelled".to_string());
        }
        let cancellation = options.control.cancellation.clone();
        let mut middleware = self.root_middleware.clone();
        middleware.extend(tool.middleware.iter().cloned());
        middleware.extend(tool.command.middleware.iter().cloned());
        let execution = command::execute(
            Arc::clone(&tool.command),
            ExecuteOptions {
                agent: true,
                argv: Vec::new(),
                defaults: None,
                display_name: self.name.clone(),
                env_fields: self.env_fields.clone(),
                env_source: environment,
                format: Format::Json,
                format_explicit: true,
                globals,
                input_options: arguments,
                middlewares: middleware,
                name: self.name.clone(),
                parse_mode: ParseMode::Flat,
                path: tool.path.clone(),
                request: options.request,
                vars_fields: Vec::new(),
                version: self.version.clone(),
            },
        );
        tokio::pin!(execution);
        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                return tool_error("CANCELLED", "Tool call cancelled".to_string());
            }
            result = &mut execution => result,
        };

        match result {
            // A wrapped process exit code is part of the command's data for
            // tool callers; it has no meaning as a transport-level status.
            command::InternalResult::Ok {
                data,
                cta,
                exit_code: _,
            } => ToolCallOutcome::Ok { data, cta },
            command::InternalResult::Error {
                code,
                message,
                retryable,
                field_errors,
                cta,
                exit_code,
            } => ToolCallOutcome::Error {
                code,
                message,
                retryable,
                field_errors: field_errors.map(field_error_outputs),
                cta,
                exit_code,
            },
            command::InternalResult::Stream(mut stream) => {
                let mut data = Vec::new();
                loop {
                    let value = tokio::select! {
                        _ = options.control.cancellation.cancelled() => {
                            return tool_error("CANCELLED", "Tool call cancelled".to_string());
                        }
                        value = stream.next() => value,
                    };
                    let Some(value) = value else {
                        break;
                    };
                    emit_event(
                        &options.control,
                        ToolEvent::Chunk {
                            data: value.clone(),
                        },
                    )
                    .await;
                    data.push(value);
                }
                ToolCallOutcome::Ok {
                    data: Value::Array(data),
                    cta: None,
                }
            }
            command::InternalResult::RecordStream(mut stream) => {
                let mut data = Vec::new();
                loop {
                    let record = tokio::select! {
                        _ = options.control.cancellation.cancelled() => {
                            return tool_error("CANCELLED", "Tool call cancelled".to_string());
                        }
                        record = stream.next() => record,
                    };
                    let Some(record) = record else {
                        break;
                    };
                    match record {
                        StreamRecord::Chunk(value) => {
                            emit_event(
                                &options.control,
                                ToolEvent::Chunk {
                                    data: value.clone(),
                                },
                            )
                            .await;
                            data.push(value);
                        }
                        StreamRecord::Ok { cta } => {
                            return ToolCallOutcome::Ok {
                                data: Value::Array(data),
                                cta,
                            };
                        }
                        StreamRecord::Error {
                            code,
                            message,
                            retryable,
                            exit_code,
                            cta,
                        } => {
                            return ToolCallOutcome::Error {
                                code,
                                message,
                                retryable: Some(retryable),
                                field_errors: None,
                                cta,
                                exit_code,
                            };
                        }
                    }
                }
                ToolCallOutcome::Ok {
                    data: Value::Array(data),
                    cta: None,
                }
            }
        }
    }

    fn resolve_config(
        &self,
        source: &ConfigSource,
        command_path: &str,
    ) -> Result<Option<BTreeMap<String, Value>>, String> {
        let tree = match source {
            ConfigSource::Disabled => return Ok(None),
            ConfigSource::Values(values) => Some(values.clone()),
            ConfigSource::Path(path) => {
                Some(crate::config::load_config(path).map_err(|error| error.to_string())?)
            }
            ConfigSource::Auto => {
                let Some(config) = &self.config else {
                    return Ok(None);
                };
                let Some(path) = crate::config::resolve_config_path(None, &config.files) else {
                    return Ok(None);
                };
                crate::config::load_config(&path).ok()
            }
        };
        tree.map(|tree| {
            crate::config::extract_command_section(&tree, &self.name, command_path)
                .map_err(|error| error.to_string())
        })
        .transpose()
        .map(Option::flatten)
    }

    pub(crate) fn resolved(&self) -> impl Iterator<Item = &ResolvedTool> {
        self.tools.values()
    }
}

async fn emit_event(control: &ToolCallControl, event: ToolEvent) {
    if let Some(events) = &control.events {
        events.emit(event).await;
    }
}

impl Cli {
    /// Tries to resolve this CLI into a reusable transport-neutral tool catalog.
    pub fn try_tool_catalog(&self) -> Result<ToolCatalog, ToolCatalogError> {
        ToolCatalog::from_parts(
            self.name.clone(),
            self.version.clone(),
            &self.commands,
            &self.middleware,
            &self.env_fields,
            &self.globals_fields,
            self.config.as_ref(),
        )
    }

    /// Resolves this CLI into a reusable transport-neutral tool catalog.
    ///
    /// # Panics
    ///
    /// Panics when two commands use the same exposed tool name. Use
    /// [`Cli::try_tool_catalog`] to handle that configuration error.
    pub fn tool_catalog(&self) -> ToolCatalog {
        self.try_tool_catalog()
            .expect("CLI command graph must have unique tool names")
    }
}

fn collect(
    commands: &BTreeMap<String, CommandEntry>,
    prefix: &[String],
    parent_middleware: &[MiddlewareFn],
    result: &mut BTreeMap<String, ResolvedTool>,
) -> Result<(), ToolCatalogError> {
    for (name, entry) in commands {
        let mut path = prefix.to_vec();
        path.push(name.clone());
        match entry {
            CommandEntry::Leaf(command) => {
                let mcp = command.handler.mcp_options().cloned().unwrap_or_default();
                if !mcp.enabled {
                    continue;
                }
                let name = mcp.name.clone().unwrap_or_else(|| path.join("_"));
                let command_path = path.join(" ");
                if let Some(previous) = result.get(&name) {
                    return Err(ToolCatalogError::DuplicateName {
                        name,
                        first: previous.path.clone(),
                        second: command_path,
                    });
                }
                let input_schema =
                    command
                        .handler
                        .mcp_input_schema()
                        .cloned()
                        .unwrap_or_else(|| {
                            crate::mcp::build_tool_schema(
                                &command.args_fields,
                                &command.options_fields,
                            )
                        });
                result.insert(
                    name.clone(),
                    ResolvedTool {
                        definition: ToolDefinition {
                            name,
                            description: mcp
                                .description
                                .clone()
                                .or_else(|| command.description.clone())
                                .unwrap_or_default(),
                            input_schema,
                            output_schema: command.output_schema.clone(),
                            annotations: mcp.annotations.clone(),
                            instructions: mcp.instructions.clone(),
                            examples: command
                                .examples
                                .iter()
                                .map(|example| ToolExample {
                                    command: example.command.clone(),
                                    description: example.description.clone(),
                                })
                                .collect(),
                            result_content: mcp.result_content.clone(),
                        },
                        command: Arc::clone(command),
                        middleware: parent_middleware.to_vec(),
                        path: command_path,
                    },
                );
            }
            CommandEntry::Group {
                commands,
                middleware,
                ..
            } => {
                let mut inherited = parent_middleware.to_vec();
                inherited.extend(middleware.iter().cloned());
                collect(commands, &path, &inherited, result)?;
            }
            CommandEntry::FetchGateway { .. } => {}
        }
    }
    Ok(())
}

fn declared_environment(
    cli_fields: &[FieldMeta],
    command_fields: &[FieldMeta],
) -> HashMap<String, String> {
    filter_environment(
        std::env::vars().collect(),
        cli_fields.iter().chain(command_fields),
    )
}

fn filter_environment<'a>(
    source: HashMap<String, String>,
    fields: impl Iterator<Item = &'a FieldMeta>,
) -> HashMap<String, String> {
    fields
        .filter_map(|field| {
            let name = field.env_name.unwrap_or(field.name);
            source
                .get(name)
                .map(|value| (name.to_string(), value.clone()))
        })
        .collect()
}

fn resolve_globals(overrides: Option<Value>, fields: &[FieldMeta]) -> Result<Value, String> {
    let input = match overrides.unwrap_or_else(|| Value::Object(serde_json::Map::new())) {
        Value::Object(values) => values.into_iter().collect(),
        _ => return Err("Global options must be an object".to_string()),
    };
    crate::parser::parse_global_input(input, fields)
        .map(|(globals, _)| globals)
        .map_err(|error| error.to_string())
}

fn tool_error(code: &str, message: String) -> ToolCallOutcome {
    ToolCallOutcome::Error {
        code: code.to_string(),
        message,
        retryable: Some(false),
        field_errors: None,
        cta: None,
        exit_code: Some(1),
    }
}

fn field_error_outputs(errors: Vec<FieldError>) -> Vec<FieldErrorOutput> {
    errors.iter().map(FieldErrorOutput::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_call_options_disable_host_sources() {
        let options = ToolCallOptions::isolated();
        assert!(matches!(options.environment, EnvironmentSource::Empty));
        assert!(matches!(options.config, ConfigSource::Disabled));
    }
    use crate::cli::ConfigOptions;
    use crate::command::{CommandContext, CommandHandler, Example, McpCommandOptions};
    use crate::output::CommandResult;
    use crate::schema::FieldType;
    use tokio::sync::Mutex;

    struct Echo;

    #[async_trait::async_trait]
    impl CommandHandler for Echo {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: ctx.options,
                cta: None,
                exit_code: None,
            }
        }
    }

    struct Context;

    #[async_trait::async_trait]
    impl CommandHandler for Context {
        async fn run(&self, ctx: CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: serde_json::json!({
                    "env": ctx.env,
                    "globals": ctx.globals,
                    "options": ctx.options,
                    "request": ctx.request.map(|request| request.path),
                }),
                cta: None,
                exit_code: None,
            }
        }
    }

    struct Streaming;

    #[async_trait::async_trait]
    impl CommandHandler for Streaming {
        async fn run(&self, _ctx: CommandContext) -> CommandResult {
            CommandResult::Stream(Box::pin(futures::stream::iter([
                serde_json::json!(1),
                serde_json::json!(2),
            ])))
        }
    }

    struct Waiting;

    #[async_trait::async_trait]
    impl CommandHandler for Waiting {
        async fn run(&self, _ctx: CommandContext) -> CommandResult {
            futures::future::pending().await
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<ToolEvent>>);

    #[async_trait::async_trait]
    impl ToolEventSink for Events {
        async fn emit(&self, event: ToolEvent) {
            self.0.lock().await.push(event);
        }
    }

    fn field(
        name: &'static str,
        env_name: Option<&'static str>,
        default: Option<Value>,
    ) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: name.replace('_', "-"),
            description: None,
            field_type: FieldType::String,
            required: false,
            default,
            alias: None,
            deprecated: false,
            env_name,
        }
    }

    #[tokio::test]
    async fn resolves_and_calls_commands() {
        let catalog = Cli::create("demo")
            .version("1.0.0")
            .command("echo", CommandDef::build("echo", Echo).done())
            .tool_catalog();

        assert_eq!(catalog.name(), "demo");
        assert_eq!(catalog.version(), Some("1.0.0"));
        assert_eq!(catalog.definitions()[0].name, "echo");

        let outcome = catalog
            .call(
                "echo",
                BTreeMap::from([("message".to_string(), Value::String("hi".to_string()))]),
                ToolCallOptions::default(),
            )
            .await;
        assert!(matches!(
            outcome,
            ToolCallOutcome::Ok { data, .. } if data["message"] == "hi"
        ));
    }

    #[tokio::test]
    async fn reports_unknown_tools() {
        let outcome = Cli::create("demo")
            .tool_catalog()
            .call("missing", BTreeMap::new(), ToolCallOptions::default())
            .await;
        assert!(matches!(
            outcome,
            ToolCallOutcome::Error { code, .. } if code == "TOOL_NOT_FOUND"
        ));
    }

    #[tokio::test]
    async fn resolves_declared_environment_globals_and_config_values() {
        let mut command = CommandDef::build("deploy", Context).done();
        command.env_fields = vec![field("token", Some("DEMO_TOKEN"), None)];
        command.options_fields = vec![field("region", None, None)];
        let catalog = Cli::create("demo")
            .globals_fields(vec![field(
                "profile",
                None,
                Some(Value::String("default".to_string())),
            )])
            .config(ConfigOptions {
                flag: "config".to_string(),
                files: Vec::new(),
            })
            .group(Cli::create("admin").command("deploy", command))
            .tool_catalog();
        let outcome = catalog
            .call(
                "admin_deploy",
                BTreeMap::new(),
                ToolCallOptions {
                    environment: EnvironmentSource::Values(HashMap::from([
                        ("DEMO_TOKEN".to_string(), "secret".to_string()),
                        ("UNDECLARED".to_string(), "hidden".to_string()),
                    ])),
                    config: ConfigSource::Values(BTreeMap::from([(
                        "commands".to_string(),
                        serde_json::json!({
                            "admin": {
                                "commands": {
                                    "deploy": {
                                        "options": { "region": "us-east" }
                                    }
                                }
                            }
                        }),
                    )])),
                    globals: None,
                    request: Some(RequestContext {
                        path: "test-request".to_string(),
                        ..RequestContext::default()
                    }),
                    control: ToolCallControl::default(),
                },
            )
            .await;

        assert!(
            matches!(
            outcome,
            ToolCallOutcome::Ok { ref data, .. }
                if *data == serde_json::json!({
                    "env": { "token": "secret" },
                    "globals": { "profile": "default" },
                    "options": { "region": "us-east" },
                    "request": "test-request",
                })
            ),
            "{outcome:#?}"
        );
    }

    #[test]
    fn exposes_examples_and_rejects_duplicate_tool_names() {
        let command = || {
            CommandDef::build("echo", Echo)
                .examples(vec![Example {
                    command: "echo --message hi".to_string(),
                    description: Some("Echo a greeting".to_string()),
                }])
                .mcp(McpCommandOptions {
                    name: Some("same".to_string()),
                    ..McpCommandOptions::default()
                })
                .done()
        };
        let cli = Cli::create("demo")
            .command("first", command())
            .command("second", command());

        let error = cli.try_tool_catalog().err().expect("duplicate must fail");
        assert!(matches!(
            error,
            ToolCatalogError::DuplicateName { name, .. } if name == "same"
        ));

        let definition = Cli::create("demo")
            .command("echo", command())
            .tool_catalog()
            .definitions()
            .remove(0);
        assert_eq!(definition.examples[0].command, "echo --message hi");
    }

    #[tokio::test]
    async fn emits_ordered_chunks_and_honors_cancellation() {
        let catalog = Cli::create("demo")
            .command("stream", CommandDef::build("stream", Streaming).done())
            .tool_catalog();
        let events = Arc::new(Events::default());
        let outcome = catalog
            .call(
                "stream",
                BTreeMap::new(),
                ToolCallOptions {
                    control: ToolCallControl {
                        events: Some(events.clone()),
                        ..ToolCallControl::default()
                    },
                    ..ToolCallOptions::default()
                },
            )
            .await;
        assert!(matches!(
            outcome,
            ToolCallOutcome::Ok { data, .. } if data == serde_json::json!([1, 2])
        ));
        assert!(matches!(
            events.0.lock().await.as_slice(),
            [ToolEvent::Chunk { data: first }, ToolEvent::Chunk { data: second }]
                if *first == serde_json::json!(1) && *second == serde_json::json!(2)
        ));

        let control = ToolCallControl::default();
        control.cancellation.cancel();
        let outcome = catalog
            .call(
                "stream",
                BTreeMap::new(),
                ToolCallOptions {
                    control,
                    ..ToolCallOptions::default()
                },
            )
            .await;
        assert!(matches!(
            outcome,
            ToolCallOutcome::Error { code, .. } if code == "CANCELLED"
        ));
    }

    #[tokio::test]
    async fn cancels_an_active_non_streaming_command() {
        let catalog = Cli::create("demo")
            .command("wait", CommandDef::build("wait", Waiting).done())
            .tool_catalog();
        let control = ToolCallControl::default();
        let task_control = control.clone();
        let call = tokio::spawn(async move {
            catalog
                .call(
                    "wait",
                    BTreeMap::new(),
                    ToolCallOptions {
                        control: task_control,
                        ..ToolCallOptions::default()
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        control.cancellation.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            outcome,
            ToolCallOutcome::Error { code, .. } if code == "CANCELLED"
        ));
    }
}
