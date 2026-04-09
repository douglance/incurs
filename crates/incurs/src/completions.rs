//! Shell completion generation for bash, zsh, fish, and nushell.
//!
//! Ported from `src/Completions.ts`. Generates shell hook scripts for dynamic
//! tab completions and computes completion candidates based on the command tree.

use std::collections::BTreeMap;

use crate::schema::{FieldMeta, FieldType};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Supported shell environments for completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Nushell,
}

impl Shell {
    /// Parses a shell name from a string.
    pub fn from_str(s: &str) -> Option<Shell> {
        match s {
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "fish" => Some(Shell::Fish),
            "nushell" => Some(Shell::Nushell),
            _ => None,
        }
    }
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shell::Bash => write!(f, "bash"),
            Shell::Zsh => write!(f, "zsh"),
            Shell::Fish => write!(f, "fish"),
            Shell::Nushell => write!(f, "nushell"),
        }
    }
}

/// A completion candidate with an optional description.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The completion value.
    pub value: String,
    /// Optional description shown alongside the candidate.
    pub description: Option<String>,
    /// When true, the shell should not append a trailing space after this candidate.
    pub no_space: bool,
}

/// A command entry in the command tree (either a leaf or a group).
///
/// This is a minimal representation sufficient for completions. The full
/// `CommandEntry` type is defined in the `command` module written by another
/// engineer. This local type avoids a circular dependency.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Whether this entry is a group (has subcommands).
    pub is_group: bool,
    /// Human-readable description.
    pub description: Option<String>,
    /// Subcommands (only populated for groups).
    pub commands: BTreeMap<String, CommandEntry>,
    /// Option field metadata for leaf commands.
    pub options_fields: Vec<FieldMeta>,
    /// Short alias mapping: option name -> alias character.
    pub aliases: BTreeMap<String, char>,
}

/// A root command definition (the CLI itself, which may have options).
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// Option field metadata.
    pub options_fields: Vec<FieldMeta>,
    /// Short alias mapping: option name -> alias character.
    pub aliases: BTreeMap<String, char>,
}

// ---------------------------------------------------------------------------
// Shell registration scripts
// ---------------------------------------------------------------------------

/// Generates a shell hook script that registers dynamic completions for the CLI.
///
/// The hook calls back into the binary with `COMPLETE=<shell>` at every tab press.
pub fn register(shell: Shell, name: &str) -> String {
    match shell {
        Shell::Bash => bash_register(name),
        Shell::Zsh => zsh_register(name),
        Shell::Fish => fish_register(name),
        Shell::Nushell => nushell_register(name),
    }
}

/// Sanitizes a CLI name into a valid shell identifier.
fn ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

fn bash_register(name: &str) -> String {
    let id = ident(name);
    format!(
        r#"_incur_complete_{id}() {{
    local IFS=$'\013'
    local _COMPLETE_INDEX=${{COMP_CWORD}}
    local _completions
    _completions=( $(
        COMPLETE="bash"
        _COMPLETE_INDEX="$_COMPLETE_INDEX"
        "{name}" -- "${{COMP_WORDS[@]}}"
    ) )
    if [[ $? != 0 ]]; then
        unset COMPREPLY
        return
    fi
    local _nospace=false
    COMPREPLY=()
    for _c in "${{_completions[@]}}"; do
        if [[ "$_c" == *$'\001' ]]; then
            _nospace=true
            COMPREPLY+=("${{_c%$'\001'}}")
        else
            COMPREPLY+=("$_c")
        fi
    done
    if [[ $_nospace == true ]]; then
        compopt -o nospace
    fi
}}
complete -o default -o bashdefault -o nosort -F _incur_complete_{id} {name}"#,
        id = id,
        name = name
    )
}

fn zsh_register(name: &str) -> String {
    let id = ident(name);
    format!(
        r#"#compdef {name}
_incur_complete_{id}() {{
    local completions=("${{(@f)$(
        _COMPLETE_INDEX=$(( CURRENT - 1 ))
        COMPLETE="zsh"
        "{name}" -- "${{words[@]}}" 2>/dev/null
    )}}")
    if [[ -n $completions ]]; then
        _describe 'values' completions -S ''
    fi
}}
compdef _incur_complete_{id} {name}"#,
        id = id,
        name = name
    )
}

