//! The main CLI type for the incurs framework.
//!
//! This module provides [`Cli`], the entry point for building command-line
//! applications with incurs. It supports:
//!
//! - Registering commands and command groups
//! - Middleware that runs around every command
//! - Built-in flags (--help, --version, --format, --json, --full-output, etc.)
//! - Config file loading for option defaults
//! - Three-transport architecture (CLI, HTTP, MCP)
//!
//! Ported from `src/Cli.ts`.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::IsTerminal;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::command::{self, CommandDef, ExecuteOptions, InternalResult, ParseMode};
use crate::config;
use crate::fetch::{self, FetchGatewayOptions, FetchHandler};
use crate::filter;
use crate::help::{self, CommandSummary, FormatCommandOptions, FormatRootOptions};
use crate::middleware::MiddlewareFn;
use crate::output::*;
use crate::schema::FieldMeta;
use crate::skill;

struct ProcessWriter;

impl std::io::Write for ProcessWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        std::io::stdout().write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

/// Injectable process inputs used by the shared CLI execution path.
pub struct Runtime {
    /// Actual binary name used in user-facing output.
    pub display_name: String,
    /// Environment variables visible to command schemas.
    pub env: HashMap<String, String>,
    /// Whether output is being rendered for a human terminal.
    pub human: bool,
}

impl Runtime {
    /// Creates a runtime using explicit process inputs.
    pub fn new(display_name: impl Into<String>, env: HashMap<String, String>, human: bool) -> Self {
        Self {
            display_name: display_name.into(),
            env,
            human,
        }
    }

    /// Creates a runtime from the current process environment.
    pub fn process(display_name: impl Into<String>, human: bool) -> Self {
        Self::new(display_name, std::env::vars().collect(), human)
    }
}

/// Controls which consumers see a root help banner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BannerMode {
    /// Show the banner to human and agent consumers.
    #[default]
    All,
    /// Show the banner only when stdout is a terminal.
    Human,
    /// Show the banner only to non-terminal agent consumers.
    Agent,
}

/// Async banner renderer used above root help output.
pub type BannerFn =
    Arc<dyn Fn(bool) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

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
        /// The fetch handler that processes requests.
        handler: Arc<dyn FetchHandler>,
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
    /// Optional content rendered above root help.
    banner: Option<(BannerMode, BannerFn)>,
    /// The command tree.
    pub(crate) commands: BTreeMap<String, CommandEntry>,
    /// Root-level middleware that runs around every command.
    pub(crate) middleware: Vec<MiddlewareFn>,
    /// Root command handler (for CLIs with a default command).
    pub(crate) root_command: Option<Arc<CommandDef>>,
    /// CLI-level environment variable fields.
    pub(crate) env_fields: Vec<FieldMeta>,
    /// CLI-level global option fields.
    pub(crate) globals_fields: Vec<FieldMeta>,
    /// CLI-level global option aliases.
    pub(crate) global_aliases: HashMap<String, char>,
    /// Middleware variable fields.
    pub(crate) vars_fields: Vec<FieldMeta>,
    /// Config file options.
    config: Option<ConfigOptions>,
    /// Default output policy.
    output_policy: Option<OutputPolicy>,
    /// Default output format.
    format: Option<Format>,
    /// MCP server instructions, discovery, and filtering options.
    pub(crate) mcp_options: crate::mcp::McpServeOptions,
}

