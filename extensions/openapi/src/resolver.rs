//! OpenAPI resolver for the isolated OpenAPI proof.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{
    ApiResponse, OpenApiError, OpenApiResult, Operation, Overlay, Parameter, RequestBody,
    ResolveOptions, ResolvedOpenApi, Server,
};

/// Resolves an OpenAPI 3.0, 3.1, or 3.2 document into the OpenAPI proof model.
pub fn resolve_document(
    document: &Value,
    mut options: ResolveOptions,
) -> OpenApiResult<ResolvedOpenApi> {
    let document = apply_overlays(document.clone(), &options.overlays)?;
    options
        .documents
        .insert(options.document_uri.clone(), document.clone());
    let root = document
        .as_object()
        .ok_or_else(|| OpenApiError("root document must be an object".to_string()))?;
    let supports_32 = validate_openapi(root)?;
    let info = object_field(root, "info")?;
    let title = info
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("OpenAPI")
        .to_string();
    let server_url = root
        .get("servers")
        .map(array_value("servers"))
        .transpose()?
        .and_then(|servers| servers.first())
        .and_then(|server| server.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let root_servers = resolve_servers(root.get("servers"))?;
    let root_security = root
        .get("security")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let mut registry = resolve_component_schemas(&document, &options)?;
    let paths = object_field(root, "paths")?;
    let mut operations = Vec::new();
    let mut seen = BTreeSet::new();
    for (path, item) in paths {
        if path.starts_with("x-") {
            continue;
        }
        if !path.starts_with('/') {
            return Err(OpenApiError(format!("path key {path} must start with /")));
        }
        let (item, item_uri) =
            resolve_ref_value(item, &options, &options.document_uri, &mut BTreeSet::new())?;
        let item_object = item
            .as_object()
            .ok_or_else(|| OpenApiError(format!("path item {path} must be an object")))?;
        let inherited_servers = match item_object.get("servers") {
            Some(value) => resolve_servers(Some(value))?,
            None => root_servers.clone(),
        };
        let inherited = optional_array(item_object, "parameters")?
            .cloned()
            .unwrap_or_default();
        for (method, operation_value) in operation_entries(item_object, supports_32)? {
            let operation_object = operation_value.as_object().ok_or_else(|| {
                OpenApiError(format!("operation {method} {path} must be an object"))
            })?;
            let name = operation_name(operation_object, &method, path)?;
            let id = format!("{}/{}", options.namespace, name);
            if !seen.insert(id.clone()) {
                return Err(OpenApiError(format!("duplicate operation id {id}")));
            }
            let mut raw_parameters = inherited.clone();
            raw_parameters.extend(
                optional_array(operation_object, "parameters")?
                    .cloned()
                    .unwrap_or_default(),
            );
            operations.push(Operation {
                id,
                name,
                description: operation_object
                    .get("description")
                    .or_else(|| operation_object.get("summary"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                method,
                path: path.clone(),
                servers: match operation_object.get("servers") {
                    Some(value) => resolve_servers(Some(value))?,
                    None => inherited_servers.clone(),
                },
                parameters: resolve_parameters(
                    &raw_parameters,
                    &options,
                    &item_uri,
                    supports_32,
                    &mut registry,
                )?,
                request_body: operation_object
                    .get("requestBody")
                    .map(|body| resolve_request_body(body, &options, &item_uri, &mut registry))
                    .transpose()?,
                responses: resolve_responses(
                    operation_object.get("responses"),
                    &options,
                    &item_uri,
                    &mut registry,
                )?,
                security: operation_object
                    .get("security")
                    .cloned()
                    .unwrap_or_else(|| root_security.clone()),
            });
        }
    }
    operations.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(ResolvedOpenApi {
        openapi_version: root["openapi"].as_str().unwrap().to_owned(),
        json_schema_dialect: root
            .get("jsonSchemaDialect")
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| OpenApiError("jsonSchemaDialect must be a string".into()))
            })
            .transpose()?,
        namespace: options.namespace,
        title,
        server_url,
        operations,
        schemas: registry.schemas,
    })
}

