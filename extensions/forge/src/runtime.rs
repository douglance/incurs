//! Explicit HTTP binding for the portable contract.
mod binding;
use crate::{ForgeError, ForgeResult, Operation};
pub use binding::{HttpBinding, HttpBindingError};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, future::Future};

/// An HTTP request after binding operation arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeHttpRequest {
    /// HTTP method with its exact wire capitalization.
    pub method: String,
    /// Absolute URL with serialized parameters.
    pub url: String,
    /// Header fields; repeated response and request values remain separate.
    pub headers: Vec<(String, String)>,
    /// Absent body differs from the bytes for JSON null.
    pub body: Option<Vec<u8>>,
}

/// An unmodified response, including non-success status and binary bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeHttpResponse {
    /// HTTP response status.
    pub status: u16,
    /// Header fields, retaining duplicates.
    pub headers: Vec<(String, String)>,
    /// Raw response bytes; decoding belongs to the consumer.
    pub body: Vec<u8>,
}

/// A host supplies asynchronous I/O; binding performs no I/O itself.
pub trait ForgeTransport {
    /// Exchange one fully bound request without discarding HTTP errors.
    fn exchange(
        &self,
        request: ForgeHttpRequest,
    ) -> impl Future<Output = ForgeResult<ForgeHttpResponse>>;
}

/// Serialize and invoke an HTTP operation through a caller-supplied transport.
///
/// This low-level function checks wire serialization only. Use [HttpBinding::invoke]
/// to enforce the owning contract schema before transport.
pub async fn invoke_http(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    server_url: &str,
    arguments: &Value,
    transport: &impl ForgeTransport,
) -> ForgeResult<ForgeHttpResponse> {
    transport
        .exchange(build_http_request(
            operation, schemas, server_url, arguments,
        )?)
        .await
}

