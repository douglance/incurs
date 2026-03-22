//! Config schema generation for the incur framework.
//!
//! Generates a JSON Schema describing the valid config file structure
//! from the CLI's command tree and root options. This allows editors and
//! validators to provide autocompletion and validation for config files.
//!
//! Ported from `src/internal/configSchema.ts`.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::cli::CommandEntry;
use crate::schema::{FieldMeta, FieldType};

/// Generates a JSON Schema for config files from the command tree.
///
/// The returned schema has the shape:
///
/// ```json
/// {
///   "type": "object",
///   "additionalProperties": false,
///   "properties": {
///     "$schema": { "type": "string" },
///     "options": { ... },
///     "commands": { ... }
///   }
/// }
/// ```
///
/// - `options` is populated from `root_options` field metadata.
/// - `commands` is populated recursively from the command tree, with each
///   leaf command contributing its own `options` sub-object.
pub fn from_command_tree(
    commands: &BTreeMap<String, CommandEntry>,
    root_options: &[FieldMeta],
) -> Value {
    let mut node = build_node(commands, root_options);

    // Insert $schema property at the root level.
    if let Value::Object(ref mut map) = node {
        let props = map
            .entry("properties")
            .or_insert_with(|| json!({}));
        if let Value::Object(props_map) = props {
            props_map.insert("$schema".to_string(), json!({ "type": "string" }));
        }
    }

    node
}

/// Builds a JSON Schema node for a command level.
///
/// Each level can have:
/// - An `options` property (from `FieldMeta` slices)
/// - A `commands` property (from subcommands in the tree)
fn build_node(
    commands: &BTreeMap<String, CommandEntry>,
    options: &[FieldMeta],
) -> Value {
    let mut properties = serde_json::Map::new();

    // Add `options` property from the options schema fields.
    if !options.is_empty() {
        let option_props = fields_to_json_schema_properties(options);
        if !option_props.is_empty() {
            properties.insert(
                "options".to_string(),
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": Value::Object(option_props),
                }),
            );
        }
    }

    // Add `commands` property with subcommand namespaces.
    let mut command_props = serde_json::Map::new();
    for (name, entry) in commands {
        match entry {
            CommandEntry::Group {
                commands: sub_commands,
                ..
            } => {
                // Recurse into group without options (groups don't have their own options).
                command_props.insert(
                    name.clone(),
                    build_node(sub_commands, &[]),
                );
            }
            CommandEntry::Leaf(def) => {
                // Leaf command: use its options_fields.
                command_props.insert(
                    name.clone(),
                    build_node(&BTreeMap::new(), &def.options_fields),
                );
            }
            CommandEntry::FetchGateway { .. } => {
                // Fetch gateways are excluded from config schema (matching TS behavior).
            }
        }
    }

    if !command_props.is_empty() {
        properties.insert(
            "commands".to_string(),
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": Value::Object(command_props),
            }),
        );
    }

    let mut node = serde_json::Map::new();
    node.insert("type".to_string(), json!("object"));
    node.insert("additionalProperties".to_string(), json!(false));
    if !properties.is_empty() {
        node.insert("properties".to_string(), Value::Object(properties));
    }

    Value::Object(node)
}

/// Converts a slice of `FieldMeta` into JSON Schema properties.
///
/// Each field becomes a property with `type`, and optionally `description`
/// and `default`.
fn fields_to_json_schema_properties(fields: &[FieldMeta]) -> serde_json::Map<String, Value> {
    let mut props = serde_json::Map::new();

    for field in fields {
        let mut prop = serde_json::Map::new();

        // Map FieldType to JSON Schema type.
        match &field.field_type {
            FieldType::String => {
                prop.insert("type".to_string(), json!("string"));
            }
            FieldType::Number => {
                prop.insert("type".to_string(), json!("number"));
            }
            FieldType::Boolean => {
                prop.insert("type".to_string(), json!("boolean"));
            }
            FieldType::Array(inner) => {
                prop.insert("type".to_string(), json!("array"));
                prop.insert("items".to_string(), json!({ "type": field_type_to_json_schema_type(inner) }));
            }
            FieldType::Enum(values) => {
                prop.insert("type".to_string(), json!("string"));
                prop.insert(
                    "enum".to_string(),
                    Value::Array(values.iter().map(|v| json!(v)).collect()),
                );
            }
            FieldType::Count => {
                prop.insert("type".to_string(), json!("number"));
            }
            FieldType::Value => {
                // Any JSON value — no type constraint.
            }
        }

        if let Some(desc) = field.description {
            prop.insert("description".to_string(), json!(desc));
        }

        if let Some(ref default) = field.default {
            prop.insert("default".to_string(), default.clone());
        }

        props.insert(field.cli_name.clone(), Value::Object(prop));
    }

    props
}