fn resolve_servers(value: Option<&Value>) -> OpenApiResult<Vec<Server>> {
    let servers: Vec<Server> = match value {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| OpenApiError(format!("invalid servers: {error}")))?,
        None => Vec::new(),
    };
    if servers.is_empty() {
        return Ok(vec![Server::default()]);
    }
    for server in &servers {
        for (name, variable) in &server.variables {
            if variable
                .values
                .as_ref()
                .is_some_and(|values| values.is_empty() || !values.contains(&variable.default))
            {
                return Err(OpenApiError(format!(
                    "server variable {name} default must belong to a nonempty enum"
                )));
            }
        }
    }
    Ok(servers)
}

fn apply_overlays(mut document: Value, overlays: &[Overlay]) -> OpenApiResult<Value> {
    for overlay in overlays {
        let Some(target) = document.pointer_mut(&overlay.pointer) else {
            return Err(OpenApiError(format!(
                "overlay pointer not found: {}",
                overlay.pointer
            )));
        };
        *target = overlay.value.clone();
    }
    Ok(document)
}

fn validate_openapi(root: &serde_json::Map<String, Value>) -> OpenApiResult<bool> {
    let version = root
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| OpenApiError("openapi version string is required".into()))?;
    let pieces: Vec<_> = version.split('.').collect();
    if pieces.len() == 3
        && pieces[0] == "3"
        && matches!(pieces[1], "0" | "1" | "2")
        && !pieces[2].is_empty()
        && pieces[2].bytes().all(|b| b.is_ascii_digit())
    {
        Ok(pieces[1] == "2")
    } else {
        Err(OpenApiError(format!(
            "unsupported openapi version {version}"
        )))
    }
}

fn operation_entries(
    item: &serde_json::Map<String, Value>,
    supports_32: bool,
) -> OpenApiResult<Vec<(String, &Value)>> {
    let fixed = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace", "query",
    ];
    if !supports_32 && (item.contains_key("query") || item.contains_key("additionalOperations")) {
        return Err(OpenApiError(
            "query and additionalOperations require OpenAPI 3.2".into(),
        ));
    }
    let mut entries = Vec::new();
    for method in fixed {
        if let Some(operation) = item.get(method) {
            entries.push((method.to_ascii_uppercase(), operation));
        }
    }
    if let Some(additional) = item.get("additionalOperations") {
        let additional = additional
            .as_object()
            .ok_or_else(|| OpenApiError("additionalOperations must be an object".into()))?;
        for (method, operation) in additional {
            crate::parameters::token(method)?;
            if fixed
                .iter()
                .any(|name| name.to_ascii_uppercase() == *method)
            {
                return Err(OpenApiError(format!(
                    "additionalOperations must not redefine {method}"
                )));
            }
            entries.push((method.clone(), operation));
        }
    }
    Ok(entries)
}

#[derive(Default)]
struct SchemaRegistry {
    schemas: BTreeMap<String, Value>,
    names: BTreeMap<String, String>,
    resolving: BTreeSet<String>,
}

