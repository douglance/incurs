//! Generated response decoding runtime emitted into Forge SDKs.

pub(super) const RUNTIME: &str = r#"
const RESPONSE_JSON_MAX_DEPTH: usize = 256;

/// Response decoding failure category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseDecodeErrorKind {
    /// The response media type cannot be decoded by the selected response branch.
    MediaType,
    /// The response body presence or representation is invalid for the selected response branch.
    Body,
    /// The response body is not a valid JSON document for JSON response decoding.
    Json,
    /// The decoded response body does not match the selected response schema.
    Schema,
    /// The decoded JSON value cannot be projected into the generated response type.
    Projection,
}

/// Error returned when a response cannot be decoded into its generated response type.
#[derive(Clone, Debug, PartialEq)]
pub struct ResponseDecodeError {
    /// Failure category.
    pub kind: ResponseDecodeErrorKind,
    /// Human-readable failure detail.
    pub message: String,
    /// Response media type used for decoding, when known.
    pub media_type: Option<String>,
    /// Original transport response that failed to decode.
    pub raw: OperationResponse,
}

impl std::fmt::Display for ResponseDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "response decode error: {}", self.message)
    }
}

impl std::error::Error for ResponseDecodeError {}

/// Error returned by generated client calls.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientError<E> {
    /// Transport layer failed before a response could be decoded.
    Transport(E),
    /// Response was received but could not be decoded into the generated response type.
    Decode(ResponseDecodeError),
}

impl<E: std::fmt::Display> std::fmt::Display for ClientError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::Decode(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl<E> std::error::Error for ClientError<E>
where
    E: std::fmt::Debug + std::fmt::Display + std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Decode(error) => Some(error),
        }
    }
}

/// Response decoded into a generated body type while retaining the raw response.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedResponse<T> {
    /// Original transport response.
    pub raw: OperationResponse,
    /// Response media type used for decoding, when known.
    pub media_type: Option<String>,
    /// Decoded body with missing, null, and value states preserved.
    pub body: Field<T>,
}

/// JSON number token retained without floating-point conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseNumber(String);

impl ResponseNumber {
    /// Create a response number from JSON number text.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if valid_json_number(&value) {
            Ok(Self(value))
        } else {
            Err("invalid JSON number".to_string())
        }
    }

    /// Return the original JSON number text.
    pub fn as_str(&self) -> &str { &self.0 }

    /// Return this number as i64 when it is integral and in range.
    pub fn as_i64(&self) -> Option<i64> {
        response_integer_decimal_string(&self.0, 20)?.parse().ok()
    }

    /// Return this number as u64 when it is integral and in range.
    pub fn as_u64(&self) -> Option<u64> {
        response_integer_decimal_string(&self.0, 20)?.parse().ok()
    }

    /// Return this number as f64 when it is finite and preserves the same decimal value after f64 display roundtrip.
    pub fn as_f64(&self) -> Option<f64> {
        response_f64_decimal_roundtrip(&self.0)
    }
}

/// JSON integer token retained without bounding its magnitude.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseInteger(String);

impl ResponseInteger {
    /// Create a response integer from JSON number text that is mathematically integral.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if !valid_json_number(&value) {
            return Err("invalid JSON number".to_string());
        }
        if response_number_is_integral(&value) {
            Ok(Self(value))
        } else {
            Err("response integer is not mathematically integral".to_string())
        }
    }

    /// Return the original JSON number text.
    pub fn as_str(&self) -> &str { &self.0 }

    /// Return this integer as i64 when it is in range.
    pub fn as_i64(&self) -> Option<i64> {
        response_integer_decimal_string(&self.0, 20)?.parse().ok()
    }

    /// Return this integer as u64 when it is nonnegative and in range.
    pub fn as_u64(&self) -> Option<u64> {
        response_integer_decimal_string(&self.0, 20)?.parse().ok()
    }

    /// Return this integer as f64 when it is finite and preserves the same decimal value after f64 display roundtrip.
    pub fn as_f64(&self) -> Option<f64> {
        response_f64_decimal_roundtrip(&self.0)
    }
}

trait DecodeResponseValue: Sized {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String>;
}

impl DecodeResponseValue for JsonValue {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        checked_response_json_value(value, 0)
    }
}

impl DecodeResponseValue for String {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::String(value) => Ok(value.clone()),
            _ => Err("expected string".to_string()),
        }
    }
}

impl DecodeResponseValue for bool {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Bool(value) => Ok(*value),
            _ => Err("expected boolean".to_string()),
        }
    }
}

