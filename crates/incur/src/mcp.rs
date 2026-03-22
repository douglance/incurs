//! MCP (Model Context Protocol) stdio server.
//!
//! Ported from `src/Mcp.ts`. Exposes CLI commands as MCP tools over a stdio
//! transport. The actual server implementation uses the `rmcp` crate and is
//! gated behind the `mcp` feature flag.

use std::collections::BTreeMap;

use crate::schema::FieldMeta;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A resolved tool entry from the command tree.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    /// Tool name (path segments joined with `_`).
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Merged JSON Schema for the tool's input.
    pub input_schema: serde_json::Value,
    /// JSON Schema for the tool's output.
    pub output_schema: Option<serde_json::Value>,
}

/// A command entry in the command tree for MCP tool collection.
///
/// This is a minimal representation to avoid depending on the `command` module
/// which is written by another engineer in parallel.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Whether this entry is a group (has subcommands).
    pub is_group: bool,
    /// Human-readable description.
    pub description: Option<String>,
    /// Subcommands (only populated for groups).
    pub commands: BTreeMap<String, CommandEntry>,
    /// Positional argument field metadata.
    pub args_fields: Vec<FieldMeta>,
    /// Named option field metadata.
    pub options_fields: Vec<FieldMeta>,
    /// JSON Schema for the command's output.
    pub output_schema: Option<serde_json::Value>,
}