fn fish_register(name: &str) -> String {
    format!(
        r#"complete --keep-order --exclusive --command {name} \
    --arguments "(COMPLETE=fish {name} -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))""#,
        name = name
    )
}

fn nushell_register(name: &str) -> String {
    let id = ident(name);
    format!(
        r#"# External completer for {name}
# Add to $env.config.completions.external.completer or use in a dispatch completer.
let _incur_complete_{id} = {{|spans|
    COMPLETE=nushell {name} -- ...$spans | from json
}}"#,
        id = id,
        name = name
    )
}

// ---------------------------------------------------------------------------
// Completion computation
// ---------------------------------------------------------------------------

/// Computes completion candidates for the given argv words and cursor index.
///
/// Walks the command tree to resolve the active command, then suggests
/// subcommands, options, or positional argument hints.
pub fn complete(
    commands: &BTreeMap<String, CommandEntry>,
    root_command: Option<&CommandDef>,
    argv: &[String],
    index: usize,
) -> Vec<Candidate> {
    let current = argv.get(index).map(|s| s.as_str()).unwrap_or("");

    // Walk argv tokens up to (but not including) the cursor word to resolve the active scope
    let mut scope_commands = commands;
    let mut scope_leaf: Option<LeafInfo> = root_command.map(|r| LeafInfo {
        options_fields: &r.options_fields,
        aliases: &r.aliases,
    });

    for i in 0..index {
        let token = match argv.get(i) {
            Some(t) => t.as_str(),
            None => break,
        };
        if token.starts_with('-') {
            continue;
        }
        if let Some(entry) = scope_commands.get(token) {
            if entry.is_group {
                scope_commands = &entry.commands;
                scope_leaf = None;
            } else {
                scope_leaf = Some(LeafInfo {
                    options_fields: &entry.options_fields,
                    aliases: &entry.aliases,
                });
                // Reached a leaf — no more subcommands
                break;
            }
        }
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    // If cursor word starts with '-', suggest options from the active leaf command
    if current.starts_with('-') {
        if let Some(leaf) = &scope_leaf {
            for field in leaf.options_fields {
                let flag = format!("--{}", field.cli_name);
                if flag.starts_with(current) {
                    candidates.push(Candidate {
                        value: flag,
                        description: field.description.map(|s| s.to_string()),
                        no_space: false,
                    });
                }
            }
            // Short aliases
            for (name, &alias_char) in leaf.aliases {
                let flag = format!("-{}", alias_char);
                if flag.starts_with(current) {
                    let desc = leaf
                        .options_fields
                        .iter()
                        .find(|f| f.name == name)
                        .and_then(|f| f.description)
                        .map(|s| s.to_string());
                    candidates.push(Candidate {
                        value: flag,
                        description: desc,
                        no_space: false,
                    });
                }
            }
        }
        return candidates;
    }

    // Check if previous token is a non-boolean option expecting a value
    if index > 0 {
        let prev = argv.get(index - 1).map(|s| s.as_str()).unwrap_or("");
        if prev.starts_with('-') {
            if let Some(leaf) = &scope_leaf {
                if let Some(field_name) = resolve_option_name(prev, leaf) {
                    // Check for enum values
                    if let Some(values) = possible_values(&field_name, leaf.options_fields) {
                        for v in values {
                            if v.starts_with(current) {
                                candidates.push(Candidate {
                                    value: v,
                                    description: None,
                                    no_space: false,
                                });
                            }
                        }
                        return candidates;
                    }
                    // If non-boolean option expecting a value, return empty
                    if !is_boolean_option(&field_name, leaf.options_fields) {
                        return candidates;
                    }
                }
            }
        }
    }

    // Suggest subcommands
    for (name, entry) in scope_commands {
        if name.starts_with(current) {
            candidates.push(Candidate {
                value: name.clone(),
                description: entry.description.clone(),
                no_space: entry.is_group,
            });
        }
    }

    candidates
}

/// Formats completion candidates into shell-specific output.
pub fn format(shell: Shell, candidates: &[Candidate]) -> String {
    match shell {
        Shell::Bash => {
            // \013-separated values; noSpace candidates end with \001
            candidates
                .iter()
                .map(|c| {
                    if c.no_space {
                        format!("{}\x01", c.value)
                    } else {
                        c.value.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join("\x0B")
        }
        Shell::Zsh => {
            // value:description newline-separated (: escaped in values)
            candidates
                .iter()
                .map(|c| {
                    let escaped = c.value.replace(':', "\\:");
                    if let Some(desc) = &c.description {
                        format!("{}:{}", escaped, desc)
                    } else {
                        escaped
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Shell::Fish => {
            // value\tdescription newline-separated
            candidates
                .iter()
                .map(|c| {
                    if let Some(desc) = &c.description {
                        format!("{}\t{}", c.value, desc)
                    } else {
                        c.value.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Shell::Nushell => {
            // JSON array of {value, description} records
            let records: Vec<serde_json::Value> = candidates
                .iter()
                .map(|c| {
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "value".to_string(),
                        serde_json::Value::String(c.value.clone()),
                    );
                    if let Some(desc) = &c.description {
                        obj.insert(
                            "description".to_string(),
                            serde_json::Value::String(desc.clone()),
                        );
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();
            serde_json::to_string(&records).unwrap_or_else(|_| "[]".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Borrowed view of a leaf command's completion-relevant data.
struct LeafInfo<'a> {
    options_fields: &'a [FieldMeta],
    aliases: &'a BTreeMap<String, char>,
}

/// Resolves a flag token (e.g. `--foo-bar` or `-f`) to its option field name.
fn resolve_option_name(token: &str, leaf: &LeafInfo<'_>) -> Option<String> {
    if token.starts_with("--") {
        let raw = &token[2..];
        // Try kebab-to-snake lookup
        let snake = crate::schema::to_snake(raw);
        if leaf.options_fields.iter().any(|f| f.name == snake) {
            return Some(snake);
        }
        // Try direct match
        if leaf.options_fields.iter().any(|f| f.name == raw) {
            return Some(raw.to_string());
        }
        None
    } else if token.starts_with('-') && token.len() == 2 {
        let short = token.chars().nth(1)?;
        for (name, &alias) in leaf.aliases {
            if alias == short {
                return Some(name.clone());
            }
        }
        None
    } else {
        None
    }
}

/// Checks if an option is boolean or count type.
fn is_boolean_option(name: &str, fields: &[FieldMeta]) -> bool {
    fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| matches!(f.field_type, FieldType::Boolean | FieldType::Count))
        .unwrap_or(false)
}

/// Extracts possible values from enum-typed fields.
fn possible_values(name: &str, fields: &[FieldMeta]) -> Option<Vec<String>> {
    fields
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| match &f.field_type {
            FieldType::Enum(values) => Some(values.clone()),
            _ => None,
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{to_kebab, FieldType};

    fn make_field(name: &'static str, ft: FieldType) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: to_kebab(name),
            description: Some("A field"),
            field_type: ft,
            required: false,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_entry(desc: &str, is_group: bool) -> CommandEntry {
        CommandEntry {
            is_group,
            description: Some(desc.to_string()),
            commands: BTreeMap::new(),
            options_fields: vec![],
            aliases: BTreeMap::new(),
        }
    }

    #[test]
    fn test_shell_parse() {
        assert_eq!(Shell::from_str("bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_str("zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_str("fish"), Some(Shell::Fish));
        assert_eq!(Shell::from_str("nushell"), Some(Shell::Nushell));
        assert_eq!(Shell::from_str("powershell"), None);
    }

    #[test]
    fn test_ident() {
        assert_eq!(ident("my-cli"), "my_cli");
        assert_eq!(ident("my_cli"), "my_cli");
        assert_eq!(ident("cli.js"), "cli_js");
    }

    #[test]
    fn test_register_bash() {
        let script = register(Shell::Bash, "mycli");
        assert!(script.contains("_incur_complete_mycli"));
        assert!(script.contains("complete -o default"));
    }

    #[test]
    fn test_register_zsh() {
        let script = register(Shell::Zsh, "mycli");
        assert!(script.contains("#compdef mycli"));
        assert!(script.contains("compdef _incur_complete_mycli mycli"));
    }

    #[test]
    fn test_register_fish() {
        let script = register(Shell::Fish, "mycli");
        assert!(script.contains("complete --keep-order"));
        assert!(script.contains("COMPLETE=fish mycli"));
    }

    #[test]
    fn test_register_nushell() {
        let script = register(Shell::Nushell, "mycli");
        assert!(script.contains("COMPLETE=nushell mycli"));
    }

    #[test]
    fn test_complete_subcommands() {
        let mut commands = BTreeMap::new();
        commands.insert("deploy".to_string(), make_entry("Deploy things", false));
        commands.insert("status".to_string(), make_entry("Show status", false));
        commands.insert("debug".to_string(), make_entry("Debug mode", false));

        let argv = vec!["mycli".to_string(), "de".to_string()];
        let candidates = complete(&commands, None, &argv, 1);
        assert_eq!(candidates.len(), 2); // both "deploy" and "debug" match "de"
        let values: Vec<&str> = candidates.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"deploy"));
        assert!(values.contains(&"debug"));
    }

    #[test]
    fn test_complete_options() {
        let mut commands = BTreeMap::new();
        let mut entry = make_entry("Deploy", false);
        entry.options_fields = vec![
            make_field("output", FieldType::String),
            make_field("verbose", FieldType::Boolean),
        ];
        commands.insert("deploy".to_string(), entry);

        let argv = vec!["mycli".to_string(), "deploy".to_string(), "--".to_string()];
        let candidates = complete(&commands, None, &argv, 2);
        assert_eq!(candidates.len(), 2);
        let values: Vec<&str> = candidates.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"--output"));
        assert!(values.contains(&"--verbose"));
    }

    #[test]
    fn test_complete_enum_values() {
        let mut commands = BTreeMap::new();
        let mut entry = make_entry("Format", false);
        entry.options_fields = vec![make_field(
            "format",
            FieldType::Enum(vec![
                "json".to_string(),
                "yaml".to_string(),
                "toon".to_string(),
            ]),
        )];
        commands.insert("output".to_string(), entry);

        let argv = vec![
            "mycli".to_string(),
            "output".to_string(),
            "--format".to_string(),
            "j".to_string(),
        ];
        let candidates = complete(&commands, None, &argv, 3);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].value, "json");
    }

    #[test]
    fn test_format_bash() {
        let candidates = vec![
            Candidate {
                value: "deploy".to_string(),
                description: None,
                no_space: false,
            },
            Candidate {
                value: "status".to_string(),
                description: None,
                no_space: true,
            },
        ];
        let output = format(Shell::Bash, &candidates);
        assert!(output.contains("deploy"));
        assert!(output.contains("status\x01"));
        assert!(output.contains('\x0B'));
    }

    #[test]
    fn test_format_zsh() {
        let candidates = vec![Candidate {
            value: "deploy".to_string(),
            description: Some("Deploy things".to_string()),
            no_space: false,
        }];
        let output = format(Shell::Zsh, &candidates);
        assert_eq!(output, "deploy:Deploy things");
    }

    #[test]
    fn test_format_fish() {
        let candidates = vec![Candidate {
            value: "deploy".to_string(),
            description: Some("Deploy things".to_string()),
            no_space: false,
        }];
        let output = format(Shell::Fish, &candidates);
        assert_eq!(output, "deploy\tDeploy things");
    }

    #[test]
    fn test_format_nushell() {
        let candidates = vec![Candidate {
            value: "deploy".to_string(),
            description: Some("Deploy things".to_string()),
            no_space: false,
        }];
        let output = format(Shell::Nushell, &candidates);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["value"], "deploy");
        assert_eq!(parsed[0]["description"], "Deploy things");
    }

    #[test]
    fn test_group_no_space() {
        let mut sub_commands = BTreeMap::new();
        sub_commands.insert("app".to_string(), make_entry("Deploy app", false));

        let mut commands = BTreeMap::new();
        let group = CommandEntry {
            is_group: true,
            description: Some("Deploy group".to_string()),
            commands: sub_commands,
            options_fields: vec![],
            aliases: BTreeMap::new(),
        };
        commands.insert("deploy".to_string(), group);

        let argv = vec!["mycli".to_string(), "".to_string()];
        let candidates = complete(&commands, None, &argv, 1);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].no_space, "Groups should have no_space=true");
    }
}
