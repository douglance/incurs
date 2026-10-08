//! Media-based parameters and literal Cookie header serialization.
use crate::{OpenApiError, OpenApiResult, Parameter};
use serde_json::Value;

pub(crate) fn content(parameter: &Parameter, value: &Value) -> OpenApiResult<String> {
    let content = parameter
        .content
        .as_ref()
        .ok_or_else(|| fail("parameter content is missing"))?;
    if parameter.schema == Value::Bool(false) {
        return Err(fail("parameter is forbidden by a false schema"));
    }
    match content.media_type.as_str() {
        "application/x-www-form-urlencoded" => {
            String::from_utf8(crate::form::encode_fields(value, Some(&content.encoding))?)
                .map_err(|e| fail(e.to_string()))
        }
        "text/plain" => value
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| fail("text/plain parameter must be a string")),
        media if media == "application/json" || media.ends_with("+json") => {
            serde_json::to_string(value).map_err(|e| fail(e.to_string()))
        }
        media => Err(fail(format!("unsupported parameter media type: {media}"))),
    }
}

pub(crate) fn validate_query(value: &str) -> OpenApiResult<()> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            if bytes.get(index + 1).is_none_or(|b| !b.is_ascii_hexdigit())
                || bytes.get(index + 2).is_none_or(|b| !b.is_ascii_hexdigit())
            {
                return Err(fail("querystring contains an invalid percent escape"));
            }
            index += 3;
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&byte) {
            index += 1;
        } else {
            return Err(fail(
                "querystring must be URI-encoded and cannot contain a fragment",
            ));
        }
    }
    Ok(())
}

pub(crate) fn cookie(parameter: &Parameter, value: &Value) -> OpenApiResult<Vec<String>> {
    if !(parameter.explode && value.is_object()) {
        token(&parameter.name)?;
    }
    let name = &parameter.name;
    match value {
        Value::Array(values) => {
            let values = values
                .iter()
                .map(cookie_scalar)
                .collect::<OpenApiResult<Vec<_>>>()?;
            if parameter.explode {
                Ok(values.into_iter().map(|v| format!("{name}={v}")).collect())
            } else {
                Ok(vec![format!("{name}={}", values.join(","))])
            }
        }
        Value::Object(values) => {
            let mut parts = Vec::new();
            for (key, value) in values {
                token(key)?;
                let value = cookie_scalar(value)?;
                if parameter.explode {
                    parts.push(format!("{key}={value}"));
                } else {
                    parts.extend([key.clone(), value]);
                }
            }
            if parameter.explode {
                Ok(parts)
            } else {
                Ok(vec![format!("{name}={}", parts.join(","))])
            }
        }
        _ => Ok(vec![format!("{name}={}", cookie_scalar(value)?)]),
    }
}

pub(crate) fn cookie_value(value: &str) -> OpenApiResult<()> {
    if value
        .bytes()
        .all(|b| matches!(b,0x21|0x23..=0x2b|0x2d..=0x3a|0x3c..=0x5b|0x5d..=0x7e))
    {
        Ok(())
    } else {
        Err(fail("cookie value requires application-defined encoding"))
    }
}
fn cookie_scalar(value: &Value) -> OpenApiResult<String> {
    let text = match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => return Err(fail("cookie values must be scalars or flat collections")),
    };
    cookie_value(&text)?;
    Ok(text)
}
pub(crate) fn token(value: &str) -> OpenApiResult<()> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_\x60|~".contains(&b))
    {
        Ok(())
    } else {
        Err(fail("HTTP method or cookie name is not a valid token"))
    }
}
fn fail(message: impl Into<String>) -> OpenApiError {
    OpenApiError(message.into())
}
