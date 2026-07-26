use std::collections::BTreeSet;

use serde_json::Value;

use crate::ConnectorDescription;

/// Makes an arbitrary tool name safe for use as a JavaScript identifier.
pub fn sanitize_identifier(value: &str) -> String {
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        if (index == 0 && !(ch == '_' || ch == '$' || ch.is_ascii_alphabetic()))
            || (index > 0 && !(ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
        {
            result.push('_');
        } else {
            result.push(ch);
        }
    }
    if result.is_empty() {
        "_".to_string()
    } else {
        result
    }
}

/// Converts a JSON Schema value into a model-facing TypeScript type.
pub fn json_schema_to_type(schema: &Value) -> String {
    convert(schema, schema, 0, &mut BTreeSet::new())
}

/// Generates the TypeScript declarations shown to the model for a connector.
pub fn generate_types(description: &ConnectorDescription) -> String {
    let mut result = String::new();
    if let Some(instructions) = &description.instructions {
        result.push_str(instructions);
        result.push_str("\n\n");
    }
    for tool in &description.tools {
        let type_name = pascal(&tool.name);
        result.push_str(&format!(
            "type {type_name}Input = {};\n",
            json_schema_to_type(&tool.input_schema)
        ));
        if let Some(output) = &tool.output_schema {
            result.push_str(&format!(
                "type {type_name}Output = {};\n",
                json_schema_to_type(output)
            ));
        }
    }
    result.push_str(&format!(
        "declare const {}: {{\n",
        sanitize_identifier(&description.name)
    ));
    for tool in &description.tools {
        if let Some(doc) = &tool.description {
            result.push_str(&format!("  /** {} */\n", escape_doc(doc)));
        }
        let type_name = pascal(&tool.name);
        let output = tool
            .output_schema
            .as_ref()
            .map(|_| format!("{type_name}Output"))
            .unwrap_or_else(|| "unknown".to_string());
        result.push_str(&format!(
            "  {}(args: {type_name}Input): Promise<{output}>;\n",
            quote_property(&tool.name)
        ));
    }
    result.push_str("};");
    result
}

fn convert(schema: &Value, root: &Value, depth: usize, seen: &mut BTreeSet<String>) -> String {
    if depth >= 20 {
        return "unknown".to_string();
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if !seen.insert(reference.to_string()) {
            return "unknown".to_string();
        }
        let resolved = resolve_ref(root, reference)
            .map(|value| convert(value, root, depth + 1, seen))
            .unwrap_or_else(|| "unknown".to_string());
        seen.remove(reference);
        return nullable(resolved, schema);
    }
    for (key, separator) in [("anyOf", " | "), ("oneOf", " | "), ("allOf", " & ")] {
        if let Some(values) = schema.get(key).and_then(Value::as_array) {
            let value = values
                .iter()
                .map(|value| convert(value, root, depth + 1, seen))
                .collect::<Vec<_>>()
                .join(separator);
            return nullable(value, schema);
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return nullable(
            values.iter().map(literal).collect::<Vec<_>>().join(" | "),
            schema,
        );
    }
    if let Some(value) = schema.get("const") {
        return nullable(literal(value), schema);
    }
    let value = match schema.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .map(|value| primitive(value.as_str().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(" | "),
        Some(Value::String(kind)) if kind == "array" => {
            if let Some(items) = schema.get("prefixItems").and_then(Value::as_array) {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .map(|item| convert(item, root, depth + 1, seen))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else if let Some(items) = schema.get("items").and_then(Value::as_array) {
                format!(
                    "[{}]",
                    items
                        .iter()
                        .map(|item| convert(item, root, depth + 1, seen))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                let item = schema
                    .get("items")
                    .map(|item| convert(item, root, depth + 1, seen))
                    .unwrap_or_else(|| "unknown".to_string());
                format!("({item})[]")
            }
        }
        Some(Value::String(kind)) if kind == "object" || schema.get("properties").is_some() => {
            object_type(schema, root, depth, seen)
        }
        Some(Value::String(kind)) => primitive(kind).to_string(),
        _ => "unknown".to_string(),
    };
    nullable(value, schema)
}

fn object_type(schema: &Value, root: &Value, depth: usize, seen: &mut BTreeSet<String>) -> String {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut fields = Vec::new();
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, value) in properties {
            let optional = if required.contains(name.as_str()) {
                ""
            } else {
                "?"
            };
            fields.push(format!(
                "  {}{optional}: {};",
                quote_property(name),
                convert(value, root, depth + 1, seen)
            ));
        }
    }
    if let Some(additional) = schema.get("additionalProperties") {
        match additional {
            Value::Bool(true) => fields.push("  [key: string]: unknown;".to_string()),
            Value::Object(_) => fields.push(format!(
                "  [key: string]: {};",
                convert(additional, root, depth + 1, seen)
            )),
            _ => {}
        }
    }
    if fields.is_empty() {
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            "{}".to_string()
        } else {
            "Record<string, unknown>".to_string()
        }
    } else {
        format!("{{\n{}\n}}", fields.join("\n"))
    }
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference == "#" {
        return Some(root);
    }
    let pointer = reference.strip_prefix('#')?;
    root.pointer(pointer)
}

fn nullable(value: String, schema: &Value) -> String {
    if schema.get("nullable").and_then(Value::as_bool) == Some(true)
        && value != "unknown"
        && value != "never"
    {
        format!("{value} | null")
    } else {
        value
    }
}

fn primitive(kind: &str) -> &'static str {
    match kind {
        "string" => "string",
        "number" | "integer" => "number",
        "boolean" => "boolean",
        "null" => "null",
        "array" => "unknown[]",
        "object" => "Record<string, unknown>",
        _ => "unknown",
    }
}

fn literal(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
}

fn pascal(value: &str) -> String {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<String>()
}

fn quote_property(value: &str) -> String {
    let sanitized = sanitize_identifier(value);
    if sanitized == value {
        value.to_string()
    } else {
        serde_json::to_string(value).unwrap()
    }
}

fn escape_doc(value: &str) -> String {
    value.replace("*/", "*\\/").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn converts_objects_unions_and_refs() {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "state": {"enum": ["open", "closed"]},
                "owner": {"$ref": "#/$defs/user"}
            },
            "required": ["id"],
            "$defs": {"user": {"type": "object", "properties": {"name": {"type": "string"}}}}
        });
        let output = json_schema_to_type(&schema);
        assert!(output.starts_with("{\n"));
        assert!(output.contains("  id: number;"));
        assert!(output.contains("  state?: \"open\" | \"closed\";"));
        assert!(output.contains("  owner?: {\n  name?: string;\n};"));
    }
}
