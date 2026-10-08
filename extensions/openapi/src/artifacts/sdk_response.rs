//! Additive typed response APIs for generated SDKs.
use super::{pascal, rust_string, status_variant, symbols::SdkSymbols};
use crate::{OpenApiResult, Operation, ResolvedOpenApi};
use serde_json::Value;
use std::collections::BTreeSet;

#[path = "sdk_response_codec.rs"]
mod codec;
#[path = "sdk_response_models.rs"]
mod models;
#[path = "sdk_response_runtime.rs"]
mod runtime;

pub(super) struct ResponseArtifacts {
    pub(super) source: String,
    pub(super) graph: Value,
}

#[derive(Clone, Copy, PartialEq)]
enum Codec {
    Json,
    Text,
    Binary,
    Unsupported,
}

impl Codec {
    fn name(self) -> &'static str {
        match self {
            Self::Json => "Json",
            Self::Text => "Text",
            Self::Binary => "Binary",
            Self::Unsupported => "Unsupported",
        }
    }
}

struct MediaEntry {
    name: String,
    pointer: String,
    ty: String,
    codec: Codec,
    binary_min: Option<u64>,
    binary_max: Option<u64>,
    binary_allowed: bool,
    binary_supported: bool,
}

pub(super) fn generate(
    contract: &ResolvedOpenApi,
    symbols: &SdkSymbols,
) -> OpenApiResult<ResponseArtifacts> {
    let compiled = models::compile(contract, symbols.reserved_names())?;
    let mut used = compiled.used_names;
    let mut methods = contract
        .operations
        .iter()
        .map(|op| symbols.operation(op).method.clone())
        .collect::<BTreeSet<_>>();
    let mut declarations = String::new();
    let mut clients = String::new();
    for operation in &contract.operations {
        let raw_type = &symbols.operation(operation).response_type;
        let typed_type = allocate(
            &format!("{}TypedResponse", pascal(&operation.name)),
            &mut used,
        );
        let typed_method = allocate_method(
            &format!("{}_typed", symbols.operation(operation).method),
            &mut methods,
        );
        let mut bodies = String::new();
        let mut variants = String::new();
        let mut arms = String::new();
        let mut statuses = String::new();
        for (status, response) in &operation.responses {
            let variant = variant(status);
            let mut entries = Vec::new();
            for name in response.content.keys() {
                let model = &compiled.media[&(operation.id.clone(), status.clone(), name.clone())];
                let shape = crate::schema::binding_view(&model.schema, &contract.schemas)?;
                let codec = classify(name, &shape, model.binary);
                let (binary_min, binary_max, binary_allowed, binary_supported) = if codec
                    == Codec::Binary
                {
                    referenced_binary_limits(&model.schema, &compiled.graph, &mut BTreeSet::new())
                } else {
                    binary_limits(&shape)
                };
                entries.push(MediaEntry {
                    name: name.clone(),
                    pointer: model.schema_pointer.clone(),
                    ty: if codec == Codec::Binary {
                        "Vec<u8>".into()
                    } else {
                        model.rust_type.clone()
                    },
                    codec,
                    binary_min,
                    binary_max,
                    binary_allowed,
                    binary_supported,
                });
            }
            let body_type = if entries.len() > 1 {
                let name = allocate(
                    &format!("{}{}Body", pascal(&operation.name), variant),
                    &mut used,
                );
                bodies.push_str(&render_body_enum(&name, &entries));
                name
            } else {
                entries
                    .first()
                    .map(|entry| entry.ty.clone())
                    .unwrap_or_else(|| "()".into())
            };
            variants.push_str(&format!(
                "    /// Decoded response for contract status {}.\n    {variant}(DecodedResponse<{body_type}>),\n",
                rust_string(status),
            ));
            statuses.push_str(&format!(
                "            Self::{variant}(value) => value.raw.status,\n"
            ));
            arms.push_str(&render_arm(
                operation,
                raw_type,
                &typed_type,
                &variant,
                &body_type,
                &entries,
            ));
        }
        declarations.push_str(&bodies);
        declarations.push_str(&format!(
            "/// Typed responses for operation {} with raw transport evidence.\n#[derive(Clone, Debug, PartialEq)]\npub enum {typed_type} {{\n{variants}    /// A status with no declared response contract.\n    Other(OperationResponse),\n}}\n\nimpl {typed_type} {{\n    /// Return the original response status.\n    pub fn status(&self) -> u16 {{\n        match self {{\n{statuses}            Self::Other(raw) => raw.status,\n        }}\n    }}\n}}\n\nimpl {raw_type} {{\n    /// Decode the selected status and media while retaining the raw response.\n    pub fn decode(self) -> Result<{typed_type}, ResponseDecodeError> {{\n        match self {{\n{arms}            Self::Other(raw) => Ok({typed_type}::Other(raw)),\n        }}\n    }}\n}}\n\n",
            rust_string(&operation.id),
        ));
        let args_type = &symbols.operation(operation).args_type;
        let raw_method = &symbols.operation(operation).method;
        clients.push_str(&format!(
            "    /// Invoke operation {} and decode its declared response media.\n    pub async fn {typed_method}(&self, args: {args_type}) -> Result<{typed_type}, ClientError<T::Error>> {{\n        self.{raw_method}(args).await.map_err(ClientError::Transport)?.decode().map_err(ClientError::Decode)\n    }}\n\n",
            rust_string(&operation.id),
        ));
    }
    let source = format!(
        "\n#[cfg(feature = \"typed-responses\")]\nmod typed_responses {{\nuse super::*;\n{}\n{}\n{}\n{}\n{}\nimpl<T: Transport> Client<T> {{\n{clients}}}\n}}\n\n#[cfg(feature = \"typed-responses\")]\npub use typed_responses::*;\n",
        runtime::RUNTIME,
        VALIDATION_RUNTIME,
        codec::RUNTIME,
        compiled.source,
        declarations,
    );
    Ok(ResponseArtifacts {
        source,
        graph: compiled.graph,
    })
}

