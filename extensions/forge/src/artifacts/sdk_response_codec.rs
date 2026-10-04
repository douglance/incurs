//! Generated response media and body decoding runtime emitted into Forge SDKs.

pub(super) const RUNTIME: &str = r#"
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseCodec {
    Json,
    Text,
    Binary,
    Unsupported,
}

#[derive(Clone, Debug)]
struct ResponseMedia {
    name: &'static str,
    pointer: &'static str,
    codec: ResponseCodec,
    binary_min: Option<u64>,
    binary_max: Option<u64>,
    binary_allowed: bool,
    binary_supported: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct DecodedPayload {
    media_index: Option<usize>,
    media_type: Option<String>,
    body: Field<JsonValue>,
}

fn decode_response_payload(
    raw: &OperationResponse,
    method: &str,
    media: &[ResponseMedia],
) -> Result<DecodedPayload, ResponseDecodeError> {
    if response_prohibits_body(raw.status, method) {
        return if response_wire_payload_absent_for_empty_status(&raw.body) {
            Ok(DecodedPayload {
                media_index: None,
                media_type: None,
                body: Field::Missing,
            })
        } else {
            Err(response_decode_error(
                ResponseDecodeErrorKind::Body,
                "response status prohibits a body",
                None,
                raw,
            ))
        };
    }

    if media.is_empty() {
        return if response_wire_payload_absent_for_empty_status(&raw.body) {
            Ok(DecodedPayload {
                media_index: None,
                media_type: None,
                body: Field::Missing,
            })
        } else {
            Err(response_decode_error(
                ResponseDecodeErrorKind::Body,
                "response declared no content but carried a body",
                None,
                raw,
            ))
        };
    }

    let selected = select_response_media(raw, media)?;
    let media_type = Some(selected.media_type.clone());
    let body = match media[selected.index].codec {
        ResponseCodec::Json => decode_json_response_body(raw, &media[selected.index], media_type.clone())?,
        ResponseCodec::Text => decode_text_response_body(raw, &media[selected.index], &selected, media_type.clone())?,
        ResponseCodec::Binary => decode_binary_response_body(raw, &media[selected.index], media_type.clone())?,
        ResponseCodec::Unsupported => {
            return Err(response_decode_error(
                ResponseDecodeErrorKind::Body,
                format!("unsupported response media type `{}`", media[selected.index].name),
                media_type,
                raw,
            ));
        }
    };

    Ok(DecodedPayload {
        media_index: Some(selected.index),
        media_type: Some(selected.media_type),
        body,
    })
}

fn response_prohibits_body(status: u16, method: &str) -> bool {
    method.eq_ignore_ascii_case("HEAD") || (100..200).contains(&status) || matches!(status, 204 | 205 | 304)
}

fn response_wire_payload_absent_for_empty_status(body: &Field<JsonValue>) -> bool {
    matches!(body, Field::Missing) || matches!(body, Field::Value(JsonValue::Bytes(bytes)) if bytes.is_empty())
}

fn decode_json_response_body(
    raw: &OperationResponse,
    media: &ResponseMedia,
    media_type: Option<String>,
) -> Result<Field<JsonValue>, ResponseDecodeError> {
    match &raw.body {
        Field::Missing => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "response body is required for declared JSON content",
            media_type,
            raw,
        )),
        Field::Null => {
            validate_response_json_schema(&JsonValue::Null, media.pointer, raw, media_type.clone())?;
            Ok(Field::Null)
        }
        Field::Value(JsonValue::Bytes(bytes)) => {
            let value = parse_response_json(bytes).map_err(|error| {
                response_decode_error(
                    ResponseDecodeErrorKind::Json,
                    format!("invalid JSON response body: {error}"),
                    media_type.clone(),
                    raw,
                )
            })?;
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(present_response_json(value))
        }
        Field::Value(value) => {
            let value = JsonValue::decode_response_value(value).map_err(|error| {
                response_decode_error(
                    ResponseDecodeErrorKind::Body,
                    format!("invalid logical JSON response body: {error}"),
                    media_type.clone(),
                    raw,
                )
            })?;
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(present_response_json(value))
        }
        Field::Default(value) => {
            let value = JsonValue::decode_response_value(value).map_err(|error| {
                response_decode_error(
                    ResponseDecodeErrorKind::Body,
                    format!("invalid logical JSON response default: {error}"),
                    media_type.clone(),
                    raw,
                )
            })?;
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(Field::Default(value))
        }
    }
}