/// Maps a `FieldType` to its JSON Schema type string.
fn field_type_to_json_schema_type(ft: &FieldType) -> &'static str {
    match ft {
        FieldType::String => "string",
        FieldType::Number | FieldType::Count => "number",
        FieldType::Boolean => "boolean",
        FieldType::Array(_) => "array",
        FieldType::Enum(_) => "string",
        FieldType::Value => "object",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandDef;
    use crate::output::CommandResult;
    use std::sync::Arc;

    /// A no-op handler for tests.
    struct NoopHandler;

    #[async_trait::async_trait]
    impl crate::command::CommandHandler for NoopHandler {
        async fn run(&self, _ctx: crate::command::CommandContext) -> CommandResult {
            CommandResult::Ok {
                data: serde_json::Value::Null,
                cta: None,
            }
        }
    }

    fn make_field(name: &'static str, ft: FieldType) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: crate::schema::to_kebab(name),
            description: None,
            field_type: ft,
            required: false,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_field_with_desc(
        name: &'static str,
        ft: FieldType,
        desc: &'static str,
    ) -> FieldMeta {
        FieldMeta {
            name,
            cli_name: crate::schema::to_kebab(name),
            description: Some(desc),
            field_type: ft,
            required: false,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }
    }

    fn make_leaf(name: &str, options: Vec<FieldMeta>) -> CommandEntry {
        CommandEntry::Leaf(Arc::new(CommandDef {
            name: name.to_string(),
            description: None,
            args_fields: vec![],
            options_fields: options,
            env_fields: vec![],
            aliases: std::collections::HashMap::new(),
            examples: vec![],
            hint: None,
            format: None,
            output_policy: None,
            handler: Box::new(NoopHandler),
            middleware: vec![],
            output_schema: None,
        }))
    }

    #[test]
    fn test_empty_tree_has_schema_property() {
        let commands = BTreeMap::new();
        let schema = from_command_tree(&commands, &[]);

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["$schema"]["type"], "string");
    }

    #[test]
    fn test_root_options_generate_options_property() {
        let commands = BTreeMap::new();
        let root_options = vec![
            make_field_with_desc("verbose", FieldType::Boolean, "Enable verbose output"),
            make_field("timeout", FieldType::Number),
        ];

        let schema = from_command_tree(&commands, &root_options);

        let options = &schema["properties"]["options"];
        assert_eq!(options["type"], "object");
        assert_eq!(options["additionalProperties"], false);
        assert_eq!(options["properties"]["verbose"]["type"], "boolean");
        assert_eq!(
            options["properties"]["verbose"]["description"],
            "Enable verbose output"
        );
        assert_eq!(options["properties"]["timeout"]["type"], "number");
    }

    #[test]
    fn test_leaf_command_generates_command_options() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "deploy".to_string(),
            make_leaf(
                "deploy",
                vec![make_field("environment", FieldType::String)],
            ),
        );

        let schema = from_command_tree(&commands, &[]);

        let deploy = &schema["properties"]["commands"]["properties"]["deploy"];
        assert_eq!(deploy["type"], "object");
        assert_eq!(
            deploy["properties"]["options"]["properties"]["environment"]["type"],
            "string"
        );
    }

    #[test]
    fn test_group_generates_nested_commands() {
        let mut sub_commands = BTreeMap::new();
        sub_commands.insert(
            "get".to_string(),
            make_leaf("get", vec![make_field("id", FieldType::String)]),
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

        let schema = from_command_tree(&commands, &[]);

        let users = &schema["properties"]["commands"]["properties"]["users"];
        assert_eq!(users["type"], "object");
        let get = &users["properties"]["commands"]["properties"]["get"];
        assert_eq!(get["type"], "object");
        assert_eq!(
            get["properties"]["options"]["properties"]["id"]["type"],
            "string"
        );
    }

    #[test]
    fn test_fetch_gateway_excluded() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "api".to_string(),
            CommandEntry::FetchGateway {
                description: Some("API gateway".to_string()),
                base_path: None,
                output_policy: None,
                handler: Arc::new(NoopFetchHandler),
            },
        );

        let schema = from_command_tree(&commands, &[]);

        // No commands property because the only entry is a FetchGateway.
        assert!(schema.get("properties").map_or(true, |p| {
            p.get("commands").is_none()
                || p["commands"]["properties"]
                    .as_object()
                    .map_or(true, |m| m.is_empty())
        }));
    }

    #[test]
    fn test_field_with_default() {
        let commands = BTreeMap::new();
        let root_options = vec![FieldMeta {
            name: "retries",
            cli_name: "retries".to_string(),
            description: Some("Number of retries"),
            field_type: FieldType::Number,
            required: false,
            default: Some(json!(3)),
            alias: None,
            deprecated: false,
            env_name: None,
        }];

        let schema = from_command_tree(&commands, &root_options);

        let retries = &schema["properties"]["options"]["properties"]["retries"];
        assert_eq!(retries["type"], "number");
        assert_eq!(retries["default"], 3);
        assert_eq!(retries["description"], "Number of retries");
    }

    #[test]
    fn test_enum_field() {
        let commands = BTreeMap::new();
        let root_options = vec![make_field(
            "format",
            FieldType::Enum(vec![
                "json".to_string(),
                "yaml".to_string(),
                "toml".to_string(),
            ]),
        )];

        let schema = from_command_tree(&commands, &root_options);

        let format = &schema["properties"]["options"]["properties"]["format"];
        assert_eq!(format["type"], "string");
        assert_eq!(format["enum"], json!(["json", "yaml", "toml"]));
    }

    #[test]
    fn test_array_field() {
        let commands = BTreeMap::new();
        let root_options = vec![make_field(
            "tags",
            FieldType::Array(Box::new(FieldType::String)),
        )];

        let schema = from_command_tree(&commands, &root_options);

        let tags = &schema["properties"]["options"]["properties"]["tags"];
        assert_eq!(tags["type"], "array");
        assert_eq!(tags["items"]["type"], "string");
    }

    /// A no-op fetch handler for tests.
    struct NoopFetchHandler;

    #[async_trait::async_trait]
    impl crate::fetch::FetchHandler for NoopFetchHandler {
        async fn handle(&self, _request: crate::fetch::FetchInput) -> crate::fetch::FetchOutput {
            crate::fetch::FetchOutput {
                ok: true,
                status: 200,
                data: serde_json::Value::Null,
                headers: vec![],
            }
        }
    }
}
