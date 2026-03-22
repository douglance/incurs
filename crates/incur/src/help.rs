//! Help text generation for the incur framework.
//!
//! Generates formatted help output for both router CLIs (command groups)
//! and leaf commands. Handles all sections: header, synopsis, arguments,
//! options, examples, hints, subcommands, global options, and env vars.
//!
//! Ported from `src/Help.ts`.

use std::collections::HashMap;

use crate::command::Example;
use crate::schema::{FieldMeta, FieldType};

/// Summary of a command for display in help text.
#[derive(Debug, Clone)]
pub struct CommandSummary {
    /// The command name.
    pub name: String,
    /// A short description of what the command does.
    pub description: Option<String>,
}

/// Options for formatting router help (command groups).
pub struct FormatRootOptions {
    /// Alternative binary names for this CLI.
    pub aliases: Option<Vec<String>>,
    /// Flag name for config file path (e.g. `"config"` renders `--config <path>`).
    pub config_flag: Option<String>,
    /// Subcommands to list.
    pub commands: Vec<CommandSummary>,
    /// A short description of the CLI or group.
    pub description: Option<String>,
    /// Whether this is the root-level CLI (shows additional built-in flags).
    pub root: bool,
    /// CLI version string.
    pub version: Option<String>,
}

/// Options for formatting leaf command help.
pub struct FormatCommandOptions {
    /// Alternative binary names for this CLI.
    pub aliases: Option<Vec<String>>,
    /// Schema for positional arguments.
    pub args_fields: Vec<FieldMeta>,
    /// Flag name for config file path.
    pub config_flag: Option<String>,
    /// Subcommands to list (for CLIs with both a root handler and subcommands).
    pub commands: Vec<CommandSummary>,
    /// A short description of what the command does.
    pub description: Option<String>,
    /// Schema for environment variables.
    pub env_fields: Vec<FieldMeta>,
    /// Usage examples.
    pub examples: Vec<Example>,
    /// Plain text hint displayed after examples.
    pub hint: Option<String>,
    /// Whether to hide the global options section.
    pub hide_global_options: bool,
    /// Schema for named options/flags.
    pub options_fields: Vec<FieldMeta>,
    /// Map of option names to single-char aliases.
    pub option_aliases: HashMap<String, char>,
    /// Whether this is the root-level CLI.
    pub root: bool,
    /// CLI version string.
    pub version: Option<String>,
}

/// Formats help text for a router CLI or command group.
///
/// Displays a header, usage synopsis, list of available commands,
/// and global options.
pub fn format_root(name: &str, options: &FormatRootOptions) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Header
    let title = match &options.version {
        Some(v) => format!("{name}@{v}"),
        None => name.to_string(),
    };
    match &options.description {
        Some(desc) => lines.push(format!("{title} \u{2014} {desc}")),
        None => lines.push(title),
    }
    lines.push(String::new());

    // Synopsis
    lines.push(format!("Usage: {name} <command>"));
    if let Some(aliases) = &options.aliases && !aliases.is_empty() {
        lines.push(format!("Aliases: {}", aliases.join(", ")));
    }

    // Commands
    if !options.commands.is_empty() {
        lines.push(String::new());
        lines.push("Commands:".to_string());
        let max_len = options.commands.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for cmd in &options.commands {
            if let Some(desc) = &cmd.description {
                let padding = " ".repeat(max_len - cmd.name.len());
                lines.push(format!("  {}{padding}  {desc}", cmd.name));
            } else {
                lines.push(format!("  {}", cmd.name));
            }
        }
    }

    // Global options
    lines.extend(global_options_lines(options.root, options.config_flag.as_deref()));

    lines.join("\n")
}

