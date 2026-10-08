use sdk::{Field, IntoJson, JsonValue};
use worker::{Context, Env, Request, Response, Result, event};

fn main() {}

fn run() -> std::result::Result<serde_json::Value, String> {
    let scalar = sdk::Loose::try_new(JsonValue::String("scalar".into()))
        .map_err(|e| e.to_string())?
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    let object = sdk::Loose::try_new(JsonValue::Object(vec![(
        "name".into(),
        JsonValue::String("ok".into()),
    )]))
    .map_err(|e| e.to_string())?
    .into_json()
    .to_json_string()
    .map_err(|e| e.to_string())?;
    let nested = sdk::Nested {
        value: sdk::NestedValue::try_new(JsonValue::Bool(true)).map_err(|e| e.to_string())?,
    }
    .into_json()
    .to_json_string()
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "scalar": scalar, "object": object, "nested": nested,
        "allow_null": sdk::Loose::try_new(JsonValue::Null).is_ok(),
        "reject_empty": sdk::Loose::try_new(JsonValue::Object(vec![])).is_err(),
        "reject_name": sdk::Loose::try_new(JsonValue::Object(vec![("name".into(), JsonValue::Integer(1))])).is_err(),
        "reject_choice_null": sdk::Choice::try_new(JsonValue::Null).is_err(),
        "reject_parent_null": sdk::Envelope { value: Field::Null }.into_json().to_json_string().is_err(),
        "reject_parent_default": sdk::Envelope { value: Field::Default(JsonValue::Integer(8)) }.into_json().to_json_string().is_err(),
        "numeric_string_allowed": sdk::Minimum::try_new(JsonValue::String("text".into())).is_ok(),
        "reject_low_number": sdk::Minimum::try_new(JsonValue::Integer(1)).is_err()
    }))
}

#[event(fetch)]
async fn fetch(_request: Request, _env: Env, _context: Context) -> Result<Response> {
    match run() {
        Ok(value) => Response::from_json(&value),
        Err(error) => Response::error(error, 500),
    }
}
