//! Unified command execution for the incurs framework.
//!
//! This module is the heart of the three-transport architecture. The
//! [`execute`] function is called by CLI, HTTP, and MCP transports with
//! different [`ParseMode`] values to handle input parsing, middleware
//! composition, and handler invocation uniformly.
//!
//! Ported from `src/internal/command.ts`.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, RwLock};

use crate::errors::FieldError;
use crate::middleware::{self, MiddlewareContext, MiddlewareFn};
use crate::output::*;
use crate::schema::FieldMeta;
use crate::streaming;

/// How to parse input for a command.
///
/// Each transport uses a different parse mode to extract positional arguments
/// and named options from the input it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    /// CLI: parse both args and options from argv tokens.
    Argv,
    /// HTTP: args from URL path segments, options from body/query.
    Split,
    /// MCP: all params from JSON, split by schema field names.
    Flat,
}

/// A usage example for a command.
#[derive(Debug, Clone)]
pub struct Example {
    /// The command invocation (without the CLI name prefix).
    pub command: String,
    /// A short description of what this example demonstrates.
    pub description: Option<String>,
}

/// Hints describing an MCP tool's behavior to clients.
#[derive(Debug, Clone, Default)]
pub struct McpAnnotations {
    /// A human-readable title for the tool.
    pub title: Option<String>,
    /// Whether the tool does not modify its environment.
    pub read_only_hint: Option<bool>,
    /// Whether the tool may perform destructive updates.
    pub destructive_hint: Option<bool>,
    /// Whether repeated calls have no additional effect.
    pub idempotent_hint: Option<bool>,
    /// Whether the tool may interact with external entities.
    pub open_world_hint: Option<bool>,
}

/// MCP exposure and metadata overrides for a command.
#[derive(Debug, Clone)]
pub struct McpCommandOptions {
    /// Whether this command is exposed through MCP.
    pub enabled: bool,
    /// Override for the exposed MCP tool name.
    pub name: Option<String>,
    /// Override for the exposed MCP tool description.
    pub description: Option<String>,
    /// Tool-specific instructions exposed through MCP metadata.
    pub instructions: Option<String>,
    /// Behavioral annotations exposed to MCP clients.
    pub annotations: Option<McpAnnotations>,
    /// Whether skill output should require user confirmation before execution.
    pub destructive: bool,
}

impl Default for McpCommandOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            name: None,
            description: None,
            instructions: None,
            annotations: None,
            destructive: false,
        }
    }
}

/// A registered command definition (leaf node in the command tree).
///
/// Contains the command's schema metadata, handler, middleware, and
/// display information (help text, examples).
pub struct CommandDef {
    /// The command name.
    pub name: String,
    /// A short description of what the command does.
    pub description: Option<String>,
    /// Schema for positional arguments.
    pub args_fields: Vec<FieldMeta>,
    /// Schema for named options/flags.
    pub options_fields: Vec<FieldMeta>,
    /// Schema for environment variables.
    pub env_fields: Vec<FieldMeta>,
    /// Map of option names to single-char aliases.
    pub aliases: HashMap<String, char>,
    /// Alternative names this command can be invoked by.
    pub command_aliases: Vec<String>,
    /// Usage examples displayed in help output.
    pub examples: Vec<Example>,
    /// Plain-text hint displayed after examples in help output.
    pub hint: Option<String>,
    /// Default output format for this command. Overridden by `--format`.
    pub format: Option<Format>,
    /// Output policy controlling who sees this command's output.
    pub output_policy: Option<OutputPolicy>,
    /// The command handler, type-erased behind the [`CommandHandler`] trait.
    pub handler: Box<dyn CommandHandler>,
    /// Per-command middleware that runs after root and group middleware.
    pub middleware: Vec<MiddlewareFn>,
    /// JSON Schema for the command's output type (used by `--schema`).
    pub output_schema: Option<Value>,
}

