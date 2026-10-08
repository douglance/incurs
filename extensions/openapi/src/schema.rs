//! Reference lookup and shallow metadata views for HTTP serialization.
use crate::{OpenApiError, OpenApiResult};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

pub(crate) fn reference_name(reference: &str) -> OpenApiResult<String> {
    let name = reference
        .strip_prefix("#/components/schemas/")
        .ok_or_else(|| OpenApiError(format!("non-canonical schema reference {reference}")))?;
    if name.contains('/') {
        return Err(OpenApiError(format!(
            "schema reference must identify a registered definition: {reference}"
        )));
    }
    Ok(name.replace("~1", "/").replace("~0", "~"))
}

pub(crate) fn target<'a>(
    reference: &str,
    schemas: &'a BTreeMap<String, Value>,
) -> OpenApiResult<&'a Value> {
    let name = reference_name(reference)?;
    schemas
        .get(&name)
        .ok_or_else(|| OpenApiError(format!("schema reference target not found: {reference}")))
}

// Resolve aliases at this position only. Nested references stay shared. This
// view reads serialization metadata; validators must retain the original graph.
pub(crate) fn binding_view<'a>(
    schema: &'a Value,
    schemas: &'a BTreeMap<String, Value>,
) -> OpenApiResult<Cow<'a, Value>> {
    fn visit<'a>(
        schema: &'a Value,
        schemas: &'a BTreeMap<String, Value>,
        active: &mut BTreeSet<String>,
    ) -> OpenApiResult<Cow<'a, Value>> {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            return Ok(Cow::Borrowed(schema));
        };
        if !active.insert(reference.to_owned()) {
            return Err(OpenApiError(format!(
                "cyclic schema alias has no serialization shape: {reference}"
            )));
        }
        let base = visit(target(reference, schemas)?, schemas, active)?;
        active.remove(reference);
        let fields = schema.as_object().unwrap();
        if fields.len() == 1 || base.as_ref() == &Value::Bool(false) {
            return Ok(base);
        }
        let mut merged = base.as_object().cloned().unwrap_or_default();
        for (key, value) in fields {
            if key != "$ref" {
                merged.insert(key.clone(), value.clone());
            }
        }
        Ok(Cow::Owned(Value::Object(merged)))
    }
    visit(schema, schemas, &mut BTreeSet::new())
}

pub(crate) fn references(schema: &Value, out: &mut BTreeSet<String>) -> OpenApiResult<()> {
    let Some(fields) = schema.as_object() else {
        return Ok(());
    };
    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
        out.insert(reference_name(reference)?);
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(children) = fields.get(key).and_then(Value::as_object) {
            for child in children.values() {
                references(child, out)?;
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "contains",
        "propertyNames",
        "if",
        "then",
        "else",
        "unevaluatedProperties",
        "unevaluatedItems",
        "additionalItems",
    ] {
        if let Some(child) = fields.get(key) {
            references(child, out)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = fields.get(key).and_then(Value::as_array) {
            for child in children {
                references(child, out)?;
            }
        }
    }
    Ok(())
}
