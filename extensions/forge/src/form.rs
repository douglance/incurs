//! URL-encoded request bodies: content serialization and explicit query styles.
use crate::{Encoding, ForgeError, ForgeResult, RequestBody};
use serde_json::Value;

pub(crate) fn encode_body(contract: &RequestBody, body: &Value) -> ForgeResult<Vec<u8>> {
    encode_fields(
        body,
        contract.encoding.get("application/x-www-form-urlencoded"),
    )
}

pub(crate) fn encode_fields(
    body: &Value,
    encodings: Option<&std::collections::BTreeMap<String, Encoding>>,
) -> ForgeResult<Vec<u8>> {
    let fields = body
        .as_object()
        .ok_or_else(|| fail("URL-encoded body must be an object"))?;
    let mut parts = Vec::new();
    for (name, value) in fields {
        let default = Encoding::default();
        let encoding = encodings.and_then(|map| map.get(name)).unwrap_or(&default);
        if encoding.style.is_some()
            || encoding.explode.is_some()
            || encoding.allow_reserved.is_some()
        {
            parts.extend(styled(name, value, encoding)?);
        } else {
            content(name, value, encoding.content_type.as_deref(), &mut parts)?;
        }
    }
    Ok(parts.join("&").into_bytes())
}

fn content(
    name: &str,
    value: &Value,
    media: Option<&str>,
    parts: &mut Vec<String>,
) -> ForgeResult<()> {
    let text = match media {
        Some(media) if media == "application/json" || media.ends_with("+json") => {
            serde_json::to_string(value).map_err(|e| fail(e.to_string()))?
        }
        Some("text/plain") => scalar(value)?,
        Some(other) => {
            return Err(fail(format!(
                "unsupported form property contentType: {other}"
            )));
        }
        None => match value {
            Value::Array(items) => {
                for item in items {
                    content(name, item, None, parts)?;
                }
                return Ok(());
            }
            Value::Object(_) => serde_json::to_string(value).map_err(|e| fail(e.to_string()))?,
            _ => scalar(value)?,
        },
    };
    parts.push(format!(
        "{}={}",
        form_component(name),
        form_component(&text)
    ));
    Ok(())
}

fn styled(name: &str, value: &Value, encoding: &Encoding) -> ForgeResult<Vec<String>> {
    let style = encoding.style.as_deref().unwrap_or("form");
    let explode = encoding.explode.unwrap_or(style == "form");
    let reserved = encoding.allow_reserved.unwrap_or(false);
    let key = style_component(name, false, "");
    let component = |value: &Value, delimiter: &str| {
        scalar(value).map(|text| style_component(&text, reserved, delimiter))
    };
    match style {
        "form" => match value {
            Value::Array(items) => {
                let items = items
                    .iter()
                    .map(|v| component(v, ","))
                    .collect::<ForgeResult<Vec<_>>>()?;
                if explode {
                    Ok(items.into_iter().map(|v| format!("{key}={v}")).collect())
                } else {
                    Ok(vec![format!("{key}={}", items.join(","))])
                }
            }
            Value::Object(fields) => {
                let mut parts = Vec::new();
                for (name, value) in fields {
                    let name = style_component(name, false, ",");
                    let value = component(value, ",")?;
                    if explode {
                        parts.push(format!("{name}={value}"));
                    } else {
                        parts.extend([name, value]);
                    }
                }
                if explode {
                    Ok(parts)
                } else {
                    Ok(vec![format!("{key}={}", parts.join(","))])
                }
            }
            _ => Ok(vec![format!("{key}={}", component(value, "")?)]),
        },
        "deepObject" => {
            if !explode {
                return Err(fail("deepObject requires explode=true"));
            }
            let fields = value
                .as_object()
                .ok_or_else(|| fail("deepObject requires an object"))?;
            fields
                .iter()
                .map(|(field, value)| {
                    Ok(format!(
                        "{}={}",
                        style_component(&format!("{name}[{field}]"), false, ""),
                        component(value, "")?
                    ))
                })
                .collect()
        }
        "pipeDelimited" | "spaceDelimited" => {
            if explode {
                return Err(fail(format!("{style} with explode=true is undefined")));
            }
            let (delimiter, wire) = if style == "pipeDelimited" {
                ("|", "%7C")
            } else {
                (" ", "%20")
            };
            let mut parts = Vec::new();
            match value {
                Value::Array(items) => {
                    for value in items {
                        parts.push(component(value, delimiter)?);
                    }
                }
                Value::Object(fields) => {
                    for (name, value) in fields {
                        parts.push(style_component(name, false, delimiter));
                        parts.push(component(value, delimiter)?);
                    }
                }
                _ => return Err(fail(format!("{style} requires an array or object value"))),
            }
            Ok(vec![format!("{key}={}", parts.join(wire))])
        }
        _ => Err(fail(format!(
            "unsupported form serialization style: {style}"
        ))),
    }
}

fn scalar(value: &Value) -> ForgeResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(fail(
            "form scalar must be a string, number, or boolean; use JSON contentType for null or nested values",
        )),
    }
}

fn form_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn style_component(value: &str, allow_reserved: bool, delimiter: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if allow_reserved
            && byte == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push_str(&value[i..i + 3]);
            i += 3;
            continue;
        }
        let permitted = byte.is_ascii_alphanumeric()
            || b"-._~".contains(&byte)
            || (allow_reserved && b":/?@!$'()*,;".contains(&byte));
        if permitted && !delimiter.as_bytes().contains(&byte) {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(out, "%{byte:02X}").unwrap();
        }
        i += 1;
    }
    out
}

fn fail(message: impl Into<String>) -> ForgeError {
    ForgeError(message.into())
}
