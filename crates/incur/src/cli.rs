//! The main CLI type for the incur framework.
//!
//! This module provides [`Cli`], the entry point for building command-line
//! applications with incur. It supports:
//!
//! - Registering commands and command groups
//! - Middleware that runs around every command
//! - Built-in flags (--help, --version, --format, --json, --verbose, etc.)
//! - Config file loading for option defaults
//! - Three-transport architecture (CLI, HTTP, MCP)
//!
//! Ported from `src/Cli.ts`.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::sync::Arc;

use serde_json::Value;

use crate::command::{self, CommandDef, ExecuteOptions, InternalResult, ParseMode};
use crate::config;
use crate::help::{self, CommandSummary, FormatCommandOptions, FormatRootOptions};
use crate::middleware::MiddlewareFn;
use crate::output::*;
use crate::schema::FieldMeta;

/// Entry in the command tree.
///
/// The command tree is a recursive structure where each node is either a
/// leaf command, a group of subcommands, or a fetch gateway.
pub enum CommandEntry {
    /// A leaf command that can be executed.
    Leaf(Arc<CommandDef>),
    /// A group of subcommands (acts as a namespace).
    Group {
        /// Description of the group.
        description: Option<String>,
        /// Subcommands within this group.
        commands: BTreeMap<String, CommandEntry>,
        /// Middleware that applies to all commands in this group.
        middleware: Vec<MiddlewareFn>,
        /// Output policy inherited by child commands.
        output_policy: Option<OutputPolicy>,
    },
    /// A fetch gateway that proxies to an HTTP handler.
    FetchGateway {
        /// Description of the gateway.
        description: Option<String>,
        /// Base path prefix for request URLs.
        base_path: Option<String>,
        /// Output policy for the gateway.
        output_policy: Option<OutputPolicy>,
    },
}

impl CommandEntry {
    /// Returns the description of this entry, regardless of variant.
    pub fn description(&self) -> Option<&str> {
        match self {
            CommandEntry::Leaf(def) => def.description.as_deref(),
            CommandEntry::Group { description, .. } => description.as_deref(),
            CommandEntry::FetchGateway { description, .. } => description.as_deref(),
        }
    }
}

/// Config file options for a CLI.
pub struct ConfigOptions {
    /// The flag name for specifying a config file (e.g. `"config"` for `--config`).
    pub flag: String,
    /// Ordered list of file paths to search for config files.
    pub files: Vec<String>,
}

/// The main CLI builder and executor.
///
/// Use [`Cli::create`] to construct a new CLI, then chain method calls to
/// configure it. Call [`Cli::serve`] to parse argv and execute the matched
/// command.
///
/// # Example
///
/// ```ignore
/// let cli = Cli::create("my-app")
///     .description("My awesome CLI")
///     .version("1.0.0")
///     .command("hello", command_def)
///     .serve()
///     .await;
/// ```
pub struct Cli {
    /// The CLI name (used as the binary name in help text).
    pub name: String,
    /// A short description of the CLI.
    pub description: Option<String>,
    /// The CLI version string.
    pub version: Option<String>,
    /// Alternative binary names for this CLI.
    pub aliases: Vec<String>,
    /// The command tree.
    commands: BTreeMap<String, CommandEntry>,
    /// Root-level middleware that runs around every command.
    middleware: Vec<MiddlewareFn>,
    /// Root command handler (for CLIs with a default command).
    root_command: Option<Arc<CommandDef>>,
    /// CLI-level environment variable fields.
    env_fields: Vec<FieldMeta>,
    /// Middleware variable fields.
    vars_fields: Vec<FieldMeta>,
    /// Config file options.
    config: Option<ConfigOptions>,
    /// Default output policy.
    output_policy: Option<OutputPolicy>,
    /// Default output format.
    format: Option<Format>,
}

impl Cli {
    /// Creates a new CLI with the given name.
    pub fn create(name: impl Into<String>) -> Self {
        Cli {
            name: name.into(),
            description: None,
            version: None,
            aliases: Vec::new(),
            commands: BTreeMap::new(),
            middleware: Vec::new(),
            root_command: None,
            env_fields: Vec::new(),
            vars_fields: Vec::new(),
            config: None,
            output_policy: None,
            format: None,
        }
    }

