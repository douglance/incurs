//! MCP structured-output projection helpers.
//!
//! The MCP specification requires `structuredContent` and `outputSchema` to be
//! JSON objects. Incurs commands may legitimately return arrays, strings,
//! numbers, booleans, or null. This module provides the shared reversible
//! wrapper used at MCP boundaries while preserving each command's original
//! output schema inside Incurs-native catalogs.

use serde_json::{Map, Value, json};
use thiserror::Error;

const MARKER_KEY: &str = "io.incurs.outputProjection";
const WRAPPER_FIELD: &str = "data";
const WRAPPER_REF_BASE: &str = "#/properties/data";
const PROMOTED_ROOT_KEYWORDS: &[&str] = &["$schema", "$vocabulary"];

const MAP_SCHEMA_KEYWORDS: &[&str] = &[
    "$defs",
    "definitions",
    "properties",
    "patternProperties",
    "dependentSchemas",
];
const ARRAY_SCHEMA_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const SINGLE_SCHEMA_KEYWORDS: &[&str] = &[
    "not",
    "if",
    "then",
    "else",
    "additionalProperties",
    "additionalItems",
    "contains",
    "propertyNames",
    "unevaluatedItems",
    "unevaluatedProperties",
    "contentSchema",
];

/// The MCP-visible shape used for a tool's structured output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpStructuredShape {
    /// The original output schema is explicitly object-shaped.
    Object,
    /// The original output is carried under `data` in an object wrapper.
    WrappedValue,
}

/// Reversible projection of an Incurs output schema into MCP object shape.
#[derive(Clone, Debug, PartialEq)]
pub struct McpOutputProjection {
    /// MCP-visible schema.
    pub schema: Value,
    /// Projection shape required to handle runtime values.
    pub shape: McpStructuredShape,
}

/// Failure while projecting runtime structured content.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum McpOutputProjectionError {
    /// Object-shaped schemas require object-shaped runtime values.
    #[error("object-shaped MCP structured output must be a JSON object")]
    ObjectShapeRequiresObject,
}

/// Projects an Incurs output schema into an MCP-compatible object schema.
///
/// Only a root schema with explicit `"type": "object"` is treated as already
/// object-shaped. Nullable unions, `$ref`, and composed schemas are wrapped so
/// the operation remains conservative and reversible.
pub fn project_output_schema(schema: &Value) -> McpOutputProjection {
    if is_explicit_object_schema(schema) {
        return McpOutputProjection {
            schema: schema.clone(),
            shape: McpStructuredShape::Object,
        };
    }

    let (data_schema, promoted) = promote_anonymous_root_keywords(schema);
    let mut wrapper = Map::new();
    for (key, value) in promoted {
        wrapper.insert(key, value);
    }
    wrapper.insert("type".to_string(), Value::String("object".to_string()));
    wrapper.insert(
        "properties".to_string(),
        json!({ WRAPPER_FIELD: transform_schema_refs(&data_schema, RefProjection::Wrap) }),
    );
    wrapper.insert("required".to_string(), json!([WRAPPER_FIELD]));
    wrapper.insert("additionalProperties".to_string(), Value::Bool(false));

    McpOutputProjection {
        schema: Value::Object(wrapper),
        shape: McpStructuredShape::WrappedValue,
    }
}

/// Returns MCP metadata identifying a reversible output projection.
///
/// Object-shaped outputs need no marker. Wrapped values use an exact private
/// marker so importers never infer from a third-party schema that naturally has
/// a `data` property.
pub fn projection_metadata(shape: McpStructuredShape) -> Option<Map<String, Value>> {
    match shape {
        McpStructuredShape::Object => None,
        McpStructuredShape::WrappedValue => {
            let mut metadata = Map::new();
            metadata.insert(MARKER_KEY.to_string(), projection_marker());
            Some(metadata)
        }
    }
}