impl CommandDef {
    /// Creates a new command builder with the given name and handler.
    ///
    /// Use with derive macros for ergonomic command definition:
    /// ```ignore
    /// #[derive(incurs::Args, serde::Deserialize)]
    /// struct GetArgs {
    ///     /// The user ID
    ///     id: u64,
    /// }
    ///
    /// #[derive(incurs::Options, serde::Deserialize)]
    /// struct GetOptions {
    ///     /// Output format
    ///     #[incur(alias = "f", default = "json")]
    ///     format: String,
    /// }
    ///
    /// CommandDef::build("get", handler)
    ///     .description("Get a user")
    ///     .args::<GetArgs>()
    ///     .options::<GetOptions>()
    ///     .done()
    /// ```
    pub fn build(
        name: impl Into<String>,
        handler: impl CommandHandler + 'static,
    ) -> CommandBuilder {
        CommandBuilder {
            def: CommandDef {
                name: name.into(),
                description: None,
                args_fields: Vec::new(),
                options_fields: Vec::new(),
                env_fields: Vec::new(),
                aliases: HashMap::new(),
                command_aliases: Vec::new(),
                examples: Vec::new(),
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(handler),
                middleware: Vec::new(),
                output_schema: None,
            },
            mcp: None,
        }
    }

    /// Creates a fully typed command builder.
    ///
    /// The input schemas are derived from [`crate::schema::IncurSchema`], and
    /// the output schema is derived from [`schemars::JsonSchema`]. The handler
    /// receives typed values after the shared transport parser validates the
    /// raw CLI, HTTP, or MCP input.
    pub fn typed<Args, Options, Env, Output, Handler, HandlerFuture>(
        name: impl Into<String>,
        handler: Handler,
    ) -> CommandBuilder
    where
        Args: crate::schema::IncurSchema + Send + Sync + 'static,
        Options: crate::schema::IncurSchema + Send + Sync + 'static,
        Env: crate::schema::IncurSchema + Send + Sync + 'static,
        Output: JsonSchema + Serialize + Send + Sync + 'static,
        Handler: Fn(TypedContext<Args, Options, Env>) -> HandlerFuture + Send + Sync + 'static,
        HandlerFuture: Future<Output = TypedResult<Output>> + Send + 'static,
    {
        let output_schema = serde_json::to_value(schemars::schema_for!(Output))
            .expect("schemars output must serialize to JSON");
        let mut builder = Self::build(
            name,
            TypedHandler::<Args, Options, Env, Output, Handler> {
                handler,
                marker: PhantomData,
            },
        )
        .args::<Args>()
        .options::<Options>()
        .env::<Env>();
        builder.def.output_schema = Some(output_schema);
        builder
    }
}

/// Typed command execution context.
pub struct TypedContext<Args, Options, Env> {
    /// Whether the consumer is an agent.
    pub agent: bool,
    /// Validated positional arguments.
    pub args: Args,
    /// Actual CLI display name used for user-facing messages.
    pub display_name: String,
    /// Validated environment variables.
    pub env: Env,
    /// Parsed CLI-level global options.
    pub globals: Value,
    /// Validated named options.
    pub options: Options,
    /// Transport request metadata for HTTP and MCP executions.
    pub request: Option<RequestContext>,
    /// Resolved output format.
    pub format: Format,
    /// Whether the output format was explicitly requested.
    pub format_explicit: bool,
    /// Canonical CLI name.
    pub name: String,
    /// Middleware variables visible to the command.
    pub vars: Value,
    /// CLI version.
    pub version: Option<String>,
}

/// Result returned by a typed command handler.
pub enum TypedResult<Output> {
    /// Successful typed output and optional CTA metadata.
    Ok {
        /// Serializable command output.
        data: Output,
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
        retryable: bool,
        /// Optional process exit code.
        exit_code: Option<i32>,
        /// Optional follow-up commands.
        cta: Option<CtaBlock>,
    },
}

impl<Output> TypedResult<Output> {
    /// Creates a successful typed result.
    pub fn ok(data: Output) -> Self {
        Self::Ok { data, cta: None }
    }

    /// Creates a successful typed result with CTA metadata.
    pub fn ok_with_cta(data: Output, cta: CtaBlock) -> Self {
        Self::Ok {
            data,
            cta: Some(cta),
        }
    }

    /// Creates a structured typed error result.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
            retryable: false,
            exit_code: Some(1),
            cta: None,
        }
    }
}

type TypedHandlerMarker<Args, Options, Env, Output> =
    PhantomData<fn() -> (Args, Options, Env, Output)>;