/// Serialize an HTTP request from location-keyed arguments.
///
/// This low-level serializer has no source-version context. Use [HttpBinding::build_request]
/// for contract-aware validation.
///
/// Keys are `path`, `query`, `querystring`, `header`, `cookie`, `body`, `body_base64`, and `media_type`.
/// Absent optional parameters stay omitted. A caller that chooses a schema default
/// supplies that value explicitly; generated SDK default states do this.
/// Form, simple, label, matrix, pipe-delimited, and space-delimited styles
/// support their defined OpenAPI scalar and flat collection combinations;
/// deepObject supports flat query objects. Other combinations fail explicitly.
/// JSON bodies retain explicit null, while an absent body remains absent.
/// Pass the owning contract schema table so referenced serialization metadata remains available.
pub fn build_http_request(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    server_url: &str,
    arguments: &Value,
) -> ForgeResult<ForgeHttpRequest> {
    let args = arguments
        .as_object()
        .ok_or_else(|| error("arguments must be an object"))?;
    for key in args.keys() {
        if ![
            "path",
            "query",
            "querystring",
            "header",
            "cookie",
            "body",
            "body_base64",
            "media_type",
        ]
        .contains(&key.as_str())
        {
            return Err(error(format!("unknown argument location: {key}")));
        }
    }
    let base =
        url::Url::parse(server_url).map_err(|e| error(format!("invalid server URL: {e}")))?;
    if !["http", "https"].contains(&base.scheme())
        || base.host_str().is_none()
        || base.query().is_some()
        || base.fragment().is_some()
        || server_url.contains(['{', '}'])
    {
        return Err(error(
            "server must be an absolute HTTP URL without query, fragment, or variables",
        ));
    }
    if !operation.path.starts_with('/') {
        return Err(error("operation path must start with /"));
    }
    crate::parameters::token(&operation.method)?;
    let whole = operation
        .parameters
        .iter()
        .filter(|p| p.location == "querystring")
        .count();
    if whole > 1 || (whole == 1 && operation.parameters.iter().any(|p| p.location == "query")) {
        return Err(error(
            "querystring parameters cannot coexist with query parameters",
        ));
    }
    let mut path = operation.path.clone();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    let mut cookies = Vec::new();
    for location in ["path", "query", "querystring", "header", "cookie"] {
        if let Some(values) = args.get(location) {
            let values = values
                .as_object()
                .ok_or_else(|| error(format!("{location} must be an object")))?;
            for name in values.keys() {
                if !operation
                    .parameters
                    .iter()
                    .any(|p| p.location == location && p.name == *name)
                {
                    return Err(error(format!("unknown {location} parameter: {name}")));
                }
            }
        }
    }
    for parameter in &operation.parameters {
        let provided = args
            .get(&parameter.location)
            .and_then(Value::as_object)
            .and_then(|values| values.get(&parameter.name));
        let value = match provided {
            Some(value) => value,
            None if parameter.required => {
                return Err(error(format!(
                    "missing {} parameter: {}",
                    parameter.location, parameter.name
                )));
            }
            None => continue,
        };
        if let Some(content) = &parameter.content {
            let text = crate::parameters::content(parameter, value)?;
            match parameter.location.as_str() {
                "path" => {
                    let marker = format!("{{{}}}", parameter.name);
                    if !path.contains(&marker) {
                        return Err(error("path parameter has no placeholder"));
                    }
                    path = path.replace(&marker, &encode(&text));
                }
                "query" => query.push(format!("{}={}", encode(&parameter.name), encode(&text))),
                "querystring" => {
                    let text = if content.media_type == "application/json"
                        || content.media_type.ends_with("+json")
                    {
                        encode(&text)
                    } else {
                        text
                    };
                    crate::parameters::validate_query(&text)?;
                    query.push(text);
                }
                "header" => {
                    validate_header_name(&parameter.name)?;
                    if text.bytes().any(|b| b < 32 || b == 127) {
                        return Err(error("header value contains control characters"));
                    }
                    headers.push((parameter.name.clone(), text));
                }
                "cookie" => {
                    crate::parameters::token(&parameter.name)?;
                    crate::parameters::cookie_value(&text)?;
                    cookies.push(format!("{}={text}", parameter.name));
                }
                other => return Err(error(format!("unsupported parameter location: {other}"))),
            }
            continue;
        }
        match parameter.location.as_str() {
            "path" if parameter.style == "simple" => {
                let marker = format!("{{{}}}", parameter.name);
                if !path.contains(&marker) {
                    return Err(error(format!(
                        "path parameter has no placeholder: {}",
                        parameter.name
                    )));
                }
                let items = serialize(value, parameter.explode, true, ",")?;
                path = path.replace(&marker, &items.join(","));
            }
            "path" if parameter.style == "matrix" || parameter.style == "label" => {
                let marker = format!("{{{}}}", parameter.name);
                if !path.contains(&marker) {
                    return Err(error(format!(
                        "path parameter has no placeholder: {}",
                        parameter.name
                    )));
                }
                let replacement = serialize_path_parameter(parameter, value)?;
                path = path.replace(&marker, &replacement);
            }
            "query" if parameter.style == "form" => {
                let name = encode(&parameter.name);
                let items = serialize(value, parameter.explode, true, ",")?;
                if let Some(object) = value.as_object().filter(|_| parameter.explode) {
                    for (key, value) in object {
                        query.push(format!("{}={}", encode(key), encode(&scalar(value)?)));
                    }
                } else if value.is_array() && parameter.explode {
                    query.extend(items.into_iter().map(|value| format!("{name}={value}")));
                } else {
                    query.push(format!("{name}={}", items.join(",")));
                }
            }
            "query"
                if parameter.style == "spaceDelimited" || parameter.style == "pipeDelimited" =>
            {
                query.push(serialize_delimited_query(parameter, value)?);
            }
            "query" if parameter.style == "deepObject" => {
                if !parameter.explode {
                    return Err(error("deepObject requires explode=true"));
                }
                let object = value
                    .as_object()
                    .ok_or_else(|| error("deepObject requires an object"))?;
                for (key, value) in object {
                    query.push(format!(
                        "{}={}",
                        encode(&format!("{}[{key}]", parameter.name)),
                        encode(&scalar(value)?)
                    ));
                }
            }
            "header" if parameter.style == "simple" => {
                validate_header_name(&parameter.name)?;
                let items = serialize(value, parameter.explode, false, ",")?;
                let value = items.join(",");
                if value.bytes().any(|b| b < 32 || b == 127) {
                    return Err(error("header value contains control characters"));
                }
                headers.push((parameter.name.clone(), value));
            }
            "cookie" if parameter.style == "cookie" => {
                cookies.extend(crate::parameters::cookie(parameter, value)?);
            }
            "cookie" if parameter.style == "form" && !value.is_object() && !value.is_array() => {
                let items = serialize(value, parameter.explode, true, ",")?;
                cookies.push(format!("{}={}", encode(&parameter.name), items.join(",")));
            }
            _ => {
                return Err(error(format!(
                    "unsupported serialization: {} {} explode={}",
                    parameter.location, parameter.style, parameter.explode
                )));
            }
        }
    }
    if path.contains(['{', '}']) {
        return Err(error("unbound path placeholder"));
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(error("dot path segments would change the operation route"));
    }
    if !cookies.is_empty() {
        if headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        {
            return Err(error(
                "cookie arguments conflict with Cookie header parameter",
            ));
        }
        headers.push(("Cookie".into(), cookies.join("; ")));
    }
    let body = bind_body(operation, schemas, args, &mut headers)?;
    let mut url = format!("{}{}", server_url.trim_end_matches('/'), path);
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    url::Url::parse(&url).map_err(|e| error(format!("invalid bound URL: {e}")))?;
    Ok(ForgeHttpRequest {
        method: operation.method.clone(),
        url,
        headers,
        body,
    })
}