    /// Sets the CLI description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Sets the CLI version.
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = Some(v.into());
        self
    }

    /// Adds alternative binary names for this CLI.
    pub fn aliases(mut self, aliases: Vec<String>) -> Self {
        self.aliases = aliases;
        self
    }

    /// Sets the default output format.
    pub fn format(mut self, format: Format) -> Self {
        self.format = Some(format);
        self
    }

    /// Sets the output policy.
    pub fn output_policy(mut self, policy: OutputPolicy) -> Self {
        self.output_policy = Some(policy);
        self
    }

    /// Sets the root command handler (for CLIs that have a default action).
    pub fn root(mut self, def: CommandDef) -> Self {
        self.root_command = Some(Arc::new(def));
        self
    }

    /// Sets the CLI-level environment variable fields.
    pub fn env_fields(mut self, fields: Vec<FieldMeta>) -> Self {
        self.env_fields = fields;
        self
    }

    /// Sets the middleware variable fields.
    pub fn vars_fields(mut self, fields: Vec<FieldMeta>) -> Self {
        self.vars_fields = fields;
        self
    }

    /// Configures config file loading.
    pub fn config(mut self, options: ConfigOptions) -> Self {
        self.config = Some(options);
        self
    }

    /// Registers a command.
    pub fn command(mut self, name: impl Into<String>, def: CommandDef) -> Self {
        self.commands
            .insert(name.into(), CommandEntry::Leaf(Arc::new(def)));
        self
    }

    /// Mounts a sub-CLI as a command group.
    ///
    /// If the sub-CLI has a root command and no subcommands, it is mounted
    /// as a leaf command (a "leaf CLI"). Otherwise it is mounted as a group.
    pub fn group(mut self, cli: Cli) -> Self {
        if let Some(root_cmd) = cli.root_command {
            if cli.commands.is_empty() {
                // Leaf CLI: mount the root command directly as a leaf.
                self.commands
                    .insert(cli.name, CommandEntry::Leaf(root_cmd));
                return self;
            }
            // Has both root command and subcommands — mount as a group but
            // insert the root command under a synthetic "" key so it can be
            // resolved. For now, treat it the same as a regular group and
            // the root command is lost. A full solution would extend
            // CommandEntry::Group with an optional root_command field.
        }
        let entry = CommandEntry::Group {
            description: cli.description,
            commands: cli.commands,
            middleware: cli.middleware,
            output_policy: cli.output_policy,
        };
        self.commands.insert(cli.name, entry);
        self
    }

    /// Registers middleware that runs around every command.
    pub fn use_middleware(mut self, handler: MiddlewareFn) -> Self {
        self.middleware.push(handler);
        self
    }

    /// Parses process argv, runs the matched command, writes output to stdout.
    pub async fn serve(&self) -> Result<(), Box<dyn std::error::Error>> {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        self.serve_with(argv).await
    }

    /// Serves with explicit argv (useful for testing).
    ///
    /// This is the main entry point that implements the full CLI lifecycle:
    ///
    /// 1. Extract built-in flags (--help, --version, --format, etc.)
    /// 2. Handle --version, --help, --schema, --llms, --mcp
    /// 3. Resolve the command from the command tree
    /// 4. Load config file defaults
    /// 5. Execute the command with middleware
    /// 6. Format and write output to stdout
    pub async fn serve_with(&self, argv: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
        let human = std::io::stdout().is_terminal();
        let config_flag = self.config.as_ref().map(|c| c.flag.as_str());

        // --- Step 1: Extract built-in flags ---
        let builtin = match extract_builtin_flags(&argv, config_flag) {
            Ok(b) => b,
            Err(e) => {
                let msg = e.to_string();
                if human {
                    writeln_stdout(&format_human_error("UNKNOWN", &msg));
                } else {
                    writeln_stdout(&format!(
                        "{{\"ok\":false,\"error\":{{\"code\":\"UNKNOWN\",\"message\":\"{}\"}}}}",
                        msg.replace('"', "\\\"")
                    ));
                }
                std::process::exit(1);
            }
        };

        // --- Step 2: Handle --version ---
        if builtin.version && !builtin.help && let Some(v) = &self.version {
            writeln_stdout(v);
            return Ok(());
        }

        // --- Step 3: Handle --help at root level ---
        if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                // Root command exists — if human and has required args with
                // none provided, show help.
                if human && has_required_args(&root_cmd.args_fields) {
                    writeln_stdout(&format_command_help(
                        &self.name,
                        root_cmd,
                        &self.commands,
                        &self.aliases,
                        config_flag,
                        self.version.as_deref(),
                        true,
                    ));
                    return Ok(());
                }
                // Otherwise fall through to execute the root command
            } else if !builtin.help {
                // No root command, no args — show root help
                writeln_stdout(&help::format_root(
                    &self.name,
                    &FormatRootOptions {
                        aliases: if self.aliases.is_empty() {
                            None
                        } else {
                            Some(self.aliases.clone())
                        },
                        config_flag: config_flag.map(|s| s.to_string()),
                        commands: collect_help_commands(&self.commands),
                        description: self.description.clone(),
                        root: true,
                        version: self.version.clone(),
                    },
                ));
                return Ok(());
            }
        }

        // --- Step 4: Resolve command from argv ---
        let resolved = if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                ResolvedCommand::Leaf {
                    command: Arc::clone(root_cmd),
                    path: self.name.clone(),
                    rest: Vec::new(),
                    collected_middleware: Vec::new(),
                    output_policy: None,
                }
            } else {
                ResolvedCommand::Help {
                    path: self.name.clone(),
                    description: self.description.clone(),
                    commands: &self.commands,
                }
            }
        } else {
            resolve_command(&self.commands, &builtin.rest)
        };

        // --- Step 5: Handle --help ---
        if builtin.help {
            match &resolved {
                ResolvedCommand::Leaf {
                    command,
                    path,
                    ..
                } => {
                    let is_root = path == &self.name;
                    let help_cmds = if is_root && !self.commands.is_empty() {
                        collect_help_commands(&self.commands)
                    } else {
                        Vec::new()
                    };
                    let command_name = if is_root {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    writeln_stdout(&help::format_command(
                        &command_name,
                        &FormatCommandOptions {
                            aliases: if is_root && !self.aliases.is_empty() {
                                Some(self.aliases.clone())
                            } else {
                                None
                            },
                            args_fields: command.args_fields.clone(),
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: help_cmds,
                            description: command.description.clone(),
                            env_fields: command.env_fields.clone(),
                            examples: command.examples.clone(),
                            hint: command.hint.clone(),
                            hide_global_options: false,
                            options_fields: command.options_fields.clone(),
                            option_aliases: command.aliases.clone(),
                            root: is_root,
                            version: if is_root {
                                self.version.clone()
                            } else {
                                None
                            },
                        },
                    ));
                }
                ResolvedCommand::Help {
                    path,
                    description,
                    commands,
                } => {
                    let help_name = if path == &self.name {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    let is_root = path == &self.name;

                    // Root with both a handler and subcommands
                    if is_root && let Some(root_cmd) = &self.root_command && !commands.is_empty() {
                        writeln_stdout(&format_command_help(
                            &self.name,
                            root_cmd,
                            commands,
                            &self.aliases,
                            config_flag,
                            self.version.as_deref(),
                            true,
                        ));
                    } else {
                        writeln_stdout(&help::format_root(
                            &help_name,
                            &FormatRootOptions {
                                aliases: if is_root && !self.aliases.is_empty() {
                                    Some(self.aliases.clone())
                                } else {
                                    None
                                },
                                config_flag: config_flag.map(|s| s.to_string()),
                                commands: collect_help_commands(commands),
                                description: description.clone(),
                                root: is_root,
                                version: if is_root {
                                    self.version.clone()
                                } else {
                                    None
                                },
                            },
                        ));
                    }
                }
                ResolvedCommand::Error { error: _, path } => {
                    let help_name = if path.is_empty() {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    writeln_stdout(&help::format_root(
                        &help_name,
                        &FormatRootOptions {
                            aliases: None,
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: collect_help_commands(&self.commands),
                            description: self.description.clone(),
                            root: path.is_empty(),
                            version: None,
                        },
                    ));
                }
            }
            return Ok(());
        }

        // --- Step 6: Handle --schema ---
        if builtin.schema {
            match &resolved {
                ResolvedCommand::Leaf { command, .. } => {
                    if let Some(schema) = &command.output_schema {
                        writeln_stdout(&serde_json::to_string_pretty(schema)?);
                    } else {
                        writeln_stdout("{}");
                    }
                }
                _ => {
                    writeln_stdout("--schema requires a command.");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        // --- Step 7: Handle command resolution errors ---
        let (command, command_path, rest, collected_mw, effective_output_policy) = match resolved {
            ResolvedCommand::Leaf {
                command,
                path,
                rest,
                collected_middleware,
                output_policy,
            } => (command, path, rest, collected_middleware, output_policy),
            ResolvedCommand::Help {
                path,
                description,
                commands,
            } => {
                let help_name = if path == self.name {
                    self.name.clone()
                } else {
                    format!("{} {path}", self.name)
                };
                writeln_stdout(&help::format_root(
                    &help_name,
                    &FormatRootOptions {
                        aliases: None,
                        config_flag: config_flag.map(|s| s.to_string()),
                        commands: collect_help_commands(commands),
                        description: description.clone(),
                        root: path == self.name,
                        version: None,
                    },
                ));
                return Ok(());
            }
            ResolvedCommand::Error { error, path } => {
                // Try falling back to root command if available
                if path.is_empty() {
                    if let Some(root_cmd) = &self.root_command {
                        (
                            Arc::clone(root_cmd),
                            self.name.clone(),
                            builtin.rest,
                            Vec::new(),
                            None,
                        )
                    } else {
                        let parent = if path.is_empty() {
                            &self.name
                        } else {
                            &path
                        };
                        let message = format!("'{error}' is not a command for '{parent}'.");
                        if human {
                            writeln_stdout(&format_human_error("COMMAND_NOT_FOUND", &message));
                            writeln_stdout(&format!(
                                "\nSuggested commands:\n  {} --help",
                                self.name
                            ));
                        } else {
                            writeln_stdout(&format!(
                                "{{\"ok\":false,\"error\":{{\"code\":\"COMMAND_NOT_FOUND\",\"message\":\"{}\"}}}}",
                                message.replace('"', "\\\"")
                            ));
                        }
                        std::process::exit(1);
                    }
                } else {
                    let parent = format!("{} {path}", self.name);
                    let message = format!("'{error}' is not a command for '{parent}'.");
                    if human {
                        writeln_stdout(&format_human_error("COMMAND_NOT_FOUND", &message));
                    } else {
                        writeln_stdout(&format!(
                            "{{\"ok\":false,\"error\":{{\"code\":\"COMMAND_NOT_FOUND\",\"message\":\"{}\"}}}}",
                            message.replace('"', "\\\"")
                        ));
                    }
                    std::process::exit(1);
                }
            }
        };

        let start = std::time::Instant::now();

        // Resolve effective format
        let format = if builtin.format_explicit {
            builtin.format
        } else {
            command
                .format
                .unwrap_or_else(|| self.format.unwrap_or(Format::Toon))
        };

        // Resolve effective output policy
        let policy = effective_output_policy
            .or(command.output_policy)
            .or(self.output_policy);
        let render_output = !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

        // --- Step 8: Load config defaults ---
        let defaults = if let Some(ref cfg) = self.config {
            if builtin.config_disabled {
                None
            } else {
                let config_path = config::resolve_config_path(
                    builtin.config_path.as_deref(),
                    &cfg.files,
                );
                if let Some(path) = config_path {
                    match config::load_config(&path) {
                        Ok(tree) => {
                            match config::extract_command_section(&tree, &self.name, &command_path) {
                                Ok(section) => section,
                                Err(e) => {
                                    writeln_stdout(&format_human_error("CONFIG_ERROR", &e.to_string()));
                                    std::process::exit(1);
                                }
                            }
                        }
                        Err(e) => {
                            // Explicit config path: error. Auto-detected: ignore.
                            if builtin.config_path.is_some() {
                                writeln_stdout(&format_human_error("CONFIG_ERROR", &e.to_string()));
                                std::process::exit(1);
                            }
                            None
                        }
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        // --- Step 9: Collect middleware ---
        let all_middleware: Vec<MiddlewareFn> = self
            .middleware
            .iter()
            .cloned()
            .chain(collected_mw.into_iter())
            .chain(command.middleware.iter().cloned())
            .collect();

        // --- Step 10: Build env source ---
        let env_source: std::collections::HashMap<String, String> = std::env::vars().collect();

        // --- Step 11: Execute command ---
        let result = command::execute(
            Arc::clone(&command),
            ExecuteOptions {
                agent: !human,
                argv: rest,
                defaults,
                env_fields: self.env_fields.clone(),
                env_source,
                format,
                format_explicit: builtin.format_explicit,
                input_options: BTreeMap::new(),
                middlewares: all_middleware,
                name: self.name.clone(),
                parse_mode: ParseMode::Argv,
                path: command_path.clone(),
                vars_fields: self.vars_fields.clone(),
                version: self.version.clone(),
            },
        )
        .await;

        let duration = start.elapsed();
        let duration_str = format!("{}ms", duration.as_millis());

        // --- Step 12: Handle result ---
        match result {
            InternalResult::Ok { data, cta } => {
                let formatted_cta = format_cta_block(&self.name, cta.as_ref());

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(true));
                    envelope.insert("data".to_string(), data);
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    if let Some(cta) = &formatted_cta {
                        meta.insert("cta".to_string(), serde_json::to_value(cta).unwrap_or(Value::Null));
                    }
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    writeln_stdout(&format_value(&Value::Object(envelope), format));
                } else if human {
                    if render_output {
                        writeln_stdout(&format_value(&data, format));
                    }
                    if let Some(cta) = &formatted_cta {
                        writeln_stdout(&format_human_cta(cta));
                    }
                } else {
                    // Agent mode: include CTA in the JSON envelope if present
                    if let Some(cta) = &formatted_cta {
                        if let Value::Object(ref map) = data {
                            let mut out = map.clone();
                            out.insert("cta".to_string(), serde_json::to_value(cta).unwrap_or(Value::Null));
                            writeln_stdout(&format_value(&Value::Object(out), format));
                        } else {
                            writeln_stdout(&format_value(&data, format));
                        }
                    } else {
                        writeln_stdout(&format_value(&data, format));
                    }
                }
            }
            InternalResult::Error {
                code,
                message,
                retryable,
                field_errors: _,
                cta,
                exit_code,
            } => {
                let formatted_cta = format_cta_block(&self.name, cta.as_ref());

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(false));
                    let mut error_obj = serde_json::Map::new();
                    error_obj.insert("code".to_string(), Value::String(code.clone()));
                    error_obj.insert("message".to_string(), Value::String(message.clone()));
                    if let Some(r) = retryable {
                        error_obj.insert("retryable".to_string(), Value::Bool(r));
                    }
                    envelope.insert("error".to_string(), Value::Object(error_obj));
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    writeln_stdout(&format_value(&Value::Object(envelope), format));
                } else if human && !builtin.format_explicit {
                    writeln_stdout(&format_human_error(&code, &message));
                    if let Some(cta) = &formatted_cta {
                        writeln_stdout(&format_human_cta(cta));
                    }
                } else {
                    let mut error_obj = serde_json::Map::new();
                    error_obj.insert("code".to_string(), Value::String(code.clone()));
                    error_obj.insert("message".to_string(), Value::String(message.clone()));
                    if let Some(cta) = &formatted_cta {
                        error_obj.insert("cta".to_string(), serde_json::to_value(cta).unwrap_or(Value::Null));
                    }
                    writeln_stdout(&format_value(&Value::Object(error_obj), format));
                }

                std::process::exit(exit_code.unwrap_or(1));
            }
            InternalResult::Stream(stream) => {
                handle_streaming(
                    stream,
                    StreamingOptions {
                        path: &command_path,
                        start,
                        format,
                        format_explicit: builtin.format_explicit,
                        human,
                        render_output,
                        verbose: builtin.verbose,
                    },
                )
                .await;
            }
        }

        Ok(())
    }

    /// Testable serve: writes output to the provided writer and returns exit code.
    ///
    /// Unlike [`serve_with`], this method:
    /// - Writes to a caller-provided writer instead of stdout
    /// - Returns the exit code instead of calling `std::process::exit`
    /// - Accepts a `human` flag instead of checking `is_terminal()`
    ///
    /// This enables integration testing without process-level side effects.
    pub async fn serve_to(
        &self,
        argv: Vec<String>,
        writer: &mut dyn std::io::Write,
        human: bool,
    ) -> Result<Option<i32>, Box<dyn std::error::Error>> {
        let config_flag = self.config.as_ref().map(|c| c.flag.as_str());

        // Inline helper to write a line to the writer
        macro_rules! wln {
            ($s:expr) => {{
                let s: &str = $s;
                if s.ends_with('\n') {
                    write!(writer, "{s}").ok();
                } else {
                    writeln!(writer, "{s}").ok();
                }
            }};
        }

        // --- Step 1: Extract built-in flags ---
        let builtin = match extract_builtin_flags(&argv, config_flag) {
            Ok(b) => b,
            Err(e) => {
                let msg = e.to_string();
                if human {
                    wln!(&format_human_error("UNKNOWN", &msg));
                } else {
                    wln!(&format!(
                        "{{\"ok\":false,\"error\":{{\"code\":\"UNKNOWN\",\"message\":\"{}\"}}}}",
                        msg.replace('"', "\\\"")
                    ));
                }
                return Ok(Some(1));
            }
        };

        // --- Step 2: Handle --version ---
        if builtin.version && !builtin.help {
            if let Some(v) = &self.version {
                wln!(v);
                return Ok(None);
            }
        }

        // --- Step 3: Handle --help at root level ---
        if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                if human && has_required_args(&root_cmd.args_fields) {
                    wln!(&format_command_help(
                        &self.name,
                        root_cmd,
                        &self.commands,
                        &self.aliases,
                        config_flag,
                        self.version.as_deref(),
                        true,
                    ));
                    return Ok(None);
                }
            } else if !builtin.help {
                wln!(&help::format_root(
                    &self.name,
                    &FormatRootOptions {
                        aliases: if self.aliases.is_empty() {
                            None
                        } else {
                            Some(self.aliases.clone())
                        },
                        config_flag: config_flag.map(|s| s.to_string()),
                        commands: collect_help_commands(&self.commands),
                        description: self.description.clone(),
                        root: true,
                        version: self.version.clone(),
                    },
                ));
                return Ok(None);
            }
        }

        // --- Step 4: Resolve command from argv ---
        let resolved = if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                ResolvedCommand::Leaf {
                    command: Arc::clone(root_cmd),
                    path: self.name.clone(),
                    rest: Vec::new(),
                    collected_middleware: Vec::new(),
                    output_policy: None,
                }
            } else {
                ResolvedCommand::Help {
                    path: self.name.clone(),
                    description: self.description.clone(),
                    commands: &self.commands,
                }
            }
        } else {
            resolve_command(&self.commands, &builtin.rest)
        };

        // --- Step 5: Handle --help ---
        if builtin.help {
            match &resolved {
                ResolvedCommand::Leaf {
                    command,
                    path,
                    ..
                } => {
                    let is_root = path == &self.name;
                    let help_cmds = if is_root && !self.commands.is_empty() {
                        collect_help_commands(&self.commands)
                    } else {
                        Vec::new()
                    };
                    let command_name = if is_root {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    wln!(&help::format_command(
                        &command_name,
                        &FormatCommandOptions {
                            aliases: if is_root && !self.aliases.is_empty() {
                                Some(self.aliases.clone())
                            } else {
                                None
                            },
                            args_fields: command.args_fields.clone(),
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: help_cmds,
                            description: command.description.clone(),
                            env_fields: command.env_fields.clone(),
                            examples: command.examples.clone(),
                            hint: command.hint.clone(),
                            hide_global_options: false,
                            options_fields: command.options_fields.clone(),
                            option_aliases: command.aliases.clone(),
                            root: is_root,
                            version: if is_root {
                                self.version.clone()
                            } else {
                                None
                            },
                        },
                    ));
                }
                ResolvedCommand::Help {
                    path,
                    description,
                    commands,
                } => {
                    let help_name = if path == &self.name {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    let is_root = path == &self.name;

                    if is_root && let Some(root_cmd) = &self.root_command && !commands.is_empty() {
                        wln!(&format_command_help(
                            &self.name,
                            root_cmd,
                            commands,
                            &self.aliases,
                            config_flag,
                            self.version.as_deref(),
                            true,
                        ));
                    } else {
                        wln!(&help::format_root(
                            &help_name,
                            &FormatRootOptions {
                                aliases: if is_root && !self.aliases.is_empty() {
                                    Some(self.aliases.clone())
                                } else {
                                    None
                                },
                                config_flag: config_flag.map(|s| s.to_string()),
                                commands: collect_help_commands(commands),
                                description: description.clone(),
                                root: is_root,
                                version: if is_root {
                                    self.version.clone()
                                } else {
                                    None
                                },
                            },
                        ));
                    }
                }
                ResolvedCommand::Error { error: _, path } => {
                    let help_name = if path.is_empty() {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    wln!(&help::format_root(
                        &help_name,
                        &FormatRootOptions {
                            aliases: None,
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: collect_help_commands(&self.commands),
                            description: self.description.clone(),
                            root: path.is_empty(),
                            version: None,
                        },
                    ));
                }
            }
            return Ok(None);
        }

        // --- Step 6: Handle --schema ---
        if builtin.schema {
            match &resolved {
                ResolvedCommand::Leaf { command, .. } => {
                    if let Some(schema) = &command.output_schema {
                        wln!(&serde_json::to_string_pretty(schema)?);
                    } else {
                        wln!("{}");
                    }
                }
                _ => {
                    wln!("--schema requires a command.");
                    return Ok(Some(1));
                }
            }
            return Ok(None);
        }

        // --- Step 7: Handle command resolution errors ---
        let (command, command_path, rest, collected_mw, effective_output_policy) = match resolved {
            ResolvedCommand::Leaf {
                command,
                path,
                rest,
                collected_middleware,
                output_policy,
            } => (command, path, rest, collected_middleware, output_policy),
            ResolvedCommand::Help {
                path,
                description,
                commands,
            } => {
                let help_name = if path == self.name {
                    self.name.clone()
                } else {
                    format!("{} {path}", self.name)
                };
                wln!(&help::format_root(
                    &help_name,
                    &FormatRootOptions {
                        aliases: None,
                        config_flag: config_flag.map(|s| s.to_string()),
                        commands: collect_help_commands(commands),
                        description: description.clone(),
                        root: path == self.name,
                        version: None,
                    },
                ));
                return Ok(None);
            }
            ResolvedCommand::Error { error, path } => {
                if path.is_empty() {
                    if let Some(root_cmd) = &self.root_command {
                        (
                            Arc::clone(root_cmd),
                            self.name.clone(),
                            builtin.rest,
                            Vec::new(),
                            None,
                        )
                    } else {
                        let parent = if path.is_empty() {
                            &self.name
                        } else {
                            &path
                        };
                        let message = format!("'{error}' is not a command for '{parent}'.");
                        if human {
                            wln!(&format_human_error("COMMAND_NOT_FOUND", &message));
                            wln!(&format!(
                                "\nSuggested commands:\n  {} --help",
                                self.name
                            ));
                        } else {
                            let cta_json = serde_json::json!({
                                "code": "COMMAND_NOT_FOUND",
                                "message": message,
                                "cta": {
                                    "description": "See available commands:",
                                    "commands": [{ "command": format!("{} --help", self.name) }]
                                }
                            });
                            wln!(&format_value(&cta_json, builtin.format));
                        }
                        return Ok(Some(1));
                    }
                } else {
                    let parent = format!("{} {path}", self.name);
                    let message = format!("'{error}' is not a command for '{parent}'.");
                    if human {
                        wln!(&format_human_error("COMMAND_NOT_FOUND", &message));
                    } else {
                        let cta_json = serde_json::json!({
                            "code": "COMMAND_NOT_FOUND",
                            "message": message,
                            "cta": {
                                "description": "See available commands:",
                                "commands": [{ "command": format!("{} --help", parent) }]
                            }
                        });
                        wln!(&format_value(&cta_json, builtin.format));
                    }
                    return Ok(Some(1));
                }
            }
        };

        let start = std::time::Instant::now();

        // Resolve effective format
        let format = if builtin.format_explicit {
            builtin.format
        } else {
            command
                .format
                .unwrap_or_else(|| self.format.unwrap_or(Format::Toon))
        };

        // Resolve effective output policy
        let policy = effective_output_policy
            .or(command.output_policy)
            .or(self.output_policy);
        let render_output = !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

        // --- Step 8: Load config defaults ---
        let defaults = if let Some(ref cfg) = self.config {
            if builtin.config_disabled {
                None
            } else {
                let config_path = config::resolve_config_path(
                    builtin.config_path.as_deref(),
                    &cfg.files,
                );
                if let Some(path) = config_path {
                    match config::load_config(&path) {
                        Ok(tree) => {
                            match config::extract_command_section(&tree, &self.name, &command_path) {
                                Ok(section) => section,
                                Err(e) => {
                                    wln!(&format_human_error("CONFIG_ERROR", &e.to_string()));
                                    return Ok(Some(1));
                                }
                            }
                        }
                        Err(e) => {
                            if builtin.config_path.is_some() {
                                wln!(&format_human_error("CONFIG_ERROR", &e.to_string()));
                                return Ok(Some(1));
                            }
                            None
                        }
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        // --- Step 9: Collect middleware ---
        let all_middleware: Vec<MiddlewareFn> = self
            .middleware
            .iter()
            .cloned()
            .chain(collected_mw.into_iter())
            .chain(command.middleware.iter().cloned())
            .collect();

        // --- Step 10: Build env source ---
        let env_source: std::collections::HashMap<String, String> = std::env::vars().collect();

        // --- Step 11: Execute command ---
        let result = command::execute(
            Arc::clone(&command),
            ExecuteOptions {
                agent: !human,
                argv: rest,
                defaults,
                env_fields: self.env_fields.clone(),
                env_source,
                format,
                format_explicit: builtin.format_explicit,
                input_options: BTreeMap::new(),
                middlewares: all_middleware,
                name: self.name.clone(),
                parse_mode: ParseMode::Argv,
                path: command_path.clone(),
                vars_fields: self.vars_fields.clone(),
                version: self.version.clone(),
            },
        )
        .await;

        let duration = start.elapsed();
        let duration_str = format!("{}ms", duration.as_millis());

        // --- Step 12: Handle result ---
        match result {
            InternalResult::Ok { data, cta } => {
                let formatted_cta = format_cta_block(&self.name, cta.as_ref());

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(true));
                    envelope.insert("data".to_string(), data);
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    if let Some(cta) = &formatted_cta {
                        meta.insert("cta".to_string(), serde_json::to_value(cta).unwrap_or(Value::Null));
                    }
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    wln!(&format_value(&Value::Object(envelope), format));
                } else if human {
                    if render_output {
                        wln!(&format_value(&data, format));
                    }
                    if let Some(cta) = &formatted_cta {
                        wln!(&format_human_cta(cta));
                    }
                } else {
                    wln!(&format_value(&data, format));
                }
                Ok(None)
            }
            InternalResult::Error {
                code,
                message,
                retryable,
                field_errors: _,
                cta,
                exit_code,
            } => {
                let formatted_cta = format_cta_block(&self.name, cta.as_ref());

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(false));
                    let mut error_obj = serde_json::Map::new();
                    error_obj.insert("code".to_string(), Value::String(code.clone()));
                    error_obj.insert("message".to_string(), Value::String(message.clone()));
                    if let Some(r) = retryable {
                        error_obj.insert("retryable".to_string(), Value::Bool(r));
                    }
                    envelope.insert("error".to_string(), Value::Object(error_obj));
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    if let Some(cta) = &formatted_cta {
                        meta.insert("cta".to_string(), serde_json::to_value(cta).unwrap_or(Value::Null));
                    }
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    wln!(&format_value(&Value::Object(envelope), format));
                } else if human && !builtin.format_explicit {
                    wln!(&format_human_error(&code, &message));
                    if let Some(cta) = &formatted_cta {
                        wln!(&format_human_cta(cta));
                    }
                } else {
                    let mut error_obj = serde_json::Map::new();
                    error_obj.insert("code".to_string(), Value::String(code.clone()));
                    error_obj.insert("message".to_string(), Value::String(message.clone()));
                    if let Some(r) = retryable {
                        error_obj.insert("retryable".to_string(), Value::Bool(r));
                    }
                    wln!(&format_value(&Value::Object(error_obj), format));
                }

                Ok(Some(exit_code.unwrap_or(1)))
            }
            InternalResult::Stream(mut stream) => {
                use futures::StreamExt;

                let use_jsonl = format == Format::Jsonl;
                let incremental = use_jsonl || (!builtin.format_explicit && format == Format::Toon);

                if incremental {
                    while let Some(value) = stream.next().await {
                        if use_jsonl {
                            let chunk = serde_json::json!({ "type": "chunk", "data": value });
                            wln!(&serde_json::to_string(&chunk).unwrap_or_default());
                        } else if render_output {
                            wln!(&format_value(&value, format));
                        }
                    }
                    if use_jsonl {
                        let done = serde_json::json!({
                            "type": "done",
                            "ok": true,
                            "meta": {
                                "command": command_path,
                                "duration": format!("{}ms", start.elapsed().as_millis()),
                            }
                        });
                        wln!(&serde_json::to_string(&done).unwrap_or_default());
                    }
                } else {
                    let mut chunks: Vec<Value> = Vec::new();
                    while let Some(value) = stream.next().await {
                        chunks.push(value);
                    }
                    let data = Value::Array(chunks);
                    let dur = format!("{}ms", start.elapsed().as_millis());

                    if builtin.verbose {
                        let envelope = serde_json::json!({
                            "ok": true,
                            "data": data,
                            "meta": {
                                "command": command_path,
                                "duration": dur,
                            }
                        });
                        wln!(&format_value(&envelope, format));
                    } else if human {
                        if render_output {
                            wln!(&format_value(&data, format));
                        }
                    } else {
                        wln!(&format_value(&data, format));
                    }
                }

                Ok(None)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command resolution
// ---------------------------------------------------------------------------

/// Result of resolving a command from argv tokens.
enum ResolvedCommand<'a> {
    /// A leaf command was found.
    Leaf {
        command: Arc<CommandDef>,
        path: String,
        rest: Vec<String>,
        collected_middleware: Vec<MiddlewareFn>,
        output_policy: Option<OutputPolicy>,
    },
    /// A group was reached but no further subcommand specified.
    Help {
        path: String,
        description: Option<String>,
        commands: &'a BTreeMap<String, CommandEntry>,
    },
    /// No matching command was found.
    Error {
        error: String,
        path: String,
    },
}

/// Walks argv tokens through the command tree to find the target command.
fn resolve_command<'a>(
    commands: &'a BTreeMap<String, CommandEntry>,
    tokens: &[String],
) -> ResolvedCommand<'a> {
    let (first, rest) = match tokens.split_first() {
        Some((f, r)) => (f, r),
        None => {
            return ResolvedCommand::Error {
                error: "(none)".to_string(),
                path: String::new(),
            }
        }
    };

    let entry = match commands.get(first.as_str()) {
        Some(e) => e,
        None => {
            return ResolvedCommand::Error {
                error: first.clone(),
                path: String::new(),
            }
        }
    };

    let mut path = vec![first.as_str()];
    let mut remaining = rest;
    let mut inherited_output_policy: Option<OutputPolicy> = None;
    let mut collected_middleware: Vec<MiddlewareFn> = Vec::new();
    let mut current = entry;

    loop {
        match current {
            CommandEntry::Leaf(def) => {
                let output_policy = def.output_policy.or(inherited_output_policy);
                return ResolvedCommand::Leaf {
                    command: Arc::clone(def),
                    path: path.join(" "),
                    rest: remaining.to_vec(),
                    collected_middleware,
                    output_policy,
                };
            }
            CommandEntry::Group {
                description,
                commands: sub_commands,
                middleware,
                output_policy,
            } => {
                if let Some(policy) = output_policy {
                    inherited_output_policy = Some(*policy);
                }
                collected_middleware.extend(middleware.iter().cloned());

                let next = match remaining.first() {
                    Some(n) => n,
                    None => {
                        return ResolvedCommand::Help {
                            path: path.join(" "),
                            description: description.clone(),
                            commands: sub_commands,
                        }
                    }
                };

                match sub_commands.get(next.as_str()) {
                    Some(child) => {
                        path.push(next.as_str());
                        remaining = &remaining[1..];
                        current = child;
                    }
                    None => {
                        return ResolvedCommand::Error {
                            error: next.clone(),
                            path: path.join(" "),
                        }
                    }
                }
            }
            CommandEntry::FetchGateway {
                description: _,
                output_policy: _,
                ..
            } => {
                // Fetch gateways are not fully resolved in the Rust port yet.
                // Return as an error so the caller can handle it.
                return ResolvedCommand::Error {
                    error: path.join(" "),
                    path: String::new(),
                };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in flag extraction
// ---------------------------------------------------------------------------

/// Extracted built-in flags from argv.
struct BuiltinFlags {
    verbose: bool,
    format: Format,
    format_explicit: bool,
    #[allow(dead_code)]
    filter_output: Option<String>,
    #[allow(dead_code)]
    token_limit: Option<usize>,
    #[allow(dead_code)]
    token_offset: Option<usize>,
    #[allow(dead_code)]
    token_count: bool,
    #[allow(dead_code)]
    llms: bool,
    #[allow(dead_code)]
    llms_full: bool,
    #[allow(dead_code)]
    mcp: bool,
    help: bool,
    version: bool,
    schema: bool,
    config_path: Option<String>,
    config_disabled: bool,
    rest: Vec<String>,
}

/// Extracts built-in flags from argv, returning the parsed flags and the
/// remaining tokens that should be passed to the command.
fn extract_builtin_flags(
    argv: &[String],
    config_flag: Option<&str>,
) -> Result<BuiltinFlags, Box<dyn std::error::Error>> {
    let mut verbose = false;
    let mut llms = false;
    let mut llms_full = false;
    let mut mcp = false;
    let mut help = false;
    let mut version = false;
    let mut schema = false;
    let mut format = Format::Toon;
    let mut format_explicit = false;
    let mut config_path: Option<String> = None;
    let mut config_disabled = false;
    let mut filter_output: Option<String> = None;
    let mut token_limit: Option<usize> = None;
    let mut token_offset: Option<usize> = None;
    let mut token_count = false;
    let mut rest: Vec<String> = Vec::new();

    let cfg_flag = config_flag.map(|f| format!("--{f}"));
    let cfg_flag_eq = config_flag.map(|f| format!("--{f}="));
    let no_cfg_flag = config_flag.map(|f| format!("--no-{f}"));

    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];

        if token == "--verbose" {
            verbose = true;
        } else if token == "--llms" {
            llms = true;
        } else if token == "--llms-full" {
            llms_full = true;
        } else if token == "--mcp" {
            mcp = true;
        } else if token == "--help" || token == "-h" {
            help = true;
        } else if token == "--version" {
            version = true;
        } else if token == "--schema" {
            schema = true;
        } else if token == "--json" {
            format = Format::Json;
            format_explicit = true;
        } else if token == "--format" {
            if let Some(next) = argv.get(i + 1) {
                if let Some(f) = Format::from_str_opt(next) {
                    format = f;
                } else {
                    format = Format::Toon; // fallback
                }
                format_explicit = true;
                i += 1;
            }
        } else if let Some(ref cfg) = cfg_flag {
            if token == cfg {
                if let Some(next) = argv.get(i + 1) {
                    config_path = Some(next.clone());
                    config_disabled = false;
                    i += 1;
                } else {
                    return Err(format!("Missing value for flag: {cfg}").into());
                }
            } else if let Some(ref eq) = cfg_flag_eq {
                if token.starts_with(eq.as_str()) {
                    let value = &token[eq.len()..];
                    if value.is_empty() {
                        return Err(format!("Missing value for flag: {cfg}").into());
                    }
                    config_path = Some(value.to_string());
                    config_disabled = false;
                } else if let Some(ref no) = no_cfg_flag {
                    if token == no {
                        config_path = None;
                        config_disabled = true;
                    } else {
                        rest.push(token.clone());
                    }
                } else {
                    rest.push(token.clone());
                }
            } else if let Some(ref no) = no_cfg_flag {
                if token == no {
                    config_path = None;
                    config_disabled = true;
                } else {
                    rest.push(token.clone());
                }
            } else {
                rest.push(token.clone());
            }
        } else if token == "--filter-output" {
            if let Some(next) = argv.get(i + 1) {
                filter_output = Some(next.clone());
                i += 1;
            }
        } else if token == "--token-limit" {
            if let Some(next) = argv.get(i + 1) {
                token_limit = next.parse().ok();
                i += 1;
            }
        } else if token == "--token-offset" {
            if let Some(next) = argv.get(i + 1) {
                token_offset = next.parse().ok();
                i += 1;
            }
        } else if token == "--token-count" {
            token_count = true;
        } else {
            rest.push(token.clone());
        }

        i += 1;
    }

    Ok(BuiltinFlags {
        verbose,
        format,
        format_explicit,
        filter_output,
        token_limit,
        token_offset,
        token_count,
        llms,
        llms_full,
        mcp,
        help,
        version,
        schema,
        config_path,
        config_disabled,
        rest,
    })
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

/// Writes a string to stdout with a trailing newline.
fn writeln_stdout(s: &str) {
    if s.ends_with('\n') {
        print!("{s}");
    } else {
        println!("{s}");
    }
}

/// Formats a JSON value using the specified format.
///
/// This is a simplified formatter. The full formatter module (being written
/// in parallel) handles toon, yaml, markdown, etc. For now, we support
/// json and fall back to pretty-printed json.
fn format_value(value: &Value, format: Format) -> String {
    match format {
        Format::Json => serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string()),
        Format::Jsonl => serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()),
        _ => {
            // Default to toon-style output (pretty JSON for now, until
            // the formatter module provides the full implementation).
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string())
        }
    }
}

/// Formats an error for human-readable TTY output.
fn format_human_error(code: &str, message: &str) -> String {
    let prefix = if code == "UNKNOWN" || code == "COMMAND_NOT_FOUND" {
        "Error"
    } else {
        &format!("Error ({code})")
    };
    format!("{prefix}: {message}")
}

/// A formatted CTA block for output.
#[derive(Debug, Clone, serde::Serialize)]
struct FormattedCtaBlock {
    description: String,
    commands: Vec<FormattedCta>,
}

/// A single formatted CTA entry.
#[derive(Debug, Clone, serde::Serialize)]
struct FormattedCta {
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// Formats a CTA block from the command result into the output shape.
fn format_cta_block(name: &str, block: Option<&CtaBlock>) -> Option<FormattedCtaBlock> {
    let block = block?;
    if block.commands.is_empty() {
        return None;
    }

    let commands = block
        .commands
        .iter()
        .map(|entry| match entry {
            CtaEntry::Simple(s) => FormattedCta {
                command: format!("{name} {s}"),
                description: None,
            },
            CtaEntry::Detailed {
                command,
                description,
            } => {
                let prefix = if command == name || command.starts_with(&format!("{name} ")) {
                    String::new()
                } else {
                    format!("{name} ")
                };
                FormattedCta {
                    command: format!("{prefix}{command}"),
                    description: description.clone(),
                }
            }
        })
        .collect();

    Some(FormattedCtaBlock {
        description: block
            .description
            .clone()
            .unwrap_or_else(|| "Suggested commands:".to_string()),
        commands,
    })
}

/// Formats a CTA block for human-readable TTY output.
fn format_human_cta(cta: &FormattedCtaBlock) -> String {
    let mut lines = vec![String::new(), cta.description.clone()];
    let max_len = cta.commands.iter().map(|c| c.command.len()).max().unwrap_or(0);
    for c in &cta.commands {
        let desc = match &c.description {
            Some(d) => {
                let padding = " ".repeat(max_len - c.command.len());
                format!("  {padding}# {d}")
            }
            None => String::new(),
        };
        lines.push(format!("  {}{desc}", c.command));
    }
    lines.join("\n")
}

/// Collects immediate child commands/groups for help output.
fn collect_help_commands(commands: &BTreeMap<String, CommandEntry>) -> Vec<CommandSummary> {
    let mut result: Vec<CommandSummary> = commands
        .iter()
        .map(|(name, entry)| CommandSummary {
            name: name.clone(),
            description: entry.description().map(|s| s.to_string()),
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Checks if any arg fields are required.
fn has_required_args(args_fields: &[FieldMeta]) -> bool {
    args_fields.iter().any(|f| f.required)
}

/// Formats command help including subcommands.
fn format_command_help(
    name: &str,
    command: &CommandDef,
    commands: &BTreeMap<String, CommandEntry>,
    aliases: &[String],
    config_flag: Option<&str>,
    version: Option<&str>,
    root: bool,
) -> String {
    help::format_command(
        name,
        &FormatCommandOptions {
            aliases: if aliases.is_empty() {
                None
            } else {
                Some(aliases.to_vec())
            },
            args_fields: command.args_fields.clone(),
            config_flag: config_flag.map(|s| s.to_string()),
            commands: collect_help_commands(commands),
            description: command.description.clone(),
            env_fields: command.env_fields.clone(),
            examples: command.examples.clone(),
            hint: command.hint.clone(),
            hide_global_options: false,
            options_fields: command.options_fields.clone(),
            option_aliases: command.aliases.clone(),
            root,
            version: version.map(|s| s.to_string()),
        },
    )
}

/// Options for streaming output handling.
struct StreamingOptions<'a> {
    path: &'a str,
    start: std::time::Instant,
    format: Format,
    format_explicit: bool,
    human: bool,
    render_output: bool,
    verbose: bool,
}

/// Handles streaming output from a command.
async fn handle_streaming(
    mut stream: std::pin::Pin<Box<dyn futures::Stream<Item = Value> + Send>>,
    opts: StreamingOptions<'_>,
) {
    let StreamingOptions {
        path,
        start,
        format,
        format_explicit,
        human,
        render_output,
        verbose,
    } = opts;
    use futures::StreamExt;

    let use_jsonl = format == Format::Jsonl;
    let incremental = use_jsonl || (!format_explicit && format == Format::Toon);

    if incremental {
        // Incremental output: write each chunk as it arrives
        while let Some(value) = stream.next().await {
            if use_jsonl {
                let chunk = serde_json::json!({ "type": "chunk", "data": value });
                writeln_stdout(&serde_json::to_string(&chunk).unwrap_or_default());
            } else if render_output {
                writeln_stdout(&format_value(&value, format));
            }
        }

        if use_jsonl {
            let done = serde_json::json!({
                "type": "done",
                "ok": true,
                "meta": {
                    "command": path,
                    "duration": format!("{}ms", start.elapsed().as_millis()),
                }
            });
            writeln_stdout(&serde_json::to_string(&done).unwrap_or_default());
        }
    } else {
        // Buffered output: collect all chunks, write as a single array
        let mut chunks: Vec<Value> = Vec::new();
        while let Some(value) = stream.next().await {
            chunks.push(value);
        }

        let data = Value::Array(chunks);
        let duration_str = format!("{}ms", start.elapsed().as_millis());

        if verbose {
            let envelope = serde_json::json!({
                "ok": true,
                "data": data,
                "meta": {
                    "command": path,
                    "duration": duration_str,
                }
            });
            writeln_stdout(&format_value(&envelope, format));
        } else if human {
            if render_output {
                writeln_stdout(&format_value(&data, format));
            }
        } else {
            writeln_stdout(&format_value(&data, format));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_builtin_flags_basic() {
        let argv: Vec<String> = vec![
            "--verbose".to_string(),
            "--json".to_string(),
            "list".to_string(),
        ];
        let result = extract_builtin_flags(&argv, None).unwrap();
        assert!(result.verbose);
        assert_eq!(result.format, Format::Json);
        assert!(result.format_explicit);
        assert_eq!(result.rest, vec!["list"]);
    }

    #[test]
    fn test_extract_builtin_flags_help() {
        let argv: Vec<String> = vec!["--help".to_string()];
        let result = extract_builtin_flags(&argv, None).unwrap();
        assert!(result.help);
        assert!(result.rest.is_empty());
    }

    #[test]
    fn test_extract_builtin_flags_format() {
        let argv: Vec<String> = vec!["--format".to_string(), "yaml".to_string()];
        let result = extract_builtin_flags(&argv, None).unwrap();
        assert_eq!(result.format, Format::Yaml);
        assert!(result.format_explicit);
    }

    #[test]
    fn test_extract_builtin_flags_config() {
        let argv: Vec<String> = vec![
            "--config".to_string(),
            "myconfig.json".to_string(),
            "deploy".to_string(),
        ];
        let result = extract_builtin_flags(&argv, Some("config")).unwrap();
        assert_eq!(result.config_path, Some("myconfig.json".to_string()));
        assert_eq!(result.rest, vec!["deploy"]);
    }

    #[test]
    fn test_resolve_command_leaf() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "list".to_string(),
            CommandEntry::Leaf(Arc::new(CommandDef {
                name: "list".to_string(),
                description: Some("List items".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: std::collections::HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(NoopHandler),
                middleware: vec![],
                output_schema: None,
            })),
        );

        let tokens = vec!["list".to_string(), "--verbose".to_string()];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Leaf {
                command,
                path,
                rest,
                ..
            } => {
                assert_eq!(path, "list");
                assert_eq!(rest, vec!["--verbose"]);
                assert_eq!(command.name, "list");
            }
            _ => panic!("Expected Leaf"),
        }
    }

    #[test]
    fn test_resolve_command_group() {
        let mut sub_commands = BTreeMap::new();
        sub_commands.insert(
            "get".to_string(),
            CommandEntry::Leaf(Arc::new(CommandDef {
                name: "get".to_string(),
                description: Some("Get a user".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: std::collections::HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(NoopHandler),
                middleware: vec![],
                output_schema: None,
            })),
        );

        let mut commands = BTreeMap::new();
        commands.insert(
            "users".to_string(),
            CommandEntry::Group {
                description: Some("User commands".to_string()),
                commands: sub_commands,
                middleware: vec![],
                output_policy: None,
            },
        );

        let tokens = vec!["users".to_string(), "get".to_string(), "alice".to_string()];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Leaf {
                command,
                path,
                rest,
                ..
            } => {
                assert_eq!(path, "users get");
                assert_eq!(rest, vec!["alice"]);
                assert_eq!(command.name, "get");
            }
            _ => panic!("Expected Leaf"),
        }
    }

    #[test]
    fn test_resolve_command_not_found() {
        let commands = BTreeMap::new();
        let tokens = vec!["nonexistent".to_string()];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Error { error, path } => {
                assert_eq!(error, "nonexistent");
                assert!(path.is_empty());
            }
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_collect_help_commands() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "deploy".to_string(),
            CommandEntry::Leaf(Arc::new(CommandDef {
                name: "deploy".to_string(),
                description: Some("Deploy the app".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: std::collections::HashMap::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(NoopHandler),
                middleware: vec![],
                output_schema: None,
            })),
        );
        commands.insert(
            "build".to_string(),
            CommandEntry::Group {
                description: Some("Build commands".to_string()),
                commands: BTreeMap::new(),
                middleware: vec![],
                output_policy: None,
            },
        );

        let summaries = collect_help_commands(&commands);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "build");
        assert_eq!(summaries[1].name, "deploy");
    }

    /// A no-op handler for tests.
    struct NoopHandler;

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for NoopHandler {
        async fn run(&self, _ctx: crate::command::CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: Value::Null,
                cta: None,
            }
        }
    }

    #[test]
    fn test_format_human_error() {
        assert_eq!(
            format_human_error("UNKNOWN", "Something went wrong"),
            "Error: Something went wrong"
        );
        assert_eq!(
            format_human_error("AUTH_FAILED", "Not logged in"),
            "Error (AUTH_FAILED): Not logged in"
        );
    }
}
