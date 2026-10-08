//! Version-aware schema normalization shared by validation consumers.
use crate::{OpenApiError, OpenApiResult, ResolvedOpenApi};
use serde_json::{Value, json};

pub(crate) fn normalize(contract: &ResolvedOpenApi) -> OpenApiResult<ResolvedOpenApi> {
    if let Some(dialect) = &contract.json_schema_dialect {
        check_dialect(dialect)?;
    }
    let legacy = contract.openapi_version.starts_with("3.0.");
    let mut out = contract.clone();
    for schema in out.schemas.values_mut() {
        *schema = normalize_schema(schema, legacy, false)?;
    }
    for operation in &mut out.operations {
        for parameter in &mut operation.parameters {
            parameter.schema = normalize_schema(&parameter.schema, legacy, false)?;
        }
        if let Some(body) = &mut operation.request_body {
            for schema in body.content.values_mut() {
                *schema = normalize_schema(schema, legacy, false)?;
            }
        }
    }
    Ok(out)
}

fn check_dialect(dialect: &str) -> OpenApiResult<()> {
    if matches!(
        dialect.trim_end_matches('#'),
        "https://json-schema.org/draft/2020-12/schema"
            | "https://spec.openapis.org/oas/3.1/dialect/base"
            | "https://spec.openapis.org/oas/3.2/dialect/base"
    ) {
        Ok(())
    } else {
        Err(OpenApiError(format!(
            "unsupported schema dialect: {dialect}"
        )))
    }
}

pub(crate) fn normalize_schema(schema: &Value, legacy: bool, graph: bool) -> OpenApiResult<Value> {
    let Some(fields) = schema.as_object() else {
        return Ok(schema.clone());
    };
    let mut out = fields.clone();
    if let Some(dialect) = out.get("$schema").and_then(Value::as_str) {
        check_dialect(dialect)?;
        out.remove("$schema");
    }
    for key in ["$id", "$anchor", "$dynamicAnchor", "$dynamicRef"] {
        if out.contains_key(key) {
            return Err(OpenApiError(format!(
                "schema resource keyword is unsupported: {key}"
            )));
        }
    }
    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
        if legacy {
            out.retain(|key, _| key == "$ref");
        }
        if graph {
            let suffix = reference
                .strip_prefix("#/components/schemas/")
                .ok_or_else(|| {
                    OpenApiError(format!("noncanonical schema reference: {reference}"))
                })?;
            out.insert("$ref".into(), json!(format!("#/$defs/{suffix}")));
        }
    }
    if legacy {
        if out.remove("nullable") == Some(Value::Bool(true))
            && let Some(Value::String(kind)) = out.get("type")
        {
            out.insert("type".into(), json!([kind, "null"]));
        }
        for (exclusive, inclusive) in [
            ("exclusiveMinimum", "minimum"),
            ("exclusiveMaximum", "maximum"),
        ] {
            if let Some(flag) = out.get(exclusive).and_then(Value::as_bool) {
                out.remove(exclusive);
                if flag && let Some(bound) = out.remove(inclusive) {
                    out.insert(exclusive.into(), bound);
                }
            }
        }
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(children)) = out.get_mut(key) {
            for child in children.values_mut() {
                *child = normalize_schema(child, legacy, graph)?;
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = out.get_mut(key) {
            *child = normalize_schema(child, legacy, graph)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = out.get_mut(key) {
            for child in children {
                *child = normalize_schema(child, legacy, graph)?;
            }
        }
    }
    Ok(Value::Object(out))
}