fn allocate(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = base.to_owned();
    let mut suffix = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}{suffix}");
        suffix += 1;
    }
    name
}

fn allocate_method(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = base.to_owned();
    let mut suffix = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    name
}

fn variant(status: &str) -> String {
    if status == "default" {
        "Default".into()
    } else {
        status_variant(status)
    }
}

fn media_variants(entries: &[MediaEntry]) -> Vec<String> {
    super::unique_names(
        entries.iter().map(|entry| pascal(&entry.name)).collect(),
        &[],
        "",
    )
}

fn render_body_enum(name: &str, entries: &[MediaEntry]) -> String {
    let mut out = format!(
        "/// Declared response media alternatives.\n#[derive(Clone, Debug, PartialEq)]\npub enum {name} {{\n"
    );
    for (entry, variant) in entries.iter().zip(media_variants(entries)) {
        out.push_str(&format!(
            "    /// Media type {}.\n    {variant}({}),\n",
            rust_string(&entry.name),
            entry.ty
        ));
    }
    out.push_str("}\n\n");
    out
}

fn render_arm(
    operation: &Operation,
    _raw_type: &str,
    typed_type: &str,
    variant: &str,
    body_type: &str,
    entries: &[MediaEntry],
) -> String {
    let mut media = String::new();
    for entry in entries {
        media.push_str(&format!(
            "ResponseMedia {{ name: {}, pointer: {}, codec: ResponseCodec::{}, binary_min: {:?}, binary_max: {:?}, binary_allowed: {}, binary_supported: {} }},",
            rust_string(&entry.name), rust_string(&entry.pointer), entry.codec.name(),
            entry.binary_min, entry.binary_max, entry.binary_allowed, entry.binary_supported,
        ));
    }
    let projection = |entry: &MediaEntry| {
        if entry.codec == Codec::Binary {
            "response_bytes(&value)".to_owned()
        } else {
            format!(
                "<{} as DecodeResponseValue>::decode_response_value(&value)",
                entry.ty
            )
        }
    };
    let project = if entries.len() > 1 {
        let mut choices = String::new();
        for (index, (entry, media_variant)) in
            entries.iter().zip(media_variants(entries)).enumerate()
        {
            choices.push_str(&format!(
                "Some({index}) => {body_type}::{media_variant}({}.map_err(|message| response_projection_error(&raw, &payload.media_type, message))?),",
                projection(entry),
            ));
        }
        format!(
            "match payload.media_index {{ {choices} _ => return Err(response_projection_error(&raw, &payload.media_type, \"missing response media selection\".into())), }}"
        )
    } else {
        let expression = entries
            .first()
            .map(projection)
            .unwrap_or_else(|| "<() as DecodeResponseValue>::decode_response_value(&value)".into());
        format!(
            "{expression}.map_err(|message| response_projection_error(&raw, &payload.media_type, message))?"
        )
    };
    format!(
        "            Self::{variant}(raw) => {{\n                let payload = decode_response_payload(&raw, {}, &[{media}])?;\n                let body = match payload.body {{\n                    Field::Missing => Field::Missing,\n                    Field::Null => Field::Null,\n                    Field::Default(value) => Field::Default(value),\n                    Field::Value(value) => Field::Value({project}),\n                }};\n                Ok({typed_type}::{variant}(DecodedResponse {{ raw, media_type: payload.media_type, body }}))\n            }}\n",
        rust_string(&operation.method),
    )
}

