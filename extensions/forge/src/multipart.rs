//! Multipart form serialization with measured boundary collision avoidance.
use crate::{Encoding, ForgeError, ForgeResult, RequestBody};
use serde_json::Value;

pub(crate) fn encode_body(
    contract: &RequestBody,
    schemas: &std::collections::BTreeMap<String, Value>,
    body: &Value,
) -> ForgeResult<(Vec<u8>, String)> {
    let fields = body
        .as_object()
        .ok_or_else(|| fail("multipart body must be an object"))?;
    let schema = crate::schema::binding_view(&contract.content["multipart/form-data"], schemas)?;
    let encodings = contract.encoding.get("multipart/form-data");
    let mut parts = Vec::new();
    for (name, value) in fields {
        let default = Encoding::default();
        let mut encoding = encodings
            .and_then(|entries| entries.get(name))
            .unwrap_or(&default)
            .clone();
        for header in encoding.headers.values_mut() {
            if let Some(value) = header.get_mut("schema") {
                *value = crate::schema::binding_view(value, schemas)?.into_owned();
            }
        }
        let encoding = &encoding;
        let property = crate::schema::binding_view(&schema["properties"][name], schemas)?;
        let property = property.as_ref();
        let item_schema = crate::schema::binding_view(&property["items"], schemas)?;
        if encoding.style.is_some()
            || encoding.explode.is_some()
            || encoding.allow_reserved.is_some()
        {
            if value.is_array()
                && (item_schema.get("contentEncoding").is_some()
                    || transfer_encoding(property, encoding)?.is_some())
            {
                return Err(fail(
                    "style-based transfer-encoded multipart collections are unsupported",
                ));
            }
            let transfer = transfer_encoding(property, encoding)?;
            for (name, text) in styled(name, value, encoding)? {
                parts.push(part(
                    &name,
                    text.as_bytes(),
                    "text/plain",
                    transfer,
                    encoding,
                )?);
            }
        } else if let Some(items) = value.as_array() {
            for item in items {
                parts.push(content_part(name, item, item_schema.as_ref(), encoding)?);
            }
        } else {
            parts.push(content_part(name, value, property, encoding)?);
        }
    }
    let boundary = (0_u64..)
        .map(|n| format!("incurs-forge-{n}"))
        .find(|candidate| {
            !parts.iter().any(|part: &Vec<u8>| {
                part.windows(candidate.len())
                    .any(|window| window == candidate.as_bytes())
            })
        })
        .ok_or_else(|| fail("unable to select multipart boundary"))?;
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(&part);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((out, format!("multipart/form-data; boundary={boundary}")))
}

fn content_part(
    name: &str,
    value: &Value,
    schema: &Value,
    encoding: &Encoding,
) -> ForgeResult<Vec<u8>> {
    let transfer = transfer_encoding(schema, encoding)?;
    let inferred =
        if transfer.is_some() || schema.get("format").and_then(Value::as_str) == Some("binary") {
            "application/octet-stream"
        } else if value.is_object() || value.is_array() || value.is_null() {
            "application/json"
        } else {
            "text/plain"
        };
    let media = encoding.content_type.as_deref().unwrap_or(inferred);
    validate_media(media)?;
    let essence = media.split(';').next().unwrap().trim();
    let bytes = if transfer.is_some() {
        value
            .as_str()
            .ok_or_else(|| fail("contentEncoding requires an encoded string value"))?
            .as_bytes()
            .to_vec()
    } else if essence == "application/json" || essence.ends_with("+json") {
        serde_json::to_vec(value).map_err(|e| fail(e.to_string()))?
    } else if essence == "text/plain" {
        scalar(value)?.into_bytes()
    } else {
        value
            .as_str()
            .ok_or_else(|| fail("non-JSON multipart content requires a string value"))?
            .as_bytes()
            .to_vec()
    };
    part(name, &bytes, media, transfer, encoding)
}

fn header_value(definition: &Value) -> Option<&Value> {
    let schema = &definition["schema"];
    schema
        .get("const")
        .or_else(|| schema.get("default"))
        .or_else(|| {
            schema
                .get("enum")
                .and_then(Value::as_array)
                .filter(|values| values.len() == 1)
                .and_then(|values| values.first())
        })
}

fn transfer_encoding<'a>(
    schema: &'a Value,
    encoding: &'a Encoding,
) -> ForgeResult<Option<&'a str>> {
    let mut transfer = schema
        .get("contentEncoding")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| fail("contentEncoding must be a string"))
        })
        .transpose()?;
    for (name, definition) in &encoding.headers {
        if name.eq_ignore_ascii_case("content-transfer-encoding")
            && let Some(value) = header_value(definition)
        {
            let value = value
                .as_str()
                .ok_or_else(|| fail("multipart transfer encoding header must be a string"))?;
            if transfer.is_some_and(|current| !current.eq_ignore_ascii_case(value)) {
                return Err(fail("multipart transfer encoding declarations conflict"));
            }
            transfer = Some(value);
        }
    }
    Ok(transfer)
}