fn pointer_name(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

fn resolve_component_schemas(
    document: &Value,
    options: &ResolveOptions,
) -> OpenApiResult<SchemaRegistry> {
    let mut registry = SchemaRegistry::default();
    if let Some(schemas) = document.pointer("/components/schemas") {
        let schemas = schemas
            .as_object()
            .ok_or_else(|| OpenApiError("components.schemas must be an object".into()))?;
        for name in schemas.keys() {
            registry.names.insert(
                format!(
                    "{}#/components/schemas/{}",
                    options.document_uri,
                    pointer_name(name)
                ),
                name.clone(),
            );
        }
        for name in schemas.keys() {
            intern_schema(
                &format!("#/components/schemas/{}", pointer_name(name)),
                options,
                &options.document_uri,
                &mut registry,
            )?;
        }
    }
    Ok(registry)
}

fn intern_schema(
    reference: &str,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<String> {
    use sha2::{Digest, Sha256};
    let (raw_uri, pointer) = split_ref(reference);
    let uri = resolve_uri(base_uri, &raw_uri);
    let key = format!("{uri}#{pointer}");
    let target = resolve_target(reference, options, base_uri, &mut BTreeSet::new())?
        .ok_or_else(|| OpenApiError(format!("unresolved schema reference {reference}")))?;
    let name = if let Some(name) = registry.names.get(&key) {
        name.clone()
    } else {
        let stem = format!("External{:x}", Sha256::digest(key.as_bytes()));
        let mut name = stem.clone();
        let mut suffix = 2;
        while registry.names.values().any(|existing| existing == &name) {
            name = format!("{stem}_{suffix}");
            suffix += 1;
        }
        registry.names.insert(key.clone(), name.clone());
        name
    };
    if !registry.schemas.contains_key(&name) && registry.resolving.insert(key.clone()) {
        let schema = resolve_schema(target.value, options, &target.uri, registry)?;
        registry.resolving.remove(&key);
        registry.schemas.insert(name.clone(), schema);
    }
    Ok(format!("#/components/schemas/{}", pointer_name(&name)))
}

fn operation_name(
    operation: &serde_json::Map<String, Value>,
    method: &str,
    path: &str,
) -> OpenApiResult<String> {
    match operation.get("operationId") {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(OpenApiError(format!(
            "operationId for {method} {path} must not be empty"
        ))),
        Some(_) => Err(OpenApiError(format!(
            "operationId for {method} {path} must be a string"
        ))),
        None => Ok(sanitize(&format!("{method}_{path}"))),
    }
}

fn resolve_parameters(
    raw: &[Value],
    options: &ResolveOptions,
    base_uri: &str,
    supports_32: bool,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<Vec<Parameter>> {
    let mut by_key = BTreeMap::new();
    for parameter in raw {
        let (parameter, source_uri) =
            resolve_ref_value(parameter, options, base_uri, &mut BTreeSet::new())?;
        let base_uri = source_uri.as_str();
        let object = parameter
            .as_object()
            .ok_or_else(|| OpenApiError("parameter must be an object".into()))?;
        let name = string_field(object, "name")?;
        let location = string_field(object, "in")?;
        if !matches!(
            location.as_str(),
            "path" | "query" | "header" | "cookie" | "querystring"
        ) {
            return Err(OpenApiError(format!(
                "unsupported parameter location {location}"
            )));
        }
        if location == "header"
            && ["accept", "content-type", "authorization"]
                .iter()
                .any(|n| name.eq_ignore_ascii_case(n))
        {
            continue;
        }
        if location == "querystring" && !supports_32 {
            return Err(OpenApiError(
                "querystring parameters require OpenAPI 3.2".into(),
            ));
        }
        let required = object
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(location == "path");
        let (schema, content) = if let Some(content) = object.get("content") {
            if ["schema", "style", "explode", "allowReserved"]
                .iter()
                .any(|key| object.contains_key(*key))
            {
                return Err(OpenApiError(format!(
                    "parameter {location}.{name} cannot combine content with schema serialization fields"
                )));
            }
            let schemas = resolve_content(content, options, base_uri, registry)?;
            if schemas.len() != 1 {
                return Err(OpenApiError(
                    "parameter content must contain exactly one media type".into(),
                ));
            }
            let (media_type, schema) = schemas.into_iter().next().unwrap();
            let mut encodings = resolve_encodings(content, options, base_uri, registry)?;
            let encoding = encodings.remove(&media_type).unwrap_or_default();
            (
                schema,
                Some(crate::ParameterContent {
                    media_type,
                    encoding,
                }),
            )
        } else {
            if location == "querystring" {
                return Err(OpenApiError(
                    "querystring parameters require content".into(),
                ));
            }
            (
                object
                    .get("schema")
                    .map(|s| resolve_schema(s, options, base_uri, registry))
                    .transpose()?
                    .unwrap_or(Value::Object(Default::default())),
                None,
            )
        };
        let style = object
            .get("style")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_style(&location).into());
        if style == "cookie" && (!supports_32 || location != "cookie") {
            return Err(OpenApiError(
                "cookie style requires an OpenAPI 3.2 cookie parameter".into(),
            ));
        }
        let explode = object
            .get("explode")
            .and_then(Value::as_bool)
            .unwrap_or(matches!(style.as_str(), "form" | "cookie"));
        by_key.insert(
            (location.clone(), name.clone()),
            Parameter {
                name,
                location,
                required,
                schema,
                style,
                explode,
                content,
            },
        );
    }
    let parameters: Vec<Parameter> = by_key.into_values().collect();
    let whole = parameters
        .iter()
        .filter(|p| p.location == "querystring")
        .count();
    if whole > 1 || (whole == 1 && parameters.iter().any(|p| p.location == "query")) {
        return Err(OpenApiError(
            "one querystring parameter cannot coexist with another querystring or query parameter"
                .into(),
        ));
    }
    Ok(parameters)
}

fn resolve_request_body(
    body: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<RequestBody> {
    let (body, source_uri) = resolve_ref_value(body, options, base_uri, &mut BTreeSet::new())?;
    let base_uri = source_uri.as_str();
    let object = body
        .as_object()
        .ok_or_else(|| OpenApiError("requestBody must be an object".to_string()))?;
    Ok(RequestBody {
        required: object
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        content: resolve_content(
            required_field(object, "content")?,
            options,
            base_uri,
            registry,
        )?,
        encoding: resolve_encodings(
            required_field(object, "content")?,
            options,
            base_uri,
            registry,
        )?,
    })
}

fn resolve_encodings(
    content: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<BTreeMap<String, BTreeMap<String, crate::model::Encoding>>> {
    let entries = content
        .as_object()
        .ok_or_else(|| OpenApiError("content must be an object".into()))?;
    let mut result = BTreeMap::new();
    for (media, entry) in entries {
        let (entry, source_uri) =
            resolve_ref_value(entry, options, base_uri, &mut BTreeSet::new())?;
        let base_uri = source_uri.as_str();
        if let Some(encoding) = entry.get("encoding") {
            let mut encoding: BTreeMap<String, crate::model::Encoding> =
                serde_json::from_value(encoding.clone())
                    .map_err(|e| OpenApiError(format!("invalid encoding for {media}: {e}")))?;
            for entry in encoding.values_mut() {
                for header in entry.headers.values_mut() {
                    *header = resolve_header(header, options, base_uri, registry)?;
                }
            }
            result.insert(media.clone(), encoding);
        }
    }
    Ok(result)
}

fn resolve_responses(
    responses: Option<&Value>,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<BTreeMap<String, ApiResponse>> {
    let object = responses
        .and_then(Value::as_object)
        .ok_or_else(|| OpenApiError("responses object is required".to_string()))?;
    let mut out = BTreeMap::new();
    for (status, response) in object {
        validate_status(status)?;
        let (response, source_uri) =
            resolve_ref_value(response, options, base_uri, &mut BTreeSet::new())?;
        let base_uri = source_uri.as_str();
        let response_object = response
            .as_object()
            .ok_or_else(|| OpenApiError(format!("response {status} must be an object")))?;
        let mut headers = BTreeMap::new();
        if let Some(header_map) = optional_object(response_object, "headers")? {
            for (name, header) in header_map {
                headers.insert(
                    name.clone(),
                    resolve_header(header, options, base_uri, registry)?,
                );
            }
        }
        out.insert(
            status.clone(),
            ApiResponse {
                description: response_object
                    .get("description")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OpenApiError(format!("response {status} description string is required"))
                    })?
                    .to_string(),
                content: response_object
                    .get("content")
                    .map(|content| resolve_content(content, options, base_uri, registry))
                    .transpose()?
                    .unwrap_or_default(),
                headers,
            },
        );
    }
    Ok(out)
}

fn resolve_header(
    header: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<Value> {
    let (header, source_uri) = resolve_ref_value(header, options, base_uri, &mut BTreeSet::new())?;
    let base_uri = source_uri.as_str();
    let Some(object) = header.as_object() else {
        return Err(OpenApiError(
            "response header must be an object".to_string(),
        ));
    };
    let mut out = object.clone();
    if let Some(schema) = object.get("schema") {
        out.insert(
            "schema".to_string(),
            resolve_schema(schema, options, base_uri, registry)?,
        );
    }
    Ok(Value::Object(out))
}

fn resolve_content(
    content: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<BTreeMap<String, Value>> {
    let object = content
        .as_object()
        .ok_or_else(|| OpenApiError("content must be an object".to_string()))?;
    let mut out = BTreeMap::new();
    for (media_type, entry) in object {
        let (entry, source_uri) =
            resolve_ref_value(entry, options, base_uri, &mut BTreeSet::new())?;
        let base_uri = source_uri.as_str();
        let entry_object = entry
            .as_object()
            .ok_or_else(|| OpenApiError(format!("content entry {media_type} must be an object")))?;
        if ["itemSchema", "prefixEncoding", "itemEncoding"]
            .iter()
            .any(|field| entry_object.contains_key(*field))
        {
            return Err(OpenApiError(format!(
                "sequential or positional media contract is not yet supported: {media_type}"
            )));
        }
        let schema = entry_object
            .get("schema")
            .map(|schema| resolve_schema(schema, options, base_uri, registry))
            .transpose()?
            .unwrap_or(Value::Object(Default::default()));
        out.insert(media_type.clone(), schema);
    }
    Ok(out)
}

fn resolve_schema(
    value: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    registry: &mut SchemaRegistry,
) -> OpenApiResult<Value> {
    let Some(object) = value.as_object() else {
        return Ok(value.clone());
    };
    let mut out = object.clone();
    if let Some(reference) = object.get("$ref") {
        let reference = reference
            .as_str()
            .ok_or_else(|| OpenApiError("schema $ref must be a string".into()))?;
        out.insert(
            "$ref".into(),
            Value::String(intern_schema(reference, options, base_uri, registry)?),
        );
    }
    // Visit schema positions only. Defaults and examples are instance data.
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(children) = optional_object(object, key)? {
            let mut mapped = serde_json::Map::new();
            for (name, child) in children {
                mapped.insert(
                    name.clone(),
                    resolve_schema(child, options, base_uri, registry)?,
                );
            }
            out.insert(key.into(), Value::Object(mapped));
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "contains",
        "propertyNames",
        "if",
        "then",
        "else",
        "unevaluatedProperties",
        "unevaluatedItems",
        "additionalItems",
    ] {
        if let Some(child) = object.get(key) {
            if !child.is_object() && !child.is_boolean() {
                return Err(OpenApiError(format!(
                    "{key} must be a boolean or schema object"
                )));
            }
            out.insert(
                key.into(),
                resolve_schema(child, options, base_uri, registry)?,
            );
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get(key) {
            let children = children
                .as_array()
                .ok_or_else(|| OpenApiError(format!("{key} must be an array")))?;
            let mapped = children
                .iter()
                .map(|child| resolve_schema(child, options, base_uri, registry))
                .collect::<OpenApiResult<Vec<_>>>()?;
            out.insert(key.into(), Value::Array(mapped));
        }
    }
    Ok(Value::Object(out))
}

struct Target<'a> {
    uri: String,
    value: &'a Value,
}

fn resolve_ref_value(
    value: &Value,
    options: &ResolveOptions,
    base_uri: &str,
    stack: &mut BTreeSet<String>,
) -> OpenApiResult<(Value, String)> {
    let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
        return Ok((value.clone(), base_uri.to_owned()));
    };
    let Some(target) = resolve_target(reference, options, base_uri, stack)? else {
        return Err(OpenApiError(format!(
            "cyclic non-schema reference {reference}"
        )));
    };
    resolve_ref_value(target.value, options, &target.uri, stack)
}

fn resolve_target<'a>(
    reference: &str,
    options: &'a ResolveOptions,
    base_uri: &str,
    stack: &mut BTreeSet<String>,
) -> OpenApiResult<Option<Target<'a>>> {
    let (raw_uri, pointer) = split_ref(reference);
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(OpenApiError(format!(
            "ref fragment must be a JSON pointer: {reference}"
        )));
    }
    let uri = resolve_uri(base_uri, &raw_uri);
    let key = format!("{uri}#{pointer}");
    if !stack.insert(key.clone()) {
        return Ok(None);
    }
    let document = options
        .documents
        .get(&uri)
        .ok_or_else(|| OpenApiError(format!("external ref document not supplied: {uri}")))?;
    let value = document
        .pointer(&pointer)
        .ok_or_else(|| OpenApiError(format!("ref pointer not found: {reference}")))?;
    Ok(Some(Target { uri, value }))
}

fn split_ref(reference: &str) -> (String, String) {
    if let Some((uri, pointer)) = reference.split_once('#') {
        (uri.to_string(), pointer.to_string())
    } else {
        (reference.to_string(), String::new())
    }
}

fn resolve_uri(base_uri: &str, reference_uri: &str) -> String {
    if reference_uri.is_empty() {
        return base_uri.to_string();
    }
    if reference_uri.contains("://") || reference_uri.starts_with('/') {
        return reference_uri.to_string();
    }
    match base_uri.rfind('/') {
        Some(index) => format!("{}{}", &base_uri[..=index], reference_uri),
        None => reference_uri.to_string(),
    }
}

fn validate_status(status: &str) -> OpenApiResult<()> {
    let valid = status == "default"
        || (status.len() == 3 && status.chars().all(|character| character.is_ascii_digit()))
        || (status.len() == 3
            && status.as_bytes()[0].is_ascii_digit()
            && status.as_bytes()[1] == b'X'
            && status.as_bytes()[2] == b'X');
    if valid {
        Ok(())
    } else {
        Err(OpenApiError(format!(
            "unsupported response status {status}"
        )))
    }
}

fn object_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> OpenApiResult<&'a serde_json::Map<String, Value>> {
    object
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| OpenApiError(format!("{name} object is required")))
}

fn optional_object<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> OpenApiResult<Option<&'a serde_json::Map<String, Value>>> {
    object
        .get(name)
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| OpenApiError(format!("{name} must be an object")))
        })
        .transpose()
}

fn optional_array<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> OpenApiResult<Option<&'a Vec<Value>>> {
    object
        .get(name)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| OpenApiError(format!("{name} must be an array")))
        })
        .transpose()
}

fn required_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> OpenApiResult<&'a Value> {
    object
        .get(name)
        .ok_or_else(|| OpenApiError(format!("{name} is required")))
}

fn array_value(name: &'static str) -> impl Fn(&Value) -> OpenApiResult<&Vec<Value>> {
    move |value| {
        value
            .as_array()
            .ok_or_else(|| OpenApiError(format!("{name} must be an array")))
    }
}

fn string_field(object: &serde_json::Map<String, Value>, name: &str) -> OpenApiResult<String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| OpenApiError(format!("{name} string is required")))
}

fn default_style(location: &str) -> &'static str {
    match location {
        "path" | "header" => "simple",
        "query" | "cookie" => "form",
        _ => "form",
    }
}

fn sanitize(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}
