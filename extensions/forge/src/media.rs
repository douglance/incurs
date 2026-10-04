//! Shared request-body media classification and encoding.
use crate::{ForgeError, ForgeResult, RequestBody};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Request body encoding selected from the declared media type and schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyCodec {
    /// JSON or structured syntax suffix JSON.
    Json,
    /// A textual body encoded as UTF-8, subject to any declared charset.
    Text,
    /// Raw bytes, with an optional plain string schema alternative.
    Binary { text_alternative: bool },
    /// JSON Lines or NDJSON carried as an exact byte stream.
    JsonLines { binary: bool, final_newline: bool },
    /// `application/x-www-form-urlencoded` serialized by the form helper.
    UrlEncodedForm,
    /// `multipart/form-data` serialized by the multipart helper.
    MultipartForm,
    /// A stable reason for unsupported or ambiguous body encodings.
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Charset {
    Utf8,
    UsAscii,
}

#[derive(Clone, Debug)]
struct MediaParts {
    essence: String,
    charset: Option<Charset>,
}

/// Select the request-body codec for one declared media type and schema.
pub(crate) fn classify(
    media: &str,
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> ForgeResult<BodyCodec> {
    let parts = parse_media(media)?;
    let schema = crate::schema::binding_view(schema, schemas)?;
    if schema.as_ref() == &Value::Bool(false) {
        return Ok(BodyCodec::Unsupported(
            "request body is forbidden by a false schema",
        ));
    }
    let schema = schema.as_ref();
    match parts.essence.as_str() {
        "application/json" => Ok(BodyCodec::Json),
        essence if essence.ends_with("+json") => Ok(BodyCodec::Json),
        "application/jsonl" => json_lines_codec(&parts, schema, schemas, false),
        "application/x-ndjson" => json_lines_codec(&parts, schema, schemas, true),
        "application/octet-stream" => Ok(BodyCodec::Binary {
            text_alternative: text_alternative_schema(schema, schemas)?.is_some(),
        }),
        "application/x-www-form-urlencoded" => Ok(BodyCodec::UrlEncodedForm),
        "multipart/form-data" => Ok(BodyCodec::MultipartForm),
        "application/javascript" => text_codec(&parts, schema, schemas),
        essence if essence.starts_with("text/") => text_codec(&parts, schema, schemas),
        _ => Ok(BodyCodec::Unsupported("unsupported request media type")),
    }
}

/// Return the non-binary string schema alternative, preserving parent metadata.
pub(crate) fn text_alternative_schema(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> ForgeResult<Option<Value>> {
    Ok(text_candidate(schema, schemas, true)?
        .map(|_| serde_json::json!({"type":"string","allOf":[schema]})))
}

/// Encode a request body and return its bytes plus the selected Content-Type.
pub(crate) fn encode(
    contract: &RequestBody,
    schemas: &BTreeMap<String, Value>,
    media: &str,
    body: Option<&Value>,
    body_base64: Option<&str>,
) -> ForgeResult<(Vec<u8>, String)> {
    let schema = contract
        .content
        .get(media)
        .ok_or_else(|| fail(format!("undeclared request media type: {media}")))?;
    if crate::schema::binding_view(schema, schemas)?.as_ref() == &Value::Bool(false) {
        return Err(fail("request body is forbidden by a false schema"));
    }
    if body.is_some() && body_base64.is_some() {
        return Err(fail(
            "request body must use either body or body_base64, not both",
        ));
    }
    let codec = classify(media, schema, schemas)?;
    match codec {
        BodyCodec::Json => {
            reject_base64(body_base64, "JSON")?;
            let body = body.ok_or_else(|| fail("request body is absent"))?;
            Ok((
                serde_json::to_vec(body).map_err(|e| fail(e.to_string()))?,
                media.to_owned(),
            ))
        }
        BodyCodec::Text => {
            reject_base64(body_base64, "text")?;
            let text = required_string(body, &format!("{media} body must be a string"))?;
            let bytes = text.as_bytes().to_vec();
            enforce_text_charset(media, &bytes)?;
            Ok((bytes, media.to_owned()))
        }
        BodyCodec::Binary { text_alternative } => {
            if let Some(encoded) = body_base64 {
                return Ok((decode_base64(encoded)?, media.to_owned()));
            }
            let text = required_string(body, "binary body requires body_base64")?;
            if !text_alternative {
                return Err(fail("binary body requires body_base64"));
            }
            let bytes = text.as_bytes().to_vec();
            enforce_text_charset(media, &bytes)?;
            Ok((bytes, media.to_owned()))
        }
        BodyCodec::JsonLines {
            binary,
            final_newline,
        } => {
            let bytes = if binary {
                if let Some(encoded) = body_base64 {
                    decode_base64(encoded)?
                } else {
                    return Err(fail("binary JSON Lines body requires body_base64"));
                }
            } else {
                reject_base64(body_base64, "JSON Lines")?;
                required_string(body, "JSON Lines body must be a string")?
                    .as_bytes()
                    .to_vec()
            };
            enforce_text_charset(media, &bytes)?;
            validate_json_lines(&bytes, final_newline)?;
            Ok((bytes, media.to_owned()))
        }
        BodyCodec::UrlEncodedForm => {
            reject_base64(body_base64, "URL-encoded form")?;
            let body = body.ok_or_else(|| fail("request body is absent"))?;
            let contract = remap_contract(contract, media, "application/x-www-form-urlencoded");
            Ok((crate::form::encode_body(&contract, body)?, media.to_owned()))
        }
        BodyCodec::MultipartForm => {
            reject_base64(body_base64, "multipart form")?;
            let body = body.ok_or_else(|| fail("request body is absent"))?;
            let contract = remap_contract(contract, media, "multipart/form-data");
            crate::multipart::encode_body(&contract, schemas, body)
        }
        BodyCodec::Unsupported(reason) => Err(fail(format!(
            "unsupported request media type: {media}; {reason}"
        ))),
    }
}

fn json_lines_codec(
    parts: &MediaParts,
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
    final_newline: bool,
) -> ForgeResult<BodyCodec> {
    text_charset(parts)?;
    if has_binary_string(schema, schemas)? {
        return Ok(BodyCodec::JsonLines {
            binary: true,
            final_newline,
        });
    }
    if text_candidate(schema, schemas, true)?.is_some() {
        return Ok(BodyCodec::JsonLines {
            binary: false,
            final_newline,
        });
    }
    Ok(BodyCodec::Unsupported(
        "JSON Lines body schema is not a string",
    ))
}

fn text_codec(
    parts: &MediaParts,
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
) -> ForgeResult<BodyCodec> {
    text_charset(parts)?;
    if text_candidate(schema, schemas, true)?.is_some() || has_binary_string(schema, schemas)? {
        Ok(BodyCodec::Text)
    } else {
        Ok(BodyCodec::Unsupported("text body schema is not a string"))
    }
}

fn parse_media(media: &str) -> ForgeResult<MediaParts> {
    let mut items = media.split(';');
    let essence = items.next().unwrap_or_default().trim().to_ascii_lowercase();
    if essence.is_empty()
        || !essence.is_ascii()
        || essence.bytes().any(|b| b < 32 || b == 127)
        || !essence.contains('/')
    {
        return Err(fail(format!("invalid media type: {media}")));
    }
    let mut charset = None;
    for parameter in items {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("charset") {
            continue;
        }
        let value = value.trim().trim_matches('"').to_ascii_lowercase();
        charset = Some(match value.as_str() {
            "utf-8" | "utf8" => Charset::Utf8,
            "us-ascii" | "ascii" => Charset::UsAscii,
            _ => {
                return Err(fail(format!(
                    "unsupported charset for textual request body: {value}"
                )));
            }
        });
    }
    Ok(MediaParts { essence, charset })
}

fn text_charset(parts: &MediaParts) -> ForgeResult<Charset> {
    Ok(parts.charset.unwrap_or(Charset::Utf8))
}

fn enforce_text_charset(media: &str, bytes: &[u8]) -> ForgeResult<()> {
    let parts = parse_media(media)?;
    if text_charset(&parts)? == Charset::UsAscii && !bytes.is_ascii() {
        return Err(fail("US-ASCII request body contains non-ASCII bytes"));
    }
    Ok(())
}

fn required_string<'a>(body: Option<&'a Value>, message: &str) -> ForgeResult<&'a str> {
    body.and_then(Value::as_str).ok_or_else(|| fail(message))
}

fn reject_base64(body_base64: Option<&str>, codec: &str) -> ForgeResult<()> {
    if body_base64.is_some() {
        Err(fail(format!(
            "body_base64 is only supported for binary bodies, not {codec}"
        )))
    } else {
        Ok(())
    }
}

fn remap_contract(contract: &RequestBody, media: &str, essence: &str) -> RequestBody {
    if media == essence {
        return contract.clone();
    }
    let mut remapped = contract.clone();
    if let Some(schema) = contract.content.get(media) {
        remapped.content.insert(essence.to_owned(), schema.clone());
    }
    if let Some(encoding) = contract.encoding.get(media) {
        remapped
            .encoding
            .insert(essence.to_owned(), encoding.clone());
    }
    remapped
}

fn has_binary_string(schema: &Value, schemas: &BTreeMap<String, Value>) -> ForgeResult<bool> {
    let schema = crate::schema::binding_view(schema, schemas)?;
    let schema = schema.as_ref();
    if is_binary_string(schema) {
        return Ok(true);
    }
    let Some(object) = schema.as_object() else {
        return Ok(false);
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            for item in items {
                if has_binary_string(item, schemas)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn text_candidate(
    schema: &Value,
    schemas: &BTreeMap<String, Value>,
    require_explicit: bool,
) -> ForgeResult<Option<Value>> {
    let schema = crate::schema::binding_view(schema, schemas)?;
    let schema = schema.as_ref();
    match schema {
        Value::Bool(true) if !require_explicit => {
            return Ok(Some(Value::Object(Map::from_iter([(
                "type".to_owned(),
                Value::String("string".to_owned()),
            )]))));
        }
        Value::Bool(_) => return Ok(None),
        Value::Object(_) => {}
        _ => return Ok(None),
    }
    if let Some(string) = explicit_non_binary_string(schema) {
        return Ok(Some(string));
    }
    let object = schema.as_object().unwrap();
    for key in ["anyOf", "oneOf"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            let mut candidates = Vec::new();
            for item in items {
                if let Some(candidate) = text_candidate(item, schemas, require_explicit)? {
                    candidates.push(candidate);
                }
            }
            if candidates.len() == 1 {
                return Ok(Some(merge_parent_metadata(
                    object,
                    key,
                    candidates.remove(0),
                )));
            }
            return Ok(None);
        }
    }
    if let Some(items) = object.get("allOf").and_then(Value::as_array) {
        let mut merged = parent_without_combinator(object, "allOf");
        let mut found = false;
        for item in items {
            let Some(candidate) = text_candidate(item, schemas, require_explicit)? else {
                return Ok(None);
            };
            found = true;
            if let Some(fields) = candidate.as_object() {
                for (key, value) in fields {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
        if found {
            return Ok(Some(Value::Object(merged)));
        }
    }
    Ok(None)
}

fn explicit_non_binary_string(schema: &Value) -> Option<Value> {
    if is_binary_string(schema) || !schema_type_includes_string(schema) {
        return None;
    }
    let mut value = schema.clone();
    if let Value::Object(fields) = &mut value {
        fields.insert("type".to_owned(), Value::String("string".to_owned()));
    }
    Some(value)
}

fn is_binary_string(schema: &Value) -> bool {
    schema_type_includes_string(schema)
        && schema.get("format").and_then(Value::as_str) == Some("binary")
}

fn schema_type_includes_string(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(value)) => value == "string",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some("string")),
        _ => false,
    }
}

fn merge_parent_metadata(parent: &Map<String, Value>, combinator: &str, child: Value) -> Value {
    let mut merged = parent_without_combinator(parent, combinator);
    if let Some(fields) = child.as_object() {
        for (key, value) in fields {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

fn parent_without_combinator(parent: &Map<String, Value>, combinator: &str) -> Map<String, Value> {
    let mut merged = parent.clone();
    merged.remove(combinator);
    merged.remove("anyOf");
    merged.remove("oneOf");
    merged.remove("allOf");
    merged
}

fn decode_base64(input: &str) -> ForgeResult<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(fail("malformed base64 body"));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let (chunks, _) = bytes.as_chunks::<4>();
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        let last = index + 1 == chunk_count;
        let pad2 = chunk[2] == b'=';
        let pad3 = chunk[3] == b'=';
        if (pad2 && !pad3) || ((pad2 || pad3) && !last) {
            return Err(fail("malformed base64 body"));
        }
        let a = base64_value(chunk[0])?;
        let b = base64_value(chunk[1])?;
        let c = if pad2 { 0 } else { base64_value(chunk[2])? };
        let d = if pad3 { 0 } else { base64_value(chunk[3])? };
        out.push((a << 2) | (b >> 4));
        if !pad2 {
            out.push(((b & 0x0f) << 4) | (c >> 2));
        }
        if !pad3 {
            out.push(((c & 0x03) << 6) | d);
        }
    }
    Ok(out)
}

fn base64_value(byte: u8) -> ForgeResult<u8> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(fail("malformed base64 body")),
    }
}

fn validate_json_lines(bytes: &[u8], final_newline: bool) -> ForgeResult<()> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(fail("JSON Lines body must not start with a BOM"));
    }
    std::str::from_utf8(bytes).map_err(|_| fail("JSON Lines body must be valid UTF-8"))?;
    if final_newline && !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(fail("NDJSON body must end with a newline"));
    }
    if bytes.is_empty() {
        return Ok(());
    }
    let records = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    for line in records.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.iter().all(u8::is_ascii_whitespace) || line.contains(&b'\r') {
            return Err(fail(
                "invalid JSON Lines record: empty line or embedded carriage return",
            ));
        }
        serde_json::from_slice::<Value>(line)
            .map_err(|e| fail(format!("invalid JSON Lines record: {e}")))?;
    }
    Ok(())
}