struct TypedHandler<Args, Options, Env, Output, Handler> {
    handler: Handler,
    marker: TypedHandlerMarker<Args, Options, Env, Output>,
}

#[async_trait::async_trait]
impl<Args, Options, Env, Output, Handler, HandlerFuture> CommandHandler
    for TypedHandler<Args, Options, Env, Output, Handler>
where
    Args: crate::schema::IncurSchema + Send + Sync + 'static,
    Options: crate::schema::IncurSchema + Send + Sync + 'static,
    Env: crate::schema::IncurSchema + Send + Sync + 'static,
    Output: Serialize + Send + Sync + 'static,
    Handler: Fn(TypedContext<Args, Options, Env>) -> HandlerFuture + Send + Sync + 'static,
    HandlerFuture: Future<Output = TypedResult<Output>> + Send + 'static,
{
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let args = match typed_input::<Args>(&ctx.args, "args") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let options = match typed_input::<Options>(&ctx.options, "options") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let env = match typed_input::<Env>(&ctx.env, "env") {
            Ok(value) => value,
            Err(result) => return result,
        };
        match (self.handler)(TypedContext {
            agent: ctx.agent,
            args,
            display_name: ctx.display_name,
            env,
            globals: ctx.globals,
            options,
            request: ctx.request,
            format: ctx.format,
            format_explicit: ctx.format_explicit,
            name: ctx.name,
            vars: ctx.vars,
            version: ctx.version,
        })
        .await
        {
            TypedResult::Ok { data, cta } => match serde_json::to_value(data) {
                Ok(data) => CommandResult::Ok { data, cta },
                Err(error) => CommandResult::Error {
                    code: "SERIALIZATION_ERROR".to_string(),
                    message: error.to_string(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                },
            },
            TypedResult::Error {
                code,
                message,
                retryable,
                exit_code,
                cta,
            } => CommandResult::Error {
                code,
                message,
                retryable,
                exit_code,
                cta,
            },
        }
    }
}

fn typed_input<Input: crate::schema::IncurSchema>(
    value: &Value,
    kind: &str,
) -> std::result::Result<Input, CommandResult> {
    let raw = value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Input::from_raw(&raw).map_err(|error| CommandResult::Error {
        code: "VALIDATION_ERROR".to_string(),
        message: format!("Failed to parse typed {kind}: {error}"),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    })
}

/// Builder for constructing a [`CommandDef`] ergonomically with derive macros.
pub struct CommandBuilder {
    def: CommandDef,
    mcp: Option<McpCommandOptions>,
}

impl CommandBuilder {
    /// Sets the command description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.def.description = Some(desc.into());
        self
    }

    /// Sets positional args from a type that implements `IncurSchema`.
    pub fn args<T: crate::schema::IncurSchema>(mut self) -> Self {
        self.def.args_fields = T::fields();
        self
    }

    /// Sets alternative names this command can be invoked by.
    pub fn command_aliases(mut self, aliases: Vec<String>) -> Self {
        self.def.command_aliases = aliases;
        self
    }

    /// Sets named options from a type that implements `IncurSchema`.
    /// Automatically extracts aliases from field metadata.
    pub fn options<T: crate::schema::IncurSchema>(mut self) -> Self {
        let fields = T::fields();
        for field in &fields {
            if let Some(alias) = field.alias {
                self.def.aliases.insert(field.name.to_string(), alias);
            }
        }
        self.def.options_fields = fields;
        self
    }

    /// Sets env var bindings from a type that implements `IncurSchema`.
    pub fn env<T: crate::schema::IncurSchema>(mut self) -> Self {
        self.def.env_fields = T::fields();
        self
    }

    /// Adds usage examples.
    pub fn examples(mut self, examples: Vec<Example>) -> Self {
        self.def.examples = examples;
        self
    }

    /// Sets the hint text.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.def.hint = Some(hint.into());
        self
    }

    /// Sets the default output format.
    pub fn format(mut self, format: crate::output::Format) -> Self {
        self.def.format = Some(format);
        self
    }

    /// Configures MCP exposure and metadata overrides.
    pub fn mcp(mut self, options: McpCommandOptions) -> Self {
        self.mcp = Some(options);
        self
    }

    /// Marks this command as destructive in generated skill guidance.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.mcp.get_or_insert_with(Default::default).destructive = destructive;
        self
    }

    /// Finishes building and returns the [`CommandDef`].
    pub fn done(mut self) -> CommandDef {
        if let Some(options) = self.mcp {
            self.def.handler = Box::new(McpHandler {
                handler: self.def.handler,
                options,
            });
        }
        self.def
    }
}