/// Projects a runtime structured result using the selected shape.
pub fn project_structured_content(
    value: Value,
    shape: McpStructuredShape,
) -> Result<Value, McpOutputProjectionError> {
    match shape {
        McpStructuredShape::Object if value.is_object() => Ok(value),
        McpStructuredShape::Object => Err(McpOutputProjectionError::ObjectShapeRequiresObject),
        McpStructuredShape::WrappedValue => Ok(json!({ WRAPPER_FIELD: value })),
    }
}

/// Restores an MCP-visible output schema to the original Incurs schema.
///
/// Restoration only happens when the exact private marker is present and the
/// schema has the exact wrapper shape produced by [`project_output_schema`].
/// Otherwise the schema is returned unchanged.
pub fn restore_output_schema(schema: Value, metadata: Option<&Map<String, Value>>) -> Value {
    if !has_projection_marker(metadata) {
        return schema;
    }
    let Some(wrapper) = wrapper_data_schema(&schema) else {
        return schema;
    };
    let mut restored = transform_schema_refs(&wrapper.data_schema, RefProjection::Restore);
    if restore_promoted_root_keywords(&mut restored, wrapper.promoted) {
        restored
    } else {
        schema
    }
}

/// Restores MCP-visible structured content to the original Incurs value.
///
/// Restoration only unwraps the exact marked `{"data": ...}` shape. Unmarked
/// or malformed values are returned unchanged.
pub fn restore_structured_content(value: Value, metadata: Option<&Map<String, Value>>) -> Value {
    if !has_projection_marker(metadata) {
        return value;
    }
    let Some(object) = value.as_object() else {
        return value;
    };
    if object.len() != 1 {
        return value;
    }
    object
        .get(WRAPPER_FIELD)
        .cloned()
        .unwrap_or(Value::Object(object.clone()))
}

fn is_explicit_object_schema(schema: &Value) -> bool {
    schema
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        == Some("object")
}

fn projection_marker() -> Value {
    json!({
        "version": 1,
        "shape": "value-wrapper",
        "field": WRAPPER_FIELD,
        "schemaRefBase": WRAPPER_REF_BASE
    })
}

fn has_projection_marker(metadata: Option<&Map<String, Value>>) -> bool {
    metadata
        .and_then(|metadata| metadata.get(MARKER_KEY))
        .is_some_and(|marker| marker == &projection_marker())
}

struct WrappedSchema {
    data_schema: Value,
    promoted: Vec<(String, Value)>,
}

fn wrapper_data_schema(schema: &Value) -> Option<WrappedSchema> {
    let object = schema.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    if object.get("required") != Some(&json!([WRAPPER_FIELD])) {
        return None;
    }
    if object.get("additionalProperties") != Some(&Value::Bool(false)) {
        return None;
    }
    let properties = object.get("properties")?.as_object()?;
    if properties.len() != 1 {
        return None;
    }
    let data_schema = properties.get(WRAPPER_FIELD).cloned()?;
    let promoted = promoted_root_keywords(object)?;
    let expected_len = 4 + promoted.len();
    if object.len() != expected_len {
        return None;
    }
    Some(WrappedSchema {
        data_schema,
        promoted,
    })
}

fn promote_anonymous_root_keywords(schema: &Value) -> (Value, Vec<(String, Value)>) {
    let Some(object) = schema.as_object() else {
        return (schema.clone(), Vec::new());
    };
    if object.contains_key("$id") {
        return (schema.clone(), Vec::new());
    }

    let mut data = object.clone();
    let mut promoted = Vec::new();
    for key in PROMOTED_ROOT_KEYWORDS {
        if let Some(value) = data.remove(*key) {
            promoted.push(((*key).to_string(), value));
        }
    }
    (Value::Object(data), promoted)
}

fn promoted_root_keywords(object: &Map<String, Value>) -> Option<Vec<(String, Value)>> {
    let mut promoted = Vec::new();
    for key in PROMOTED_ROOT_KEYWORDS {
        if let Some(value) = object.get(*key) {
            promoted.push(((*key).to_string(), value.clone()));
        }
    }
    if promoted.len() > PROMOTED_ROOT_KEYWORDS.len() {
        return None;
    }
    Some(promoted)
}