/// Options for the MCP server.
#[cfg(feature = "mcp")]
#[derive(Debug, Clone, Default)]
pub struct McpServeOptions {
    /// CLI version string.
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool collection
// ---------------------------------------------------------------------------

/// Recursively collects leaf commands as MCP tool entries.
///
/// Groups are traversed but not emitted as tools — only leaf commands become
/// tools. Tool names use underscore-joined path segments (e.g. `deploy_app`).
pub fn collect_tools(
    commands: &BTreeMap<String, CommandEntry>,
    prefix: &[String],
) -> Vec<ToolEntry> {
    let mut result: Vec<ToolEntry> = Vec::new();

    for (name, entry) in commands {
        let mut path = prefix.to_vec();
        path.push(name.clone());

        if entry.is_group {
            result.extend(collect_tools(&entry.commands, &path));
        } else {
            let tool_name = path.join("_");
            let input_schema = build_tool_schema(&entry.args_fields, &entry.options_fields);
            result.push(ToolEntry {
                name: tool_name,
                description: entry.description.clone(),
                input_schema,
                output_schema: entry.output_schema.clone(),
            });
        }
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Builds a merged JSON Schema from args and options field metadata.
fn build_tool_schema(
    args_fields: &[FieldMeta],
    options_fields: &[FieldMeta],
) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<String> = Vec::new();

    for field in args_fields.iter().chain(options_fields.iter()) {
        let mut prop = serde_json::Map::new();
        prop.insert(
            "type".to_string(),
            serde_json::Value::String(field_type_to_json_type(&field.field_type)),
        );
        if let Some(desc) = field.description {
            prop.insert(
                "description".to_string(),
                serde_json::Value::String(desc.to_string()),
            );
        }
        if let Some(default) = &field.default {
            prop.insert("default".to_string(), default.clone());
        }
        properties.insert(field.name.to_string(), serde_json::Value::Object(prop));

        if field.required {
            required.push(field.name.to_string());
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    schema.insert(
        "properties".to_string(),
        serde_json::Value::Object(properties),
    );
    if !required.is_empty() {
        schema.insert(
            "required".to_string(),
            serde_json::Value::Array(
                required
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }

    serde_json::Value::Object(schema)
}

/// Maps a FieldType to its JSON Schema type string.
fn field_type_to_json_type(ft: &crate::schema::FieldType) -> String {
    use crate::schema::FieldType;
    match ft {
        FieldType::String => "string".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array(_) => "array".to_string(),
        FieldType::Enum(_) => "string".to_string(),
        FieldType::Count => "number".to_string(),
        FieldType::Value => "string".to_string(),
    }
}

// ---------------------------------------------------------------------------
// MCP Server (behind feature flag)
// ---------------------------------------------------------------------------

/// Starts a stdio MCP server that exposes commands as tools.
///
/// Uses the `rmcp` crate for the actual MCP protocol implementation.
/// Each leaf command in the command tree becomes an MCP tool.
#[cfg(feature = "mcp")]
pub async fn serve(
    name: &str,
    version: &str,
    commands: &BTreeMap<String, CommandEntry>,
    _options: &McpServeOptions,
) -> Result<(), crate::errors::Error> {
    // TODO: Implement rmcp-based MCP server.
    //
    // The implementation should:
    // 1. Create an rmcp Server with the given name and version.
    // 2. Register each tool from collect_tools() with the server.
    // 3. For each tool call, deserialize the input params, execute the
    //    command, and return the result as tool output.
    // 4. Connect using a stdio transport (stdin/stdout).
    //
    // This is deferred because rmcp's API is still stabilizing and the
    // integration requires the full command execution pipeline from
    // the `command` module.
    let _ = (name, version, commands);
    Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "MCP server not yet implemented for Rust — use the TypeScript version",
    ))))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{to_kebab, FieldType};

    fn make_field(name: &'static str, ft: FieldType, required: bool) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: to_kebab(name),
            description: Some("A field"),
            field_type: ft,
            required,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_leaf(desc: &str) -> CommandEntry {
        CommandEntry {
            is_group: false,
            description: Some(desc.to_string()),
            commands: BTreeMap::new(),
            args_fields: vec![],
            options_fields: vec![],
            output_schema: None,
        }
    }

    fn make_group(desc: &str, commands: BTreeMap<String, CommandEntry>) -> CommandEntry {
        CommandEntry {
            is_group: true,
            description: Some(desc.to_string()),
            commands,
            args_fields: vec![],
            options_fields: vec![],
            output_schema: None,
        }
    }

    #[test]
    fn test_collect_tools_flat() {
        let mut commands = BTreeMap::new();
        commands.insert("deploy".to_string(), make_leaf("Deploy app"));
        commands.insert("status".to_string(), make_leaf("Show status"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "deploy");
        assert_eq!(tools[1].name, "status");
    }

    #[test]
    fn test_collect_tools_nested() {
        let mut sub = BTreeMap::new();
        sub.insert("app".to_string(), make_leaf("Deploy app"));
        sub.insert("config".to_string(), make_leaf("Deploy config"));

        let mut commands = BTreeMap::new();
        commands.insert("deploy".to_string(), make_group("Deploy group", sub));
        commands.insert("status".to_string(), make_leaf("Show status"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "deploy_app");
        assert_eq!(tools[1].name, "deploy_config");
        assert_eq!(tools[2].name, "status");
    }

    #[test]
    fn test_collect_tools_sorted() {
        let mut commands = BTreeMap::new();
        commands.insert("zebra".to_string(), make_leaf("Z"));
        commands.insert("alpha".to_string(), make_leaf("A"));

        let tools = collect_tools(&commands, &[]);
        assert_eq!(tools[0].name, "alpha");
        assert_eq!(tools[1].name, "zebra");
    }

    #[test]
    fn test_build_tool_schema_basic() {
        let args = vec![make_field("target", FieldType::String, true)];
        let opts = vec![make_field("verbose", FieldType::Boolean, false)];

        let schema = build_tool_schema(&args, &opts);
        let obj = schema.as_object().unwrap();
        assert_eq!(obj["type"], "object");

        let props = obj["properties"].as_object().unwrap();
        assert!(props.contains_key("target"));
        assert!(props.contains_key("verbose"));

        let required = obj["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "target");
    }

    #[test]
    fn test_build_tool_schema_no_required() {
        let schema = build_tool_schema(&[], &[make_field("verbose", FieldType::Boolean, false)]);
        let obj = schema.as_object().unwrap();
        assert!(!obj.contains_key("required"));
    }

    #[test]
    fn test_field_type_to_json_type() {
        assert_eq!(field_type_to_json_type(&FieldType::String), "string");
        assert_eq!(field_type_to_json_type(&FieldType::Number), "number");
        assert_eq!(field_type_to_json_type(&FieldType::Boolean), "boolean");
        assert_eq!(
            field_type_to_json_type(&FieldType::Array(Box::new(FieldType::String))),
            "array"
        );
        assert_eq!(
            field_type_to_json_type(&FieldType::Enum(vec!["a".to_string()])),
            "string"
        );
        assert_eq!(field_type_to_json_type(&FieldType::Count), "number");
    }
}