struct McpHandler {
    handler: Box<dyn CommandHandler>,
    options: McpCommandOptions,
}

#[async_trait::async_trait]
impl CommandHandler for McpHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        self.handler.run(ctx).await
    }

    fn mcp_options(&self) -> Option<&McpCommandOptions> {
        Some(&self.options)
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        self.handler.mcp_input_schema()
    }
}

/// Trait for command handlers.
///
/// This trait provides type erasure for command handlers so that the framework
/// can store heterogeneous handlers in the command tree. Implementations
/// receive a [`CommandContext`] and return a [`CommandResult`].
#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync {
    /// Execute the command with the given context.
    async fn run(&self, ctx: CommandContext) -> CommandResult;

    /// Returns MCP metadata supplied by the command builder, when present.
    fn mcp_options(&self) -> Option<&McpCommandOptions> {
        None
    }

    /// Returns an exact MCP input schema override, when available.
    fn mcp_input_schema(&self) -> Option<&Value> {
        None
    }
}

/// Transport request metadata available to HTTP and MCP command executions.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    /// Request headers as lowercase names and string values.
    pub headers: HashMap<String, String>,
    /// Request method or transport operation.
    pub method: String,
    /// Request path or MCP tool name.
    pub path: String,
}

/// The context passed to a command's `run` function.
///
/// Contains all parsed input (args, options, env) and metadata about the
/// current execution (format, agent mode, CLI name, etc.).
pub struct CommandContext {
    /// Whether the consumer is an agent (stdout is not a TTY).
    pub agent: bool,
    /// Parsed positional arguments as a JSON value.
    pub args: Value,
    /// Parsed environment variables as a JSON value.
    pub env: Value,
    /// The actual CLI display name used for user-facing messages.
    pub display_name: String,
    /// Parsed CLI-level global options.
    pub globals: Value,
    /// Parsed named options as a JSON value.
    pub options: Value,
    /// Transport request metadata for HTTP and MCP invocations.
    pub request: Option<RequestContext>,
    /// The resolved output format.
    pub format: Format,
    /// Whether the format was explicitly requested by the user.
    pub format_explicit: bool,
    /// The CLI name.
    pub name: String,
    /// Middleware variables set by upstream middleware.
    pub vars: Value,
    /// The CLI version string.
    pub version: Option<String>,
}

/// Options for the unified [`execute`] function.
///
/// Each transport constructs these options differently, but the execute
/// function processes them uniformly.
pub struct ExecuteOptions {
    /// Whether the consumer is an agent.
    pub agent: bool,
    /// Raw positional tokens. For HTTP/MCP, pass an empty vec.
    pub argv: Vec<String>,
    /// Default option values from config file.
    pub defaults: Option<BTreeMap<String, Value>>,
    /// The actual CLI display name used for user-facing messages.
    pub display_name: String,
    /// CLI-level env field metadata.
    pub env_fields: Vec<FieldMeta>,
    /// Source for environment variables (key-value map).
    pub env_source: HashMap<String, String>,
    /// The resolved output format.
    pub format: Format,
    /// Whether the format was explicitly requested.
    pub format_explicit: bool,
    /// Parsed CLI-level global options.
    pub globals: Value,
    /// Raw parsed options (from query params, JSON body, or MCP params).
    pub input_options: BTreeMap<String, Value>,
    /// Middleware handlers (root + group + command, already collected).
    pub middlewares: Vec<MiddlewareFn>,
    /// The CLI name.
    pub name: String,
    /// How to parse input for this invocation.
    pub parse_mode: ParseMode,
    /// The resolved command path (e.g. `"users list"`).
    pub path: String,
    /// Transport request metadata for HTTP and MCP invocations.
    pub request: Option<RequestContext>,
    /// Vars field metadata for middleware variables.
    pub vars_fields: Vec<FieldMeta>,
    /// The CLI version string.
    pub version: Option<String>,
}

