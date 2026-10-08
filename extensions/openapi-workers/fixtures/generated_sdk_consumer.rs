use sdk::{Field, IntoJson, JsonValue};
use worker::{Context, Env, Request, Response, Result, event};
fn main() {}
fn run() -> std::result::Result<serde_json::Value, String> {
    let body = sdk::Bounded::try_new(JsonValue::Integer(4))
        .map_err(|e| e.to_string())?
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    let recursive = sdk::RecursiveAll {
        value: 1,
        next: Field::Value(Box::new(sdk::RecursiveAll {
            value: 2,
            next: Field::Missing,
        })),
    }
    .into_json()
    .to_json_string()
    .map_err(|e| e.to_string())?;
    let overlap = sdk::AnyNumeric::Integer(3)
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    let exact = sdk::ExactUnsigned::try_new(JsonValue::Unsigned(u64::MAX))
        .map_err(|e| e.to_string())?
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "body":body,"recursive":recursive,"overlap":overlap,"exact":exact,
        "invalid_low":sdk::Bounded::try_new(JsonValue::Integer(0)).is_err(),
        "invalid_pattern":sdk::PatternChoice::String("ab".into()).into_json().to_json_string().is_err(),
        "invalid_multiple":sdk::PatternChoice::Integer(7).into_json().to_json_string().is_err(),
        "impossible":sdk::Impossible::try_new(JsonValue::Null).is_err(),
        "decimal":sdk::Decimal::try_new(JsonValue::Number(0.3)).is_ok(),
        "invalid_decimal":sdk::Decimal::try_new(JsonValue::Number(0.31)).is_err()
    }))
}
#[event(fetch)]
async fn fetch(_request: Request, _env: Env, _context: Context) -> Result<Response> {
    match run() {
        Ok(value) => Response::from_json(&value),
        Err(error) => Response::error(error, 500),
    }
}