impl DecodeResponseValue for () {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Null => Ok(()),
            _ => Err("expected null".to_string()),
        }
    }
}

impl DecodeResponseValue for ResponseNumber {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Integer(value) => Self::new(value.to_string()),
            JsonValue::Unsigned(value) => Self::new(value.to_string()),
            JsonValue::ExactNumber(value) => Self::new(value.clone()),
            JsonValue::Number(value) if value.is_finite() => Self::new(value.to_string()),
            JsonValue::Number(_) => Err("expected finite number".to_string()),
            _ => Err("expected number".to_string()),
        }
    }
}

impl DecodeResponseValue for ResponseInteger {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Integer(value) => Self::new(value.to_string()),
            JsonValue::Unsigned(value) => Self::new(value.to_string()),
            JsonValue::ExactNumber(value) => Self::new(value.clone()),
            JsonValue::Number(value) if value.is_finite() => Self::new(value.to_string()),
            JsonValue::Number(_) => Err("expected finite integer".to_string()),
            _ => Err("expected integer".to_string()),
        }
    }
}

impl<T: DecodeResponseValue> DecodeResponseValue for Vec<T> {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Array(values) => values.iter().map(T::decode_response_value).collect(),
            _ => Err("expected array".to_string()),
        }
    }
}

impl<T: DecodeResponseValue> DecodeResponseValue for Option<T> {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Null => Ok(None),
            value => T::decode_response_value(value).map(Some),
        }
    }
}

impl<T: DecodeResponseValue> DecodeResponseValue for Box<T> {
    fn decode_response_value(value: &JsonValue) -> Result<Self, String> {
        T::decode_response_value(value).map(Box::new)
    }
}

fn parse_response_json(bytes: &[u8]) -> Result<JsonValue, String> {
    use serde::Deserialize as _;
    let text = std::str::from_utf8(bytes).map_err(|error| format!("invalid UTF-8: {error}"))?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let raw = Box::<serde_json::value::RawValue>::deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    parse_response_raw_value(raw.get(), 0)
}

fn response_field<T: DecodeResponseValue>(object: &[(String, JsonValue)], name: &str) -> Result<Field<T>, String> {
    match object.iter().find(|(field_name, _)| field_name == name) {
        None => Ok(Field::Missing),
        Some((_, JsonValue::Null)) => Ok(Field::Null),
        Some((_, value)) => T::decode_response_value(value).map(Field::Value),
    }
}

fn response_required<T: DecodeResponseValue>(object: &[(String, JsonValue)], name: &str) -> Result<T, String> {
    match object.iter().find(|(field_name, _)| field_name == name) {
        None => Err(format!("missing required response field `{name}`")),
        Some((_, value)) => T::decode_response_value(value)
            .map_err(|error| format!("invalid required response field `{name}`: {error}")),
    }
}

fn response_object(value: &JsonValue) -> Result<&[(String, JsonValue)], String> {
    match value {
        JsonValue::Object(fields) => {
            reject_duplicate_response_fields(fields)?;
            Ok(fields.as_slice())
        }
        _ => Err("expected object".to_string()),
    }
}