fn part(
    name: &str,
    bytes: &[u8],
    media: &str,
    transfer: Option<&str>,
    encoding: &Encoding,
) -> ForgeResult<Vec<u8>> {
    let mut headers = format!(
        "Content-Disposition: form-data; name=\"{}\"\r\nContent-Type: {media}\r\n",
        quoted_name(name)
    );
    if let Some(transfer) = transfer {
        if !matches!(
            transfer.to_ascii_lowercase().as_str(),
            "base64" | "quoted-printable" | "7bit" | "8bit" | "binary"
        ) {
            return Err(fail(format!(
                "unsupported multipart contentEncoding: {transfer}"
            )));
        }
        headers.push_str(&format!("Content-Transfer-Encoding: {transfer}\r\n"));
    }
    for (name, definition) in &encoding.headers {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        if name.eq_ignore_ascii_case("content-disposition") {
            return Err(fail(
                "multipart Content-Disposition is owned by the property binding",
            ));
        }
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == 96 || b"!#$%&'*+-.^_|~".contains(&b))
        {
            return Err(fail("invalid multipart header name"));
        }
        let Some(value) = header_value(definition) else {
            if definition.get("required").and_then(Value::as_bool) == Some(true) {
                return Err(fail(format!(
                    "required multipart header {name} has no bound value"
                )));
            }
            continue;
        };
        let text = scalar(value)?;
        if text.bytes().any(|b| b < 32 || b == 127) {
            return Err(fail("multipart header value contains control characters"));
        }
        if name.eq_ignore_ascii_case("content-transfer-encoding")
            && let Some(transfer) = transfer
        {
            if !text.eq_ignore_ascii_case(transfer) {
                return Err(fail(
                    "multipart transfer encoding conflicts with schema contentEncoding",
                ));
            }
            continue;
        }
        headers.push_str(&format!("{name}: {text}\r\n"));
    }
    headers.push_str("\r\n");
    let mut out = headers.into_bytes();
    out.extend_from_slice(bytes);
    Ok(out)
}

fn validate_media(media: &str) -> ForgeResult<()> {
    if media.is_empty()
        || !media.is_ascii()
        || media.bytes().any(|b| b < 32 || b == 127)
        || media.contains([',', '*'])
        || !media.split(';').next().unwrap().contains('/')
    {
        return Err(fail(
            "multipart contentType must select one concrete media type without control characters",
        ));
    }
    Ok(())
}

fn quoted_name(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character == '"' || character == '\\' || character == '%' || character.is_control() {
            use std::fmt::Write;
            for byte in character.to_string().bytes() {
                write!(out, "%{byte:02X}").unwrap();
            }
        } else {
            out.push(character);
        }
    }
    out
}

fn scalar(value: &Value) -> ForgeResult<String> {
    match value {
        Value::String(v) => Ok(v.clone()),
        Value::Bool(v) => Ok(v.to_string()),
        Value::Number(v) => Ok(v.to_string()),
        _ => Err(fail(
            "multipart style values must be scalars or flat collections",
        )),
    }
}

fn styled(name: &str, value: &Value, encoding: &Encoding) -> ForgeResult<Vec<(String, String)>> {
    let style = encoding.style.as_deref().unwrap_or("form");
    let explode = encoding.explode.unwrap_or(style == "form");
    match style {
        "form" => match value {
            Value::Array(values) => {
                let items = values.iter().map(scalar).collect::<ForgeResult<Vec<_>>>()?;
                if explode {
                    Ok(items.into_iter().map(|v| (name.into(), v)).collect())
                } else {
                    Ok(vec![(name.into(), items.join(","))])
                }
            }
            Value::Object(values) => {
                if explode {
                    values
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), scalar(v)?)))
                        .collect()
                } else {
                    let mut items = Vec::new();
                    for (k, v) in values {
                        items.extend([k.clone(), scalar(v)?]);
                    }
                    Ok(vec![(name.into(), items.join(","))])
                }
            }
            _ => Ok(vec![(name.into(), scalar(value)?)]),
        },
        "deepObject" => {
            if !explode {
                return Err(fail("deepObject requires explode=true"));
            }
            let values = value
                .as_object()
                .ok_or_else(|| fail("deepObject requires an object"))?;
            values
                .iter()
                .map(|(k, v)| Ok((format!("{name}[{k}]"), scalar(v)?)))
                .collect()
        }
        "pipeDelimited" | "spaceDelimited" => {
            if explode {
                return Err(fail(format!("{style} with explode=true is undefined")));
            }
            let mut items = Vec::new();
            match value {
                Value::Array(values) => {
                    for v in values {
                        items.push(scalar(v)?);
                    }
                }
                Value::Object(values) => {
                    for (k, v) in values {
                        items.extend([k.clone(), scalar(v)?]);
                    }
                }
                _ => return Err(fail(format!("{style} requires an array or object"))),
            }
            Ok(vec![(
                name.into(),
                items.join(if style == "pipeDelimited" { "|" } else { " " }),
            )])
        }
        _ => Err(fail(format!(
            "unsupported multipart serialization style: {style}"
        ))),
    }
}

fn fail(message: impl Into<String>) -> ForgeError {
    ForgeError(message.into())
}