fn classify(media: &str, schema: &Value, binary: bool) -> Codec {
    let essence = media
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if essence == "application/json" || essence.ends_with("+json") {
        Codec::Json
    } else if essence.starts_with("text/")
        || matches!(
            essence.as_str(),
            "application/javascript"
                | "application/xml"
                | "application/x-pem-file"
                | "message/rfc822"
        )
        || (!binary
            && matches!(
                essence.as_str(),
                "application/jsonl" | "application/x-ndjson"
            ))
    {
        Codec::Text
    } else if binary
        || essence.starts_with("image/")
        || essence.starts_with("audio/")
        || essence.starts_with("video/")
        || matches!(
            essence.as_str(),
            "application/octet-stream"
                | "application/pdf"
                | "application/zip"
                | "application/vnd.tcpdump.pcap"
        )
        || schema == &Value::Bool(true)
        || schema.as_object().is_some_and(|map| map.is_empty())
    {
        Codec::Binary
    } else if schema.get("type").and_then(Value::as_str) == Some("string") {
        Codec::Text
    } else {
        Codec::Unsupported
    }
}

fn referenced_binary_limits(
    schema: &Value,
    graph: &Value,
    active: &mut BTreeSet<String>,
) -> (Option<u64>, Option<u64>, bool, bool) {
    let Some(fields) = schema.as_object() else {
        return binary_limits(schema);
    };
    let mut own = fields.clone();
    let reference = own.remove("$ref");
    let branches = own.remove("allOf");
    let mut result = binary_limits(&Value::Object(own));
    if let Some(reference) = reference {
        let Some(reference) = reference.as_str() else {
            return (None, None, true, false);
        };
        let name = reference
            .strip_prefix("#/components/schemas/")
            .or_else(|| reference.strip_prefix("#/$defs/"))
            .map(|name| name.replace("~1", "/").replace("~0", "~"));
        let target = name.as_ref().and_then(|name| graph.get("$defs")?.get(name));
        if !active.insert(reference.to_string()) {
            return (None, None, true, false);
        }
        let nested = target.map_or((None, None, true, false), |target| {
            referenced_binary_limits(target, graph, active)
        });
        active.remove(reference);
        intersect_binary_limits(&mut result, nested);
    }
    if let Some(branches) = branches {
        let Some(branches) = branches.as_array().filter(|branches| !branches.is_empty()) else {
            return (None, None, true, false);
        };
        for branch in branches {
            intersect_binary_limits(&mut result, referenced_binary_limits(branch, graph, active));
        }
    }
    result
}

fn intersect_binary_limits(
    current: &mut (Option<u64>, Option<u64>, bool, bool),
    other: (Option<u64>, Option<u64>, bool, bool),
) {
    current.0 = match (current.0, other.0) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    };
    current.1 = match (current.1, other.1) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    current.2 &= other.2;
    current.3 &= other.3;
}