fn checked_response_json_value(value: &JsonValue, depth: usize) -> Result<JsonValue, String> {
    if depth > RESPONSE_JSON_MAX_DEPTH {
        return Err("response JSON exceeds maximum nesting depth".to_string());
    }
    match value {
        JsonValue::Invalid => Err("invalid JSON value".to_string()),
        JsonValue::Bytes(_) => Err("bytes are not JSON".to_string()),
        JsonValue::Null => Ok(JsonValue::Null),
        JsonValue::Bool(value) => Ok(JsonValue::Bool(*value)),
        JsonValue::Number(value) if value.is_finite() => Ok(JsonValue::Number(*value)),
        JsonValue::Number(_) => Err("non-finite JSON number".to_string()),
        JsonValue::ExactNumber(value) if valid_json_number(value) => Ok(JsonValue::ExactNumber(value.clone())),
        JsonValue::ExactNumber(_) => Err("malformed JSON number".to_string()),
        JsonValue::Integer(value) => Ok(JsonValue::Integer(*value)),
        JsonValue::Unsigned(value) => Ok(JsonValue::Unsigned(*value)),
        JsonValue::String(value) => Ok(JsonValue::String(value.clone())),
        JsonValue::Array(values) => values.iter()
            .map(|value| checked_response_json_value(value, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        JsonValue::Object(fields) => {
            reject_duplicate_response_fields(fields)?;
            fields.iter()
                .map(|(name, value)| checked_response_json_value(value, depth + 1).map(|value| (name.clone(), value)))
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Object)
        }
    }
}

fn reject_duplicate_response_fields(fields: &[(String, JsonValue)]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for (name, _) in fields {
        if !seen.insert(name.as_str()) {
            return Err(format!("duplicate object key `{name}`"));
        }
    }
    Ok(())
}

fn parse_response_raw_value(text: &str, depth: usize) -> Result<JsonValue, String> {
    if depth > RESPONSE_JSON_MAX_DEPTH {
        return Err("response JSON exceeds maximum nesting depth".to_string());
    }
    let text = text.trim();
    match text.as_bytes().first().copied() {
        Some(b'n') => {
            if text == "null" { Ok(JsonValue::Null) } else { Err("invalid null token".to_string()) }
        }
        Some(b't') => {
            if text == "true" { Ok(JsonValue::Bool(true)) } else { Err("invalid true token".to_string()) }
        }
        Some(b'f') => {
            if text == "false" { Ok(JsonValue::Bool(false)) } else { Err("invalid false token".to_string()) }
        }
        Some(b'\"') => serde_json::from_str::<String>(text)
            .map(JsonValue::String)
            .map_err(|error| error.to_string()),
        Some(b'[') => parse_response_raw_array(text, depth),
        Some(b'{') => parse_response_raw_object(text, depth),
        Some(b'-' | b'0'..=b'9') => parse_response_raw_number(text),
        _ => Err("invalid JSON value".to_string()),
    }
}

fn parse_response_raw_number(text: &str) -> Result<JsonValue, String> {
    if !valid_json_number(text) {
        return Err("malformed JSON number".to_string());
    }
    if text.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        return Ok(JsonValue::ExactNumber(text.to_string()));
    }
    if text.starts_with('-') {
        if let Ok(value) = text.parse::<i64>() {
            Ok(JsonValue::Integer(value))
        } else {
            ResponseNumber::new(text).map(|_| JsonValue::ExactNumber(text.to_string()))
        }
    } else if let Ok(value) = text.parse::<u64>() {
        Ok(JsonValue::Unsigned(value))
    } else {
        ResponseNumber::new(text).map(|_| JsonValue::ExactNumber(text.to_string()))
    }
}

fn parse_response_raw_array(text: &str, depth: usize) -> Result<JsonValue, String> {
    use serde::de::{SeqAccess, Visitor};
    use serde::Deserializer as _;

    struct ArrayVisitor { depth: usize }

    impl<'de> Visitor<'de> for ArrayVisitor {
        type Value = JsonValue;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON array")
        }

        fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut values = Vec::new();
            while let Some(raw) = access.next_element::<Box<serde_json::value::RawValue>>()? {
                values.push(parse_response_raw_value(raw.get(), self.depth + 1).map_err(serde::de::Error::custom)?);
            }
            Ok(JsonValue::Array(values))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.deserialize_seq(ArrayVisitor { depth }).map_err(|error| error.to_string())
}