fn present_response_json(value: JsonValue) -> Field<JsonValue> {
    match value {
        JsonValue::Null => Field::Null,
        value => Field::Value(value),
    }
}

fn decode_text_response_body(
    raw: &OperationResponse,
    media: &ResponseMedia,
    selected: &SelectedResponseMedia,
    media_type: Option<String>,
) -> Result<Field<JsonValue>, ResponseDecodeError> {
    let charset = selected_response_charset(selected).map_err(|error| {
        response_decode_error(ResponseDecodeErrorKind::Body, error, media_type.clone(), raw)
    })?;
    match &raw.body {
        Field::Missing => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "response body is required for declared text content",
            media_type,
            raw,
        )),
        Field::Null => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "text response body cannot be null",
            media_type,
            raw,
        )),
        Field::Value(JsonValue::Bytes(bytes)) => {
            let text = decode_response_text_bytes(bytes, charset, raw, media_type.clone())?;
            let value = JsonValue::String(text);
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(Field::Value(value))
        }
        Field::Value(JsonValue::String(value)) => {
            validate_response_text_charset(value, charset).map_err(|error| {
                response_decode_error(ResponseDecodeErrorKind::Body, error, media_type.clone(), raw)
            })?;
            let value = JsonValue::String(value.clone());
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(Field::Value(value))
        }
        Field::Value(_) => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "text response body must be bytes or a string",
            media_type,
            raw,
        )),
        Field::Default(JsonValue::String(value)) => {
            validate_response_text_charset(value, charset).map_err(|error| {
                response_decode_error(ResponseDecodeErrorKind::Body, error, media_type.clone(), raw)
            })?;
            let value = JsonValue::String(value.clone());
            validate_response_json_schema(&value, media.pointer, raw, media_type.clone())?;
            Ok(Field::Default(value))
        }
        Field::Default(_) => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "text response default must be a string",
            media_type,
            raw,
        )),
    }
}

fn decode_binary_response_body(
    raw: &OperationResponse,
    media: &ResponseMedia,
    media_type: Option<String>,
) -> Result<Field<JsonValue>, ResponseDecodeError> {
    match &raw.body {
        Field::Missing => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "response body is required for declared binary content",
            media_type,
            raw,
        )),
        Field::Value(JsonValue::Bytes(bytes)) => {
            if !media.binary_supported {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::Schema,
                    "binary response schema is not supported by this generated SDK",
                    media_type,
                    raw,
                ));
            }
            if !media.binary_allowed {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::Schema,
                    "binary response schema rejects bytes",
                    media_type,
                    raw,
                ));
            }
            let len = bytes.len() as u64;
            if media.binary_min.is_some_and(|min| len < min) {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::Schema,
                    format!("binary response body has {len} bytes, below minimum"),
                    media_type,
                    raw,
                ));
            }
            if media.binary_max.is_some_and(|max| len > max) {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::Schema,
                    format!("binary response body has {len} bytes, above maximum"),
                    media_type,
                    raw,
                ));
            }
            Ok(Field::Value(JsonValue::Bytes(bytes.clone())))
        }
        _ => Err(response_decode_error(
            ResponseDecodeErrorKind::Body,
            "binary response body must be raw bytes",
            media_type,
            raw,
        )),
    }
}

fn validate_response_json_schema(
    value: &JsonValue,
    pointer: &str,
    raw: &OperationResponse,
    media_type: Option<String>,
) -> Result<(), ResponseDecodeError> {
    if response_json_matches(value, pointer) {
        Ok(())
    } else {
        Err(response_decode_error(
            ResponseDecodeErrorKind::Schema,
            "response body does not match the selected response schema",
            media_type,
            raw,
        ))
    }
}

#[derive(Clone, Debug)]
struct SelectedResponseMedia {
    index: usize,
    media_type: String,
    actual: Option<ParsedMediaType>,
    declared: ParsedMediaType,
}

