//! Skill file (SKILL.md) generation for agent discovery.
//!
//! Ported from `src/Skill.ts`. Generates Markdown skill files that AI coding
//! agents use to discover and understand CLI commands. Supports compact index
//! generation (`--llms`), full skill file generation, depth-based splitting,
//! and SHA-256 hashing for staleness detection.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::schema::FieldMeta;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Information about a single command, used for skill file generation.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    /// The command name (may include spaces for subcommands, e.g. "deploy app").
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Positional argument field metadata.
    pub args_fields: Vec<FieldMeta>,
    /// Named option field metadata.
    pub options_fields: Vec<FieldMeta>,
    /// Environment variable field metadata.
    pub env_fields: Vec<FieldMeta>,
    /// Actionable hint for users.
    pub hint: Option<String>,
    /// Usage examples.
    pub examples: Vec<Example>,
    /// JSON Schema for command output (as serde_json::Value).
    pub output_schema: Option<serde_json::Value>,
}

/// A usage example for a command.
#[derive(Debug, Clone)]
pub struct Example {
    /// The command invocation (e.g. "deploy --force").
    pub command: String,
    /// Optional description of what this example demonstrates.
    pub description: Option<String>,
}

/// A generated skill file with its directory name and content.
#[derive(Debug, Clone)]
pub struct SkillFile {
    /// Directory name relative to output root (empty string for depth 0).
    pub dir: String,
    /// Markdown content.
    pub content: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generates a compact Markdown command index for `--llms`.
///
/// Produces a table summarizing all commands with their signatures
/// and descriptions.
pub fn index(name: &str, commands: &[CommandInfo], description: Option<&str>) -> String {
    let mut lines = vec![format!("# {}", name)];
    if let Some(desc) = description {
        lines.push(String::new());
        lines.push(desc.to_string());
    }
    lines.push(String::new());
    lines.push("| Command | Description |".to_string());
    lines.push("|---------|-------------|".to_string());

    for cmd in commands {
        let signature = build_signature(name, cmd);
        let desc = cmd.description.as_deref().unwrap_or("");
        lines.push(format!("| `{}` | {} |", signature, desc));
    }

    lines.push(String::new());
    lines.push(format!(
        "Run `{} --llms-full` for full manifest. Run `{} <command> --schema` for argument details.",
        name, name
    ));

    lines.join("\n")
}

/// Generates a full Markdown skill file from a CLI name and collected commands.
///
/// When `groups` is non-empty, commands are organized under group headings.
pub fn generate(
    name: &str,
    commands: &[CommandInfo],
    groups: &BTreeMap<String, String>,
) -> String {
    if groups.is_empty() {
        return commands
            .iter()
            .map(|cmd| render_command_body(name, cmd, 1))
            .collect::<Vec<_>>()
            .join("\n\n");
    }

    let mut sections = vec![format!("# {}", name)];
    let mut last_group: Option<String> = None;

    for cmd in commands {
        let segment = cmd.name.split(' ').next().unwrap_or("");
        if last_group.as_deref() != Some(segment) {
            last_group = Some(segment.to_string());
            let heading = if let Some(desc) = groups.get(segment) {
                format!("## {} {}\n\n{}", name, segment, desc)
            } else {
                format!("## {} {}", name, segment)
            };
            sections.push(heading);
        }
        sections.push(render_command_body(name, cmd, 3));
    }

    sections.join("\n\n")
}

/// Splits commands into multiple skill files grouped by command depth.
///
/// At depth 0, all commands go into a single file. At depth 1, commands are
/// grouped by their first path segment, etc.
pub fn split(
    name: &str,
    commands: &[CommandInfo],
    depth: usize,
    groups: &BTreeMap<String, String>,
) -> Vec<SkillFile> {
    if depth == 0 {
        return vec![SkillFile {
            dir: String::new(),
            content: render_group(name, name, commands, groups, Some(name)),
        }];
    }

    let mut buckets: BTreeMap<String, Vec<&CommandInfo>> = BTreeMap::new();
    for cmd in commands {
        let segments: Vec<&str> = cmd.name.split(' ').collect();
        let key = segments[..depth.min(segments.len())].join("-");
        buckets.entry(key).or_default().push(cmd);
    }

    buckets
        .into_iter()
        .map(|(dir, cmds)| {
            let prefix = cmds[0]
                .name
                .split(' ')
                .take(depth)
                .collect::<Vec<_>>()
                .join(" ");
            let title = format!("{} {}", name, prefix);
            SkillFile {
                dir,
                content: render_group(name, &title, &cmds_to_owned(&cmds), groups, Some(&prefix)),
            }
        })
        .collect()
}

/// Computes a SHA-256 hash of command structure for staleness detection.
///
/// Returns the first 16 hex characters of the hash.
pub fn hash(commands: &[CommandInfo]) -> String {
    let data: Vec<serde_json::Value> = commands
        .iter()
        .map(|cmd| {
            let mut obj = serde_json::Map::new();
            obj.insert("name".to_string(), serde_json::Value::String(cmd.name.clone()));
            if let Some(desc) = &cmd.description {
                obj.insert(
                    "description".to_string(),
                    serde_json::Value::String(desc.clone()),
                );
            }
            if !cmd.args_fields.is_empty() {
                obj.insert("args".to_string(), fields_to_json(&cmd.args_fields));
            }
            if !cmd.env_fields.is_empty() {
                obj.insert("env".to_string(), fields_to_json(&cmd.env_fields));
            }
            if !cmd.options_fields.is_empty() {
                obj.insert("options".to_string(), fields_to_json(&cmd.options_fields));
            }
            if let Some(output) = &cmd.output_schema {
                obj.insert("output".to_string(), output.clone());
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    let json = serde_json::to_string(&data).unwrap_or_default();
    let digest = Sha256::digest(json.as_bytes());
    hex::encode(&digest[..8])
}

// ---------------------------------------------------------------------------
// Internal rendering
// ---------------------------------------------------------------------------

/// Builds a command signature with arg placeholders.
fn build_signature(cli: &str, cmd: &CommandInfo) -> String {
    let base = format!("{} {}", cli, cmd.name);
    if cmd.args_fields.is_empty() {
        return base;
    }
    let arg_names: Vec<String> = cmd
        .args_fields
        .iter()
        .map(|f| {
            if f.required {
                format!("<{}>", f.name)
            } else {
                format!("[{}]", f.name)
            }
        })
        .collect();
    format!("{} {}", base, arg_names.join(" "))
}

/// Renders a group-level frontmatter + command bodies.
fn render_group(
    cli: &str,
    title: &str,
    cmds: &[CommandInfo],
    groups: &BTreeMap<String, String>,
    prefix: Option<&str>,
) -> String {
    let group_desc = prefix.and_then(|p| groups.get(p).map(|s| s.as_str()));
    let child_descs: Vec<&str> = cmds
        .iter()
        .filter_map(|c| c.description.as_deref())
        .collect();

    let mut desc_parts: Vec<String> = Vec::new();
    if let Some(gd) = group_desc {
        desc_parts.push(gd.trim_end_matches('.').to_string());
    }
    if !child_descs.is_empty() {
        desc_parts.push(child_descs.join(", "));
    }

    let description = if desc_parts.is_empty() {
        format!("Run `{} --help` for usage details.", title)
    } else {
        format!(
            "{}. Run `{} --help` for usage details.",
            desc_parts.join(". "),
            title
        )
    };

    let slug = slugify(title);
    let fm = vec![
        "---".to_string(),
        format!("name: {}", slug),
        format!("description: {}", description),
        format!("requires_bin: {}", cli),
        format!("command: {}", title),
        "---".to_string(),
    ];

    let body: Vec<String> = cmds
        .iter()
        .map(|cmd| render_command_body(cli, cmd, 1))
        .collect();

    let fm_str = fm.join("\n");
    let body_str = body.join("\n\n---\n\n");
    format!("{}\n\n{}", fm_str, body_str)
}

/// Renders a command's heading and sections without frontmatter.
fn render_command_body(cli: &str, cmd: &CommandInfo, level: usize) -> String {
    let full_name = format!("{} {}", cli, cmd.name);
    let mut sections: Vec<String> = Vec::new();
    let h = "#".repeat(level);

    let mut heading = format!("{} {}", h, full_name);
    if let Some(desc) = &cmd.description {
        write!(heading, "\n\n{}", desc).unwrap();
    }
    sections.push(heading);

    let sub = "#".repeat(level + 1);

    // Arguments table
    if !cmd.args_fields.is_empty() {
        let mut table = format!(
            "{} Arguments\n\n| Name | Type | Required | Description |\n|------|------|----------|-------------|",
            sub
        );
        for field in &cmd.args_fields {
            let type_name = field.field_type.display_name();
            let req = if field.required { "yes" } else { "no" };
            let desc = field.description.unwrap_or("");
            write!(table, "\n| `{}` | `{}` | {} | {} |", field.name, type_name, req, desc)
                .unwrap();
        }
        sections.push(table);
    }

    // Environment Variables table
    if !cmd.env_fields.is_empty() {
        let mut table = format!(
            "{} Environment Variables\n\n| Name | Type | Required | Default | Description |\n|------|------|----------|---------|-------------|",
            sub
        );
        for field in &cmd.env_fields {
            let type_name = field.field_type.display_name();
            let req = if field.required { "yes" } else { "no" };
            let default_str = field
                .default
                .as_ref()
                .map(|d| format!("`{}`", d))
                .unwrap_or_default();
            let desc = field.description.unwrap_or("");
            write!(
                table,
                "\n| `{}` | `{}` | {} | {} | {} |",
                field.env_name.unwrap_or(field.name),
                type_name,
                req,
                default_str,
                desc
            )
            .unwrap();
        }
        sections.push(table);
    }

    // Options table
    if !cmd.options_fields.is_empty() {
        let mut table = format!(
            "{} Options\n\n| Flag | Type | Default | Description |\n|------|------|---------|-------------|",
            sub
        );
        for field in &cmd.options_fields {
            let type_name = field.field_type.display_name();
            let default_str = field
                .default
                .as_ref()
                .map(|d| format!("`{}`", d))
                .unwrap_or_default();
            let raw_desc = field.description.unwrap_or("");
            let desc = if field.deprecated {
                format!("**Deprecated.** {}", raw_desc)
            } else {
                raw_desc.to_string()
            };
            write!(
                table,
                "\n| `--{}` | `{}` | {} | {} |",
                field.cli_name, type_name, default_str, desc
            )
            .unwrap();
        }
        sections.push(table);
    }

    // Output section
    if let Some(output) = &cmd.output_schema {
        if let Some(table) = schema_to_table(output, "") {
            sections.push(format!("{} Output\n\n{}", sub, table));
        } else {
            let type_name = resolve_type_name(Some(output));
            sections.push(format!("{} Output\n\nType: `{}`", sub, type_name));
        }
    }

    // Examples
    if !cmd.examples.is_empty() {
        let mut lines: Vec<String> = Vec::new();
        for ex in &cmd.examples {
            if let Some(desc) = &ex.description {
                lines.push(format!("# {}", desc));
            }
            lines.push(format!("{} {}", cli, ex.command));
            lines.push(String::new());
        }
        // Remove trailing empty line
        if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        sections.push(format!("{} Examples\n\n```sh\n{}\n```", sub, lines.join("\n")));
    }

    // Hint
    if let Some(hint) = &cmd.hint {
        sections.push(format!("> {}", hint));
    }

    sections.join("\n\n")
}

// ---------------------------------------------------------------------------
// Schema helpers
// ---------------------------------------------------------------------------

/// Renders a JSON Schema object as a Markdown table.
/// Returns `None` for non-object schemas.
fn schema_to_table(schema: &serde_json::Value, prefix: &str) -> Option<String> {
    let obj = schema.as_object()?;
    if obj.get("type").and_then(|t| t.as_str()) != Some("object") {
        return None;
    }
    let properties = obj.get("properties")?.as_object()?;
    if properties.is_empty() {
        return None;
    }
    let required: std::collections::HashSet<&str> = obj
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut rows: Vec<String> = Vec::new();
    for (key, prop) in properties {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };
        let type_name = resolve_type_name(Some(prop));
        let req = if required.contains(key.as_str()) {
            "yes"
        } else {
            "no"
        };
        let desc = prop
            .as_object()
            .and_then(|o| o.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        rows.push(format!("| `{}` | `{}` | {} | {} |", name, type_name, req, desc));

        // Expand nested objects
        if let Some(prop_obj) = prop.as_object() {
            if prop_obj.get("type").and_then(|t| t.as_str()) == Some("object") {
                if prop_obj.contains_key("properties") {
                    if let Some(nested) = schema_to_table(prop, &name) {
                        // Skip header + separator (first 2 lines)
                        for line in nested.lines().skip(2) {
                            rows.push(line.to_string());
                        }
                    }
                }
            }
            // Expand array item objects
            if prop_obj.get("type").and_then(|t| t.as_str()) == Some("array") {
                if let Some(items) = prop_obj.get("items") {
                    if let Some(items_obj) = items.as_object() {
                        if items_obj.get("type").and_then(|t| t.as_str()) == Some("object") {
                            let array_prefix = format!("{}[]", name);
                            if let Some(nested) = schema_to_table(items, &array_prefix) {
                                for line in nested.lines().skip(2) {
                                    rows.push(line.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Some(format!(
        "| Field | Type | Required | Description |\n|-------|------|----------|-------------|\n{}",
        rows.join("\n")
    ))
}

/// Resolves a simple type name from a JSON Schema property.
fn resolve_type_name(prop: Option<&serde_json::Value>) -> String {
    let prop = match prop {
        Some(p) => p,
        None => return "unknown".to_string(),
    };
    if let Some(obj) = prop.as_object() {
        if let Some(type_val) = obj.get("type") {
            if let Some(t) = type_val.as_str() {
                return if t == "integer" {
                    "number".to_string()
                } else {
                    t.to_string()
                };
            }
        }
    }
    "unknown".to_string()
}

/// Converts a title string to a URL slug.
fn slugify(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut last_was_dash = false;

    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            if ch == '-' {
                if !last_was_dash {
                    slug.push('-');
                    last_was_dash = true;
                }
            } else {
                slug.push(ch);
                last_was_dash = false;
            }
        } else {
            if !last_was_dash && !slug.is_empty() {
                slug.push('-');
                last_was_dash = true;
            }
        }
    }

    // Trim leading/trailing dashes
    slug.trim_matches('-').to_string()
}

/// Serializes field metadata to JSON for hashing.
fn fields_to_json(fields: &[FieldMeta]) -> serde_json::Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();

    for field in fields {
        let mut field_obj = serde_json::Map::new();
        field_obj.insert(
            "type".to_string(),
            serde_json::Value::String(field.field_type.display_name()),
        );
        if let Some(desc) = field.description {
            field_obj.insert(
                "description".to_string(),
                serde_json::Value::String(desc.to_string()),
            );
        }
        if let Some(default) = &field.default {
            field_obj.insert("default".to_string(), default.clone());
        }
        props.insert(field.name.to_string(), serde_json::Value::Object(field_obj));
        if field.required {
            required.push(serde_json::Value::String(field.name.to_string()));
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    schema.insert(
        "properties".to_string(),
        serde_json::Value::Object(props),
    );
    if !required.is_empty() {
        schema.insert("required".to_string(), serde_json::Value::Array(required));
    }
    serde_json::Value::Object(schema)
}

/// Helper to convert borrowed CommandInfo refs to owned for render_group.
fn cmds_to_owned(cmds: &[&CommandInfo]) -> Vec<CommandInfo> {
    cmds.iter().map(|c| (*c).clone()).collect()
}

// ---------------------------------------------------------------------------
// Inline hex encoding (avoids `hex` crate dependency)
// ---------------------------------------------------------------------------

mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX_CHARS[(b >> 4) as usize] as char);
            s.push(HEX_CHARS[(b & 0xf) as usize] as char);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldType;

    fn make_field(name: &'static str, ft: FieldType, required: bool) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: crate::schema::to_kebab(name),
            description: None,
            field_type: ft,
            required,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_cmd(name: &str) -> CommandInfo {
        CommandInfo {
            name: name.to_string(),
            description: Some(format!("Does {}", name)),
            args_fields: vec![],
            options_fields: vec![],
            env_fields: vec![],
            hint: None,
            examples: vec![],
            output_schema: None,
        }
    }

    #[test]
    fn test_index_basic() {
        let cmds = vec![make_cmd("deploy"), make_cmd("status")];
        let result = index("mycli", &cmds, Some("A test CLI"));
        assert!(result.contains("# mycli"));
        assert!(result.contains("A test CLI"));
        assert!(result.contains("| `mycli deploy` | Does deploy |"));
        assert!(result.contains("| `mycli status` | Does status |"));
    }

    #[test]
    fn test_build_signature_with_args() {
        let cmd = CommandInfo {
            name: "deploy".to_string(),
            description: None,
            args_fields: vec![
                make_field("target", FieldType::String, true),
                make_field("env", FieldType::String, false),
            ],
            options_fields: vec![],
            env_fields: vec![],
            hint: None,
            examples: vec![],
            output_schema: None,
        };
        let sig = build_signature("mycli", &cmd);
        assert_eq!(sig, "mycli deploy <target> [env]");
    }

    #[test]
    fn test_hash_deterministic() {
        let cmds = vec![make_cmd("deploy"), make_cmd("status")];
        let h1 = hash(&cmds);
        let h2 = hash(&cmds);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn test_hash_changes_on_mutation() {
        let cmds1 = vec![make_cmd("deploy")];
        let cmds2 = vec![make_cmd("deploy"), make_cmd("status")];
        assert_ne!(hash(&cmds1), hash(&cmds2));
    }

    #[test]
    fn test_split_depth_zero() {
        let cmds = vec![make_cmd("deploy"), make_cmd("status")];
        let groups = BTreeMap::new();
        let files = split("mycli", &cmds, 0, &groups);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].dir, "");
    }

    #[test]
    fn test_split_depth_one() {
        let cmds = vec![
            make_cmd("deploy app"),
            make_cmd("deploy config"),
            make_cmd("status check"),
        ];
        let groups = BTreeMap::new();
        let files = split("mycli", &cmds, 1, &groups);
        assert_eq!(files.len(), 2);
        let dirs: Vec<&str> = files.iter().map(|f| f.dir.as_str()).collect();
        assert!(dirs.contains(&"deploy"));
        assert!(dirs.contains(&"status"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("mycli deploy"), "mycli-deploy");
        assert_eq!(slugify("My CLI / Deploy"), "my-cli-deploy");
        assert_eq!(slugify("--edge--case--"), "edge-case");
    }

    #[test]
    fn test_resolve_type_name() {
        let obj = serde_json::json!({"type": "string"});
        assert_eq!(resolve_type_name(Some(&obj)), "string");

        let int = serde_json::json!({"type": "integer"});
        assert_eq!(resolve_type_name(Some(&int)), "number");

        assert_eq!(resolve_type_name(None), "unknown");
    }
}