fn parse_response_raw_object(text: &str, depth: usize) -> Result<JsonValue, String> {
    use serde::de::{MapAccess, Visitor};
    use serde::Deserializer as _;

    struct ObjectVisitor { depth: usize }

    impl<'de> Visitor<'de> for ObjectVisitor {
        type Value = JsonValue;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut fields = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            while let Some((name, raw)) = access.next_entry::<String, Box<serde_json::value::RawValue>>()? {
                if !seen.insert(name.clone()) {
                    return Err(serde::de::Error::custom(format!("duplicate object key `{name}`")));
                }
                let value = parse_response_raw_value(raw.get(), self.depth + 1).map_err(serde::de::Error::custom)?;
                fields.push((name, value));
            }
            Ok(JsonValue::Object(fields))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.deserialize_map(ObjectVisitor { depth }).map_err(|error| error.to_string())
}

fn response_f64_decimal_roundtrip(value: &str) -> Option<f64> {
    let parsed = value.parse::<f64>().ok()?;
    if !parsed.is_finite() {
        return None;
    }
    if response_decimal_canonical(value)? == response_decimal_canonical(&parsed.to_string())? {
        Some(parsed)
    } else {
        None
    }
}

#[derive(Eq, PartialEq)]
struct CanonicalResponseDecimal {
    negative: bool,
    digits: String,
    exponent: i128,
}

fn response_decimal_canonical(value: &str) -> Option<CanonicalResponseDecimal> {
    let parts = ResponseNumberParts::parse(value)?;
    let mut digits = String::with_capacity(parts.integer_digits.len() + parts.fraction_digits.len());
    digits.push_str(parts.integer_digits);
    digits.push_str(parts.fraction_digits);
    if digits.bytes().all(|byte| byte == b'0') {
        return Some(CanonicalResponseDecimal {
            negative: false,
            digits: "0".to_string(),
            exponent: 0,
        });
    }
    let leading_zeros = digits.bytes().take_while(|byte| *byte == b'0').count();
    if leading_zeros > 0 {
        digits.drain(..leading_zeros);
    }
    let mut exponent = parts.exponent - parts.fraction_digits.len() as i128;
    while digits.as_bytes().last() == Some(&b'0') {
        digits.pop();
        exponent += 1;
    }
    Some(CanonicalResponseDecimal {
        negative: parts.negative,
        digits,
        exponent,
    })
}

fn response_number_is_integral(value: &str) -> bool {
    let Some(parts) = ResponseNumberParts::parse(value) else { return false; };
    let decimal_position = parts.integer_digits.len() as i128 + parts.exponent;
    let digit_count = parts.integer_digits.len() + parts.fraction_digits.len();
    if decimal_position >= digit_count as i128 {
        return true;
    }
    if decimal_position <= 0 {
        return parts.integer_digits.bytes().chain(parts.fraction_digits.bytes()).all(|byte| byte == b'0');
    }
    combined_digits_are_zero_from(parts.integer_digits, parts.fraction_digits, decimal_position as usize)
}

fn response_integer_decimal_string(value: &str, max_digits: usize) -> Option<String> {
    let parts = ResponseNumberParts::parse(value)?;
    if !response_number_is_integral(value) {
        return None;
    }
    let decimal_position = parts.integer_digits.len() as i128 + parts.exponent;
    if decimal_position <= 0 {
        return Some("0".to_string());
    }
    if decimal_position > max_digits as i128 {
        return None;
    }
    let output_len = decimal_position as usize;
    let mut digits = String::with_capacity(output_len + usize::from(parts.negative));
    push_combined_digits(&mut digits, parts.integer_digits, parts.fraction_digits, output_len);
    while digits.len() < output_len {
        digits.push('0');
    }
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some("0".to_string());
    }
    if parts.negative {
        Some(format!("-{trimmed}"))
    } else {
        Some(trimmed.to_string())
    }
}

fn combined_digits_are_zero_from(integer_digits: &str, fraction_digits: &str, start: usize) -> bool {
    let integer_len = integer_digits.len();
    if start < integer_len {
        integer_digits.as_bytes()[start..].iter().chain(fraction_digits.as_bytes()).all(|byte| *byte == b'0')
    } else {
        let fraction_start = start.saturating_sub(integer_len).min(fraction_digits.len());
        fraction_digits.as_bytes()[fraction_start..].iter().all(|byte| *byte == b'0')
    }
}

fn push_combined_digits(out: &mut String, integer_digits: &str, fraction_digits: &str, count: usize) {
    for byte in integer_digits.bytes().chain(fraction_digits.bytes()).take(count) {
        out.push(char::from(byte));
    }
}

struct ResponseNumberParts<'a> {
    negative: bool,
    integer_digits: &'a str,
    fraction_digits: &'a str,
    exponent: i128,
}

impl<'a> ResponseNumberParts<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        if !valid_json_number(value) {
            return None;
        }
        let (negative, unsigned) = value.strip_prefix('-').map_or((false, value), |value| (true, value));
        let exponent_index = unsigned.find(|character| matches!(character, 'e' | 'E'));
        let (significand, exponent_text) = match exponent_index {
            Some(index) => (&unsigned[..index], Some(&unsigned[index + 1..])),
            None => (unsigned, None),
        };
        let decimal_index = significand.find('.');
        let (integer_digits, fraction_digits) = match decimal_index {
            Some(index) => (&significand[..index], &significand[index + 1..]),
            None => (significand, ""),
        };
        Some(Self {
            negative,
            integer_digits,
            fraction_digits,
            exponent: parse_response_exponent(exponent_text.unwrap_or("0")),
        })
    }
}

fn parse_response_exponent(value: &str) -> i128 {
    let (negative, digits) = value.strip_prefix('-').map_or_else(
        || value.strip_prefix('+').map_or((false, value), |value| (false, value)),
        |value| (true, value),
    );
    let mut exponent = 0i128;
    for byte in digits.bytes() {
        exponent = exponent.saturating_mul(10).saturating_add(i128::from(byte - b'0'));
        if exponent > 1_000_000 {
            return if negative { -1_000_000 } else { 1_000_000 };
        }
    }
    if negative { -exponent } else { exponent }
}
"#;