fn select_response_media(
    raw: &OperationResponse,
    media: &[ResponseMedia],
) -> Result<SelectedResponseMedia, ResponseDecodeError> {
    let content_type = response_content_type(raw)?;
    let declared = media
        .iter()
        .map(|item| parse_response_media_type(item.name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            response_decode_error(
                ResponseDecodeErrorKind::MediaType,
                format!("invalid declared response media type: {error}"),
                None,
                raw,
            )
        })?;

    let Some(content_type) = content_type else {
        let concrete = declared
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_concrete())
            .collect::<Vec<_>>();
        return match concrete.as_slice() {
            [(index, item)] => Ok(SelectedResponseMedia {
                index: *index,
                media_type: item.essence(),
                actual: None,
                declared: (*item).clone(),
            }),
            _ => Err(response_decode_error(
                ResponseDecodeErrorKind::MediaType,
                "response Content-Type header is required for this response",
                None,
                raw,
            )),
        };
    };

    let actual = parse_response_media_type(&content_type).map_err(|error| {
        response_decode_error(
            ResponseDecodeErrorKind::MediaType,
            format!("invalid response Content-Type header: {error}"),
            Some(content_type.clone()),
            raw,
        )
    })?;
    let mut best: Option<(usize, u8)> = None;
    for (index, candidate) in declared.iter().enumerate() {
        let Some(rank) = response_media_match_rank(candidate, &actual) else {
            continue;
        };
        match best {
            None => best = Some((index, rank)),
            Some((_, current)) if rank > current => best = Some((index, rank)),
            Some((_, current)) if rank == current => {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::MediaType,
                    format!("ambiguous response media type `{}`", actual.essence()),
                    Some(actual.essence()),
                    raw,
                ));
            }
            Some(_) => {}
        }
    }
    let Some((index, _)) = best else {
        return Err(response_decode_error(
            ResponseDecodeErrorKind::MediaType,
            format!("unmatched response media type `{}`", actual.essence()),
            Some(actual.essence()),
            raw,
        ));
    };
    Ok(SelectedResponseMedia {
        index,
        media_type: actual.essence(),
        actual: Some(actual),
        declared: declared[index].clone(),
    })
}

fn response_content_type(raw: &OperationResponse) -> Result<Option<String>, ResponseDecodeError> {
    let mut found: Option<String> = None;
    for (name, value) in &raw.headers {
        if name.eq_ignore_ascii_case("content-type") {
            if found.is_some() {
                return Err(response_decode_error(
                    ResponseDecodeErrorKind::MediaType,
                    "response has more than one Content-Type header",
                    None,
                    raw,
                ));
            }
            found = Some(value.clone());
        }
    }
    Ok(found)
}

fn response_media_match_rank(declared: &ParsedMediaType, actual: &ParsedMediaType) -> Option<u8> {
    if !declared.parameters_match(actual) {
        return None;
    }
    if declared.ty == "*" && declared.subtype == "*" {
        return Some(1);
    }
    if declared.ty == actual.ty && declared.subtype == "*" {
        return Some(2);
    }
    if declared.ty == actual.ty && declared.subtype == actual.subtype {
        return Some(if declared.params.is_empty() { 3 } else { 4 });
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedMediaType {
    ty: String,
    subtype: String,
    params: Vec<(String, String)>,
}

impl ParsedMediaType {
    fn essence(&self) -> String {
        format!("{}/{}", self.ty, self.subtype)
    }

    fn is_concrete(&self) -> bool {
        self.ty != "*" && self.subtype != "*"
    }

    fn parameter(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    fn parameters_match(&self, actual: &ParsedMediaType) -> bool {
        self.params.iter().all(|(name, value)| {
            actual
                .parameter(name)
                .is_some_and(|actual_value| response_parameter_value_matches(name, value, actual_value))
        })
    }
}

fn response_parameter_value_matches(name: &str, declared: &str, actual: &str) -> bool {
    if name.eq_ignore_ascii_case("charset") {
        declared.eq_ignore_ascii_case(actual)
    } else {
        declared == actual
    }
}

fn parse_response_media_type(value: &str) -> Result<ParsedMediaType, String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("media type contains a control character".to_string());
    }
    let parts = split_response_media_type(value)?;
    let essence = parts
        .first()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "media type is empty".to_string())?;
    let (ty, subtype) = essence
        .split_once('/')
        .ok_or_else(|| "media type must contain a type and subtype".to_string())?;
    let ty = ty.trim().to_ascii_lowercase();
    let subtype = subtype.trim().to_ascii_lowercase();
    if !valid_response_media_token(&ty) || !valid_response_media_token(&subtype) {
        return Err("media type contains an invalid token".to_string());
    }
    if ty == "*" && subtype != "*" {
        return Err("wildcard media type must use */*".to_string());
    }
    let mut params = Vec::new();
    for part in parts.iter().skip(1) {
        let part = part.trim();
        if part.is_empty() {
            return Err("media type contains an empty parameter".to_string());
        }
        let (name, value) = part
            .split_once('=')
            .ok_or_else(|| "media type parameter must contain `=`".to_string())?;
        let name = name.trim().to_ascii_lowercase();
        if !valid_response_media_token(&name) {
            return Err("media type parameter contains an invalid name".to_string());
        }
        if params.iter().any(|(seen, _)| seen == &name) {
            return Err(format!("duplicate media type parameter `{name}`"));
        }
        let value = parse_response_parameter_value(value.trim())?;
        params.push((name, value));
    }
    Ok(ParsedMediaType { ty, subtype, params })
}

