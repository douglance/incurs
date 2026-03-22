//! Unified command execution for the incur framework.
//!
//! This module is the heart of the three-transport architecture. The
//! [`execute`] function is called by CLI, HTTP, and MCP transports with
//! different [`ParseMode`] values to handle input parsing, middleware
//! composition, and handler invocation uniformly.
//!
//! Ported from `src/internal/command.ts`.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
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

/// Trait for command handlers.
///
/// This trait provides type erasure for command handlers so that the framework
/// can store heterogeneous handlers in the command tree. Implementations
/// receive a [`CommandContext`] and return a [`CommandResult`].
#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync {
    /// Execute the command with the given context.
    async fn run(&self, ctx: CommandContext) -> CommandResult;
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
    /// Parsed named options as a JSON value.
    pub options: Value,
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
    /// CLI-level env field metadata.
    pub env_fields: Vec<FieldMeta>,
    /// Source for environment variables (key-value map).
    pub env_source: HashMap<String, String>,
    /// The resolved output format.
    pub format: Format,
    /// Whether the format was explicitly requested.
    pub format_explicit: bool,
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
    Ok {
        data: Value,
        cta: Option<CtaBlock>,
    },
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
        env_fields,
        env_source,
        format,
        format_explicit,
        input_options,
        middlewares,
        name,
        parse_mode,
        path,
        vars_fields: _,
        version,
    } = options;

    // Clone values that need to be used both inside the closure and after it.
    let env_source_for_cli = env_source.clone();
    let name_for_mw = name.clone();
    let version_for_mw = version.clone();

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
            let (args, parsed_options) = match parse_mode {
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
                    (args, parsed_options)
                }
                ParseMode::Flat => {
                    // MCP mode: split input_options into args vs options by field names.
                    split_flat_params(&input_options, &command.args_fields, &command.options_fields)
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
                env: command_env,
                options: parsed_options,
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
            env: cli_env,
            format,
            format_explicit,
            name: name_for_mw,
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
            result_guard
                .take()
                .unwrap_or(InternalResult::Ok {
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
) -> (Value, Value) {
    let mut args_map = serde_json::Map::new();
    let mut opts_map = serde_json::Map::new();

    // Build reverse alias map: char -> field name
    let reverse_aliases: HashMap<char, &str> = aliases
        .iter()
        .map(|(name, &ch)| (ch, name.as_str()))
        .collect();

    // Build set of known option names for lookup
    let option_names: HashMap<String, &FieldMeta> = options_fields
        .iter()
        .map(|f| (f.cli_name.clone(), f))
        .collect();

    let mut positional_idx = 0;
    let mut i = 0;

    while i < argv.len() {
        let token = &argv[i];

        if let Some(name) = token.strip_prefix("--") {
            // Long option

            // Handle --key=value
            if let Some(eq_pos) = name.find('=') {
                let key = &name[..eq_pos];
                let value = &name[eq_pos + 1..];
                let snake_key = crate::schema::to_snake(key);
                opts_map.insert(snake_key, Value::String(value.to_string()));
                i += 1;
                continue;
            }

            let snake_name = crate::schema::to_snake(name);

            // Check if it's a boolean flag
            if let Some(field) = option_names.get(name) {
                match &field.field_type {
                    crate::schema::FieldType::Boolean => {
                        opts_map.insert(snake_name, Value::Bool(true));
                        i += 1;
                        continue;
                    }
                    crate::schema::FieldType::Count => {
                        let current = opts_map
                            .get(&snake_name)
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        opts_map.insert(snake_name, Value::from(current + 1));
                        i += 1;
                        continue;
                    }
                    _ => {}
                }
            }

            // Option with a value
            if i + 1 < argv.len() {
                let value = &argv[i + 1];
                opts_map.insert(snake_name, parse_option_value(value));
                i += 2;
            } else {
                // No value, treat as boolean
                opts_map.insert(snake_name, Value::Bool(true));
                i += 1;
            }
        } else if token.starts_with('-') && token.len() == 2 {
            // Short option
            let ch = token.chars().nth(1).unwrap();

            if let Some(&field_name) = reverse_aliases.get(&ch) {
                let snake_name = crate::schema::to_snake(field_name);

                // Check if it's a boolean field
                let is_bool = option_names
                    .get(field_name)
                    .map(|f| matches!(f.field_type, crate::schema::FieldType::Boolean))
                    .unwrap_or(false);

                if is_bool {
                    opts_map.insert(snake_name, Value::Bool(true));
                    i += 1;
                } else if i + 1 < argv.len() {
                    let value = &argv[i + 1];
                    opts_map.insert(snake_name, parse_option_value(value));
                    i += 2;
                } else {
                    opts_map.insert(snake_name, Value::Bool(true));
                    i += 1;
                }
            } else {
                // Unknown short flag — treat as positional
                if positional_idx < args_fields.len() {
                    let field = &args_fields[positional_idx];
                    args_map.insert(field.name.to_string(), Value::String(token.clone()));
                    positional_idx += 1;
                }
                i += 1;
            }
        } else {
            // Positional argument
            if positional_idx < args_fields.len() {
                let field = &args_fields[positional_idx];
                args_map.insert(field.name.to_string(), parse_option_value(token));
                positional_idx += 1;
            }
            i += 1;
        }
    }

    // Apply defaults
    if let Some(defaults) = defaults {
        for (key, value) in defaults {
            let snake_key = crate::schema::to_snake(key);
            if !opts_map.contains_key(&snake_key) {
                opts_map.insert(snake_key, value.clone());
            }
        }
    }

    // Apply field defaults
    for field in options_fields {
        let key = field.name.to_string();
        if !opts_map.contains_key(&key) && let Some(default) = &field.default {
            opts_map.insert(key, default.clone());
        }
    }

    (Value::Object(args_map), Value::Object(opts_map))
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
    let arg_names: std::collections::HashSet<&str> =
        args_fields.iter().map(|f| f.name).collect();
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
fn parse_env_fields(
    env_fields: &[FieldMeta],
    env_source: &HashMap<String, String>,
) -> Value {
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
        crate::schema::FieldType::Boolean => {
            Value::Bool(matches!(value, "1" | "true" | "yes"))
        }
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
        assert_eq!(parse_option_value("3.14"), Value::from(3.14));
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