/// Internal execute result (before output envelope wrapping).
///
/// The three-transport architecture returns this from [`execute`]. Each
/// transport then wraps it in its own output format (CLI writes to stdout,
/// HTTP returns a Response, MCP returns tool results).
pub enum InternalResult {
    /// Successful execution with data.
    Ok { data: Value, cta: Option<CtaBlock> },
    /// Failed execution with error details.
    Error {
        code: String,
        message: String,
        retryable: Option<bool>,
        field_errors: Option<Vec<FieldError>>,
        cta: Option<CtaBlock>,
        exit_code: Option<i32>,
    },
    /// Streaming output. The transport consumes this stream and writes
    /// each item incrementally (JSONL) or buffers them.
    Stream(Pin<Box<dyn Stream<Item = Value> + Send>>),
    /// Streaming output with explicit terminal semantics.
    RecordStream(Pin<Box<dyn Stream<Item = crate::output::StreamRecord> + Send>>),
}

struct InputError {
    message: String,
    field_errors: Option<Vec<FieldError>>,
}

impl From<crate::errors::ParseError> for InputError {
    fn from(error: crate::errors::ParseError) -> Self {
        Self {
            message: error.to_string(),
            field_errors: None,
        }
    }
}

/// Unified command execution used by CLI, HTTP, and MCP transports.
///
/// This function:
/// 1. Parses input based on [`ParseMode`] (argv, split, or flat).
/// 2. Parses environment variables from the env source.
/// 3. Builds a [`CommandContext`].
/// 4. Composes and runs middleware in onion style.
/// 5. Calls the command handler.
/// 6. Returns an [`InternalResult`] for the transport to render.
///
/// For streaming handlers with middleware, the function uses a oneshot
/// channel so that middleware "after" hooks run after the stream is
/// consumed by the transport.
pub async fn execute(command: Arc<CommandDef>, options: ExecuteOptions) -> InternalResult {
    let ExecuteOptions {
        agent,
        argv,
        defaults,
        display_name,
        env_fields,
        env_source,
        format,
        format_explicit,
        globals,
        input_options,
        middlewares,
        name,
        parse_mode,
        path,
        request,
        vars_fields: _,
        version,
    } = options;

    // Clone values that need to be used both inside the closure and after it.
    let env_source_for_cli = env_source.clone();
    let name_for_mw = name.clone();
    let version_for_mw = version.clone();
    let globals_for_mw = globals.clone();
    let display_name_for_mw = display_name.clone();
    let request_for_mw = request.clone();

    // Initialize vars map (middleware variables).
    let vars_map = Arc::new(RwLock::new(serde_json::Map::new()));

    // Shared result slot: the handler and middleware write into this.
    // We use Mutex (not RwLock) because InternalResult::Stream contains
    // Pin<Box<dyn Stream + Send>> which is Send but not Sync.
    let result: Arc<TokioMutex<Option<InternalResult>>> = Arc::new(TokioMutex::new(None));

    // For streaming with middleware: a channel that fires when the stream
    // is fully consumed, keeping the middleware chain alive.
    let (stream_consumed_tx, stream_consumed_rx) = tokio::sync::oneshot::channel::<()>();
    let stream_consumed_tx = Arc::new(tokio::sync::Mutex::new(Some(stream_consumed_tx)));

    // Signal that the result slot has been populated (for streams, this
    // fires before the middleware chain finishes).
    let (result_ready_tx, result_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let result_ready_tx = Arc::new(tokio::sync::Mutex::new(Some(result_ready_tx)));

    // Clone references for the inner closure.
    let result_inner = Arc::clone(&result);
    let result_ready_inner = Arc::clone(&result_ready_tx);
    let stream_consumed_inner = Arc::clone(&stream_consumed_tx);
    let vars_map_inner = Arc::clone(&vars_map);
    let has_middleware = !middlewares.is_empty();

    let command_inner = Arc::clone(&command);
    let run_command = move || -> middleware::BoxFuture<()> {
        let command = command_inner;
        Box::pin(async move {
            // --- Step 1: Parse args and options based on parse_mode ---
            let parsed = match parse_mode {
                ParseMode::Argv => {
                    // CLI mode: parse both args and options from argv tokens.
                    // The parser module handles this; we provide a stub that
                    // passes through as JSON values.
                    parse_argv_mode(
                        &argv,
                        &command.args_fields,
                        &command.options_fields,
                        &command.aliases,
                        &defaults,
                    )
                }
                ParseMode::Split => {
                    // HTTP mode: args from argv, options from input_options.
                    let args = parse_args_from_argv(&argv, &command.args_fields);
                    let parsed_options = input_options_to_value(&input_options);
                    validate_parsed_input(args, parsed_options, &command)
                }
                ParseMode::Flat => {
                    // MCP mode: split input_options into args vs options by field names.
                    let (args, parsed_options) = split_flat_params(
                        &input_options,
                        &command.args_fields,
                        &command.options_fields,
                    );
                    validate_parsed_input(args, parsed_options, &command)
                }
            };
            let (args, parsed_options) = match parsed {
                Ok(parsed) => parsed,
                Err(error) => {
                    let mut result_guard = result_inner.lock().await;
                    *result_guard = Some(InternalResult::Error {
                        code: "VALIDATION_ERROR".to_string(),
                        message: error.message,
                        retryable: None,
                        field_errors: error.field_errors,
                        cta: None,
                        exit_code: None,
                    });
                    return;
                }
            };

            // --- Step 2: Parse command env from env_source ---
            let command_env = parse_env_fields(&command.env_fields, &env_source);

            // --- Step 3: Build CommandContext ---
            let vars_value = {
                let vars_guard = vars_map_inner.read().await;
                Value::Object(vars_guard.clone())
            };

            let ctx = CommandContext {
                agent,
                args,
                display_name,
                env: command_env,
                globals,
                options: parsed_options,
                request,
                format,
                format_explicit,
                name: name.clone(),
                vars: vars_value,
                version: version.clone(),
            };

            // --- Step 4: Call the handler ---
            let handler_result = command.handler.run(ctx).await;

            // --- Step 5: Handle the result ---
            match handler_result {
                CommandResult::Ok { data, cta } => {
                    let mut result_guard = result_inner.lock().await;
                    *result_guard = Some(InternalResult::Ok { data, cta });
                }
                CommandResult::Error {
                    code,
                    message,
                    retryable,
                    exit_code,
                    cta,
                } => {
                    let mut result_guard = result_inner.lock().await;
                    *result_guard = Some(InternalResult::Error {
                        code,
                        message,
                        retryable: if retryable { Some(true) } else { None },
                        field_errors: None,
                        cta,
                        exit_code,
                    });
                }
                CommandResult::Stream(stream) => {
                    if has_middleware {
                        // Wrap the stream so middleware "after" runs after consumption.
                        let signal = {
                            let mut tx = stream_consumed_inner.lock().await;
                            tx.take()
                        };

                        let wrapped = if let Some(signal) = signal {
                            streaming::wrap_stream_with_signal(stream, signal)
                        } else {
                            stream
                        };

                        {
                            let mut result_guard = result_inner.lock().await;
                            *result_guard = Some(InternalResult::Stream(wrapped));
                        }

                        // Signal that the result is ready (the transport can
                        // start consuming the stream).
                        if let Some(tx) = result_ready_inner.lock().await.take() {
                            let _ = tx.send(());
                        }

                        // Suspend until the stream is fully consumed, keeping
                        // the middleware chain alive so "after" hooks run.
                        let _ = stream_consumed_rx.await;
                    } else {
                        let mut result_guard = result_inner.lock().await;
                        *result_guard = Some(InternalResult::Stream(stream));
                    }
                }
                CommandResult::RecordStream(stream) => {
                    if has_middleware {
                        let signal = {
                            let mut tx = stream_consumed_inner.lock().await;
                            tx.take()
                        };
                        let wrapped = if let Some(signal) = signal {
                            streaming::wrap_record_stream_with_signal(stream, signal)
                        } else {
                            stream
                        };
                        {
                            let mut result_guard = result_inner.lock().await;
                            *result_guard = Some(InternalResult::RecordStream(wrapped));
                        }
                        if let Some(tx) = result_ready_inner.lock().await.take() {
                            let _ = tx.send(());
                        }
                        let _ = stream_consumed_rx.await;
                    } else {
                        let mut result_guard = result_inner.lock().await;
                        *result_guard = Some(InternalResult::RecordStream(stream));
                    }
                }
            }
        })
    };

    // --- Step 6: Run middleware chain ---
    // Parse CLI-level env.
    let cli_env = parse_env_fields(&env_fields, &env_source_for_cli);

    if !middlewares.is_empty() {
        let mw_ctx = MiddlewareContext {
            agent,
            command: path,
            display_name: display_name_for_mw,
            env: cli_env,
            format,
            format_explicit,
            globals: globals_for_mw,
            name: name_for_mw,
            request: request_for_mw,
            vars: Arc::clone(&vars_map),
            version: version_for_mw,
        };

        let chain = middleware::compose(&middlewares, mw_ctx, run_command);

        // Race: the chain might suspend on stream consumption, but result_ready
        // fires as soon as the stream is available for the transport.
        tokio::select! {
            _ = chain => {},
            _ = result_ready_rx => {},
        }
    } else {
        run_command().await;
    }

    // Extract the result.
    let result_guard = result.lock().await;
    match result_guard.as_ref() {
        Some(_) => {
            // We need to take ownership. Drop the guard and re-acquire mutably.
            drop(result_guard);
            let mut result_guard = result.lock().await;
            result_guard.take().unwrap_or(InternalResult::Ok {
                data: Value::Null,
                cta: None,
            })
        }
        None => InternalResult::Ok {
            data: Value::Null,
            cta: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Internal parsing helpers
// ---------------------------------------------------------------------------

/// Parses args and options from argv tokens (CLI mode).
///
/// This is a simplified implementation that extracts positional arguments
/// and flags from argv. The full parser module (being written in parallel)
/// handles the complete parsing logic including aliases and defaults.
fn parse_argv_mode(
    argv: &[String],
    args_fields: &[FieldMeta],
    options_fields: &[FieldMeta],
    aliases: &HashMap<String, char>,
    defaults: &Option<BTreeMap<String, Value>>,
) -> Result<(Value, Value), InputError> {
    let parsed = crate::parser::parse(
        argv,
        &crate::parser::ParseOptions {
            args_fields: args_fields.to_vec(),
            options_fields: options_fields.to_vec(),
            aliases: aliases.clone(),
            defaults: defaults.clone(),
        },
    )?;
    Ok((
        Value::Object(parsed.args.into_iter().collect()),
        Value::Object(parsed.options.into_iter().collect()),
    ))
}

fn validate_parsed_input(
    args: Value,
    options: Value,
    command: &CommandDef,
) -> Result<(Value, Value), InputError> {
    let args_map = args
        .as_object()
        .map(|values| values.clone().into_iter().collect())
        .unwrap_or_default();
    let options_map = options
        .as_object()
        .map(|values| values.clone().into_iter().collect())
        .unwrap_or_default();
    let args_map = crate::parser::coerce_fields(args_map, &command.args_fields);
    let options_map = crate::parser::coerce_fields(options_map, &command.options_fields);
    let field_errors = crate::parser::field_errors(&args_map, &command.args_fields)
        .into_iter()
        .chain(crate::parser::field_errors(
            &options_map,
            &command.options_fields,
        ))
        .collect::<Vec<_>>();
    if !field_errors.is_empty() {
        return Err(InputError {
            message: "Validation failed".to_string(),
            field_errors: Some(field_errors),
        });
    }
    Ok((
        Value::Object(args_map.into_iter().collect()),
        Value::Object(options_map.into_iter().collect()),
    ))
}

/// Parses positional arguments from argv tokens.
fn parse_args_from_argv(argv: &[String], args_fields: &[FieldMeta]) -> Value {
    let mut args_map = serde_json::Map::new();

    for (i, token) in argv.iter().enumerate() {
        if i < args_fields.len() {
            let field = &args_fields[i];
            args_map.insert(field.name.to_string(), parse_option_value(token));
        }
    }

    Value::Object(args_map)
}

/// Converts a BTreeMap of input options to a JSON Value.
fn input_options_to_value(options: &BTreeMap<String, Value>) -> Value {
    let map: serde_json::Map<String, Value> = options
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::Object(map)
}

/// Splits flat params into args vs options by field names (MCP mode).
fn split_flat_params(
    params: &BTreeMap<String, Value>,
    args_fields: &[FieldMeta],
    options_fields: &[FieldMeta],
) -> (Value, Value) {
    let arg_names: std::collections::HashSet<&str> = args_fields.iter().map(|f| f.name).collect();
    let _option_names: std::collections::HashSet<&str> =
        options_fields.iter().map(|f| f.name).collect();

    let mut args_map = serde_json::Map::new();
    let mut opts_map = serde_json::Map::new();

    for (key, value) in params {
        let snake_key = crate::schema::to_snake(key);
        if arg_names.contains(snake_key.as_str()) {
            args_map.insert(snake_key, value.clone());
        } else {
            opts_map.insert(snake_key, value.clone());
        }
    }

    (Value::Object(args_map), Value::Object(opts_map))
}

/// Parses environment variables from the env source using field metadata.
fn parse_env_fields(env_fields: &[FieldMeta], env_source: &HashMap<String, String>) -> Value {
    let mut env_map = serde_json::Map::new();

    for field in env_fields {
        let env_name = field.env_name.unwrap_or(field.name);
        if let Some(value) = env_source.get(env_name) {
            env_map.insert(
                field.name.to_string(),
                parse_env_value(value, &field.field_type),
            );
        } else if let Some(default) = &field.default {
            env_map.insert(field.name.to_string(), default.clone());
        }
    }

    Value::Object(env_map)
}

/// Parses a string value into the appropriate JSON type based on field type.
fn parse_env_value(value: &str, field_type: &crate::schema::FieldType) -> Value {
    match field_type {
        crate::schema::FieldType::Boolean => Value::Bool(matches!(value, "1" | "true" | "yes")),
        crate::schema::FieldType::Number => {
            if let Ok(n) = value.parse::<f64>() {
                Value::from(n)
            } else {
                Value::String(value.to_string())
            }
        }
        _ => Value::String(value.to_string()),
    }
}

/// Parses an option value string, attempting to interpret it as a number
/// or boolean when appropriate.
fn parse_option_value(value: &str) -> Value {
    // Try parsing as integer
    if let Ok(n) = value.parse::<i64>() {
        return Value::from(n);
    }
    // Try parsing as float
    if let Ok(n) = value.parse::<f64>() {
        return Value::from(n);
    }
    // Check for boolean
    match value {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    // Default to string
    Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_option_value() {
        assert_eq!(parse_option_value("42"), Value::from(42));
        assert_eq!(
            parse_option_value("3.14"),
            Value::from("3.14".parse::<f64>().unwrap())
        );
        assert_eq!(parse_option_value("true"), Value::Bool(true));
        assert_eq!(parse_option_value("false"), Value::Bool(false));
        assert_eq!(
            parse_option_value("hello"),
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_split_flat_params() {
        let mut params = BTreeMap::new();
        params.insert("name".to_string(), Value::String("alice".to_string()));
        params.insert("verbose".to_string(), Value::Bool(true));

        let args_fields = vec![FieldMeta {
            name: "name",
            cli_name: "name".to_string(),
            description: None,
            field_type: crate::schema::FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }];
        let options_fields = vec![FieldMeta {
            name: "verbose",
            cli_name: "verbose".to_string(),
            description: None,
            field_type: crate::schema::FieldType::Boolean,
            required: false,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }];

        let (args, opts) = split_flat_params(&params, &args_fields, &options_fields);

        assert_eq!(args["name"], Value::String("alice".to_string()));
        assert_eq!(opts["verbose"], Value::Bool(true));
    }

    #[test]
    fn test_parse_env_fields() {
        let fields = vec![FieldMeta {
            name: "api_key",
            cli_name: "api-key".to_string(),
            description: Some("API key"),
            field_type: crate::schema::FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: Some("API_KEY"),
        }];

        let mut env_source = HashMap::new();
        env_source.insert("API_KEY".to_string(), "secret123".to_string());

        let result = parse_env_fields(&fields, &env_source);
        assert_eq!(result["api_key"], Value::String("secret123".to_string()));
    }
}
