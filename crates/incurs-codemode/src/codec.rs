use base64::Engine;
use serde_json::{Map, Value, json};

const TAG: &str = "__codemode_type";

/// A value that preserves JavaScript-only primitives across JSON and RPC.
#[derive(Debug, Clone, PartialEq)]
pub enum CodeValue {
    /// JavaScript `undefined`.
    Undefined,
    /// A regular JSON value.
    Json(Value),
    /// An arbitrary-precision integer represented in base 10.
    BigInt(String),
    /// Raw bytes.
    Binary(Vec<u8>),
}

/// Encodes a Code Mode value into a JSON-safe tagged representation.
pub fn encode_value(value: &CodeValue) -> Value {
    match value {
        CodeValue::Undefined => json!({ TAG: "undefined" }),
        CodeValue::Json(value) => encode_json(value),
        CodeValue::BigInt(value) => json!({ TAG: "bigint", "value": value }),
        CodeValue::Binary(value) => json!({
            TAG: "binary",
            "value": base64::engine::general_purpose::STANDARD.encode(value),
        }),
    }
}

/// Decodes a JSON-safe tagged representation into a Code Mode value.
pub fn decode_value(value: Value) -> Result<CodeValue, String> {
    if let Value::Object(map) = &value
        && let Some(Value::String(kind)) = map.get(TAG)
    {
        return match kind.as_str() {
            "undefined" => Ok(CodeValue::Undefined),
            "bigint" => string_value(map).map(CodeValue::BigInt),
            "binary" => base64::engine::general_purpose::STANDARD
                .decode(string_value(map)?)
                .map(CodeValue::Binary)
                .map_err(|error| error.to_string()),
            _ => Ok(CodeValue::Json(decode_json(value)?)),
        };
    }
    Ok(CodeValue::Json(decode_json(value)?))
}

fn string_value(map: &Map<String, Value>) -> Result<String, String> {
    map.get("value")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Tagged Code Mode value is missing a string value".to_string())
}

fn encode_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(encode_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), encode_json(value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn decode_json(value: Value) -> Result<Value, String> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(decode_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| decode_json(value).map(|value| (key, value)))
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        value => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_extended_values() {
        for value in [
            CodeValue::Undefined,
            CodeValue::BigInt("9007199254740993".to_string()),
            CodeValue::Binary(vec![0, 1, 255]),
            CodeValue::Json(json!({"ok": [true, null]})),
        ] {
            assert_eq!(decode_value(encode_value(&value)).unwrap(), value);
        }
    }
}