fn restore_promoted_root_keywords(schema: &mut Value, promoted: Vec<(String, Value)>) -> bool {
    if promoted.is_empty() {
        return true;
    }
    let Some(object) = schema.as_object_mut() else {
        return false;
    };
    if PROMOTED_ROOT_KEYWORDS
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return false;
    }
    for (key, value) in promoted {
        object.insert(key, value);
    }
    true
}

#[derive(Clone, Copy)]
enum RefProjection {
    Wrap,
    Restore,
}

fn transform_schema_refs(value: &Value, projection: RefProjection) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    if object.contains_key("$id") {
        return value.clone();
    }

    let mut transformed = object.clone();
    transform_current_refs(&mut transformed, projection);
    transform_map_schema_keywords(&mut transformed, projection);
    transform_dependencies(&mut transformed, projection);
    transform_array_schema_keywords(&mut transformed, projection);
    transform_items(&mut transformed, projection);
    transform_single_schema_keywords(&mut transformed, projection);
    Value::Object(transformed)
}

fn transform_current_refs(object: &mut Map<String, Value>, projection: RefProjection) {
    for key in ["$ref", "$dynamicRef"] {
        if let Some(reference) = object.get(key).and_then(Value::as_str) {
            let transformed = transform_reference(reference, projection);
            object.insert(key.to_string(), Value::String(transformed));
        }
    }
}

fn transform_map_schema_keywords(object: &mut Map<String, Value>, projection: RefProjection) {
    for key in MAP_SCHEMA_KEYWORDS {
        let Some(entries) = object.get_mut(*key).and_then(Value::as_object_mut) else {
            continue;
        };
        for schema in entries.values_mut() {
            *schema = transform_schema_refs(schema, projection);
        }
    }
}

fn transform_dependencies(object: &mut Map<String, Value>, projection: RefProjection) {
    let Some(entries) = object
        .get_mut("dependencies")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for dependency in entries.values_mut() {
        if dependency.is_object() || dependency.is_boolean() {
            *dependency = transform_schema_refs(dependency, projection);
        }
    }
}

fn transform_array_schema_keywords(object: &mut Map<String, Value>, projection: RefProjection) {
    for key in ARRAY_SCHEMA_KEYWORDS {
        let Some(items) = object.get_mut(*key).and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            *item = transform_schema_refs(item, projection);
        }
    }
}

fn transform_items(object: &mut Map<String, Value>, projection: RefProjection) {
    let Some(items) = object.get_mut("items") else {
        return;
    };
    if let Some(tuple_items) = items.as_array_mut() {
        for item in tuple_items {
            *item = transform_schema_refs(item, projection);
        }
    } else {
        *items = transform_schema_refs(items, projection);
    }
}

fn transform_single_schema_keywords(object: &mut Map<String, Value>, projection: RefProjection) {
    for key in SINGLE_SCHEMA_KEYWORDS {
        let Some(schema) = object.get_mut(*key) else {
            continue;
        };
        *schema = transform_schema_refs(schema, projection);
    }
}

fn transform_reference(reference: &str, projection: RefProjection) -> String {
    match projection {
        RefProjection::Wrap => wrap_reference(reference),
        RefProjection::Restore => restore_reference(reference),
    }
}

fn wrap_reference(reference: &str) -> String {
    if reference == "#" {
        return WRAPPER_REF_BASE.to_string();
    }
    reference.strip_prefix("#/").map_or_else(
        || reference.to_string(),
        |path| format!("{WRAPPER_REF_BASE}/{path}"),
    )
}

fn restore_reference(reference: &str) -> String {
    if reference == WRAPPER_REF_BASE {
        return "#".to_string();
    }
    reference
        .strip_prefix("#/properties/data/")
        .map_or_else(|| reference.to_string(), |path| format!("#/{path}"))
}