fn fail(message: impl Into<String>) -> ForgeError {
    ForgeError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract(media: &str, schema: Value) -> RequestBody {
        RequestBody {
            required: true,
            content: BTreeMap::from([(media.to_owned(), schema)]),
            encoding: BTreeMap::new(),
        }
    }

    fn schemas() -> BTreeMap<String, Value> {
        BTreeMap::new()
    }

    fn err_text(result: ForgeResult<(Vec<u8>, String)>) -> String {
        result.unwrap_err().0
    }

    #[test]
    fn classifies_kv_binary_with_projected_text_alternative() {
        let schema = json!({
            "description":"A byte sequence to be stored, up to 25 MiB in length.",
            "example":"Some Value",
            "anyOf":[
                {"type":"string"},
                {"type":"string","format":"binary"}
            ]
        });
        assert_eq!(
            classify("Application/Octet-Stream", &schema, &schemas()).unwrap(),
            BodyCodec::Binary {
                text_alternative: true
            }
        );
        let projected = text_alternative_schema(&schema, &schemas())
            .unwrap()
            .unwrap();
        let validator = jsonschema::draft202012::options()
            .offline()
            .build(&projected)
            .unwrap();
        assert!(validator.is_valid(&json!("Some Value")));
        assert!(!validator.is_valid(&json!(null)));
        assert!(!validator.is_valid(&json!({})));
    }

    #[test]
    fn binary_body_base64_decodes_to_literal_bytes() {
        assert_eq!(decode_base64("SGVsbG8=").unwrap(), b"Hello");
        let request = contract(
            "application/octet-stream",
            json!({"type":"string","format":"binary"}),
        );
        let (bytes, content_type) = encode(
            &request,
            &schemas(),
            "application/octet-stream",
            None,
            Some("AP+ADQo="),
        )
        .unwrap();
        assert_eq!(bytes, vec![0, 255, 128, 13, 10]);
        assert_eq!(content_type, "application/octet-stream");
    }

    #[test]
    fn binary_rejects_wrong_slots_and_malformed_base64() {
        let binary = contract(
            "application/octet-stream",
            json!({"type":"string","format":"binary"}),
        );
        assert!(
            err_text(encode(
                &binary,
                &schemas(),
                "application/octet-stream",
                Some(&json!("text")),
                Some("dGV4dA=="),
            ))
            .contains("either body or body_base64")
        );
        assert!(
            err_text(encode(
                &binary,
                &schemas(),
                "application/octet-stream",
                None,
                Some("A==="),
            ))
            .contains("malformed base64")
        );
        assert!(
            err_text(encode(
                &binary,
                &schemas(),
                "application/octet-stream",
                Some(&json!("text")),
                None,
            ))
            .contains("requires body_base64")
        );
        let text = contract("text/plain", json!({"type":"string"}));
        assert!(
            err_text(encode(
                &text,
                &schemas(),
                "text/plain",
                None,
                Some("dGV4dA=="),
            ))
            .contains("only supported for binary")
        );
    }

    #[test]
    fn binary_string_body_requires_explicit_text_alternative() {
        let request = contract(
            "application/octet-stream",
            json!({
                "anyOf":[
                    {"type":"string"},
                    {"type":"string","format":"binary"}
                ]
            }),
        );
        assert_eq!(
            encode(
                &request,
                &schemas(),
                "application/octet-stream",
                Some(&json!("hello")),
                None,
            )
            .unwrap()
            .0,
            b"hello"
        );
    }

    #[test]
    fn empty_string_null_and_empty_binary_are_distinct() {
        let text = contract("text/plain", json!({"type":"string"}));
        assert_eq!(
            encode(&text, &schemas(), "text/plain", Some(&json!("")), None)
                .unwrap()
                .0,
            b""
        );
        let json_body = contract("application/json", json!({}));
        assert_eq!(
            encode(
                &json_body,
                &schemas(),
                "application/json",
                Some(&Value::Null),
                None,
            )
            .unwrap()
            .0,
            b"null"
        );
        let binary = contract(
            "application/octet-stream",
            json!({"type":"string","format":"binary"}),
        );
        assert_eq!(
            encode(
                &binary,
                &schemas(),
                "application/octet-stream",
                None,
                Some(""),
            )
            .unwrap()
            .0,
            b""
        );
    }

    #[test]
    fn json_media_ignores_binary_annotations() {
        let request = contract(
            "application/json",
            json!({"type":"string","format":"binary"}),
        );
        assert_eq!(
            classify(
                "application/json",
                &request.content["application/json"],
                &schemas()
            )
            .unwrap(),
            BodyCodec::Json
        );
        assert_eq!(
            encode(
                &request,
                &schemas(),
                "application/json",
                Some(&json!("AP+A")),
                None,
            )
            .unwrap()
            .0,
            br#""AP+A""#
        );
        assert!(
            err_text(encode(
                &request,
                &schemas(),
                "application/json",
                None,
                Some("AP+A"),
            ))
            .contains("only supported for binary")
        );
    }

    #[test]
    fn rejects_false_schema_and_undeclared_media() {
        let request = contract("text/plain", Value::Bool(false));
        assert!(
            err_text(encode(
                &request,
                &schemas(),
                "text/plain",
                Some(&json!("x")),
                None,
            ))
            .contains("forbidden")
        );
        assert!(
            err_text(encode(
                &request,
                &schemas(),
                "application/json",
                Some(&json!("x")),
                None,
            ))
            .contains("undeclared")
        );
    }

    #[test]
    fn validates_json_lines_and_ndjson_framing() {
        let jsonl = contract("application/jsonl", json!({"type":"string"}));
        let body = "{\"a\":1}\r\n{\"b\":2}";
        assert_eq!(
            encode(
                &jsonl,
                &schemas(),
                "application/jsonl",
                Some(&json!(body)),
                None,
            )
            .unwrap()
            .0,
            body.as_bytes()
        );
        let ndjson = contract("application/x-ndjson", json!({"type":"string"}));
        assert!(
            err_text(encode(
                &ndjson,
                &schemas(),
                "application/x-ndjson",
                Some(&json!("{\"a\":1}")),
                None,
            ))
            .contains("end with a newline")
        );
        assert_eq!(
            encode(
                &ndjson,
                &schemas(),
                "application/x-ndjson",
                Some(&json!("{\"a\":1}\r\n")),
                None,
            )
            .unwrap()
            .0,
            b"{\"a\":1}\r\n"
        );
        assert_eq!(
            encode(
                &ndjson,
                &schemas(),
                "application/x-ndjson",
                Some(&json!("")),
                None,
            )
            .unwrap()
            .0,
            b""
        );
    }

    #[test]
    fn rejects_invalid_json_lines_streams() {
        let jsonl = contract("application/jsonl", json!({"type":"string"}));
        assert!(
            err_text(encode(
                &jsonl,
                &schemas(),
                "application/jsonl",
                Some(&json!("{\"a\":1\n")),
                None,
            ))
            .contains("invalid JSON Lines")
        );
        assert!(
            err_text(encode(
                &jsonl,
                &schemas(),
                "application/jsonl",
                Some(&json!("\u{feff}{\"a\":1}\n")),
                None,
            ))
            .contains("BOM")
        );
        let binary_jsonl = contract(
            "application/jsonl",
            json!({"type":"string","format":"binary"}),
        );
        assert_eq!(
            classify(
                "application/jsonl",
                &binary_jsonl.content["application/jsonl"],
                &schemas()
            )
            .unwrap(),
            BodyCodec::JsonLines {
                binary: true,
                final_newline: false
            }
        );
        assert!(
            err_text(encode(
                &binary_jsonl,
                &schemas(),
                "application/jsonl",
                None,
                Some("//4="),
            ))
            .contains("valid UTF-8")
        );
    }

    #[test]
    fn text_charset_rules_are_explicit() {
        let utf8 = contract("text/plain;charset=UTF-8", json!({"type":"string"}));
        assert_eq!(
            encode(
                &utf8,
                &schemas(),
                "text/plain;charset=UTF-8",
                Some(&json!("snowman \u{2603}")),
                None,
            )
            .unwrap()
            .0,
            "snowman \u{2603}".as_bytes()
        );
        let ascii = contract("text/plain;charset=US-ASCII", json!({"type":"string"}));
        assert!(
            err_text(encode(
                &ascii,
                &schemas(),
                "text/plain;charset=US-ASCII",
                Some(&json!("caf\u{00e9}")),
                None,
            ))
            .contains("US-ASCII")
        );
        let latin1 = contract("text/plain;charset=iso-8859-1", json!({"type":"string"}));
        assert!(
            err_text(encode(
                &latin1,
                &schemas(),
                "text/plain;charset=iso-8859-1",
                Some(&json!("hello")),
                None,
            ))
            .contains("unsupported charset")
        );
    }

    #[test]
    fn text_media_rejects_object_only_schema() {
        let request = contract("text/plain;charset=UTF-8", json!({"type":"object"}));
        assert_eq!(
            classify(
                "text/plain;charset=UTF-8",
                &request.content["text/plain;charset=UTF-8"],
                &schemas(),
            )
            .unwrap(),
            BodyCodec::Unsupported("text body schema is not a string")
        );
        assert!(
            err_text(encode(
                &request,
                &schemas(),
                "text/plain;charset=UTF-8",
                Some(&json!({"x":1})),
                None,
            ))
            .contains("text body schema is not a string")
        );
    }

    #[test]
    fn form_and_multipart_delegate_to_existing_helpers() {
        let form = contract(
            "application/x-www-form-urlencoded",
            json!({"type":"object"}),
        );
        assert_eq!(
            encode(
                &form,
                &schemas(),
                "application/x-www-form-urlencoded",
                Some(&json!({"a":"b c"})),
                None,
            )
            .unwrap()
            .0,
            b"a=b+c"
        );
        let multipart = contract(
            "multipart/form-data",
            json!({
                "type":"object",
                "properties":{"file":{"type":"string","format":"binary"}}
            }),
        );
        let (bytes, content_type) = encode(
            &multipart,
            &schemas(),
            "multipart/form-data",
            Some(&json!({"file":"AP8B"})),
            None,
        )
        .unwrap();
        assert_eq!(content_type, "multipart/form-data; boundary=incurs-forge-0");
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("Content-Type: application/octet-stream")
        );
    }
}
