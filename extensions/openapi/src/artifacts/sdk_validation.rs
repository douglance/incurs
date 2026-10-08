//! One shared validation graph for generated composition types.
pub(super) use crate::schema_validation::normalize;
use crate::schema_validation::normalize_schema;
use crate::{OpenApiError, OpenApiResult, ResolvedOpenApi};
use serde_json::{Map, Value, json};

pub(super) fn key(name: &str) -> String {
    format!("#/$defs/{}", name.replace('~', "~0").replace('/', "~1"))
}

pub(super) fn graph(contract: &ResolvedOpenApi) -> OpenApiResult<Value> {
    let mut definitions = Map::new();
    let mut properties = Map::new();
    for (name, schema) in &contract.schemas {
        let pointer = key(name);
        definitions.insert(name.clone(), normalize_schema(schema, false, true)?);
        properties.insert(pointer.clone(), json!({"$ref":pointer}));
        for keyword in ["oneOf", "anyOf"] {
            if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
                for index in 0..branches.len() {
                    let branch = format!("{pointer}/{keyword}/{index}");
                    properties.insert(branch.clone(), json!({"$ref":branch}));
                }
            }
        }
    }
    let graph = json!({"type":"object","properties":properties,"additionalProperties":false,"$defs":definitions});
    jsonschema::draft202012::options()
        .offline()
        .should_validate_formats(false)
        .build(&graph)
        .map_err(|error| {
            OpenApiError(format!("invalid generated SDK validation graph: {error}"))
        })?;
    Ok(graph)
}

pub(super) const RUNTIME: &str = r#"
std::thread_local! {
    static SCHEMA_VALIDATOR: Result<jsonschema::Validator, String> = {
        serde_json::from_str::<serde_json::Value>(include_str!("schemas.json"))
            .map_err(|error| error.to_string())
            .and_then(|schema| jsonschema::draft202012::options().offline()
                .should_validate_formats(false).build(&schema).map_err(|error| error.to_string()))
    };
}
fn validation_value(value: &JsonValue) -> Option<serde_json::Value> {
    use serde_json::Value;
    match value {
        JsonValue::Invalid | JsonValue::Bytes(_) => None,
        JsonValue::Null => Some(Value::Null),
        JsonValue::Bool(value) => Some(Value::Bool(*value)),
        JsonValue::String(value) => Some(Value::String(value.clone())),
        JsonValue::Integer(value) => Some(Value::Number((*value).into())),
        JsonValue::Unsigned(value) => Some(Value::Number((*value).into())),
        JsonValue::Number(value) => serde_json::Number::from_f64(*value).map(Value::Number),
        JsonValue::ExactNumber(value) if valid_json_number(value) => value.parse().ok().map(Value::Number),
        JsonValue::ExactNumber(_) => None,
        JsonValue::Array(values) => values.iter().map(validation_value).collect::<Option<Vec<_>>>().map(Value::Array),
        JsonValue::Object(fields) => {
            let mut values = serde_json::Map::new();
            for (key, value) in fields {
                if values.insert(key.clone(), validation_value(value)?).is_some() { return None; }
            }
            Some(Value::Object(values))
        }
    }
}
fn json_matches(value: &JsonValue, pointer: &str) -> bool {
    let Some(value) = validation_value(value) else { return false; };
    let instance = serde_json::Value::Object([(pointer.to_owned(), value)].into_iter().collect());
    SCHEMA_VALIDATOR.with(|validator| validator.as_ref().is_ok_and(|validator| validator.is_valid(&instance)))
}
"#;