fn bind_body(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    args: &Map<String, Value>,
    headers: &mut Vec<(String, String)>,
) -> ForgeResult<Option<Vec<u8>>> {
    let Some(contract) = &operation.request_body else {
        if ["body", "body_base64", "media_type"]
            .iter()
            .any(|key| args.contains_key(*key))
        {
            return Err(error("operation has no request body"));
        }
        return Ok(None);
    };
    let body = args.get("body");
    let body_base64 = match args.get("body_base64") {
        Some(Value::String(encoded)) => Some(encoded.as_str()),
        Some(_) => return Err(error("body_base64 must be a base64 string")),
        None => None,
    };
    if body.is_none() && body_base64.is_none() {
        if contract.required {
            return Err(error("missing required request body"));
        }
        if args.contains_key("media_type") {
            return Err(error("media_type requires a body"));
        }
        return Ok(None);
    }
    let media = match args.get("media_type") {
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return Err(error("media_type must be a string")),
        None if contract.content.len() == 1 => contract.content.keys().next().unwrap(),
        None => {
            return Err(error(
                "select media_type for a body with multiple content types",
            ));
        }
    };
    let (bytes, content_type) = crate::media::encode(contract, schemas, media, body, body_base64)?;
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        return Err(error(
            "Content-Type is selected by the request body contract",
        ));
    }
    headers.push(("Content-Type".into(), content_type));
    Ok(Some(bytes))
}

fn scalar(value: &Value) -> ForgeResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(error(
            "parameter values must be scalars or flat collections; null parameters are unsupported",
        )),
    }
}