fn split_response_media_type(value: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            current.push(character);
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            current.push(character);
            continue;
        }
        if !quoted && character == ';' {
            parts.push(current);
            current = String::new();
            continue;
        }
        current.push(character);
    }
    if escaped || quoted {
        return Err("media type contains an unterminated quoted string".to_string());
    }
    parts.push(current);
    Ok(parts)
}

fn parse_response_parameter_value(value: &str) -> Result<String, String> {
    if let Some(rest) = value.strip_prefix('"') {
        let Some(rest) = rest.strip_suffix('"') else {
            return Err("media type quoted parameter is unterminated".to_string());
        };
        let mut output = String::new();
        let mut escaped = false;
        for character in rest.chars() {
            if escaped {
                if character.is_control() {
                    return Err("media type parameter contains a control character".to_string());
                }
                output.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                if character.is_control() {
                    return Err("media type parameter contains a control character".to_string());
                }
                output.push(character);
            }
        }
        if escaped {
            return Err("media type quoted parameter ends with an escape".to_string());
        }
        Ok(output)
    } else {
        if !valid_response_media_token(value) {
            return Err("media type parameter contains an invalid value".to_string());
        }
        Ok(value.to_string())
    }
}

fn valid_response_media_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~')
        })
}

#[derive(Clone, Copy)]
enum ResponseTextCharset {
    Utf8,
    Ascii,
}

fn selected_response_charset(selected: &SelectedResponseMedia) -> Result<ResponseTextCharset, String> {
    let value = selected
        .actual
        .as_ref()
        .and_then(|media| media.parameter("charset"))
        .or_else(|| selected.declared.parameter("charset"))
        .unwrap_or("utf-8")
        .to_ascii_lowercase();
    match value.as_str() {
        "utf-8" | "utf8" => Ok(ResponseTextCharset::Utf8),
        "us-ascii" | "ascii" => Ok(ResponseTextCharset::Ascii),
        _ => Err(format!("unsupported response charset `{value}`")),
    }
}

fn decode_response_text_bytes(
    bytes: &[u8],
    charset: ResponseTextCharset,
    raw: &OperationResponse,
    media_type: Option<String>,
) -> Result<String, ResponseDecodeError> {
    let value = std::str::from_utf8(bytes).map_err(|error| {
        response_decode_error(
            ResponseDecodeErrorKind::Body,
            format!("invalid UTF-8 response body: {error}"),
            media_type.clone(),
            raw,
        )
    })?;
    validate_response_text_charset(value, charset).map_err(|error| {
        response_decode_error(ResponseDecodeErrorKind::Body, error, media_type, raw)
    })?;
    Ok(value.to_string())
}

fn validate_response_text_charset(value: &str, charset: ResponseTextCharset) -> Result<(), String> {
    match charset {
        ResponseTextCharset::Utf8 => Ok(()),
        ResponseTextCharset::Ascii if value.is_ascii() => Ok(()),
        ResponseTextCharset::Ascii => Err("US-ASCII response body contains non-ASCII data".to_string()),
    }
}

fn response_decode_error(
    kind: ResponseDecodeErrorKind,
    message: impl Into<String>,
    media_type: Option<String>,
    raw: &OperationResponse,
) -> ResponseDecodeError {
    ResponseDecodeError {
        kind,
        message: message.into(),
        media_type,
        raw: raw.clone(),
    }
}
"#;