impl Cli {
    /// Creates a new CLI with the given name.
    pub fn create(name: impl Into<String>) -> Self {
        Cli {
            name: name.into(),
            description: None,
            version: None,
            aliases: Vec::new(),
            banner: None,
            commands: BTreeMap::new(),
            middleware: Vec::new(),
            root_command: None,
            env_fields: Vec::new(),
            globals_fields: Vec::new(),
            global_aliases: HashMap::new(),
            vars_fields: Vec::new(),
            config: None,
            output_policy: None,
            format: None,
            mcp_options: crate::mcp::McpServeOptions::default(),
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

    /// Configures an async root help banner renderer.
    pub fn banner(mut self, mode: BannerMode, banner: BannerFn) -> Self {
        self.banner = Some((mode, banner));
        self
    }

    /// Configures static content above root help output.
    pub fn banner_text(self, mode: BannerMode, banner: impl Into<String>) -> Self {
        let banner = banner.into();
        self.banner(
            mode,
            Arc::new(move |_| {
                let banner = banner.clone();
                Box::pin(async move { Some(banner) })
            }),
        )
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

    /// Configures MCP server instructions, discovery, and tool filtering.
    pub fn mcp(mut self, options: crate::mcp::McpServeOptions) -> Self {
        self.mcp_options = options;
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

    /// Sets CLI-level global option fields and extracts their aliases.
    pub fn globals_fields(mut self, fields: Vec<FieldMeta>) -> Self {
        const BUILTIN_FLAGS: &[&str] = &[
            "config-schema",
            "filter-output",
            "format",
            "full-output",
            "help",
            "json",
            "llms",
            "llms-full",
            "mcp",
            "schema",
            "token-count",
            "token-limit",
            "token-offset",
            "version",
        ];
        for field in &fields {
            assert!(
                !BUILTIN_FLAGS.contains(&field.cli_name.as_str()),
                "Global option --{} conflicts with a built-in flag",
                field.cli_name
            );
            if let Some(alias) = field.alias {
                assert!(
                    alias != 'h',
                    "Global alias -{alias} conflicts with a built-in short flag"
                );
                self.global_aliases.insert(field.name.to_string(), alias);
            }
        }
        self.globals_fields = fields;
        if let Some(root) = &self.root_command {
            self.assert_no_global_conflicts(root);
        }
        self.assert_no_group_global_conflicts(&self.commands);
        self
    }

    /// Sets CLI-level global options from a type that implements `IncurSchema`.
    pub fn globals<T: crate::schema::IncurSchema>(self) -> Self {
        self.globals_fields(T::fields())
    }

    /// Adds or overrides aliases for CLI-level global options.
    pub fn global_aliases(mut self, aliases: HashMap<String, char>) -> Self {
        for alias in aliases.values() {
            assert!(
                *alias != 'h',
                "Global alias -{alias} conflicts with a built-in short flag"
            );
        }
        self.global_aliases.extend(aliases);
        if let Some(root) = &self.root_command {
            self.assert_no_global_conflicts(root);
        }
        self.assert_no_group_global_conflicts(&self.commands);
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
        self.assert_no_global_conflicts(&def);
        self.commands
            .insert(name.into(), CommandEntry::Leaf(Arc::new(def)));
        self
    }

    /// Mounts a sub-CLI as a command group.
    ///
    /// If the sub-CLI has a root command and no subcommands, it is mounted
    /// as a leaf command (a "leaf CLI"). Otherwise it is mounted as a group.
    pub fn group(mut self, cli: Cli) -> Self {
        if let Some(root) = &cli.root_command {
            self.assert_no_global_conflicts(root);
        }
        self.assert_no_group_global_conflicts(&cli.commands);
        if let Some(root_cmd) = cli.root_command
            && cli.commands.is_empty()
        {
            // Leaf CLI: mount the root command directly as a leaf.
            self.commands.insert(cli.name, CommandEntry::Leaf(root_cmd));
            return self;
        }
        // Has both root command and subcommands — mount as a group but
        // insert the root command under a synthetic "" key so it can be
        // resolved. For now, treat it the same as a regular group and
        // the root command is lost. A full solution would extend
        // CommandEntry::Group with an optional root_command field.
        let entry = CommandEntry::Group {
            description: cli.description,
            commands: cli.commands,
            middleware: cli.middleware,
            output_policy: cli.output_policy,
        };
        self.commands.insert(cli.name, entry);
        self
    }

    fn assert_no_global_conflicts(&self, command: &CommandDef) {
        for field in &command.options_fields {
            assert!(
                !self
                    .globals_fields
                    .iter()
                    .any(|global| global.name == field.name),
                "Command option --{} conflicts with a global option",
                field.cli_name
            );
            if let Some(alias) = command.aliases.get(field.name) {
                assert!(
                    !self.global_aliases.values().any(|global| global == alias),
                    "Command alias -{alias} conflicts with a global alias"
                );
            }
        }
    }

    fn assert_no_group_global_conflicts(&self, commands: &BTreeMap<String, CommandEntry>) {
        for entry in commands.values() {
            match entry {
                CommandEntry::Leaf(command) => self.assert_no_global_conflicts(command),
                CommandEntry::Group { commands, .. } => {
                    self.assert_no_group_global_conflicts(commands)
                }
                CommandEntry::FetchGateway { .. } => {}
            }
        }
    }

    /// Registers a fetch gateway command.
    ///
    /// A fetch gateway proxies curl-style argv into a [`FetchHandler`].
    /// Remaining tokens after the gateway name are parsed with
    /// [`fetch::parse_argv`] and forwarded to the handler.
    pub fn fetch_gateway(
        mut self,
        name: impl Into<String>,
        handler: impl FetchHandler + 'static,
        options: FetchGatewayOptions,
    ) -> Self {
        self.commands.insert(
            name.into(),
            CommandEntry::FetchGateway {
                description: options.description,
                base_path: options.base_path,
                output_policy: options.output_policy,
                handler: Arc::new(handler),
            },
        );
        self
    }

    /// Mounts tools from a remote MCP-over-HTTP server as a command group.
    #[cfg(all(feature = "mcp", feature = "http"))]
    pub async fn remote_mcp(
        mut self,
        name: impl Into<String>,
        uri: impl Into<String>,
        description: Option<String>,
    ) -> Result<Self, crate::errors::Error> {
        let commands = crate::mcp::remote_commands(uri).await?;
        let commands = commands
            .into_iter()
            .map(|(name, command)| (name, CommandEntry::Leaf(Arc::new(command))))
            .collect();
        self.commands.insert(
            name.into(),
            CommandEntry::Group {
                description,
                commands,
                middleware: Vec::new(),
                output_policy: None,
            },
        );
        Ok(self)
    }

    /// Generates and mounts a command group from an OpenAPI document.
    #[cfg(feature = "openapi")]
    pub async fn openapi_group(
        mut self,
        name: impl Into<String>,
        spec: &Value,
        fetch: crate::openapi::FetchFn,
        options: crate::openapi::GenerateOptions,
        description: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let commands = crate::openapi::generate_commands(spec, fetch, &options).await?;
        let mut entries = BTreeMap::new();
        for (path, command) in commands {
            insert_generated_command(&mut entries, &path, command);
        }
        self.commands.insert(
            name.into(),
            CommandEntry::Group {
                description,
                commands: entries,
                middleware: Vec::new(),
                output_policy: None,
            },
        );
        Ok(self)
    }

    /// Loads an OpenAPI document source and mounts its generated commands.
    #[cfg(feature = "openapi")]
    pub async fn openapi_source_group(
        self,
        name: impl Into<String>,
        source: crate::openapi::OpenApiSource,
        fetch: crate::openapi::FetchFn,
        options: crate::openapi::GenerateOptions,
        description: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let spec = crate::openapi::load_source(source).await?;
        self.openapi_group(name, &spec, fetch, options, description)
            .await
    }

    /// Registers middleware that runs around every command.
    pub fn use_middleware(mut self, handler: MiddlewareFn) -> Self {
        self.middleware.push(handler);
        self
    }

    /// Generates the skill files exposed by HTTP discovery and `skills add`.
    #[cfg(feature = "http")]
    pub(crate) fn skill_files(&self, depth: usize) -> Vec<skill::SkillFile> {
        skill::split(
            &self.name,
            &collect_all_command_info(self.root_command.as_ref(), &self.commands),
            depth,
            &collect_group_descriptions(&self.commands, &[]),
        )
    }

    /// Parses process argv, runs the matched command, writes output to stdout.
    pub async fn serve(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut process = std::env::args();
        let display_name = process
            .next()
            .and_then(|path| {
                std::path::Path::new(&path)
                    .file_name()?
                    .to_str()
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| self.name.clone());
        let human = std::io::stdout().is_terminal();
        let mut writer = ProcessWriter;
        if let Some(code) = self
            .serve_to_with_display_name(process.collect(), &mut writer, human, display_name)
            .await?
        {
            std::process::exit(code);
        }
        Ok(())
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
    ///
    /// Computes the "Skills are out of date" CTA: when installed skills exist
    /// for this CLI and the stored hash differs from the live command tree.
    /// Ported from `Cli.ts` `skillsCta`.
    fn compute_skills_cta(&self) -> Option<FormattedCtaBlock> {
        let stored = crate::sync_skills::read_hash(&self.name)?;
        if !crate::sync_skills::has_installed_skills(&self.name, None) {
            return None;
        }
        let live = skill::hash(&collect_all_command_info(
            self.root_command.as_ref(),
            &self.commands,
        ));
        if live == stored {
            return None;
        }
        Some(FormattedCtaBlock {
            description: "Skills are out of date:".to_string(),
            commands: vec![FormattedCta {
                command: format!("{} skills add", self.name),
                description: Some("sync outdated skills".to_string()),
            }],
        })
    }

    async fn render_banner(&self, human: bool) -> Option<String> {
        let (mode, banner) = self.banner.as_ref()?;
        if matches!(mode, BannerMode::Human) && !human || matches!(mode, BannerMode::Agent) && human
        {
            return None;
        }
        banner(human).await
    }

    pub async fn serve_with(&self, argv: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
        let human = std::io::stdout().is_terminal();
        let mut writer = ProcessWriter;
        if let Some(code) = self
            .serve_to_with_display_name(argv, &mut writer, human, self.name.clone())
            .await?
        {
            std::process::exit(code);
        }
        Ok(())
    }

    #[allow(dead_code)]
    async fn serve_with_display_name(
        &self,
        argv: Vec<String>,
        display_name: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let human = std::io::stdout().is_terminal();
        let config_flag = self.config.as_ref().map(|c| c.flag.as_str());

        // --- Step 1: Extract built-in flags ---
        let mut builtin = match extract_builtin_flags(&argv, config_flag) {
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

        let globals = match crate::parser::parse_globals(
            &builtin.rest,
            &self.globals_fields,
            &self.global_aliases,
        ) {
            Ok(parsed) => {
                builtin.rest = parsed.rest;
                Value::Object(parsed.parsed.into_iter().collect())
            }
            Err(error) => {
                let msg = error.to_string();
                if human {
                    writeln_stdout(&format_human_error("VALIDATION_ERROR", &msg));
                } else {
                    writeln_stdout(&format!(
                        "{{\"ok\":false,\"error\":{{\"code\":\"VALIDATION_ERROR\",\"message\":\"{}\"}}}}",
                        msg.replace('"', "\\\"")
                    ));
                }
                std::process::exit(1);
            }
        };

        // --- Step 2: Handle --version ---
        if builtin.version
            && !builtin.help
            && let Some(v) = &self.version
        {
            writeln_stdout(v);
            return Ok(());
        }

        // --- Step 2b: Handle --llms / --llms-full ---
        if builtin.llms || builtin.llms_full {
            let commands_info =
                collect_all_command_info(self.root_command.as_ref(), &self.commands);
            if builtin.format_explicit && builtin.format != Format::Markdown {
                let manifest = if builtin.llms_full {
                    build_llms_manifest(&self.commands, &self.globals_fields, true)
                } else {
                    build_llms_manifest(&self.commands, &self.globals_fields, false)
                };
                writeln_stdout(&format_value(&manifest, builtin.format));
            } else if builtin.llms_full {
                let groups = collect_group_descriptions(&self.commands, &[]);
                let output = skill::generate(&self.name, &commands_info, &groups);
                writeln_stdout(&output);
            } else {
                let output = skill::index(&self.name, &commands_info, self.description.as_deref());
                writeln_stdout(&output);
            }
            return Ok(());
        }

        // --- Step 2c: Handle --mcp ---
        if builtin.mcp {
            #[cfg(feature = "mcp")]
            {
                let version = self.version.as_deref().unwrap_or("0.0.0");
                crate::mcp::serve(
                    &self.name,
                    version,
                    &self.commands,
                    &self.middleware,
                    &self.env_fields,
                    &self.mcp_options,
                )
                .await?;
                return Ok(());
            }
            #[cfg(not(feature = "mcp"))]
            {
                writeln_stdout("MCP support requires the 'mcp' feature flag.");
                std::process::exit(1);
            }
        }

        if let Some(output) = completion_output(
            &self.name,
            &self.aliases,
            &self.commands,
            self.root_command.as_ref(),
            &self.globals_fields,
            &self.global_aliases,
            &argv,
            std::env::var("COMPLETE").ok().as_deref(),
            std::env::var("_COMPLETE_INDEX").ok().as_deref(),
        ) {
            writeln_stdout(&output);
            return Ok(());
        }

        let builtins = builtin_commands(&self.name);

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "completions") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "completions")
                .expect("completions builtin must exist");
            let shell = builtin.rest.get(index + 1).map(|token| token.as_str());

            if builtin.help || shell.is_none() {
                writeln_stdout(&help::format_command(
                    &format!("{} completions", self.name),
                    &FormatCommandOptions {
                        aliases: None,
                        args_fields: builtin_def.args_fields.clone(),
                        config_flag: None,
                        commands: Vec::new(),
                        description: Some(builtin_def.description.to_string()),
                        env_fields: Vec::new(),
                        examples: Vec::new(),
                        global_aliases: HashMap::new(),
                        globals_fields: Vec::new(),
                        hint: builtin_def.hint.clone(),
                        hide_global_options: true,
                        options_fields: Vec::new(),
                        option_aliases: HashMap::new(),
                        root: false,
                        version: None,
                    },
                ));
                return Ok(());
            }

            let shell = shell.expect("checked above");
            if crate::completions::Shell::from_str(shell).is_none() {
                writeln_stdout(&format_human_error(
                    "INVALID_SHELL",
                    &format!("Unknown shell '{shell}'. Supported: bash, fish, nushell, zsh"),
                ));
                std::process::exit(1);
            }

            let output = std::iter::once(self.name.clone())
                .chain(self.aliases.iter().cloned())
                .map(|name| {
                    crate::completions::register(
                        crate::completions::Shell::from_str(shell).expect("checked above"),
                        &name,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            writeln_stdout(&output);
            return Ok(());
        }

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "skills") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "skills")
                .expect("skills builtin must exist");

            let skills_sub_token = builtin.rest.get(index + 1).map(|token| token.as_str());
            let skills_sub = skills_sub_token.map(|tok| {
                builtin_def
                    .subcommands
                    .iter()
                    .find(|s| s.name == tok || s.aliases.contains(&tok))
                    .map(|s| s.name)
                    .unwrap_or(tok)
            });

            // `skills list` / `skills ls`: list skills with installed status.
            if skills_sub == Some("list") {
                if builtin.help {
                    writeln_stdout(&format_builtin_subcommand_help(
                        &self.name,
                        builtin_def,
                        "list",
                    ));
                    return Ok(());
                }
                let skills = crate::sync_skills::list(
                    &self.name,
                    &collect_all_command_info(self.root_command.as_ref(), &self.commands),
                    1,
                    self.description.as_deref(),
                );
                if skills.is_empty() {
                    writeln_stdout("No skills found.");
                    return Ok(());
                }
                let max_len = skills.iter().map(|s| s.name.len()).max().unwrap_or(0);
                let mut lines: Vec<String> = Vec::new();
                for s in &skills {
                    let icon = if s.installed { "\u{2713}" } else { "\u{2717}" };
                    let padding = match &s.description {
                        Some(d) => format!("{}  {d}", " ".repeat(max_len - s.name.len())),
                        None => String::new(),
                    };
                    lines.push(format!("  {icon} {}{padding}", s.name));
                }
                let installed_count = skills.iter().filter(|s| s.installed).count();
                lines.push(String::new());
                lines.push(format!(
                    "{} skill{} ({installed_count} installed)",
                    skills.len(),
                    if skills.len() == 1 { "" } else { "s" },
                ));
                writeln_stdout(&lines.join("\n"));
                return Ok(());
            }

            if skills_sub != Some("add") {
                writeln_stdout(&format_builtin_help(&self.name, builtin_def));
                return Ok(());
            }

            if builtin.help {
                writeln_stdout(&format_builtin_subcommand_help(
                    &self.name,
                    builtin_def,
                    "add",
                ));
                return Ok(());
            }

            let rest = builtin.rest[(index + 2)..].to_vec();
            let depth = if let Some(depth_index) = rest.iter().position(|token| token == "--depth")
            {
                rest.get(depth_index + 1)
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1)
            } else if let Some(token) = rest.iter().find(|token| token.starts_with("--depth=")) {
                token
                    .split_once('=')
                    .and_then(|(_, value)| value.parse::<usize>().ok())
                    .unwrap_or(1)
            } else {
                1
            };

            let result = crate::sync_skills::sync(
                &self.name,
                &collect_all_command_info(self.root_command.as_ref(), &self.commands),
                &crate::sync_skills::SyncOptions {
                    cwd: None,
                    depth: Some(depth),
                    description: self.description.clone(),
                    global: !rest.iter().any(|token| token == "--no-global"),
                    include: None,
                },
            )
            .await;

            match result {
                Ok(result) => {
                    let mut lines = vec![format!(
                        "Synced {} skill{}",
                        result.skills.len(),
                        if result.skills.len() == 1 { "" } else { "s" },
                    )];
                    for skill in &result.skills {
                        lines.push(format!("  {}", skill.name));
                    }
                    writeln_stdout(&lines.join("\n"));

                    if builtin.verbose || builtin.format_explicit {
                        let mut output = serde_json::Map::new();
                        output.insert(
                            "skills".to_string(),
                            Value::Array(
                                result
                                    .paths
                                    .iter()
                                    .map(|path| Value::String(path.to_string_lossy().to_string()))
                                    .collect(),
                            ),
                        );
                        if builtin.verbose {
                            output.insert(
                                "agents".to_string(),
                                Value::Array(
                                    result
                                        .agents
                                        .iter()
                                        .map(|agent| {
                                            serde_json::json!({
                                                "agent": agent.agent,
                                                "path": agent.path.to_string_lossy().to_string(),
                                                "mode": match agent.mode {
                                                    crate::agents::InstallMode::Symlink => "symlink",
                                                    crate::agents::InstallMode::Copy => "copy",
                                                },
                                            })
                                        })
                                        .collect(),
                                ),
                            );
                        }
                        writeln_stdout(&format_value(
                            &Value::Object(output),
                            if builtin.format_explicit {
                                builtin.format
                            } else {
                                Format::Toon
                            },
                        ));
                    }
                    return Ok(());
                }
                Err(error) => {
                    writeln_stdout(&format_human_error(
                        "SYNC_SKILLS_FAILED",
                        &error.to_string(),
                    ));
                    std::process::exit(1);
                }
            }
        }

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "mcp") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "mcp")
                .expect("mcp builtin must exist");

            let mcp_sub_token = builtin.rest.get(index + 1).map(|token| token.as_str());
            let mcp_sub = mcp_sub_token.map(|token| {
                builtin_def
                    .subcommands
                    .iter()
                    .find(|sub| sub.name == token || sub.aliases.contains(&token))
                    .map(|sub| sub.name)
                    .unwrap_or(token)
            });

            if mcp_sub.is_none() {
                writeln_stdout(&format_builtin_help(&self.name, builtin_def));
                return Ok(());
            }

            if builtin.help {
                writeln_stdout(&format_builtin_subcommand_help(
                    &self.name,
                    builtin_def,
                    mcp_sub.expect("checked above"),
                ));
                return Ok(());
            }

            if mcp_sub == Some("doctor") {
                writeln_stdout(&format_value(
                    &mcp_doctor_result(&self.commands, &self.mcp_options.tools),
                    builtin.format,
                ));
                return Ok(());
            }

            if mcp_sub != Some("add") {
                writeln_stdout(&format_builtin_help(&self.name, builtin_def));
                return Ok(());
            }

            let rest = builtin.rest[(index + 2)..].to_vec();
            let mut command = None;
            let mut agents = Vec::new();
            let mut cursor = 0;

            while cursor < rest.len() {
                if (rest[cursor] == "--command" || rest[cursor] == "-c")
                    && let Some(value) = rest.get(cursor + 1)
                {
                    command = Some(value.clone());
                    cursor += 2;
                    continue;
                }
                if rest[cursor] == "--agent"
                    && let Some(value) = rest.get(cursor + 1)
                {
                    agents.push(value.clone());
                    cursor += 2;
                    continue;
                }
                cursor += 1;
            }

            let result = crate::sync_mcp::register(
                &self.name,
                &crate::sync_mcp::RegisterOptions {
                    agents: if agents.is_empty() {
                        None
                    } else {
                        Some(agents)
                    },
                    command,
                    global: !rest.iter().any(|token| token == "--no-global"),
                },
            )
            .await;

            match result {
                Ok(result) => {
                    let mut lines = vec![format!("Registered {} as MCP server", self.name)];
                    if !result.agents.is_empty() {
                        lines.push(format!("Agents: {}", result.agents.join(", ")));
                    }
                    writeln_stdout(&lines.join("\n"));

                    if builtin.verbose || builtin.format_explicit {
                        writeln_stdout(&format_value(
                            &serde_json::json!({
                                "name": self.name,
                                "command": result.command,
                                "agents": result.agents,
                            }),
                            if builtin.format_explicit {
                                builtin.format
                            } else {
                                Format::Toon
                            },
                        ));
                    }
                    return Ok(());
                }
                Err(error) => {
                    writeln_stdout(&format_human_error("MCP_ADD_FAILED", &error.to_string()));
                    std::process::exit(1);
                }
            }
        }

        if builtin.config_schema {
            if self.config.is_none() {
                writeln_stdout(&format_human_error(
                    "CONFIG_SCHEMA_UNAVAILABLE",
                    "--config-schema requires CLI config support.",
                ));
                std::process::exit(1);
            }
            writeln_stdout(&format_config_schema(
                self.root_command.as_ref(),
                &self.commands,
            )?);
            return Ok(());
        }

        // --- Step 3: Handle --help at root level ---
        if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                // Root command exists — if human and has required args with
                // none provided, show help.
                if human && has_required_args(&root_cmd.args_fields) {
                    if let Some(banner) = self.render_banner(human).await {
                        writeln_stdout(&banner);
                    }
                    writeln_stdout(&format_command_help(
                        &self.name,
                        root_cmd,
                        &self.commands,
                        &self.aliases,
                        config_flag,
                        self.version.as_deref(),
                        &self.globals_fields,
                        &self.global_aliases,
                        true,
                    ));
                    return Ok(());
                }
                // Otherwise fall through to execute the root command
            } else if !builtin.help {
                // No root command, no args — show root help
                if let Some(banner) = self.render_banner(human).await {
                    writeln_stdout(&banner);
                }
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
                        global_aliases: self.global_aliases.clone(),
                        globals_fields: self.globals_fields.clone(),
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
                ResolvedCommand::Leaf { command, path, .. } => {
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
                    if is_root && let Some(banner) = self.render_banner(human).await {
                        writeln_stdout(&banner);
                    }
                    writeln_stdout(&help::format_command(
                        &command_name,
                        &FormatCommandOptions {
                            aliases: if is_root && !self.aliases.is_empty() {
                                Some(self.aliases.clone())
                            } else if !is_root && !command.command_aliases.is_empty() {
                                Some(command.command_aliases.clone())
                            } else {
                                None
                            },
                            args_fields: command.args_fields.clone(),
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: help_cmds,
                            description: command.description.clone(),
                            env_fields: command.env_fields.clone(),
                            examples: command.examples.clone(),
                            global_aliases: self.global_aliases.clone(),
                            globals_fields: self.globals_fields.clone(),
                            hint: command.hint.clone(),
                            hide_global_options: false,
                            options_fields: command.options_fields.clone(),
                            option_aliases: command.aliases.clone(),
                            root: is_root,
                            version: if is_root { self.version.clone() } else { None },
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
                    if is_root
                        && let Some(root_cmd) = &self.root_command
                        && !commands.is_empty()
                    {
                        if let Some(banner) = self.render_banner(human).await {
                            writeln_stdout(&banner);
                        }
                        writeln_stdout(&format_command_help(
                            &self.name,
                            root_cmd,
                            commands,
                            &self.aliases,
                            config_flag,
                            self.version.as_deref(),
                            &self.globals_fields,
                            &self.global_aliases,
                            true,
                        ));
                    } else {
                        if is_root && let Some(banner) = self.render_banner(human).await {
                            writeln_stdout(&banner);
                        }
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
                                global_aliases: self.global_aliases.clone(),
                                globals_fields: self.globals_fields.clone(),
                                root: is_root,
                                version: if is_root { self.version.clone() } else { None },
                            },
                        ));
                    }
                }
                ResolvedCommand::Gateway { path, .. } => {
                    let help_name = format!("{} {path}", self.name);
                    writeln_stdout(&format!(
                        "{help_name}: fetch gateway (use curl-style arguments)"
                    ));
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
                            global_aliases: self.global_aliases.clone(),
                            globals_fields: self.globals_fields.clone(),
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
                    let schema = build_command_schema(command, &self.globals_fields);
                    let fmt = if builtin.format_explicit {
                        builtin.format
                    } else {
                        Format::Toon
                    };
                    writeln_stdout(&format_value(&schema, fmt));
                }
                _ => {
                    writeln_stdout("--schema requires a command.");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        // --- Step 6b: Handle FetchGateway resolution ---
        if let ResolvedCommand::Gateway {
            handler,
            path,
            rest,
            base_path,
            output_policy,
        } = &resolved
        {
            let format = if builtin.format_explicit {
                builtin.format
            } else {
                self.format.unwrap_or(Format::Json)
            };

            let policy = output_policy.or(self.output_policy);
            let render_output =
                !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

            let mut fetch_input = match fetch::parse_argv_checked(rest) {
                Ok(input) => input,
                Err(error) => {
                    writeln_stdout(&format_human_error("VALIDATION_ERROR", &error.to_string()));
                    std::process::exit(1);
                }
            };

            // Prepend base_path to the request path if configured.
            if let Some(bp) = base_path {
                let trimmed = bp.trim_end_matches('/');
                fetch_input.path = format!("{trimmed}{}", fetch_input.path);
            }

            let output = handler.handle(fetch_input).await;
            let data = format_fetch_output(&output);

            if builtin.verbose {
                let mut envelope = serde_json::Map::new();
                envelope.insert("ok".to_string(), Value::Bool(output.ok));
                envelope.insert("data".to_string(), data);
                let mut meta = serde_json::Map::new();
                meta.insert("command".to_string(), Value::String(path.clone()));
                meta.insert("status".to_string(), Value::Number(output.status.into()));
                envelope.insert("meta".to_string(), Value::Object(meta));
                writeln_stdout(&format_value(&Value::Object(envelope), format));
            } else if render_output {
                writeln_stdout(&format_value(&data, format));
            }

            if !output.ok {
                std::process::exit(1);
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
            // Gateway is handled above in Step 6b; unreachable here.
            ResolvedCommand::Gateway { .. } => unreachable!("Gateway handled before step 7"),
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
                        global_aliases: self.global_aliases.clone(),
                        globals_fields: self.globals_fields.clone(),
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
                            builtin.rest.clone(),
                            Vec::new(),
                            None,
                        )
                    } else {
                        let parent = if path.is_empty() { &self.name } else { &path };
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
        let render_output =
            !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

        // --- Step 8: Load config defaults ---
        let defaults = if let Some(ref cfg) = self.config {
            if builtin.config_disabled {
                None
            } else {
                let config_path =
                    config::resolve_config_path(builtin.config_path.as_deref(), &cfg.files);
                if let Some(path) = config_path {
                    match config::load_config(&path) {
                        Ok(tree) => {
                            match config::extract_command_section(&tree, &self.name, &command_path)
                            {
                                Ok(section) => section,
                                Err(e) => {
                                    writeln_stdout(&format_human_error(
                                        "CONFIG_ERROR",
                                        &e.to_string(),
                                    ));
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
            .chain(collected_mw)
            .chain(command.middleware.iter().cloned())
            .collect();

        // --- Step 10: Build env source ---
        let env_source: std::collections::HashMap<String, String> = std::env::vars().collect();

        // --- Step 10b: Emit deprecation warnings (human/TTY mode only) ---
        if human {
            for warning in deprecation_warnings(&rest, &command.options_fields, &command.aliases) {
                eprintln!("{warning}");
            }
        }

        // --- Step 11: Execute command ---
        let result = command::execute(
            Arc::clone(&command),
            ExecuteOptions {
                agent: !human,
                argv: rest,
                defaults,
                display_name,
                env_fields: self.env_fields.clone(),
                env_source,
                format,
                format_explicit: builtin.format_explicit,
                globals,
                input_options: BTreeMap::new(),
                middlewares: all_middleware,
                name: self.name.clone(),
                parse_mode: ParseMode::Argv,
                path: command_path.clone(),
                request: None,
                vars_fields: self.vars_fields.clone(),
                version: self.version.clone(),
            },
        )
        .await;

        let duration = start.elapsed();
        let duration_str = format!("{}ms", duration.as_millis());

        // Compute the "Skills are out of date" CTA (merged into output CTAs).
        let skills_cta = self.compute_skills_cta();

        // --- Step 12: Handle result ---
        match result {
            InternalResult::Ok { data, cta } => {
                // Apply --filter-output
                let data = if let Some(ref expr) = builtin.filter_output {
                    let paths = filter::parse(expr);
                    filter::apply(&data, &paths)
                } else {
                    data
                };

                let formatted_cta = merge_cta(
                    format_cta_block(&self.name, cta.as_ref()),
                    skills_cta.as_ref(),
                );

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(true));
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    if let Some(cta) = &formatted_cta {
                        meta.insert(
                            "cta".to_string(),
                            serde_json::to_value(cta).unwrap_or(Value::Null),
                        );
                    }
                    // Truncate `data` separately so meta (incl. nextOffset) stays visible.
                    #[cfg(feature = "tokens")]
                    let data = {
                        let data_formatted = format_value(&data, format);
                        if let Some((text, next_offset)) =
                            truncate_tokens(&data_formatted, &builtin)
                        {
                            if let Some(n) = next_offset {
                                meta.insert("nextOffset".to_string(), Value::from(n));
                            }
                            Value::String(text)
                        } else {
                            data
                        }
                    };
                    envelope.insert("data".to_string(), data);
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    let output = format_value(&Value::Object(envelope), format);
                    writeln_stdout(&output);
                } else if human {
                    if render_output {
                        let output = format_value(&data, format);
                        write_with_token_ops(&output, &builtin, writeln_stdout);
                    }
                    if let Some(cta) = &formatted_cta {
                        writeln_stdout(&format_human_cta(cta));
                    }
                } else {
                    // Agent mode: include CTA in the JSON envelope if present
                    if let Some(cta) = &formatted_cta {
                        if let Value::Object(ref map) = data {
                            let mut out = map.clone();
                            out.insert(
                                "cta".to_string(),
                                serde_json::to_value(cta).unwrap_or(Value::Null),
                            );
                            let output = format_value(&Value::Object(out), format);
                            write_with_token_ops(&output, &builtin, writeln_stdout);
                        } else {
                            let output = format_value(&data, format);
                            write_with_token_ops(&output, &builtin, writeln_stdout);
                        }
                    } else {
                        let output = format_value(&data, format);
                        write_with_token_ops(&output, &builtin, writeln_stdout);
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
                let formatted_cta = merge_cta(
                    format_cta_block(&self.name, cta.as_ref()),
                    skills_cta.as_ref(),
                );

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
                        error_obj.insert(
                            "cta".to_string(),
                            serde_json::to_value(cta).unwrap_or(Value::Null),
                        );
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
            InternalResult::RecordStream(stream) => {
                if let Some(exit_code) = handle_record_stream(
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
                .await
                {
                    std::process::exit(exit_code);
                }
            }
        }

        Ok(())
    }

    /// Testable serve: writes output to the provided writer and returns exit code.
    ///
    /// Unlike [`Self::serve_with`], this method:
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
        self.serve_to_with_display_name(argv, writer, human, self.name.clone())
            .await
    }

    async fn serve_to_with_display_name(
        &self,
        argv: Vec<String>,
        writer: &mut dyn std::io::Write,
        human: bool,
        display_name: String,
    ) -> Result<Option<i32>, Box<dyn std::error::Error>> {
        self.run_to(argv, writer, Runtime::process(display_name, human))
            .await
    }

    /// Executes with fully injectable process inputs and captures the exit code.
    pub async fn run_to(
        &self,
        argv: Vec<String>,
        writer: &mut dyn std::io::Write,
        runtime: Runtime,
    ) -> Result<Option<i32>, Box<dyn std::error::Error>> {
        let Runtime {
            display_name,
            env,
            human,
        } = runtime;
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
        let mut builtin = match extract_builtin_flags(&argv, config_flag) {
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

        let globals = match crate::parser::parse_globals(
            &builtin.rest,
            &self.globals_fields,
            &self.global_aliases,
        ) {
            Ok(parsed) => {
                builtin.rest = parsed.rest;
                Value::Object(parsed.parsed.into_iter().collect())
            }
            Err(error) => {
                let msg = error.to_string();
                if human {
                    wln!(&format_human_error("VALIDATION_ERROR", &msg));
                } else {
                    wln!(&format!(
                        "{{\"ok\":false,\"error\":{{\"code\":\"VALIDATION_ERROR\",\"message\":\"{}\"}}}}",
                        msg.replace('"', "\\\"")
                    ));
                }
                return Ok(Some(1));
            }
        };

        // --- Step 2: Handle --version ---
        if builtin.version
            && !builtin.help
            && let Some(v) = &self.version
        {
            wln!(v);
            return Ok(None);
        }

        // --- Step 2b: Handle --llms / --llms-full ---
        if builtin.llms || builtin.llms_full {
            let commands_info =
                collect_all_command_info(self.root_command.as_ref(), &self.commands);
            if builtin.format_explicit && builtin.format != Format::Markdown {
                let manifest = if builtin.llms_full {
                    build_llms_manifest(&self.commands, &self.globals_fields, true)
                } else {
                    build_llms_manifest(&self.commands, &self.globals_fields, false)
                };
                wln!(&format_value(&manifest, builtin.format));
            } else if builtin.llms_full {
                let groups = collect_group_descriptions(&self.commands, &[]);
                let output = skill::generate(&self.name, &commands_info, &groups);
                wln!(&output);
            } else {
                let output = skill::index(&self.name, &commands_info, self.description.as_deref());
                wln!(&output);
            }
            return Ok(None);
        }

        // --- Step 2c: Handle --mcp ---
        if builtin.mcp {
            #[cfg(feature = "mcp")]
            {
                let version = self.version.as_deref().unwrap_or("0.0.0");
                crate::mcp::serve(
                    &self.name,
                    version,
                    &self.commands,
                    &self.middleware,
                    &self.env_fields,
                    &self.mcp_options,
                )
                .await?;
                return Ok(None);
            }
            #[cfg(not(feature = "mcp"))]
            {
                wln!("MCP support requires the 'mcp' feature flag.");
                return Ok(Some(1));
            }
        }

        if let Some(output) = completion_output(
            &self.name,
            &self.aliases,
            &self.commands,
            self.root_command.as_ref(),
            &self.globals_fields,
            &self.global_aliases,
            &argv,
            std::env::var("COMPLETE").ok().as_deref(),
            std::env::var("_COMPLETE_INDEX").ok().as_deref(),
        ) {
            wln!(&output);
            return Ok(None);
        }

        let builtins = builtin_commands(&self.name);

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "completions") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "completions")
                .expect("completions builtin must exist");
            let shell = builtin.rest.get(index + 1).map(|token| token.as_str());

            if builtin.help || shell.is_none() {
                wln!(&help::format_command(
                    &format!("{} completions", self.name),
                    &FormatCommandOptions {
                        aliases: None,
                        args_fields: builtin_def.args_fields.clone(),
                        config_flag: None,
                        commands: Vec::new(),
                        description: Some(builtin_def.description.to_string()),
                        env_fields: Vec::new(),
                        examples: Vec::new(),
                        global_aliases: HashMap::new(),
                        globals_fields: Vec::new(),
                        hint: builtin_def.hint.clone(),
                        hide_global_options: true,
                        options_fields: Vec::new(),
                        option_aliases: HashMap::new(),
                        root: false,
                        version: None,
                    },
                ));
                return Ok(None);
            }

            let shell = shell.expect("checked above");
            if crate::completions::Shell::from_str(shell).is_none() {
                wln!(&format_human_error(
                    "INVALID_SHELL",
                    &format!("Unknown shell '{shell}'. Supported: bash, fish, nushell, zsh"),
                ));
                return Ok(Some(1));
            }

            let output = std::iter::once(self.name.clone())
                .chain(self.aliases.iter().cloned())
                .map(|name| {
                    crate::completions::register(
                        crate::completions::Shell::from_str(shell).expect("checked above"),
                        &name,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            wln!(&output);
            return Ok(None);
        }

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "skills") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "skills")
                .expect("skills builtin must exist");

            let skills_sub_token = builtin.rest.get(index + 1).map(|token| token.as_str());
            // Resolve the subcommand, honoring declared aliases (e.g. `ls`→`list`).
            let skills_sub = skills_sub_token.map(|tok| {
                builtin_def
                    .subcommands
                    .iter()
                    .find(|s| s.name == tok || s.aliases.contains(&tok))
                    .map(|s| s.name)
                    .unwrap_or(tok)
            });

            // `skills list` / `skills ls`: list skills with installed status.
            if skills_sub == Some("list") {
                if builtin.help {
                    wln!(&format_builtin_subcommand_help(
                        &self.name,
                        builtin_def,
                        "list"
                    ));
                    return Ok(None);
                }
                let skills = crate::sync_skills::list(
                    &self.name,
                    &collect_all_command_info(self.root_command.as_ref(), &self.commands),
                    1,
                    self.description.as_deref(),
                );
                if skills.is_empty() {
                    wln!("No skills found.");
                    return Ok(None);
                }
                let max_len = skills.iter().map(|s| s.name.len()).max().unwrap_or(0);
                let mut lines: Vec<String> = Vec::new();
                for s in &skills {
                    let icon = if s.installed { "\u{2713}" } else { "\u{2717}" };
                    let padding = match &s.description {
                        Some(d) => format!("{}  {d}", " ".repeat(max_len - s.name.len())),
                        None => String::new(),
                    };
                    lines.push(format!("  {icon} {}{padding}", s.name));
                }
                let installed_count = skills.iter().filter(|s| s.installed).count();
                lines.push(String::new());
                lines.push(format!(
                    "{} skill{} ({installed_count} installed)",
                    skills.len(),
                    if skills.len() == 1 { "" } else { "s" },
                ));
                wln!(&lines.join("\n"));
                return Ok(None);
            }

            if skills_sub != Some("add") {
                wln!(&format_builtin_help(&self.name, builtin_def));
                return Ok(None);
            }

            if builtin.help {
                wln!(&format_builtin_subcommand_help(
                    &self.name,
                    builtin_def,
                    "add"
                ));
                return Ok(None);
            }

            let rest = builtin.rest[(index + 2)..].to_vec();
            let depth = if let Some(depth_index) = rest.iter().position(|token| token == "--depth")
            {
                rest.get(depth_index + 1)
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1)
            } else if let Some(token) = rest.iter().find(|token| token.starts_with("--depth=")) {
                token
                    .split_once('=')
                    .and_then(|(_, value)| value.parse::<usize>().ok())
                    .unwrap_or(1)
            } else {
                1
            };

            let result = crate::sync_skills::sync(
                &self.name,
                &collect_all_command_info(self.root_command.as_ref(), &self.commands),
                &crate::sync_skills::SyncOptions {
                    cwd: None,
                    depth: Some(depth),
                    description: self.description.clone(),
                    global: !rest.iter().any(|token| token == "--no-global"),
                    include: None,
                },
            )
            .await;

            match result {
                Ok(result) => {
                    let mut lines = vec![format!(
                        "Synced {} skill{}",
                        result.skills.len(),
                        if result.skills.len() == 1 { "" } else { "s" },
                    )];
                    for skill in &result.skills {
                        lines.push(format!("  {}", skill.name));
                    }
                    wln!(&lines.join("\n"));

                    if builtin.verbose || builtin.format_explicit {
                        let mut output = serde_json::Map::new();
                        output.insert(
                            "skills".to_string(),
                            Value::Array(
                                result
                                    .paths
                                    .iter()
                                    .map(|path| Value::String(path.to_string_lossy().to_string()))
                                    .collect(),
                            ),
                        );
                        if builtin.verbose {
                            output.insert(
                                "agents".to_string(),
                                Value::Array(
                                    result
                                        .agents
                                        .iter()
                                        .map(|agent| {
                                            serde_json::json!({
                                                "agent": agent.agent,
                                                "path": agent.path.to_string_lossy().to_string(),
                                                "mode": match agent.mode {
                                                    crate::agents::InstallMode::Symlink => "symlink",
                                                    crate::agents::InstallMode::Copy => "copy",
                                                },
                                            })
                                        })
                                        .collect(),
                                ),
                            );
                        }
                        wln!(&format_value(
                            &Value::Object(output),
                            if builtin.format_explicit {
                                builtin.format
                            } else {
                                Format::Toon
                            },
                        ));
                    }
                    return Ok(None);
                }
                Err(error) => {
                    wln!(&format_human_error(
                        "SYNC_SKILLS_FAILED",
                        &error.to_string()
                    ));
                    return Ok(Some(1));
                }
            }
        }

        if let Some(index) = builtin_command_index(&builtin.rest, &self.name, "mcp") {
            let builtin_def = builtins
                .iter()
                .find(|item| item.name == "mcp")
                .expect("mcp builtin must exist");

            let mcp_sub_token = builtin.rest.get(index + 1).map(|token| token.as_str());
            let mcp_sub = mcp_sub_token.map(|token| {
                builtin_def
                    .subcommands
                    .iter()
                    .find(|sub| sub.name == token || sub.aliases.contains(&token))
                    .map(|sub| sub.name)
                    .unwrap_or(token)
            });

            if mcp_sub.is_none() {
                wln!(&format_builtin_help(&self.name, builtin_def));
                return Ok(None);
            }

            if builtin.help {
                wln!(&format_builtin_subcommand_help(
                    &self.name,
                    builtin_def,
                    mcp_sub.expect("checked above")
                ));
                return Ok(None);
            }

            if mcp_sub == Some("doctor") {
                wln!(&format_value(
                    &mcp_doctor_result(&self.commands, &self.mcp_options.tools),
                    builtin.format,
                ));
                return Ok(None);
            }

            if mcp_sub != Some("add") {
                wln!(&format_builtin_help(&self.name, builtin_def));
                return Ok(None);
            }

            let rest = builtin.rest[(index + 2)..].to_vec();
            let mut command = None;
            let mut agents = Vec::new();
            let mut cursor = 0;

            while cursor < rest.len() {
                if (rest[cursor] == "--command" || rest[cursor] == "-c")
                    && let Some(value) = rest.get(cursor + 1)
                {
                    command = Some(value.clone());
                    cursor += 2;
                    continue;
                }
                if rest[cursor] == "--agent"
                    && let Some(value) = rest.get(cursor + 1)
                {
                    agents.push(value.clone());
                    cursor += 2;
                    continue;
                }
                cursor += 1;
            }

            let result = crate::sync_mcp::register(
                &self.name,
                &crate::sync_mcp::RegisterOptions {
                    agents: if agents.is_empty() {
                        None
                    } else {
                        Some(agents)
                    },
                    command,
                    global: !rest.iter().any(|token| token == "--no-global"),
                },
            )
            .await;

            match result {
                Ok(result) => {
                    let mut lines = vec![format!("Registered {} as MCP server", self.name)];
                    if !result.agents.is_empty() {
                        lines.push(format!("Agents: {}", result.agents.join(", ")));
                    }
                    wln!(&lines.join("\n"));

                    if builtin.verbose || builtin.format_explicit {
                        wln!(&format_value(
                            &serde_json::json!({
                                "name": self.name,
                                "command": result.command,
                                "agents": result.agents,
                            }),
                            if builtin.format_explicit {
                                builtin.format
                            } else {
                                Format::Toon
                            },
                        ));
                    }
                    return Ok(None);
                }
                Err(error) => {
                    wln!(&format_human_error("MCP_ADD_FAILED", &error.to_string()));
                    return Ok(Some(1));
                }
            }
        }

        if builtin.config_schema {
            if self.config.is_none() {
                wln!(&format_human_error(
                    "CONFIG_SCHEMA_UNAVAILABLE",
                    "--config-schema requires CLI config support.",
                ));
                return Ok(Some(1));
            }
            wln!(&format_config_schema(
                self.root_command.as_ref(),
                &self.commands
            )?);
            return Ok(None);
        }

        // --- Step 3: Handle --help at root level ---
        if builtin.rest.is_empty() {
            if let Some(root_cmd) = &self.root_command {
                if human && has_required_args(&root_cmd.args_fields) {
                    if let Some(banner) = self.render_banner(human).await {
                        wln!(&banner);
                    }
                    wln!(&format_command_help(
                        &self.name,
                        root_cmd,
                        &self.commands,
                        &self.aliases,
                        config_flag,
                        self.version.as_deref(),
                        &self.globals_fields,
                        &self.global_aliases,
                        true,
                    ));
                    return Ok(None);
                }
            } else if !builtin.help {
                if let Some(banner) = self.render_banner(human).await {
                    wln!(&banner);
                }
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
                        global_aliases: self.global_aliases.clone(),
                        globals_fields: self.globals_fields.clone(),
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
                ResolvedCommand::Leaf { command, path, .. } => {
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
                    if is_root && let Some(banner) = self.render_banner(human).await {
                        wln!(&banner);
                    }
                    wln!(&help::format_command(
                        &command_name,
                        &FormatCommandOptions {
                            aliases: if is_root && !self.aliases.is_empty() {
                                Some(self.aliases.clone())
                            } else if !is_root && !command.command_aliases.is_empty() {
                                Some(command.command_aliases.clone())
                            } else {
                                None
                            },
                            args_fields: command.args_fields.clone(),
                            config_flag: config_flag.map(|s| s.to_string()),
                            commands: help_cmds,
                            description: command.description.clone(),
                            env_fields: command.env_fields.clone(),
                            examples: command.examples.clone(),
                            global_aliases: self.global_aliases.clone(),
                            globals_fields: self.globals_fields.clone(),
                            hint: command.hint.clone(),
                            hide_global_options: false,
                            options_fields: command.options_fields.clone(),
                            option_aliases: command.aliases.clone(),
                            root: is_root,
                            version: if is_root { self.version.clone() } else { None },
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

                    if is_root
                        && let Some(root_cmd) = &self.root_command
                        && !commands.is_empty()
                    {
                        if let Some(banner) = self.render_banner(human).await {
                            wln!(&banner);
                        }
                        wln!(&format_command_help(
                            &self.name,
                            root_cmd,
                            commands,
                            &self.aliases,
                            config_flag,
                            self.version.as_deref(),
                            &self.globals_fields,
                            &self.global_aliases,
                            true,
                        ));
                    } else {
                        if is_root && let Some(banner) = self.render_banner(human).await {
                            wln!(&banner);
                        }
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
                                global_aliases: self.global_aliases.clone(),
                                globals_fields: self.globals_fields.clone(),
                                root: is_root,
                                version: if is_root { self.version.clone() } else { None },
                            },
                        ));
                    }
                }
                ResolvedCommand::Gateway { path, .. } => {
                    let help_name = format!("{} {path}", self.name);
                    wln!(&format!(
                        "{help_name}: fetch gateway (use curl-style arguments)"
                    ));
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
                            global_aliases: self.global_aliases.clone(),
                            globals_fields: self.globals_fields.clone(),
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
                    let schema = build_command_schema(command, &self.globals_fields);
                    let fmt = if builtin.format_explicit {
                        builtin.format
                    } else {
                        Format::Toon
                    };
                    wln!(&format_value(&schema, fmt));
                }
                _ => {
                    wln!("--schema requires a command.");
                    return Ok(Some(1));
                }
            }
            return Ok(None);
        }

        // --- Step 6b: Handle FetchGateway resolution ---
        if let ResolvedCommand::Gateway {
            handler,
            path,
            rest,
            base_path,
            output_policy,
        } = &resolved
        {
            let format = if builtin.format_explicit {
                builtin.format
            } else {
                self.format.unwrap_or(Format::Json)
            };

            let policy = output_policy.or(self.output_policy);
            let render_output =
                !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

            let mut fetch_input = match fetch::parse_argv_checked(rest) {
                Ok(input) => input,
                Err(error) => {
                    wln!(&format_human_error("VALIDATION_ERROR", &error.to_string()));
                    return Ok(Some(1));
                }
            };

            // Prepend base_path to the request path if configured.
            if let Some(bp) = base_path {
                let trimmed = bp.trim_end_matches('/');
                fetch_input.path = format!("{trimmed}{}", fetch_input.path);
            }

            let output = handler.handle(fetch_input).await;
            let data = format_fetch_output(&output);

            if builtin.verbose {
                let mut envelope = serde_json::Map::new();
                envelope.insert("ok".to_string(), Value::Bool(output.ok));
                envelope.insert("data".to_string(), data);
                let mut meta = serde_json::Map::new();
                meta.insert("command".to_string(), Value::String(path.clone()));
                meta.insert("status".to_string(), Value::Number(output.status.into()));
                envelope.insert("meta".to_string(), Value::Object(meta));
                wln!(&format_value(&Value::Object(envelope), format));
            } else if render_output {
                wln!(&format_value(&data, format));
            }

            if !output.ok {
                return Ok(Some(1));
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
            // Gateway is handled above in Step 6b; unreachable here.
            ResolvedCommand::Gateway { .. } => unreachable!("Gateway handled before step 7"),
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
                        global_aliases: self.global_aliases.clone(),
                        globals_fields: self.globals_fields.clone(),
                        root: path == self.name,
                        version: None,
                    },
                ));
                return Ok(None);
            }
            ResolvedCommand::Error { error, path } => {
                // Build the candidate set for did-you-mean. At the root level,
                // also consider builtin command names.
                let mut candidates = candidates_at_path(&self.commands, &path);
                if path.is_empty() {
                    for b in builtin_commands(&self.name) {
                        candidates.push(b.name.to_string());
                    }
                }
                let suggestion = suggest(&error, candidates.iter().map(|s| s.as_str()));

                // Root fallback is blocked when the unknown token looks like a
                // typo of a known command (TS rootFallbackBlocked guard).
                if path.is_empty()
                    && suggestion.is_none()
                    && let Some(root_cmd) = &self.root_command
                {
                    (
                        Arc::clone(root_cmd),
                        self.name.clone(),
                        builtin.rest.clone(),
                        Vec::new(),
                        None,
                    )
                } else {
                    let parent = if path.is_empty() {
                        self.name.clone()
                    } else {
                        format!("{} {path}", self.name)
                    };
                    let help_cmd = if path.is_empty() {
                        format!("{} --help", self.name)
                    } else {
                        format!("{parent} --help")
                    };
                    let did_you_mean = suggestion
                        .as_ref()
                        .map(|s| format!(" Did you mean '{s}'?"))
                        .unwrap_or_default();
                    let message =
                        format!("'{error}' is not a command for '{parent}'.{did_you_mean}");

                    // Build CTA commands: corrected command (if any), then help.
                    let mut cta_commands: Vec<Value> = Vec::new();
                    if let Some(s) = &suggestion {
                        let corrected: Vec<String> = builtin
                            .rest
                            .iter()
                            .map(|t| if t == &error { s.clone() } else { t.clone() })
                            .collect();
                        cta_commands.push(serde_json::json!({
                            "command": format!("{} {}", self.name, corrected.join(" "))
                        }));
                    }
                    cta_commands.push(serde_json::json!({
                        "command": help_cmd,
                        "description": "see all available commands"
                    }));
                    let cta_desc = if cta_commands.len() == 1 {
                        "Suggested command:"
                    } else {
                        "Suggested commands:"
                    };

                    if human {
                        wln!(&format_human_error("COMMAND_NOT_FOUND", &message));
                        let cta_block = FormattedCtaBlock {
                            description: cta_desc.to_string(),
                            commands: cta_commands
                                .iter()
                                .map(|c| FormattedCta {
                                    command: c["command"].as_str().unwrap_or("").to_string(),
                                    description: c
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .map(|s| s.to_string()),
                                })
                                .collect(),
                        };
                        wln!(&format_human_cta(&cta_block));
                    } else {
                        let cta_json = serde_json::json!({
                            "code": "COMMAND_NOT_FOUND",
                            "message": message,
                            "cta": {
                                "description": cta_desc,
                                "commands": cta_commands,
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
        let render_output =
            !(human && !builtin.format_explicit && policy == Some(OutputPolicy::AgentOnly));

        // --- Step 8: Load config defaults ---
        let defaults = if let Some(ref cfg) = self.config {
            if builtin.config_disabled {
                None
            } else {
                let config_path =
                    config::resolve_config_path(builtin.config_path.as_deref(), &cfg.files);
                if let Some(path) = config_path {
                    match config::load_config(&path) {
                        Ok(tree) => {
                            match config::extract_command_section(&tree, &self.name, &command_path)
                            {
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
            .chain(collected_mw)
            .chain(command.middleware.iter().cloned())
            .collect();

        // --- Step 10: Build env source ---
        let env_source = env;

        // --- Step 10b: Emit deprecation warnings (human/TTY mode only) ---
        if human {
            for warning in deprecation_warnings(&rest, &command.options_fields, &command.aliases) {
                wln!(&warning);
            }
        }

        // --- Step 11: Execute command ---
        let result = command::execute(
            Arc::clone(&command),
            ExecuteOptions {
                agent: !human,
                argv: rest,
                defaults,
                display_name,
                env_fields: self.env_fields.clone(),
                env_source,
                format,
                format_explicit: builtin.format_explicit,
                globals,
                input_options: BTreeMap::new(),
                middlewares: all_middleware,
                name: self.name.clone(),
                parse_mode: ParseMode::Argv,
                path: command_path.clone(),
                request: None,
                vars_fields: self.vars_fields.clone(),
                version: self.version.clone(),
            },
        )
        .await;

        let duration = start.elapsed();
        let duration_str = format!("{}ms", duration.as_millis());

        // Compute the "Skills are out of date" CTA (merged into output CTAs).
        let skills_cta = self.compute_skills_cta();

        // --- Step 12: Handle result ---
        match result {
            InternalResult::Ok { data, cta } => {
                // Apply --filter-output
                let data = if let Some(ref expr) = builtin.filter_output {
                    let paths = filter::parse(expr);
                    filter::apply(&data, &paths)
                } else {
                    data
                };

                let formatted_cta = merge_cta(
                    format_cta_block(&self.name, cta.as_ref()),
                    skills_cta.as_ref(),
                );

                // Macro to apply token ops before writing
                macro_rules! wln_tok {
                    ($s:expr) => {{
                        let s: &str = $s;
                        if let Some(token_output) = apply_token_ops(s, &builtin) {
                            wln!(&token_output);
                        } else {
                            wln!(s);
                        }
                    }};
                }

                if builtin.verbose {
                    let mut envelope = serde_json::Map::new();
                    envelope.insert("ok".to_string(), Value::Bool(true));
                    let mut meta = serde_json::Map::new();
                    meta.insert("command".to_string(), Value::String(command_path));
                    meta.insert("duration".to_string(), Value::String(duration_str));
                    if let Some(cta) = &formatted_cta {
                        meta.insert(
                            "cta".to_string(),
                            serde_json::to_value(cta).unwrap_or(Value::Null),
                        );
                    }
                    // Truncate `data` separately so meta (incl. nextOffset) stays visible.
                    #[cfg(feature = "tokens")]
                    let data = {
                        let data_formatted = format_value(&data, format);
                        if let Some((text, next_offset)) =
                            truncate_tokens(&data_formatted, &builtin)
                        {
                            if let Some(n) = next_offset {
                                meta.insert("nextOffset".to_string(), Value::from(n));
                            }
                            Value::String(text)
                        } else {
                            data
                        }
                    };
                    envelope.insert("data".to_string(), data);
                    envelope.insert("meta".to_string(), Value::Object(meta));
                    let output = format_value(&Value::Object(envelope), format);
                    wln!(&output);
                } else if human {
                    if render_output {
                        let output = format_value(&data, format);
                        wln_tok!(&output);
                    }
                    if let Some(cta) = &formatted_cta {
                        wln!(&format_human_cta(cta));
                    }
                } else {
                    let data = match (data, formatted_cta.as_ref()) {
                        (Value::Object(mut data), Some(cta)) => {
                            data.insert(
                                "cta".to_string(),
                                serde_json::to_value(cta).unwrap_or(Value::Null),
                            );
                            Value::Object(data)
                        }
                        (data, _) => data,
                    };
                    let output = format_value(&data, format);
                    wln_tok!(&output);
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
                let formatted_cta = merge_cta(
                    format_cta_block(&self.name, cta.as_ref()),
                    skills_cta.as_ref(),
                );

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
                        meta.insert(
                            "cta".to_string(),
                            serde_json::to_value(cta).unwrap_or(Value::Null),
                        );
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
                    if let Some(cta) = &formatted_cta {
                        error_obj.insert(
                            "cta".to_string(),
                            serde_json::to_value(cta).unwrap_or(Value::Null),
                        );
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
            InternalResult::RecordStream(mut stream) => {
                use futures::StreamExt;

                let use_jsonl = format == Format::Jsonl;
                let incremental = use_jsonl || (!builtin.format_explicit && format == Format::Toon);
                let mut chunks = Vec::new();
                let mut terminal = None;
                while let Some(record) = stream.next().await {
                    match record {
                        StreamRecord::Chunk(value) => {
                            if incremental {
                                if use_jsonl {
                                    let chunk =
                                        serde_json::json!({ "type": "chunk", "data": value });
                                    wln!(&serde_json::to_string(&chunk).unwrap_or_default());
                                } else if render_output {
                                    wln!(&format_value(&value, format));
                                }
                            } else {
                                chunks.push(value);
                            }
                        }
                        record => {
                            terminal = Some(record);
                            break;
                        }
                    }
                }

                let duration = format!("{}ms", start.elapsed().as_millis());
                match terminal {
                    Some(StreamRecord::Error {
                        code,
                        message,
                        retryable,
                        exit_code,
                        cta,
                    }) => {
                        let formatted_cta = format_cta_block(&self.name, cta.as_ref());
                        if use_jsonl {
                            let mut error = serde_json::json!({
                                "type": "error",
                                "ok": false,
                                "error": { "code": code, "message": message },
                            });
                            if retryable {
                                error["error"]["retryable"] = Value::Bool(true);
                            }
                            wln!(&serde_json::to_string(&error).unwrap_or_default());
                        } else if builtin.verbose || !incremental {
                            let mut error = serde_json::json!({
                                "ok": false,
                                "error": { "code": code, "message": message },
                                "meta": { "command": command_path, "duration": duration },
                            });
                            if retryable {
                                error["error"]["retryable"] = Value::Bool(true);
                            }
                            if let Some(cta) = formatted_cta {
                                error["meta"]["cta"] =
                                    serde_json::to_value(cta).unwrap_or(Value::Null);
                            }
                            wln!(&format_value(&error, format));
                        } else {
                            wln!(&format_human_error(&code, &message));
                        }
                        Ok(Some(exit_code.unwrap_or(1)))
                    }
                    terminal => {
                        let cta = match terminal {
                            Some(StreamRecord::Ok { cta }) => {
                                format_cta_block(&self.name, cta.as_ref())
                            }
                            _ => None,
                        };
                        if use_jsonl {
                            let mut done = serde_json::json!({
                                "type": "done", "ok": true,
                                "meta": { "command": command_path, "duration": duration },
                            });
                            if let Some(cta) = cta {
                                done["meta"]["cta"] =
                                    serde_json::to_value(cta).unwrap_or(Value::Null);
                            }
                            wln!(&serde_json::to_string(&done).unwrap_or_default());
                        } else if !incremental {
                            let data = Value::Array(chunks);
                            if builtin.verbose {
                                let envelope = serde_json::json!({
                                    "ok": true, "data": data,
                                    "meta": { "command": command_path, "duration": duration },
                                });
                                wln!(&format_value(&envelope, format));
                            } else if !human || render_output {
                                wln!(&format_value(&data, format));
                            }
                        }
                        Ok(None)
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command resolution
// ---------------------------------------------------------------------------

#[cfg(feature = "openapi")]
fn insert_generated_command(
    commands: &mut BTreeMap<String, CommandEntry>,
    path: &str,
    mut command: CommandDef,
) {
    let mut segments = path.split_whitespace();
    let Some(head) = segments.next() else { return };
    let tail = segments.collect::<Vec<_>>();
    if tail.is_empty() {
        command.name = head.to_string();
        commands.insert(head.to_string(), CommandEntry::Leaf(Arc::new(command)));
        return;
    }

    let entry = commands
        .entry(head.to_string())
        .or_insert_with(|| CommandEntry::Group {
            description: command.description.clone(),
            commands: BTreeMap::new(),
            middleware: Vec::new(),
            output_policy: None,
        });
    if let CommandEntry::Group { commands, .. } = entry {
        insert_generated_command(commands, &tail.join(" "), command);
    }
}

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
    /// A fetch gateway was found; remaining tokens are curl-style input.
    Gateway {
        handler: &'a Arc<dyn FetchHandler>,
        path: String,
        rest: Vec<String>,
        base_path: Option<String>,
        output_policy: Option<OutputPolicy>,
    },
    /// A group was reached but no further subcommand specified.
    Help {
        path: String,
        description: Option<String>,
        commands: &'a BTreeMap<String, CommandEntry>,
    },
    /// No matching command was found.
    Error { error: String, path: String },
}

/// Computes the Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp: Vec<usize> = (0..=n).collect();
    for i in 1..=m {
        let mut prev = dp[0];
        dp[0] = i;
        for j in 1..=n {
            let tmp = dp[j];
            dp[j] = if a[i - 1] == b[j - 1] {
                prev
            } else {
                1 + prev.min(dp[j]).min(dp[j - 1])
            };
            prev = tmp;
        }
    }
    dp[n]
}

/// Suggests the closest command name from a set using tiered matching:
/// prefix match → contains match → fuzzy (edit distance) match.
fn suggest<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let threshold = if input.len() <= 4 { 2 } else { input.len() / 2 };
    let lower = input.to_lowercase();

    let mut best: Option<String> = None;
    let mut best_score = usize::MAX;

    for c in candidates {
        let lc = c.to_lowercase();
        let dist = levenshtein(&lower, &lc);
        let score = if lc.starts_with(&lower) && lc != lower {
            dist
        } else if lc.contains(&lower) {
            100 + dist
        } else if dist <= threshold {
            200 + dist
        } else {
            continue;
        };
        if score < best_score {
            best_score = score;
            best = Some(c.to_string());
        }
    }
    best
}

/// Looks up an entry by name, falling back to a leaf command's declared
/// `command_aliases`. Returns the canonical name and the entry.
fn lookup_entry<'a>(
    commands: &'a BTreeMap<String, CommandEntry>,
    token: &str,
) -> Option<(&'a str, &'a CommandEntry)> {
    if let Some((name, entry)) = commands.get_key_value(token) {
        return Some((name.as_str(), entry));
    }
    for (name, entry) in commands {
        if let CommandEntry::Leaf(def) = entry
            && def.command_aliases.iter().any(|a| a == token)
        {
            return Some((name.as_str(), entry));
        }
    }
    None
}

/// Collects candidate command names (including leaf aliases) at the group
/// reached by following `path` (space-separated). An empty path returns the
/// top-level names.
fn candidates_at_path(commands: &BTreeMap<String, CommandEntry>, path: &str) -> Vec<String> {
    let mut current = commands;
    if !path.is_empty() {
        for segment in path.split(' ') {
            match current.get(segment) {
                Some(CommandEntry::Group {
                    commands: sub_commands,
                    ..
                }) => current = sub_commands,
                _ => return Vec::new(),
            }
        }
    }
    let mut names = Vec::new();
    for (name, entry) in current {
        names.push(name.clone());
        if let CommandEntry::Leaf(def) = entry {
            for alias in &def.command_aliases {
                names.push(alias.clone());
            }
        }
    }
    names
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
            };
        }
    };

    let (first_name, entry) = match lookup_entry(commands, first.as_str()) {
        Some(e) => e,
        None => {
            return ResolvedCommand::Error {
                error: first.clone(),
                path: String::new(),
            };
        }
    };

    let mut path = vec![first_name];
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
                        };
                    }
                };

                match lookup_entry(sub_commands, next.as_str()) {
                    Some((child_name, child)) => {
                        path.push(child_name);
                        remaining = &remaining[1..];
                        current = child;
                    }
                    None => {
                        return ResolvedCommand::Error {
                            error: next.clone(),
                            path: path.join(" "),
                        };
                    }
                }
            }
            CommandEntry::FetchGateway {
                base_path,
                output_policy,
                handler,
                ..
            } => {
                let effective_policy = output_policy.or(inherited_output_policy);
                return ResolvedCommand::Gateway {
                    handler,
                    path: path.join(" "),
                    rest: remaining.to_vec(),
                    base_path: base_path.clone(),
                    output_policy: effective_policy,
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
    filter_output: Option<String>,
    token_limit: Option<usize>,
    token_offset: Option<usize>,
    token_count: bool,
    llms: bool,
    llms_full: bool,
    mcp: bool,
    help: bool,
    version: bool,
    schema: bool,
    config_schema: bool,
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
    let mut config_schema = false;
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

        if token == "--full-output" {
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
        } else if token == "--config-schema" {
            config_schema = true;
        } else if token == "--json" {
            format = Format::Json;
            format_explicit = true;
        } else if token == "--format" {
            if let Some(next) = argv.get(i + 1) {
                if let Some(f) = Format::from_str_opt(next) {
                    format = f;
                } else {
                    return Err(format!(
                        "Invalid format: \"{next}\". Expected one of: toon, json, yaml, md, jsonl"
                    )
                    .into());
                }
                format_explicit = true;
                i += 1;
            } else {
                return Err("Missing value for flag: --format".into());
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
            } else {
                return Err("Missing value for flag: --filter-output".into());
            }
        } else if token == "--token-limit" {
            if let Some(next) = argv.get(i + 1) {
                token_limit = Some(next.parse().map_err(|_| {
                    format!("Invalid value for --token-limit: \"{next}\". Expected a number")
                })?);
                i += 1;
            } else {
                return Err("Missing value for flag: --token-limit".into());
            }
        } else if token == "--token-offset" {
            if let Some(next) = argv.get(i + 1) {
                token_offset = Some(next.parse().map_err(|_| {
                    format!("Invalid value for --token-offset: \"{next}\". Expected a number")
                })?);
                i += 1;
            } else {
                return Err("Missing value for flag: --token-offset".into());
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
        config_schema,
        config_path,
        config_disabled,
        rest,
    })
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

/// Formats a `FetchOutput` into a JSON `Value` for rendering.
fn format_fetch_output(output: &fetch::FetchOutput) -> Value {
    if output.ok {
        output.data.clone()
    } else {
        serde_json::json!({
            "ok": false,
            "status": output.status,
            "error": output.data,
        })
    }
}

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
fn format_value(value: &Value, fmt: Format) -> String {
    crate::formatter::format(value, fmt)
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

/// Merges an optional secondary CTA block (e.g. the skills-staleness CTA) into
/// a base CTA. When both exist, the secondary block's commands are appended to
/// the base; otherwise whichever exists is returned.
fn merge_cta(
    base: Option<FormattedCtaBlock>,
    extra: Option<&FormattedCtaBlock>,
) -> Option<FormattedCtaBlock> {
    match (base, extra) {
        (Some(mut b), Some(e)) => {
            b.commands.extend(e.commands.iter().cloned());
            Some(b)
        }
        (Some(b), None) => Some(b),
        (None, Some(e)) => Some(e.clone()),
        (None, None) => None,
    }
}

/// Formats a CTA block for human-readable TTY output.
fn format_human_cta(cta: &FormattedCtaBlock) -> String {
    let mut lines = vec![String::new(), cta.description.clone()];
    let max_len = cta
        .commands
        .iter()
        .map(|c| c.command.len())
        .max()
        .unwrap_or(0);
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

/// Recursively collects all leaf commands as `CommandInfo` for skill generation.
fn collect_command_info(
    commands: &BTreeMap<String, CommandEntry>,
    prefix: &[&str],
) -> Vec<skill::CommandInfo> {
    let mut result = Vec::new();
    for (name, entry) in commands {
        let mut path_parts: Vec<&str> = prefix.to_vec();
        path_parts.push(name);
        match entry {
            CommandEntry::Leaf(def) => {
                result.push(skill_command_info(path_parts.join(" "), def));
            }
            CommandEntry::Group { commands: sub, .. } => {
                result.extend(collect_command_info(sub, &path_parts));
            }
            CommandEntry::FetchGateway { .. } => {}
        }
    }
    result
}

fn collect_all_command_info(
    root: Option<&Arc<CommandDef>>,
    commands: &BTreeMap<String, CommandEntry>,
) -> Vec<skill::CommandInfo> {
    let mut result = root
        .into_iter()
        .map(|command| skill_command_info(String::new(), command))
        .collect::<Vec<_>>();
    result.extend(collect_command_info(commands, &[]));
    result
}

fn skill_command_info(name: String, command: &CommandDef) -> skill::CommandInfo {
    let mcp = command.handler.mcp_options().cloned().unwrap_or_default();
    let destructive = mcp.destructive
        || mcp
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.destructive_hint)
            == Some(true);
    let hint = if destructive {
        const CONFIRM: &str = "Confirm with the user before executing this destructive command.";
        match &command.hint {
            Some(hint) if hint.contains(CONFIRM) => Some(hint.clone()),
            Some(hint) => Some(format!("{hint} {CONFIRM}")),
            None => Some(CONFIRM.to_string()),
        }
    } else {
        command.hint.clone()
    };
    skill::CommandInfo {
        name,
        description: command.description.clone(),
        args_fields: command.args_fields.clone(),
        options_fields: command.options_fields.clone(),
        env_fields: command.env_fields.clone(),
        hint,
        examples: command
            .examples
            .iter()
            .map(|example| skill::Example {
                command: example.command.clone(),
                description: example.description.clone(),
            })
            .collect(),
        output_schema: command.output_schema.clone(),
    }
}

/// Collects group descriptions for skill file generation.
fn collect_group_descriptions(
    commands: &BTreeMap<String, CommandEntry>,
    prefix: &[&str],
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for (name, entry) in commands {
        if let CommandEntry::Group {
            description,
            commands: sub,
            ..
        } = entry
        {
            let mut path_parts: Vec<&str> = prefix.to_vec();
            path_parts.push(name);
            let key = path_parts.join(" ");
            if let Some(desc) = description {
                result.insert(key.clone(), desc.clone());
            }
            result.extend(collect_group_descriptions(sub, &path_parts));
        }
    }
    result
}

fn build_llms_manifest(
    commands: &BTreeMap<String, CommandEntry>,
    globals_fields: &[FieldMeta],
    full: bool,
) -> Value {
    fn collect(
        commands: &BTreeMap<String, CommandEntry>,
        prefix: &[String],
        full: bool,
        result: &mut Vec<Value>,
    ) {
        for (name, entry) in commands {
            let mut path = prefix.to_vec();
            path.push(name.clone());
            match entry {
                CommandEntry::Leaf(command) => {
                    let mut value = serde_json::Map::new();
                    value.insert("name".to_string(), Value::String(path.join(" ")));
                    if let Some(description) = &command.description {
                        value.insert(
                            "description".to_string(),
                            Value::String(description.clone()),
                        );
                    }
                    if full {
                        let schema = build_command_schema(command, &[]);
                        if schema.as_object().is_some_and(|schema| !schema.is_empty()) {
                            value.insert("schema".to_string(), schema);
                        }
                        if !command.examples.is_empty() {
                            value.insert(
                                "examples".to_string(),
                                serde_json::to_value(
                                    command
                                        .examples
                                        .iter()
                                        .map(|example| {
                                            serde_json::json!({
                                                "command": example.command,
                                                "description": example.description,
                                            })
                                        })
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap_or(Value::Null),
                            );
                        }
                    }
                    result.push(Value::Object(value));
                }
                CommandEntry::Group { commands, .. } => collect(commands, &path, full, result),
                CommandEntry::FetchGateway { .. } => {}
            }
        }
    }

    let mut command_values = Vec::new();
    collect(commands, &[], full, &mut command_values);
    command_values.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut manifest = serde_json::Map::from_iter([
        ("version".to_string(), Value::String("incur.v1".to_string())),
        ("commands".to_string(), Value::Array(command_values)),
    ]);
    if !globals_fields.is_empty() {
        manifest.insert(
            "globals".to_string(),
            crate::schema::to_json_schema(globals_fields),
        );
    }
    Value::Object(manifest)
}

/// Builds a `--schema` JSON object for a command from its FieldMeta, with
/// `args`, `env`, `options`, and `output` sections (each present only when the
/// command declares them). Ported from `Cli.ts` `--schema` handling.
fn build_command_schema(command: &CommandDef, globals_fields: &[FieldMeta]) -> Value {
    let mut result = serde_json::Map::new();
    if !globals_fields.is_empty() {
        result.insert(
            "globals".to_string(),
            crate::schema::to_json_schema(globals_fields),
        );
    }
    if !command.args_fields.is_empty() {
        result.insert(
            "args".to_string(),
            crate::schema::to_json_schema(&command.args_fields),
        );
    }
    if !command.env_fields.is_empty() {
        result.insert(
            "env".to_string(),
            crate::schema::to_json_schema(&command.env_fields),
        );
    }
    if !command.options_fields.is_empty() {
        result.insert(
            "options".to_string(),
            crate::schema::to_json_schema(&command.options_fields),
        );
    }
    if let Some(output) = &command.output_schema {
        result.insert("output".to_string(), output.clone());
    }
    Value::Object(result)
}

/// Scans command tokens for supplied deprecated options and returns the
/// `Warning: --<cli-name> is deprecated` lines. Recognizes long flags, their
/// `--no-` form, and short aliases. Ported from `Cli.ts` `emitDeprecationWarnings`.
fn deprecation_warnings(
    rest: &[String],
    options_fields: &[FieldMeta],
    aliases: &HashMap<String, char>,
) -> Vec<String> {
    use std::collections::HashSet;
    let mut deprecated_flags: HashSet<&str> = HashSet::new();
    let mut deprecated_shorts: HashMap<char, &str> = HashMap::new();
    for field in options_fields {
        if field.deprecated {
            deprecated_flags.insert(field.cli_name.as_str());
            if let Some(&ch) = aliases.get(field.name) {
                deprecated_shorts.insert(ch, field.cli_name.as_str());
            }
        }
    }
    if deprecated_flags.is_empty() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    for token in rest {
        if let Some(stripped) = token.strip_prefix("--") {
            let stripped = stripped.split('=').next().unwrap_or("");
            let raw = if !deprecated_flags.contains(stripped) {
                stripped.strip_prefix("no-").unwrap_or(stripped)
            } else {
                stripped
            };
            if deprecated_flags.contains(raw) {
                warnings.push(format!("Warning: --{raw} is deprecated"));
            }
        } else if let Some(shorts) = token.strip_prefix('-')
            && !token.is_empty()
        {
            for ch in shorts.chars() {
                if let Some(name) = deprecated_shorts.get(&ch) {
                    warnings.push(format!("Warning: --{name} is deprecated"));
                }
            }
        }
    }
    warnings
}

/// Checks if any arg fields are required.
fn has_required_args(args_fields: &[FieldMeta]) -> bool {
    args_fields.iter().any(|f| f.required)
}

/// Formats command help including subcommands.
#[allow(clippy::too_many_arguments)]
fn format_command_help(
    name: &str,
    command: &CommandDef,
    commands: &BTreeMap<String, CommandEntry>,
    aliases: &[String],
    config_flag: Option<&str>,
    version: Option<&str>,
    globals_fields: &[FieldMeta],
    global_aliases: &HashMap<String, char>,
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
            global_aliases: global_aliases.clone(),
            globals_fields: globals_fields.to_vec(),
            hint: command.hint.clone(),
            hide_global_options: false,
            options_fields: command.options_fields.clone(),
            option_aliases: command.aliases.clone(),
            root,
            version: version.map(|s| s.to_string()),
        },
    )
}

struct BuiltinCommand {
    name: &'static str,
    description: &'static str,
    args_fields: Vec<FieldMeta>,
    hint: Option<String>,
    subcommands: Vec<BuiltinSubcommand>,
}

struct BuiltinSubcommand {
    name: &'static str,
    description: &'static str,
    options_fields: Vec<FieldMeta>,
    option_aliases: HashMap<String, char>,
    aliases: Vec<&'static str>,
}

fn builtin_commands(cli_name: &str) -> Vec<BuiltinCommand> {
    let completions_rows = [
        (
            "bash",
            format!("eval \"$({cli_name} completions bash)\""),
            "# add to ~/.bashrc".to_string(),
        ),
        (
            "fish",
            format!("{cli_name} completions fish | source"),
            "# add to ~/.config/fish/config.fish".to_string(),
        ),
        (
            "nushell",
            format!("see `{cli_name} completions nushell`"),
            "# add to config.nu".to_string(),
        ),
        (
            "zsh",
            format!("eval \"$({cli_name} completions zsh)\""),
            "# add to ~/.zshrc".to_string(),
        ),
    ];
    let shell_w = completions_rows
        .iter()
        .map(|(shell, _, _)| shell.len())
        .max()
        .unwrap_or(0);
    let cmd_w = completions_rows
        .iter()
        .map(|(_, cmd, _)| cmd.len())
        .max()
        .unwrap_or(0);

    vec![
        BuiltinCommand {
            name: "completions",
            description: "Generate shell completion script",
            args_fields: vec![FieldMeta {
                name: "shell",
                cli_name: "shell".to_string(),
                description: Some("Shell to generate completions for"),
                field_type: crate::schema::FieldType::Enum(
                    ["bash", "fish", "nushell", "zsh"]
                        .into_iter()
                        .map(|value| value.to_string())
                        .collect(),
                ),
                required: true,
                default: None,
                alias: None,
                deprecated: false,
                env_name: None,
            }],
            hint: Some(format!(
                "Setup:\n{}",
                completions_rows
                    .iter()
                    .map(|(shell, cmd, comment)| {
                        format!("  {:<shell_w$}  {:<cmd_w$}  {}", shell, cmd, comment)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            subcommands: Vec::new(),
        },
        BuiltinCommand {
            name: "mcp",
            description: "Register as MCP server",
            args_fields: Vec::new(),
            hint: None,
            subcommands: vec![
                BuiltinSubcommand {
                    name: "add",
                    description: "Register as MCP server",
                    options_fields: vec![
                        FieldMeta {
                            name: "agent",
                            cli_name: "agent".to_string(),
                            description: Some("Target a specific agent (e.g. claude-code, cursor)"),
                            field_type: crate::schema::FieldType::String,
                            required: false,
                            default: None,
                            alias: None,
                            deprecated: false,
                            env_name: None,
                        },
                        FieldMeta {
                            name: "command",
                            cli_name: "command".to_string(),
                            description: Some(
                                "Override the command agents will run (e.g. \"my-cli --mcp\")",
                            ),
                            field_type: crate::schema::FieldType::String,
                            required: false,
                            default: None,
                            alias: Some('c'),
                            deprecated: false,
                            env_name: None,
                        },
                        FieldMeta {
                            name: "no_global",
                            cli_name: "no-global".to_string(),
                            description: Some("Install to project instead of globally"),
                            field_type: crate::schema::FieldType::Boolean,
                            required: false,
                            default: Some(Value::Bool(false)),
                            alias: None,
                            deprecated: false,
                            env_name: None,
                        },
                    ],
                    option_aliases: HashMap::from([(String::from("command"), 'c')]),
                    aliases: Vec::new(),
                },
                BuiltinSubcommand {
                    name: "doctor",
                    description: "Validate MCP server startup and tool listing",
                    options_fields: Vec::new(),
                    option_aliases: HashMap::new(),
                    aliases: Vec::new(),
                },
            ],
        },
        BuiltinCommand {
            name: "skills",
            description: "Sync skill files to agents",
            args_fields: Vec::new(),
            hint: None,
            subcommands: vec![
                BuiltinSubcommand {
                    name: "add",
                    description: "Sync skill files to agents",
                    options_fields: vec![
                        FieldMeta {
                            name: "depth",
                            cli_name: "depth".to_string(),
                            description: Some("Grouping depth for skill files (default: 1)"),
                            field_type: crate::schema::FieldType::Number,
                            required: false,
                            default: Some(Value::Number(serde_json::Number::from(1))),
                            alias: None,
                            deprecated: false,
                            env_name: None,
                        },
                        FieldMeta {
                            name: "no_global",
                            cli_name: "no-global".to_string(),
                            description: Some("Install to project instead of globally"),
                            field_type: crate::schema::FieldType::Boolean,
                            required: false,
                            default: Some(Value::Bool(false)),
                            alias: None,
                            deprecated: false,
                            env_name: None,
                        },
                    ],
                    option_aliases: HashMap::new(),
                    aliases: Vec::new(),
                },
                BuiltinSubcommand {
                    name: "list",
                    description: "List skills and whether they are installed",
                    options_fields: Vec::new(),
                    option_aliases: HashMap::new(),
                    aliases: vec!["ls"],
                },
            ],
        },
    ]
}

fn format_builtin_help(cli_name: &str, builtin: &BuiltinCommand) -> String {
    help::format_root(
        &format!("{cli_name} {}", builtin.name),
        &FormatRootOptions {
            aliases: None,
            config_flag: None,
            commands: builtin
                .subcommands
                .iter()
                .map(|sub| CommandSummary {
                    name: sub.name.to_string(),
                    description: Some(sub.description.to_string()),
                })
                .collect(),
            description: Some(builtin.description.to_string()),
            global_aliases: HashMap::new(),
            globals_fields: Vec::new(),
            root: false,
            version: None,
        },
    )
}

fn format_builtin_subcommand_help(
    cli_name: &str,
    builtin: &BuiltinCommand,
    sub_name: &str,
) -> String {
    let sub = builtin.subcommands.iter().find(|sub| sub.name == sub_name);

    help::format_command(
        &format!("{cli_name} {} {sub_name}", builtin.name),
        &FormatCommandOptions {
            aliases: None,
            args_fields: Vec::new(),
            config_flag: None,
            commands: Vec::new(),
            description: sub.map(|item| item.description.to_string()),
            env_fields: Vec::new(),
            examples: Vec::new(),
            global_aliases: HashMap::new(),
            globals_fields: Vec::new(),
            hint: None,
            hide_global_options: true,
            options_fields: sub
                .map(|item| item.options_fields.clone())
                .unwrap_or_default(),
            option_aliases: sub
                .map(|item| item.option_aliases.clone())
                .unwrap_or_default(),
            root: false,
            version: None,
        },
    )
}

fn builtin_command_index(tokens: &[String], cli_name: &str, builtin_name: &str) -> Option<usize> {
    if tokens.first().map(|token| token.as_str()) == Some(builtin_name) {
        return Some(0);
    }
    if tokens.first().map(|token| token.as_str()) == Some(cli_name)
        && tokens.get(1).map(|token| token.as_str()) == Some(builtin_name)
    {
        return Some(1);
    }
    None
}

fn convert_to_completion_commands(
    commands: &BTreeMap<String, CommandEntry>,
    globals_fields: &[FieldMeta],
    global_aliases: &HashMap<String, char>,
) -> BTreeMap<String, crate::completions::CommandEntry> {
    let mut result = BTreeMap::new();

    for (name, entry) in commands {
        match entry {
            CommandEntry::Leaf(def) => {
                result.insert(
                    name.clone(),
                    crate::completions::CommandEntry {
                        is_group: false,
                        description: def.description.clone(),
                        commands: BTreeMap::new(),
                        options_fields: def
                            .options_fields
                            .iter()
                            .chain(globals_fields)
                            .cloned()
                            .collect(),
                        aliases: def
                            .aliases
                            .iter()
                            .chain(global_aliases)
                            .map(|(key, value)| (key.clone(), *value))
                            .collect(),
                    },
                );
            }
            CommandEntry::Group {
                description,
                commands: sub_commands,
                ..
            } => {
                result.insert(
                    name.clone(),
                    crate::completions::CommandEntry {
                        is_group: true,
                        description: description.clone(),
                        commands: convert_to_completion_commands(
                            sub_commands,
                            globals_fields,
                            global_aliases,
                        ),
                        options_fields: globals_fields.to_vec(),
                        aliases: global_aliases
                            .iter()
                            .map(|(key, value)| (key.clone(), *value))
                            .collect(),
                    },
                );
            }
            CommandEntry::FetchGateway { .. } => {}
        }
    }

    result
}

fn completion_root_command(
    root_command: Option<&Arc<CommandDef>>,
    globals_fields: &[FieldMeta],
    global_aliases: &HashMap<String, char>,
) -> Option<crate::completions::CommandDef> {
    if root_command.is_none() && globals_fields.is_empty() {
        return None;
    }
    Some(crate::completions::CommandDef {
        options_fields: root_command
            .into_iter()
            .flat_map(|command| command.options_fields.iter())
            .chain(globals_fields)
            .cloned()
            .collect(),
        aliases: root_command
            .into_iter()
            .flat_map(|command| command.aliases.iter())
            .chain(global_aliases)
            .map(|(key, value)| (key.clone(), *value))
            .collect(),
    })
}

#[allow(clippy::too_many_arguments)]
fn completion_output(
    cli_name: &str,
    aliases: &[String],
    commands: &BTreeMap<String, CommandEntry>,
    root_command: Option<&Arc<CommandDef>>,
    globals_fields: &[FieldMeta],
    global_aliases: &HashMap<String, char>,
    argv: &[String],
    complete_shell: Option<&str>,
    complete_index: Option<&str>,
) -> Option<String> {
    let shell = crate::completions::Shell::from_str(complete_shell?)?;
    let separator = argv.iter().position(|token| token == "--");
    let words = separator
        .map(|index| argv[(index + 1)..].to_vec())
        .unwrap_or_else(|| argv.to_vec());

    if words.is_empty() {
        let names = std::iter::once(cli_name.to_string())
            .chain(aliases.iter().cloned())
            .collect::<Vec<_>>();
        return Some(
            names
                .iter()
                .map(|name| crate::completions::register(shell, name))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    let index = complete_index
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| words.len().saturating_sub(1));
    let commands = convert_to_completion_commands(commands, globals_fields, global_aliases);
    let root = completion_root_command(root_command, globals_fields, global_aliases);
    let mut candidates = crate::completions::complete(&commands, root.as_ref(), &words, index);
    let builtins = builtin_commands(cli_name);
    let current = words.get(index).map(|word| word.as_str()).unwrap_or("");
    let mut non_flags = words
        .iter()
        .take(index)
        .filter(|word| !word.starts_with('-'))
        .map(|word| word.to_string())
        .collect::<Vec<_>>();

    if let Some(first) = non_flags.first()
        && (first == cli_name || aliases.iter().any(|alias| alias == first))
    {
        non_flags.remove(0);
    }

    if non_flags.is_empty() {
        for builtin in &builtins {
            if builtin.name.starts_with(current)
                && !candidates
                    .iter()
                    .any(|candidate| candidate.value == builtin.name)
            {
                candidates.push(crate::completions::Candidate {
                    value: builtin.name.to_string(),
                    description: Some(builtin.description.to_string()),
                    no_space: !builtin.subcommands.is_empty(),
                });
            }
        }
    } else if non_flags.len() == 1
        && let Some(parent) = non_flags.last()
        && let Some(builtin) = builtins.iter().find(|builtin| builtin.name == parent)
    {
        for subcommand in &builtin.subcommands {
            if subcommand.name.starts_with(current) {
                candidates.push(crate::completions::Candidate {
                    value: subcommand.name.to_string(),
                    description: Some(subcommand.description.to_string()),
                    no_space: false,
                });
            }
        }
    }

    Some(crate::completions::format(shell, &candidates))
}

fn format_config_schema(
    root_command: Option<&Arc<CommandDef>>,
    commands: &BTreeMap<String, CommandEntry>,
) -> Result<String, Box<dyn std::error::Error>> {
    let root_options = root_command
        .map(|command| command.options_fields.as_slice())
        .unwrap_or(&[]);
    let schema = crate::config_schema::from_command_tree(commands, root_options);
    Ok(serde_json::to_string_pretty(&schema)?)
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

async fn handle_record_stream(
    mut stream: std::pin::Pin<Box<dyn futures::Stream<Item = StreamRecord> + Send>>,
    opts: StreamingOptions<'_>,
) -> Option<i32> {
    use futures::StreamExt;

    let mut chunks = Vec::new();
    let mut terminal = None;
    let use_jsonl = opts.format == Format::Jsonl;
    let incremental = use_jsonl || (!opts.format_explicit && opts.format == Format::Toon);
    while let Some(record) = stream.next().await {
        match record {
            StreamRecord::Chunk(value) => {
                if incremental {
                    if use_jsonl {
                        let chunk = serde_json::json!({ "type": "chunk", "data": value });
                        writeln_stdout(&serde_json::to_string(&chunk).unwrap_or_default());
                    } else if opts.render_output {
                        writeln_stdout(&format_value(&value, opts.format));
                    }
                } else {
                    chunks.push(value);
                }
            }
            record => {
                terminal = Some(record);
                break;
            }
        }
    }

    let duration = format!("{}ms", opts.start.elapsed().as_millis());
    match terminal {
        Some(StreamRecord::Error {
            code,
            message,
            retryable,
            exit_code,
            ..
        }) => {
            if use_jsonl {
                let mut error = serde_json::json!({
                    "type": "error", "ok": false,
                    "error": { "code": code, "message": message },
                });
                if retryable {
                    error["error"]["retryable"] = Value::Bool(true);
                }
                writeln_stdout(&serde_json::to_string(&error).unwrap_or_default());
            } else if opts.verbose || !incremental {
                let mut error = serde_json::json!({
                    "ok": false,
                    "error": { "code": code, "message": message },
                    "meta": { "command": opts.path, "duration": duration },
                });
                if retryable {
                    error["error"]["retryable"] = Value::Bool(true);
                }
                writeln_stdout(&format_value(&error, opts.format));
            } else {
                writeln_stdout(&format_human_error(&code, &message));
            }
            Some(exit_code.unwrap_or(1))
        }
        _ => {
            if use_jsonl {
                let done = serde_json::json!({
                    "type": "done", "ok": true,
                    "meta": { "command": opts.path, "duration": duration },
                });
                writeln_stdout(&serde_json::to_string(&done).unwrap_or_default());
            } else if !incremental {
                let data = Value::Array(chunks);
                if opts.verbose {
                    let envelope = serde_json::json!({
                        "ok": true, "data": data,
                        "meta": { "command": opts.path, "duration": duration },
                    });
                    writeln_stdout(&format_value(&envelope, opts.format));
                } else if !opts.human || opts.render_output {
                    writeln_stdout(&format_value(&data, opts.format));
                }
            }
            None
        }
    }
}

fn mcp_doctor_result(
    commands: &BTreeMap<String, CommandEntry>,
    filter: &crate::mcp::McpToolFilter,
) -> Value {
    fn collect(
        commands: &BTreeMap<String, CommandEntry>,
        prefix: &[String],
        filter: &crate::mcp::McpToolFilter,
        tools: &mut Vec<Value>,
    ) {
        for (name, entry) in commands {
            let mut path = prefix.to_vec();
            path.push(name.clone());
            match entry {
                CommandEntry::Leaf(command)
                    if command
                        .handler
                        .mcp_options()
                        .is_none_or(|options| options.enabled) =>
                {
                    let mcp = command.handler.mcp_options().cloned().unwrap_or_default();
                    let name = mcp.name.clone().unwrap_or_else(|| path.join("_"));
                    if !crate::mcp::matches_tool_filter(&name, filter) {
                        continue;
                    }
                    let mut value =
                        serde_json::Map::from_iter([("name".to_string(), Value::String(name))]);
                    if let Some(description) =
                        mcp.description.as_ref().or(command.description.as_ref())
                    {
                        value.insert(
                            "description".to_string(),
                            Value::String(description.clone()),
                        );
                    }
                    tools.push(Value::Object(value));
                }
                CommandEntry::Group { commands, .. } => collect(commands, &path, filter, tools),
                CommandEntry::Leaf(_) | CommandEntry::FetchGateway { .. } => {}
            }
        }
    }

    let mut tools = Vec::new();
    collect(commands, &[], filter, &mut tools);
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let warnings = if tools.is_empty() {
        vec![Value::String("No MCP tools exposed.".to_string())]
    } else {
        Vec::new()
    };
    serde_json::json!({
        "ok": true,
        "toolCount": tools.len(),
        "tools": tools,
        "warnings": warnings,
        "errors": [],
    })
}

// ---------------------------------------------------------------------------
// Token operations
// ---------------------------------------------------------------------------

/// Applies token operations (count, offset, limit) to a formatted output string.
fn apply_token_ops(output: &str, builtin: &BuiltinFlags) -> Option<String> {
    if builtin.token_count {
        #[cfg(feature = "tokens")]
        {
            let count = tiktoken_rs::cl100k_base()
                .ok()
                .map(|bpe| bpe.encode_with_special_tokens(output).len())
                .unwrap_or(0);
            return Some(count.to_string());
        }
        #[cfg(not(feature = "tokens"))]
        return Some(output.split_whitespace().count().to_string());
    }
    if builtin.token_offset.is_some() || builtin.token_limit.is_some() {
        #[cfg(feature = "tokens")]
        {
            if let Ok(bpe) = tiktoken_rs::cl100k_base() {
                let tokens = bpe.encode_with_special_tokens(output);
                let total = tokens.len();
                let offset = builtin.token_offset.unwrap_or(0);
                let end = match builtin.token_limit {
                    Some(limit) => offset + limit,
                    None => total,
                };
                // No truncation needed when the full output fits in the window.
                if offset == 0 && end >= total {
                    return None;
                }
                let start = offset.min(total);
                let actual_end = end.min(total);
                let limited = &tokens[start..actual_end];
                let sliced = bpe.decode(limited.to_vec()).unwrap_or_default();
                return Some(format!(
                    "{sliced}\n[truncated: showing tokens {offset}\u{2013}{actual_end} of {total}]"
                ));
            }
        }
    }
    None
}

/// Truncates a formatted string by token window, returning the truncated text
/// (with the `[truncated: …]` marker) and the next offset for pagination, or
/// `None` when no truncation is needed. Used by the full-output envelope path
/// so that `meta.nextOffset` can be surfaced. Ported from `Cli.ts` `truncate`.
#[cfg(feature = "tokens")]
fn truncate_tokens(output: &str, builtin: &BuiltinFlags) -> Option<(String, Option<usize>)> {
    if builtin.token_offset.is_none() && builtin.token_limit.is_none() {
        return None;
    }
    let bpe = tiktoken_rs::cl100k_base().ok()?;
    let tokens = bpe.encode_with_special_tokens(output);
    let total = tokens.len();
    let offset = builtin.token_offset.unwrap_or(0);
    let end = match builtin.token_limit {
        Some(limit) => offset + limit,
        None => total,
    };
    if offset == 0 && end >= total {
        return None;
    }
    let start = offset.min(total);
    let actual_end = end.min(total);
    let sliced = bpe
        .decode(tokens[start..actual_end].to_vec())
        .unwrap_or_default();
    let next_offset = if actual_end < total {
        Some(actual_end)
    } else {
        None
    };
    Some((
        format!("{sliced}\n[truncated: showing tokens {offset}\u{2013}{actual_end} of {total}]"),
        next_offset,
    ))
}

/// Writes output to stdout, applying token operations if any are set.
fn write_with_token_ops(output: &str, builtin: &BuiltinFlags, write_fn: fn(&str)) {
    if let Some(token_output) = apply_token_ops(output, builtin) {
        write_fn(&token_output);
    } else {
        write_fn(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_builtin_flags_basic() {
        let argv: Vec<String> = vec![
            "--full-output".to_string(),
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
                command_aliases: Vec::new(),
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
                command_aliases: Vec::new(),
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
                command_aliases: Vec::new(),
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

    struct CaptureGlobalsHandler(Arc<tokio::sync::Mutex<Option<Value>>>);

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for CaptureGlobalsHandler {
        async fn run(&self, ctx: crate::command::CommandContext) -> CommandResult {
            *self.0.lock().await = Some(ctx.globals);
            CommandResult::Ok {
                data: Value::Null,
                cta: None,
            }
        }
    }

    fn global_fields() -> Vec<FieldMeta> {
        vec![
            FieldMeta {
                name: "profile",
                cli_name: "profile".to_string(),
                description: Some("Profile to use"),
                field_type: crate::schema::FieldType::String,
                required: false,
                default: Some(Value::String("default".to_string())),
                alias: Some('p'),
                deprecated: false,
                env_name: None,
            },
            FieldMeta {
                name: "trace",
                cli_name: "trace".to_string(),
                description: Some("Enable tracing"),
                field_type: crate::schema::FieldType::Boolean,
                required: false,
                default: Some(Value::Bool(false)),
                alias: None,
                deprecated: false,
                env_name: None,
            },
        ]
    }

    #[tokio::test]
    async fn test_serve_to_passes_globals_to_handler_at_any_position() {
        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let mut command = make_leaf_command("ping", Some("Ping the server"));
        command.handler = Box::new(CaptureGlobalsHandler(Arc::clone(&captured)));
        let cli = Cli::create("test")
            .globals_fields(global_fields())
            .command("ping", command);
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec![
                    "--profile".to_string(),
                    "work".to_string(),
                    "ping".to_string(),
                    "--trace".to_string(),
                ],
                &mut output,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        assert_eq!(
            captured.lock().await.clone(),
            Some(serde_json::json!({ "profile": "work", "trace": true }))
        );
    }

    #[tokio::test]
    async fn test_serve_to_passes_globals_to_middleware_with_alias_and_defaults() {
        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let captured_middleware = Arc::clone(&captured);
        let middleware: MiddlewareFn = Arc::new(move |ctx, next| {
            let captured = Arc::clone(&captured_middleware);
            Box::pin(async move {
                *captured.lock().await = Some(ctx.globals);
                next().await;
            })
        });
        let cli = make_test_cli()
            .globals_fields(global_fields())
            .use_middleware(middleware);
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec!["ping".to_string(), "-p".to_string(), "work".to_string()],
                &mut output,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        assert_eq!(
            captured.lock().await.clone(),
            Some(serde_json::json!({ "profile": "work", "trace": false }))
        );
    }

    #[tokio::test]
    async fn test_serve_to_help_lists_custom_global_options_without_validating_them() {
        let cli = make_test_cli().globals_fields(global_fields());
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec!["ping".to_string(), "--help".to_string()],
                &mut output,
                true,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Custom Global Options:"));
        assert!(output.contains("-p, --profile <string>"));
        assert!(output.contains("--trace"));
    }

    #[test]
    #[should_panic(expected = "conflicts with a built-in flag")]
    fn test_globals_reject_builtin_flag_conflicts() {
        let mut fields = global_fields();
        fields[0].name = "format";
        fields[0].cli_name = "format".to_string();
        let _ = Cli::create("test").globals_fields(fields);
    }

    #[test]
    #[should_panic(expected = "conflicts with a global option")]
    fn test_command_options_reject_global_conflicts() {
        let mut command = make_leaf_command("ping", None);
        command.options_fields = vec![global_fields().remove(0)];
        let _ = Cli::create("test")
            .globals_fields(global_fields())
            .command("ping", command);
    }

    #[tokio::test]
    async fn test_schema_includes_globals() {
        let cli = make_test_cli().globals_fields(global_fields());
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec![
                    "ping".to_string(),
                    "--schema".to_string(),
                    "--json".to_string(),
                ],
                &mut output,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["globals"]["properties"]["profile"]["type"], "string");
    }

    #[tokio::test]
    async fn test_root_help_renders_banner_for_matching_mode() {
        let cli = make_test_cli().banner_text(BannerMode::Human, "Test Banner");
        let mut human_output = Vec::new();
        let mut agent_output = Vec::new();

        cli.serve_to(vec![], &mut human_output, true).await.unwrap();
        cli.serve_to(vec![], &mut agent_output, false)
            .await
            .unwrap();

        assert!(
            String::from_utf8(human_output)
                .unwrap()
                .starts_with("Test Banner\n")
        );
        assert!(
            !String::from_utf8(agent_output)
                .unwrap()
                .contains("Test Banner")
        );
    }

    fn make_leaf_command(name: &str, description: Option<&str>) -> CommandDef {
        CommandDef {
            name: name.to_string(),
            description: description.map(|value| value.to_string()),
            args_fields: vec![],
            options_fields: vec![],
            env_fields: vec![],
            aliases: HashMap::new(),
            command_aliases: Vec::new(),
            examples: vec![],
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(NoopHandler),
            middleware: vec![],
            output_schema: None,
        }
    }

    fn make_test_cli() -> Cli {
        Cli::create("test")
            .description("Test CLI")
            .command("ping", make_leaf_command("ping", Some("Ping the server")))
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

    #[test]
    fn test_extract_builtin_flags_config_schema() {
        let argv: Vec<String> = vec!["--config-schema".to_string()];
        let result = extract_builtin_flags(&argv, Some("config")).unwrap();
        assert!(result.config_schema);
    }

    #[test]
    fn test_completion_output_includes_builtins_at_root() {
        let output = completion_output(
            "test",
            &[],
            &make_test_cli().commands,
            None,
            &[],
            &HashMap::new(),
            &["--".to_string(), "test".to_string(), "".to_string()],
            Some("bash"),
            Some("1"),
        )
        .unwrap();

        assert!(output.contains("completions"));
        assert!(output.contains("mcp"));
        assert!(output.contains("skills"));
    }

    #[test]
    fn test_completion_output_includes_builtin_subcommands() {
        let output = completion_output(
            "test",
            &[],
            &make_test_cli().commands,
            None,
            &[],
            &HashMap::new(),
            &[
                "--".to_string(),
                "test".to_string(),
                "skills".to_string(),
                "".to_string(),
            ],
            Some("bash"),
            Some("2"),
        )
        .unwrap();

        assert!(output.contains("add"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_completions_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec!["completions".to_string(), "--help".to_string()],
                &mut output,
                true,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test completions"));
        assert!(output.contains("Generate shell completion script"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_completions_shell() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec!["completions".to_string(), "bash".to_string()],
                &mut output,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("COMPLETE=\"bash\""));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_skills_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(vec!["skills".to_string()], &mut output, true)
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test skills"));
        assert!(output.contains("add"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_skills_add_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec![
                    "skills".to_string(),
                    "add".to_string(),
                    "--help".to_string(),
                ],
                &mut output,
                true,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test skills add"));
        assert!(output.contains("--depth"));
        assert!(output.contains("--no-global"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_mcp_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(vec!["mcp".to_string()], &mut output, true)
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test mcp"));
        assert!(output.contains("add"));
        assert!(output.contains("doctor"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_mcp_add_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec!["mcp".to_string(), "add".to_string(), "--help".to_string()],
                &mut output,
                true,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test mcp add"));
        assert!(output.contains("--command"));
        assert!(output.contains("--agent"));
        assert!(output.contains("--no-global"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_mcp_doctor_help() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec![
                    "mcp".to_string(),
                    "doctor".to_string(),
                    "--help".to_string(),
                ],
                &mut output,
                true,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("test mcp doctor"));
        assert!(output.contains("Validate MCP server startup and tool listing"));
    }

    #[tokio::test]
    async fn test_serve_to_builtin_mcp_doctor_lists_tools_without_calling_them() {
        let cli = make_test_cli();
        let mut output = Vec::new();

        let result = cli
            .serve_to(
                vec![
                    "mcp".to_string(),
                    "doctor".to_string(),
                    "--json".to_string(),
                ],
                &mut output,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result, None);
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            output,
            serde_json::json!({
                "ok": true,
                "toolCount": 1,
                "tools": [{ "name": "ping", "description": "Ping the server" }],
                "warnings": [],
                "errors": [],
            })
        );
    }

    #[tokio::test]
    async fn test_serve_to_config_schema() {
        let cli = Cli::create("test")
            .root(CommandDef {
                name: "test".to_string(),
                description: Some("Test CLI".to_string()),
                args_fields: vec![],
                options_fields: vec![FieldMeta {
                    name: "repo",
                    cli_name: "repo".to_string(),
                    description: Some("Repository path"),
                    field_type: crate::schema::FieldType::String,
                    required: false,
                    default: Some(Value::String(".".to_string())),
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                env_fields: vec![],
                aliases: HashMap::new(),
                command_aliases: Vec::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(NoopHandler),
                middleware: vec![],
                output_schema: None,
            })
            .command("ping", make_leaf_command("ping", Some("Ping the server")))
            .config(ConfigOptions {
                flag: "config".to_string(),
                files: vec!["test.config.json".to_string()],
            });
        let mut output = Vec::new();

        let result = cli
            .serve_to(vec!["--config-schema".to_string()], &mut output, false)
            .await
            .unwrap();

        assert_eq!(result, None);
        let output = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["properties"]["$schema"]["type"], "string");
        assert_eq!(
            parsed["properties"]["options"]["properties"]["repo"]["type"],
            "string"
        );
        assert!(parsed["properties"]["commands"]["properties"]["ping"].is_object());
    }

    // -----------------------------------------------------------------------
    // FetchGateway tests
    // -----------------------------------------------------------------------

    /// A test fetch handler that echoes the request back as JSON.
    struct EchoFetchHandler;

    #[async_trait::async_trait]
    impl crate::fetch::FetchHandler for EchoFetchHandler {
        async fn handle(&self, request: crate::fetch::FetchInput) -> crate::fetch::FetchOutput {
            let data = serde_json::json!({
                "path": request.path,
                "method": request.method,
                "headers": request.headers.iter()
                    .map(|(k, v)| serde_json::json!([k, v]))
                    .collect::<Vec<_>>(),
                "body": request.body,
                "query": request.query.iter()
                    .map(|(k, v)| serde_json::json!([k, v]))
                    .collect::<Vec<_>>(),
            });
            crate::fetch::FetchOutput {
                ok: true,
                status: 200,
                data,
                headers: vec![],
            }
        }
    }

    /// A fetch handler that always returns an error.
    struct ErrorFetchHandler;

    #[async_trait::async_trait]
    impl crate::fetch::FetchHandler for ErrorFetchHandler {
        async fn handle(&self, _request: crate::fetch::FetchInput) -> crate::fetch::FetchOutput {
            crate::fetch::FetchOutput {
                ok: false,
                status: 500,
                data: serde_json::json!({ "message": "Internal Server Error" }),
                headers: vec![],
            }
        }
    }

    #[test]
    fn test_fetch_gateway_builder() {
        let cli = Cli::create("test-cli").fetch_gateway(
            "api",
            EchoFetchHandler,
            crate::fetch::FetchGatewayOptions {
                description: Some("API gateway".to_string()),
                base_path: Some("/v1".to_string()),
                output_policy: None,
            },
        );

        assert!(cli.commands.contains_key("api"));
        match &cli.commands["api"] {
            CommandEntry::FetchGateway {
                description,
                base_path,
                ..
            } => {
                assert_eq!(description.as_deref(), Some("API gateway"));
                assert_eq!(base_path.as_deref(), Some("/v1"));
            }
            _ => panic!("Expected FetchGateway"),
        }
    }

    #[test]
    fn test_resolve_command_fetch_gateway() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "api".to_string(),
            CommandEntry::FetchGateway {
                description: Some("API gateway".to_string()),
                base_path: Some("/v1".to_string()),
                output_policy: None,
                handler: Arc::new(EchoFetchHandler),
            },
        );

        let tokens = vec![
            "api".to_string(),
            "users".to_string(),
            "123".to_string(),
            "--limit".to_string(),
            "10".to_string(),
        ];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Gateway {
                path,
                rest,
                base_path,
                ..
            } => {
                assert_eq!(path, "api");
                assert_eq!(rest, vec!["users", "123", "--limit", "10"]);
                assert_eq!(base_path.as_deref(), Some("/v1"));
            }
            _ => panic!("Expected Gateway"),
        }
    }

    #[test]
    fn test_resolve_command_fetch_gateway_no_args() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "api".to_string(),
            CommandEntry::FetchGateway {
                description: None,
                base_path: None,
                output_policy: None,
                handler: Arc::new(EchoFetchHandler),
            },
        );

        let tokens = vec!["api".to_string()];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Gateway {
                path,
                rest,
                base_path,
                ..
            } => {
                assert_eq!(path, "api");
                assert!(rest.is_empty());
                assert!(base_path.is_none());
            }
            _ => panic!("Expected Gateway"),
        }
    }

    #[tokio::test]
    async fn test_serve_to_fetch_gateway_basic() {
        let cli = Cli::create("test-cli").fetch_gateway(
            "api",
            EchoFetchHandler,
            crate::fetch::FetchGatewayOptions {
                description: None,
                base_path: None,
                output_policy: None,
            },
        );

        let mut output = Vec::new();
        let argv = vec!["api".to_string(), "users".to_string(), "123".to_string()];
        let result = cli.serve_to(argv, &mut output, false).await.unwrap();

        assert_eq!(result, None);
        let output_str = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(parsed["path"], "/users/123");
        assert_eq!(parsed["method"], "GET");
    }

    #[tokio::test]
    async fn test_serve_to_fetch_gateway_with_base_path() {
        let cli = Cli::create("test-cli").fetch_gateway(
            "api",
            EchoFetchHandler,
            crate::fetch::FetchGatewayOptions {
                description: None,
                base_path: Some("/v2".to_string()),
                output_policy: None,
            },
        );

        let mut output = Vec::new();
        let argv = vec!["api".to_string(), "items".to_string()];
        let result = cli.serve_to(argv, &mut output, false).await.unwrap();

        assert_eq!(result, None);
        let output_str = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(parsed["path"], "/v2/items");
    }

    #[tokio::test]
    async fn test_serve_to_fetch_gateway_with_curl_args() {
        let cli = Cli::create("test-cli").fetch_gateway(
            "api",
            EchoFetchHandler,
            crate::fetch::FetchGatewayOptions {
                description: None,
                base_path: None,
                output_policy: None,
            },
        );

        let mut output = Vec::new();
        let argv = vec![
            "api".to_string(),
            "-X".to_string(),
            "POST".to_string(),
            "-d".to_string(),
            r#"{"name":"test"}"#.to_string(),
            "-H".to_string(),
            "Content-Type: application/json".to_string(),
            "users".to_string(),
        ];
        let result = cli.serve_to(argv, &mut output, false).await.unwrap();

        assert_eq!(result, None);
        let output_str = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(parsed["path"], "/users");
        assert_eq!(parsed["method"], "POST");
        assert_eq!(parsed["body"], r#"{"name":"test"}"#);
    }

    #[tokio::test]
    async fn test_serve_to_fetch_gateway_error_returns_exit_code() {
        let cli = Cli::create("test-cli").fetch_gateway(
            "api",
            ErrorFetchHandler,
            crate::fetch::FetchGatewayOptions {
                description: None,
                base_path: None,
                output_policy: None,
            },
        );

        let mut output = Vec::new();
        let argv = vec!["api".to_string(), "users".to_string()];
        let result = cli.serve_to(argv, &mut output, false).await.unwrap();

        assert_eq!(result, Some(1));
        let output_str = String::from_utf8(output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(output_str.trim()).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["status"], 500);
    }

    #[test]
    fn test_collect_help_commands_includes_fetch_gateway() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "api".to_string(),
            CommandEntry::FetchGateway {
                description: Some("API gateway".to_string()),
                base_path: None,
                output_policy: None,
                handler: Arc::new(EchoFetchHandler),
            },
        );
        commands.insert(
            "deploy".to_string(),
            CommandEntry::Leaf(Arc::new(CommandDef {
                name: "deploy".to_string(),
                description: Some("Deploy the app".to_string()),
                args_fields: vec![],
                options_fields: vec![],
                env_fields: vec![],
                aliases: std::collections::HashMap::new(),
                command_aliases: Vec::new(),
                examples: vec![],
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(NoopHandler),
                middleware: vec![],
                output_schema: None,
            })),
        );

        let summaries = collect_help_commands(&commands);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "api");
        assert_eq!(summaries[0].description.as_deref(), Some("API gateway"));
        assert_eq!(summaries[1].name, "deploy");
    }

    #[test]
    fn test_resolve_gateway_in_group() {
        let mut sub_commands = BTreeMap::new();
        sub_commands.insert(
            "fetch".to_string(),
            CommandEntry::FetchGateway {
                description: Some("Fetch endpoint".to_string()),
                base_path: Some("/api".to_string()),
                output_policy: None,
                handler: Arc::new(EchoFetchHandler),
            },
        );

        let mut commands = BTreeMap::new();
        commands.insert(
            "service".to_string(),
            CommandEntry::Group {
                description: Some("Service commands".to_string()),
                commands: sub_commands,
                middleware: vec![],
                output_policy: None,
            },
        );

        let tokens = vec![
            "service".to_string(),
            "fetch".to_string(),
            "users".to_string(),
        ];
        match resolve_command(&commands, &tokens) {
            ResolvedCommand::Gateway {
                path,
                rest,
                base_path,
                ..
            } => {
                assert_eq!(path, "service fetch");
                assert_eq!(rest, vec!["users"]);
                assert_eq!(base_path.as_deref(), Some("/api"));
            }
            _ => panic!("Expected Gateway"),
        }
    }
}