/// Formats help text for a leaf command.
///
/// Displays a header, usage synopsis, arguments, options (with defaults
/// and deprecation markers), examples, hints, subcommands, global options,
/// and environment variables.
pub fn format_command(name: &str, options: &FormatCommandOptions) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Header
    let title = match &options.version {
        Some(v) => format!("{name}@{v}"),
        None => name.to_string(),
    };
    match &options.description {
        Some(desc) => lines.push(format!("{title} \u{2014} {desc}")),
        None => lines.push(title),
    }
    lines.push(String::new());

    // Synopsis
    let synopsis = build_synopsis(name, &options.args_fields);
    let options_suffix = if options.options_fields.is_empty() {
        ""
    } else {
        " [options]"
    };
    let commands_suffix = if options.commands.is_empty() {
        ""
    } else {
        " | <command>"
    };
    lines.push(format!("Usage: {synopsis}{options_suffix}{commands_suffix}"));
    if let Some(aliases) = &options.aliases && !aliases.is_empty() {
        lines.push(format!("Aliases: {}", aliases.join(", ")));
    }

    // Arguments
    if !options.args_fields.is_empty() {
        let entries: Vec<(&str, String)> = options
            .args_fields
            .iter()
            .map(|f| {
                let desc = f.description.unwrap_or("");
                (f.name, desc.to_string())
            })
            .collect();
        if !entries.is_empty() {
            lines.push(String::new());
            lines.push("Arguments:".to_string());
            let max_len = entries.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
            for (field_name, desc) in &entries {
                let padding = " ".repeat(max_len - field_name.len());
                lines.push(format!("  {field_name}{padding}  {desc}"));
            }
        }
    }

    // Options
    if !options.options_fields.is_empty() {
        let entries: Vec<OptionEntry> = options
            .options_fields
            .iter()
            .map(|f| {
                let type_name = f.field_type.display_name();
                let short = options.option_aliases.get(f.name).copied();
                let flag = if let Some(ch) = short {
                    format!("--{}, -{ch} <{type_name}>", f.cli_name)
                } else {
                    format!("--{} <{type_name}>", f.cli_name)
                };
                OptionEntry {
                    flag,
                    description: f.description.unwrap_or("").to_string(),
                    default_value: f.default.as_ref().map(|d| format!("{d}")),
                    deprecated: f.deprecated,
                }
            })
            .collect();

        if !entries.is_empty() {
            lines.push(String::new());
            lines.push("Options:".to_string());
            let max_len = entries.iter().map(|e| e.flag.len()).max().unwrap_or(0);
            for entry in &entries {
                let padding = " ".repeat(max_len - entry.flag.len());
                let prefix = if entry.deprecated { "[deprecated] " } else { "" };
                let desc = match &entry.default_value {
                    Some(dv) => format!("{prefix}{} (default: {dv})", entry.description),
                    None => format!("{prefix}{}", entry.description),
                };
                lines.push(format!("  {}{padding}  {desc}", entry.flag));
            }
        }
    }

    // Examples
    if !options.examples.is_empty() {
        lines.push(String::new());
        lines.push("Examples:".to_string());
        let max_len = options
            .examples
            .iter()
            .map(|e| {
                if e.command.is_empty() {
                    name.len()
                } else {
                    name.len() + 1 + e.command.len()
                }
            })
            .max()
            .unwrap_or(0);
        for ex in &options.examples {
            let cmd = if ex.command.is_empty() {
                name.to_string()
            } else {
                format!("{name} {}", ex.command)
            };
            if let Some(desc) = &ex.description {
                let padding = " ".repeat(max_len - cmd.len());
                lines.push(format!("  {cmd}{padding}  # {desc}"));
            } else {
                lines.push(format!("  {cmd}"));
            }
        }
    }

    // Hint
    if let Some(hint) = &options.hint {
        lines.push(String::new());
        lines.push(hint.clone());
    }

    // Subcommands
    if !options.commands.is_empty() {
        lines.push(String::new());
        lines.push("Commands:".to_string());
        let max_len = options.commands.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for cmd in &options.commands {
            if let Some(desc) = &cmd.description {
                let padding = " ".repeat(max_len - cmd.name.len());
                lines.push(format!("  {}{padding}  {desc}", cmd.name));
            } else {
                lines.push(format!("  {}", cmd.name));
            }
        }
    }

    // Global options
    if !options.hide_global_options {
        lines.extend(global_options_lines(options.root, options.config_flag.as_deref()));
    }

    // Environment Variables
    if !options.env_fields.is_empty() {
        let entries: Vec<EnvEntry> = options
            .env_fields
            .iter()
            .map(|f| {
                let env_name = f.env_name.unwrap_or(f.name);
                EnvEntry {
                    name: env_name.to_string(),
                    description: f.description.unwrap_or("").to_string(),
                    default_value: f.default.as_ref().map(|d| format!("{d}")),
                }
            })
            .collect();

        if !entries.is_empty() {
            lines.push(String::new());
            lines.push("Environment Variables:".to_string());
            let max_len = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for entry in &entries {
                let padding = " ".repeat(max_len - entry.name.len());
                let mut parts = vec![entry.description.clone()];
                // Note: we don't check current env values in this implementation
                // since we don't have access to the env source here. The TS version
                // optionally checks process.env.
                if let Some(dv) = &entry.default_value {
                    parts.push(format!("default: {dv}"));
                }
                let desc = if parts.len() > 1 {
                    format!("{} ({})", parts[0], parts[1..].join(", "))
                } else {
                    parts[0].clone()
                };
                lines.push(format!("  {}{padding}  {desc}", entry.name));
            }
        }
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// An option entry for help display.
struct OptionEntry {
    flag: String,
    description: String,
    default_value: Option<String>,
    deprecated: bool,
}

/// An environment variable entry for help display.
struct EnvEntry {
    name: String,
    description: String,
    default_value: Option<String>,
}

/// Builds the synopsis string with `<required>` and `[optional]` placeholders.
fn build_synopsis(name: &str, args_fields: &[FieldMeta]) -> String {
    if args_fields.is_empty() {
        return name.to_string();
    }

    let mut parts = vec![name.to_string()];
    for field in args_fields {
        let label = match &field.field_type {
            FieldType::Enum(values) => values.join("|"),
            _ => field.name.to_string(),
        };
        if field.required {
            parts.push(format!("<{label}>"));
        } else {
            parts.push(format!("[{label}]"));
        }
    }
    parts.join(" ")
}

/// Renders the global options block (built-in flags).
///
/// Root-level CLIs get additional flags like `--version` and `--mcp`.
fn global_options_lines(root: bool, config_flag: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();

    // Integrations section (root only)
    if root {
        let builtins = vec![
            ("completions", "Generate shell completion script"),
            ("mcp add", "Register as MCP server"),
            ("skills add", "Sync skill files to agents"),
        ];
        let max_cmd = builtins.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        lines.push(String::new());
        lines.push("Integrations:".to_string());
        for (name, desc) in &builtins {
            let padding = " ".repeat(max_cmd - name.len());
            lines.push(format!("  {name}{padding}  {desc}"));
        }
    }

    // Global flags
    // Build flags list. We need owned strings for config-related flags.
    let mut owned_flags: Vec<(String, String)> = Vec::new();

    if let Some(cfg) = config_flag {
        owned_flags.push((
            format!("--{cfg} <path>"),
            "Load JSON option defaults from a file".to_string(),
        ));
    }

    owned_flags.push((
        "--filter-output <keys>".to_string(),
        "Filter output by key paths (e.g. foo,bar.baz,a[0,3])".to_string(),
    ));
    owned_flags.push((
        "--format <toon|json|yaml|md|jsonl>".to_string(),
        "Output format".to_string(),
    ));
    owned_flags.push(("--help".to_string(), "Show help".to_string()));
    owned_flags.push((
        "--llms, --llms-full".to_string(),
        "Print LLM-readable manifest".to_string(),
    ));

    if root {
        owned_flags.push(("--mcp".to_string(), "Start as MCP stdio server".to_string()));
    }

    if let Some(cfg) = config_flag {
        owned_flags.push((
            format!("--no-{cfg}"),
            "Disable JSON option defaults for this run".to_string(),
        ));
    }

    owned_flags.push((
        "--schema".to_string(),
        "Show JSON Schema for command".to_string(),
    ));
    owned_flags.push((
        "--token-count".to_string(),
        "Print token count of output (instead of output)".to_string(),
    ));
    owned_flags.push((
        "--token-limit <n>".to_string(),
        "Limit output to n tokens".to_string(),
    ));
    owned_flags.push((
        "--token-offset <n>".to_string(),
        "Skip first n tokens of output".to_string(),
    ));
    owned_flags.push((
        "--verbose".to_string(),
        "Show full output envelope".to_string(),
    ));

    if root {
        owned_flags.push(("--version".to_string(), "Show version".to_string()));
    }

    // Sort by flag name
    owned_flags.sort_by(|a, b| a.0.cmp(&b.0));

    let max_len = owned_flags.iter().map(|(f, _)| f.len()).max().unwrap_or(0);

    lines.push(String::new());
    lines.push("Global Options:".to_string());
    for (flag, desc) in &owned_flags {
        let padding = " ".repeat(max_len - flag.len());
        lines.push(format!("  {flag}{padding}  {desc}"));
    }

    lines
}

/// Redacts a value, showing only the last 4 characters.
#[allow(dead_code)]
fn redact(value: &str) -> String {
    if value.len() <= 4 {
        "****".to_string()
    } else {
        format!("****{}", &value[value.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_root_basic() {
        let help = format_root(
            "my-cli",
            &FormatRootOptions {
                aliases: None,
                config_flag: None,
                commands: vec![
                    CommandSummary {
                        name: "list".to_string(),
                        description: Some("List items".to_string()),
                    },
                    CommandSummary {
                        name: "get".to_string(),
                        description: Some("Get an item".to_string()),
                    },
                ],
                description: Some("A test CLI".to_string()),
                root: false,
                version: Some("1.0.0".to_string()),
            },
        );

        assert!(help.contains("my-cli@1.0.0 \u{2014} A test CLI"));
        assert!(help.contains("Usage: my-cli <command>"));
        assert!(help.contains("Commands:"));
        assert!(help.contains("list"));
        assert!(help.contains("get"));
    }

    #[test]
    fn test_format_command_basic() {
        let help = format_command(
            "my-cli deploy",
            &FormatCommandOptions {
                aliases: None,
                args_fields: vec![FieldMeta {
                    name: "environment",
                    cli_name: "environment".to_string(),
                    description: Some("Target environment"),
                    field_type: FieldType::String,
                    required: true,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }],
                config_flag: None,
                commands: vec![],
                description: Some("Deploy the app".to_string()),
                env_fields: vec![],
                examples: vec![Example {
                    command: "production".to_string(),
                    description: Some("Deploy to prod".to_string()),
                }],
                hint: None,
                hide_global_options: false,
                options_fields: vec![FieldMeta {
                    name: "verbose",
                    cli_name: "verbose".to_string(),
                    description: Some("Verbose output"),
                    field_type: FieldType::Boolean,
                    required: false,
                    default: None,
                    alias: Some('v'),
                    deprecated: false,
                    env_name: None,
                }],
                option_aliases: {
                    let mut m = HashMap::new();
                    m.insert("verbose".to_string(), 'v');
                    m
                },
                root: false,
                version: None,
            },
        );

        assert!(help.contains("my-cli deploy \u{2014} Deploy the app"));
        assert!(help.contains("Usage: my-cli deploy <environment> [options]"));
        assert!(help.contains("Arguments:"));
        assert!(help.contains("environment"));
        assert!(help.contains("Options:"));
        assert!(help.contains("--verbose, -v <boolean>"));
        assert!(help.contains("Examples:"));
        assert!(help.contains("production"));
    }

    #[test]
    fn test_build_synopsis_no_args() {
        assert_eq!(build_synopsis("test", &[]), "test");
    }

    #[test]
    fn test_build_synopsis_with_args() {
        let fields = vec![
            FieldMeta {
                name: "name",
                cli_name: "name".to_string(),
                description: None,
                field_type: FieldType::String,
                required: true,
                default: None,
                alias: None,
                deprecated: false,
                env_name: None,
            },
            FieldMeta {
                name: "count",
                cli_name: "count".to_string(),
                description: None,
                field_type: FieldType::Number,
                required: false,
                default: None,
                alias: None,
                deprecated: false,
                env_name: None,
            },
        ];
        assert_eq!(build_synopsis("test", &fields), "test <name> [count]");
    }

    #[test]
    fn test_redact() {
        assert_eq!(redact("secret123"), "****t123");
        assert_eq!(redact("abc"), "****");
        assert_eq!(redact(""), "****");
    }
}