fn binary_limits(schema: &Value) -> (Option<u64>, Option<u64>, bool, bool) {
    if schema == &Value::Bool(false) {
        return (None, None, false, true);
    }
    if schema == &Value::Bool(true) {
        return (None, None, true, true);
    }
    let Some(object) = schema.as_object() else {
        return (None, None, true, false);
    };
    let supported = object.keys().all(|key| {
        key.starts_with("x-")
            || matches!(
                key.as_str(),
                "type"
                    | "format"
                    | "minLength"
                    | "maxLength"
                    | "title"
                    | "description"
                    | "$comment"
                    | "readOnly"
                    | "writeOnly"
                    | "deprecated"
                    | "default"
                    | "example"
                    | "examples"
                    | "externalDocs"
                    | "xml"
                    | "contentMediaType"
                    | "contentEncoding"
                    | "nullable"
            )
    }) && object.get("type").is_none_or(|kind| {
        kind.as_str() == Some("string")
            || kind.as_array().is_some_and(|kinds| {
                kinds
                    .iter()
                    .all(|kind| matches!(kind.as_str(), Some("string" | "null")))
            })
    }) && object
        .get("minLength")
        .is_none_or(|value| value.as_u64().is_some());
    (
        object.get("minLength").and_then(Value::as_u64),
        object.get("maxLength").and_then(Value::as_u64),
        true,
        supported,
    )
}

const VALIDATION_RUNTIME: &str = r#"
std::thread_local! {
    static RESPONSE_SCHEMA_VALIDATOR: Result<jsonschema::Validator, String> = {
        serde_json::from_str::<serde_json::Value>(include_str!("response-schemas.json"))
            .map_err(|error| error.to_string())
            .and_then(|schema| jsonschema::draft202012::options().offline()
                .should_validate_formats(false).build(&schema).map_err(|error| error.to_string()))
    };
}

fn response_validation_value(value: &JsonValue) -> Option<serde_json::Value> {
    use serde_json::Value;
    match value {
        JsonValue::Invalid | JsonValue::Bytes(_) => None,
        JsonValue::Null => Some(Value::Null),
        JsonValue::Bool(value) => Some(Value::Bool(*value)),
        JsonValue::String(value) => Some(Value::String(value.clone())),
        JsonValue::Integer(value) => Some(Value::Number((*value).into())),
        JsonValue::Unsigned(value) => Some(Value::Number((*value).into())),
        JsonValue::Number(value) => serde_json::Number::from_f64(*value).map(Value::Number),
        JsonValue::ExactNumber(value) if valid_json_number(value) => value.parse().ok().map(Value::Number),
        JsonValue::ExactNumber(_) => None,
        JsonValue::Array(values) => values.iter().map(response_validation_value).collect::<Option<Vec<_>>>().map(Value::Array),
        JsonValue::Object(fields) => {
            let mut values = serde_json::Map::new();
            for (key, value) in fields {
                if values.insert(key.clone(), response_validation_value(value)?).is_some() { return None; }
            }
            Some(Value::Object(values))
        }
    }
}

fn response_json_matches(value: &JsonValue, pointer: &str) -> bool {
    let Some(value) = response_validation_value(value) else { return false; };
    let instance = serde_json::Value::Object([(pointer.to_owned(), value)].into_iter().collect());
    RESPONSE_SCHEMA_VALIDATOR.with(|validator| validator.as_ref().is_ok_and(|validator| validator.is_valid(&instance)))
}

fn response_bytes(value: &JsonValue) -> Result<Vec<u8>, String> {
    match value {
        JsonValue::Bytes(bytes) => Ok(bytes.clone()),
        _ => Err("expected raw response bytes".into()),
    }
}

fn response_projection_error(raw: &OperationResponse, media_type: &Option<String>, message: String) -> ResponseDecodeError {
    ResponseDecodeError {
        kind: ResponseDecodeErrorKind::Projection,
        message,
        media_type: media_type.clone(),
        raw: raw.clone(),
    }
}
"#;