fn serialize(
    value: &Value,
    explode: bool,
    encoded: bool,
    extra_reserved: &str,
) -> ForgeResult<Vec<String>> {
    let component = |value: &Value| -> ForgeResult<String> {
        let value = scalar(value)?;
        Ok(if encoded {
            encode_with(&value, extra_reserved)
        } else {
            value
        })
    };
    if value.as_array().is_some_and(Vec::is_empty) || value.as_object().is_some_and(Map::is_empty) {
        return Err(error(
            "empty parameter collections have no unambiguous wire representation",
        ));
    }
    match value {
        Value::Array(values) => values.iter().map(component).collect(),
        Value::Object(values) => {
            let mut parts = Vec::new();
            for (key, value) in values {
                let key = if encoded {
                    encode_with(key, extra_reserved)
                } else {
                    key.clone()
                };
                let value = component(value)?;
                if explode {
                    parts.push(format!("{key}={value}"));
                } else {
                    parts.push(key);
                    parts.push(value);
                }
            }
            Ok(parts)
        }
        value => Ok(vec![component(value)?]),
    }
}

fn serialize_path_parameter(parameter: &crate::Parameter, value: &Value) -> ForgeResult<String> {
    match parameter.style.as_str() {
        "matrix" => serialize_matrix_path(parameter, value),
        "label" => serialize_label_path(parameter, value),
        _ => Err(error(format!(
            "unsupported path style: {}",
            parameter.style
        ))),
    }
}

fn serialize_matrix_path(parameter: &crate::Parameter, value: &Value) -> ForgeResult<String> {
    let name = encode_with(&parameter.name, ";=");
    match value {
        Value::Array(values) if parameter.explode => values
            .iter()
            .map(|value| Ok(format!(";{name}={}", encode_with(&scalar(value)?, ";="))))
            .collect::<ForgeResult<Vec<_>>>()
            .map(|parts| parts.join("")),
        Value::Object(_) if parameter.explode => {
            let items = serialize(value, true, true, ";=")?;
            Ok(format!(";{}", items.join(";")))
        }
        Value::Array(_) | Value::Object(_) => {
            let items = serialize(value, false, true, ";=")?;
            Ok(format!(";{name}={}", items.join(",")))
        }
        value => Ok(format!(";{name}={}", encode_with(&scalar(value)?, ";="))),
    }
}

fn serialize_label_path(parameter: &crate::Parameter, value: &Value) -> ForgeResult<String> {
    match value {
        Value::Array(_) | Value::Object(_) => {
            let items = serialize(value, parameter.explode, true, ".")?;
            Ok(format!(
                ".{}",
                items.join(if parameter.explode { "." } else { "," })
            ))
        }
        value => Ok(format!(".{}", encode_with(&scalar(value)?, "."))),
    }
}

fn serialize_delimited_query(parameter: &crate::Parameter, value: &Value) -> ForgeResult<String> {
    if parameter.explode {
        return Err(error(format!(
            "{} with explode=true is undefined",
            parameter.style
        )));
    }
    if !(value.is_array() || value.is_object()) {
        return Err(error(format!(
            "{} requires an array or object value",
            parameter.style
        )));
    }
    let delimiter = match parameter.style.as_str() {
        "spaceDelimited" => "%20",
        "pipeDelimited" => "%7C",
        _ => {
            return Err(error(format!(
                "unsupported query style: {}",
                parameter.style
            )));
        }
    };
    let items = serialize(
        value,
        false,
        true,
        if parameter.style == "spaceDelimited" {
            " "
        } else {
            "|"
        },
    )?;
    Ok(format!(
        "{}={}",
        encode(&parameter.name),
        items.join(delimiter)
    ))
}

fn encode(value: &str) -> String {
    encode_with(value, "")
}

fn encode_with(value: &str, extra_reserved: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if (byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
            && !extra_reserved.as_bytes().contains(&byte)
        {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(out, "%{byte:02X}").unwrap();
        }
    }
    out
}

fn validate_header_name(name: &str) -> ForgeResult<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        return Err(error("invalid header name"));
    }
    Ok(())
}

fn error(message: impl Into<String>) -> ForgeError {
    ForgeError(message.into())
}
