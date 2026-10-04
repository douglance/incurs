#[path = "sdk_composition.rs"]
mod composition;
#[path = "sdk_media.rs"]
mod media;
#[path = "sdk_response.rs"]
mod response;
#[path = "sdk_symbols.rs"]
mod symbols;
#[path = "sdk_validation.rs"]
mod validation;
use crate::artifacts::{Artifact, ArtifactOptions};
use crate::model::{ForgeError, ForgeResult, Operation, Parameter, ResolvedOpenApi};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn generate(
    contract: &ResolvedOpenApi,
    options: &ArtifactOptions,
    digest: &str,
) -> ForgeResult<Vec<Artifact>> {
    let source_contract = contract;
    let prepared = composition::prepare(contract)?;
    let contract = &prepared;
    let symbols = symbols::SdkSymbols::new(contract)?;
    let mut lib = render_lib(contract, digest, &symbols)?;
    let responses = response::generate(source_contract, &symbols)?;
    lib.push_str(&responses.source);
    let smoke = render_smoke_test(contract, options, &symbols)?;
    let composed = composition::used(contract);
    let mut artifacts = vec![
        Artifact {
            path: "sdk/Cargo.toml".to_string(),
            bytes: render_cargo_toml(options, composed).into_bytes(),
        },
        Artifact {
            path: "sdk/Cargo.lock".to_string(),
            bytes: render_cargo_lock(options, composed).into_bytes(),
        },
        Artifact {
            path: "sdk/README.md".to_string(),
            bytes: render_readme(contract, options, digest).into_bytes(),
        },
        Artifact {
            path: "sdk/src/lib.rs".to_string(),
            bytes: lib.into_bytes(),
        },
        Artifact {
            path: "sdk/tests/smoke.rs".to_string(),
            bytes: smoke.into_bytes(),
        },
    ];
    artifacts.push(Artifact {
        path: "sdk/src/response-schemas.json".into(),
        bytes: serde_json::to_vec(&responses.graph)
            .map_err(|error| ForgeError(error.to_string()))?,
    });
    if composed {
        artifacts.push(Artifact {
            path: "sdk/src/schemas.json".into(),
            bytes: serde_json::to_vec(&validation::graph(contract)?)
                .map_err(|e| ForgeError(e.to_string()))?,
        });
    }
    Ok(artifacts)
}

fn render_cargo_toml(options: &ArtifactOptions, composed: bool) -> String {
    let mut manifest = format!(
        "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[lib]\npath = \"src/lib.rs\"\n\n[lints.rust]\nmissing_docs = \"deny\"\n",
        options.package_name, options.crate_version
    );
    if composed {
        manifest.push_str("\n[features]\ntyped-responses = [\"dep:serde\", \"serde_json/raw_value\"]\n\n[dependencies]\njsonschema = { version = \"=0.58.2\", default-features = false, features = [\"arbitrary-precision\"] }\nserde_json = { version = \"=1.0.151\", features = [\"arbitrary_precision\"] }\nserde = { version = \"=1.0.229\", optional = true }\n");
    } else {
        manifest.push_str("\n[features]\ntyped-responses = [\"dep:serde\", \"dep:serde_json\", \"dep:jsonschema\"]\n\n[dependencies]\njsonschema = { version = \"=0.58.2\", optional = true, default-features = false, features = [\"arbitrary-precision\"] }\nserde_json = { version = \"=1.0.151\", optional = true, features = [\"arbitrary_precision\", \"raw_value\"] }\nserde = { version = \"=1.0.229\", optional = true }\n");
    }
    manifest
}

fn render_cargo_lock(options: &ArtifactOptions, _composed: bool) -> String {
    include_str!("sdk_validation.lock")
        .replace("incurs-jsonschema-proof", &options.package_name)
        .replace(
            &format!("name = \"{}\"\nversion = \"0.1.0\"", options.package_name),
            &format!(
                "name = \"{}\"\nversion = \"{}\"",
                options.package_name, options.crate_version
            ),
        )
}

fn render_readme(contract: &ResolvedOpenApi, options: &ArtifactOptions, digest: &str) -> String {
    format!(
        "# {}\n\nGenerated Rust SDK for `{}`.\n\nContract digest: `{}`.\n\nThe SDK is transport neutral and async: provide a `Transport` implementation for HTTP, MCP, local ToolCatalog, or tests. Optional values use `Field<T>` so missing, null, explicit values, and schema defaults remain distinct. `OperationRequest::arguments_json()` emits the adapter seam with `path`, `query`, `querystring`, `header`, `cookie`, `body`, `body_base64`, and `media_type`; raw methods retain status, headers, and `Field<JsonValue>` payloads. Enable the optional `typed-responses` Cargo feature for response read models, `decode()` on raw response variants, and typed client methods. The buffered decoder selects declared JSON, text, or binary media, validates the response schema, and retains the complete raw response on success or decoding failure. Byte-based JSON decoding preserves exact numeric tokens and rejects duplicate object keys. Missing and null remain distinct, and response defaults are not inserted. Structured XML parsing, incremental streams, and vendor-specific envelope unwrapping require a separate adapter. Unsupported schema shapes fail generation instead of being erased.\n",
        options.package_name, contract.title, digest
    )
}

fn render_lib(
    contract: &ResolvedOpenApi,
    digest: &str,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let schemas = &contract.schemas;
    let mut out = String::new();
    out.push_str("//! Generated Rust SDK for a resolved Forge contract.\n");
    out.push_str(
        "//! This crate contains no built-in network client and performs no filesystem access.\n",
    );
    out.push_str("#![allow(unused_mut)]\n#![deny(missing_docs)]\n\n");
    out.push_str(&format!("/// Stable digest for the contract used to generate this SDK.\npub const CONTRACT_DIGEST: &str = {};\n\n", rust_string(digest)));
    out.push_str(&format!("/// Stable namespace for generated operation identities.\npub const NAMESPACE: &str = {};\n\n", rust_string(&contract.namespace)));
    out.push_str(BASE_TYPES);
    if composition::used(contract) {
        out.push_str(validation::RUNTIME);
    }
    out.push('\n');
    out.push_str(&render_schemas(
        &contract.schemas,
        composition::used(contract),
        symbols,
    )?);
    out.push_str(&render_operations(&contract.operations, schemas, symbols)?);
    Ok(out)
}

fn render_schemas(
    schemas: &BTreeMap<String, Value>,
    validate: bool,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let mut out = String::new();
    for (name, schema) in schemas {
        let type_name = symbols.schema_type(name);
        if let Some(rendered) = composition::render(name, schema, schemas, symbols)
            .map_err(|error| ForgeError(format!("schema {name}: {error}")))?
        {
            out.push_str(&rendered);
        } else if let Some(values) = string_enum(schema) {
            let variants =
                unique_names(values.iter().map(|value| pascal(value)).collect(), &[], "");
            out.push_str(&format!("/// Schema `{type_name}`.\n#[derive(Clone, Debug, Eq, PartialEq)]\npub enum {type_name} {{\n"));
            for (value, variant) in values.iter().zip(&variants) {
                out.push_str(&format!(
                    "    /// Wire value {}.\n    {variant},\n",
                    rust_string(value)
                ));
            }
            out.push_str("}\n\n");
            out.push_str(&format!("impl IntoJson for {type_name} {{\n    fn into_json(self) -> JsonValue {{\n        match self {{\n"));
            for (value, variant) in values.iter().zip(&variants) {
                out.push_str(&format!(
                    "            Self::{} => JsonValue::String({}.to_string()),\n",
                    variant,
                    rust_string(value)
                ));
            }
            out.push_str("        }\n    }\n}\n\n");
        } else if schema.get("type").and_then(Value::as_str) == Some("object") {
            let guard =
                validate.then(|| format!("json_matches(&value, {:?})", validation::key(name)));
            out.push_str(&render_object_schema(
                type_name,
                schema,
                guard.as_deref(),
                schemas,
                symbols,
            )?);
        } else {
            let ty = type_for_schema(schema, Some(type_name), schemas, symbols)?;
            let ty = if is_nullable(schema, schemas) && ty != "JsonValue" && ty != "()" {
                format!("Option<{ty}>")
            } else {
                ty
            };
            out.push_str(&format!(
                "/// Value of schema {type_name}.\npub type {type_name} = {ty};\n\n"
            ));
        }
    }
    Ok(out)
}

fn render_object_schema(
    type_name: &str,
    schema: &Value,
    guard: Option<&str>,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let required = required_set(schema);
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let fields = field_names(properties.keys().map(String::as_str), &[]);
    let default_methods = default_method_names(&fields, &[]);
    let mut out = String::new();
    let mut defaults = String::new();
    out.push_str(&format!(
        "/// Schema `{type_name}`.\n#[derive(Clone, Debug, PartialEq)]\npub struct {type_name} {{\n"
    ));
    for (index, ((name, property), field)) in properties.iter().zip(&fields).enumerate() {
        let doc_name = name.escape_debug();
        let ty = type_for_schema(property, Some(type_name), schemas, symbols)?;
        if required.contains(name) && !is_nullable(property, schemas) {
            out.push_str(&format!(
                "    /// Wire field `{doc_name}`.\n    pub {field}: {ty},\n"
            ));
        } else {
            out.push_str(&format!(
                "    /// Wire field `{doc_name}`.\n    pub {field}: Field<{ty}>,\n"
            ));
        }
        let metadata = crate::schema::binding_view(property, schemas)?;
        if let Some(default) = metadata.get("default") {
            let method = &default_methods[index];
            defaults.push_str(&format!("    /// Return the schema default for `{field}` without treating it as missing.\n    pub fn {method}() -> Field<{ty}> {{\n        Field::Default({})\n    }}\n\n", default_expr(default)?));
        }
    }
    out.push_str("}\n\n");
    if !defaults.is_empty() {
        out.push_str(&format!("impl {type_name} {{\n{defaults}}}\n\n"));
    }
    out.push_str(&format!("impl IntoJson for {type_name} {{\n    fn into_json(self) -> JsonValue {{\n        let mut fields = Vec::new();\n"));
    for ((name, property), field) in properties.iter().zip(&fields) {
        if required.contains(name) && !is_nullable(property, schemas) {
            out.push_str(&format!(
                "        fields.push(({}.to_string(), self.{field}.into_json()));\n",
                rust_string(name)
            ));
        } else {
            out.push_str(&format!(
                "        self.{field}.append_object_field(&mut fields, {});\n",
                rust_string(name)
            ));
        }
    }
    out.push_str("        let value = JsonValue::Object(fields);\n");
    if let Some(guard) = guard {
        out.push_str(&format!(
            "        if {guard} {{ value }} else {{ JsonValue::Invalid }}\n"
        ));
    } else {
        out.push_str("        value\n");
    }
    out.push_str("    }\n}\n\n");
    Ok(out)
}

fn render_operations(
    operations: &[Operation],
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let mut out = String::new();
    for operation in operations {
        out.push_str(
            &render_operation_types(operation, schemas, symbols)
                .map_err(|error| ForgeError(format!("operation {}: {error}", operation.id)))?,
        );
    }
    out.push_str("/// Transport-neutral generated SDK client.\npub struct Client<T> {\n    transport: T,\n}\n\n");
    out.push_str("impl<T> Client<T> {\n    /// Create a client from a caller-supplied transport.\n    pub fn new(transport: T) -> Self {\n        Self { transport }\n    }\n\n    /// Consume the client and return its transport.\n    pub fn into_transport(self) -> T {\n        self.transport\n    }\n}\n\n");
    out.push_str("impl<T: Transport> Client<T> {\n");
    for operation in operations {
        let operation_symbols = symbols.operation(operation);
        let method = &operation_symbols.method;
        let args_type = &operation_symbols.args_type;
        let response_type = &operation_symbols.response_type;
        out.push_str(&format!("    /// Invoke this generated operation.\n    pub async fn {method}(&self, args: {args_type}) -> Result<{response_type}, T::Error> {{\n        let response = self.transport.call(args.into_request()).await?;\n        Ok({response_type}::from_response(response))\n    }}\n\n"));
    }
    out.push_str("}\n\n");
    Ok(out)
}

fn render_operation_types(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let operation_symbols = symbols.operation(operation);
    let args_type = &operation_symbols.args_type;
    let response_type = &operation_symbols.response_type;
    let fields = operation_field_names(operation);
    let default_methods = default_method_names(&fields, &["into_request"]);
    let body_model = media::model(operation, schemas, symbols)?;
    let mut out = String::new();
    let mut defaults = String::new();
    if let Some(model) = &body_model {
        out.push_str(&model.declaration());
    }
    out.push_str(&format!("/// Arguments for one generated operation.\n#[derive(Clone, Debug, PartialEq)]\npub struct {args_type} {{\n"));
    for (index, (parameter, field)) in operation.parameters.iter().zip(&fields).enumerate() {
        let wire_name = &parameter.name;
        let doc_wire_name = wire_name.escape_debug();
        let ty = type_for_schema(&parameter.schema, None, schemas, symbols)?;
        if parameter.required && !is_nullable(&parameter.schema, schemas) {
            out.push_str(&format!(
                "    /// Wire field `{doc_wire_name}`.\n    pub {field}: {ty},\n"
            ));
        } else {
            out.push_str(&format!(
                "    /// Wire field `{doc_wire_name}`.\n    pub {field}: Field<{ty}>,\n"
            ));
        }
        let metadata = crate::schema::binding_view(&parameter.schema, schemas)?;
        if let Some(default) = metadata.get("default") {
            let method = &default_methods[index];
            defaults.push_str(&format!("    /// Return the schema default for `{field}` without treating it as missing.\n    pub fn {method}() -> Field<{ty}> {{\n        Field::Default({})\n    }}\n\n", default_expr(default)?));
        }
    }
    if let Some(body) = &operation.request_body {
        let body_type = if let Some(model) = &body_model {
            model.name.clone()
        } else {
            request_body_type(operation, schemas, symbols)?
        };
        if body_model.is_some() || body.required {
            out.push_str(&format!(
                "    /// Request body in the generated operation media type.\n    pub body: {body_type},\n"
            ));
        } else {
            out.push_str(&format!(
                "    /// Request body in the generated operation media type.\n    pub body: Field<{body_type}>,\n"
            ));
        }
    }
    out.push_str("}\n\n");
    out.push_str(&format!("impl {args_type} {{\n"));
    out.push_str(&defaults);
    out.push_str("    /// Convert arguments into the transport-neutral request shape.\n    pub fn into_request(self) -> OperationRequest {\n        let mut path_params = Vec::new();\n        let mut query_params = Vec::new();\n        let mut query_strings = Vec::new();\n        let mut headers = Vec::new();\n        let mut cookies = Vec::new();\n");
    for (parameter, field) in operation.parameters.iter().zip(&fields) {
        push_parameter(&mut out, parameter, field, schemas, symbols)?;
    }
    if let Some(model) = &body_model {
        out.push_str(&model.binding());
    } else if let Some(body) = &operation.request_body {
        if body.required {
            out.push_str("        let body = Field::Value(self.body.into_json());\n");
        } else {
            out.push_str("        let body = self.body.into_json_field();\n");
        }
        let (media, schema) = request_body_schema(operation)?;
        if crate::schema::binding_view(schema, schemas)?.as_ref() == &Value::Bool(false) {
            out.push_str("        let body = if matches!(body, Field::Missing) { Field::Missing } else { Field::Value(JsonValue::Invalid) };\n");
        }
        out.push_str(&format!(
            "        let media_type = if matches!(body, Field::Missing) {{ Field::Missing }} else {{ Field::Value({}.to_string()) }};\n",
            rust_string(media)
        ));
    } else {
        out.push_str(
            "        let body = Field::Missing;\n        let media_type = Field::Missing;\n",
        );
    }
    if body_model.is_none() {
        out.push_str("        let body_bytes = None;\n");
    }
    out.push_str(&format!("        OperationRequest {{\n            operation_id: {},\n            method: {},\n            path_template: {},\n            path_params,\n            query_params,\n            query_strings,\n            headers,\n            cookies,\n            body,\n            body_bytes,\n            media_type,\n        }}\n    }}\n}}\n\n", rust_string(&operation.id), rust_string(&operation.method), rust_string(&operation.path)));
    out.push_str(&render_response_type(operation, response_type));
    Ok(out)
}

fn render_response_type(operation: &Operation, response_type: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("/// Response variants for one generated operation.\n#[derive(Clone, Debug, PartialEq)]\npub enum {response_type} {{\n"));
    for status in operation.responses.keys() {
        if status == "default" {
            out.push_str("    /// Contract default response.\n    Default(OperationResponse),\n");
        } else {
            out.push_str(&format!(
                "    /// Contract response for this status.\n    {}(OperationResponse),\n",
                status_variant(status)
            ));
        }
    }
    out.push_str("    /// Response status not declared by the contract.\n    Other(OperationResponse),\n}\n\n");
    out.push_str(&format!("impl {response_type} {{\n    /// Classify by exact status, then status family, then default or Other.\n    pub fn from_response(response: OperationResponse) -> Self {{\n        match response.status {{\n"));
    for status in operation.responses.keys() {
        if status.chars().all(|character| character.is_ascii_digit()) {
            out.push_str(&format!(
                "            {status} => Self::{}(response),\n",
                status_variant(status)
            ));
        }
    }
    for status in operation.responses.keys() {
        let Some(class) = status
            .strip_suffix("XX")
            .and_then(|class| class.parse::<u16>().ok())
            .filter(|class| (1..=5).contains(class))
        else {
            continue;
        };
        let lower = class * 100;
        let upper = lower + 99;
        out.push_str(&format!(
            "            {lower}..={upper} => Self::{}(response),\n",
            status_variant(status)
        ));
    }
    if operation.responses.contains_key("default") {
        out.push_str("            _ => Self::Default(response),\n");
    } else {
        out.push_str("            _ => Self::Other(response),\n");
    }
    out.push_str("        }\n    }\n\n    /// Return the HTTP status carried by this response.\n    pub fn status(&self) -> u16 {\n        match self {\n");
    for status in operation.responses.keys() {
        if status == "default" {
            out.push_str("            Self::Default(response) => response.status,\n");
        } else {
            out.push_str(&format!(
                "            Self::{}(response) => response.status,\n",
                status_variant(status)
            ));
        }
    }
    out.push_str("            Self::Other(response) => response.status,\n        }\n    }\n}\n\n");
    out
}

fn push_parameter(
    out: &mut String,
    parameter: &Parameter,
    field: &str,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<()> {
    let target = match parameter.location.as_str() {
        "path" => "path_params",
        "query" => "query_params",
        "querystring" => "query_strings",
        "header" => "headers",
        "cookie" => "cookies",
        other => {
            return Err(ForgeError(format!(
                "unsupported parameter location {other}"
            )));
        }
    };
    let _ = type_for_schema(&parameter.schema, None, schemas, symbols)?;
    if parameter.required
        && parameter.location == "path"
        && !is_nullable(&parameter.schema, schemas)
    {
        out.push_str(&format!(
            "        {target}.push(({}, self.{field}.into_json()));\n",
            rust_string(&parameter.name)
        ));
    } else if parameter.location == "path" {
        out.push_str(&format!(
            "        {target}.push(({}, match self.{field}.into_json_field() {{ Field::Missing => JsonValue::Invalid, Field::Null => JsonValue::Null, Field::Value(value) | Field::Default(value) => value }}));\n",
            rust_string(&parameter.name)
        ));
    } else if parameter.required && !is_nullable(&parameter.schema, schemas) {
        out.push_str(&format!(
            "        {target}.push(({}, Field::Value(self.{field}.into_json())));\n",
            rust_string(&parameter.name)
        ));
    } else {
        out.push_str(&format!(
            "        {target}.push(({}, self.{field}.into_json_field()));\n",
            rust_string(&parameter.name)
        ));
    }
    Ok(())
}

fn render_smoke_test(
    contract: &ResolvedOpenApi,
    options: &ArtifactOptions,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let schemas = &contract.schemas;
    let crate_name = options.package_name.replace('-', "_");
    let Some(operation) = contract.operations.iter().find(|operation| {
        if media::model(operation, schemas, symbols).is_ok_and(|model| model.is_some()) {
            return false;
        }
        let sampleable = |schema: &Value| {
            type_for_schema(schema, None, schemas, symbols).is_ok_and(|ty| {
                matches!(
                    ty.as_str(),
                    "String" | "bool" | "i64" | "f64" | "()" | "JsonValue"
                ) || ty.starts_with("Vec<")
            })
        };
        if operation
            .parameters
            .iter()
            .any(|p| p.required && !is_nullable(&p.schema, schemas) && !sampleable(&p.schema))
            || operation.request_body.as_ref().is_some_and(|body| {
                body.required
                    && request_body_schema(operation).is_ok_and(|(_, schema)| !sampleable(schema))
            })
        {
            return false;
        }
        !operation
            .parameters
            .iter()
            .any(|parameter| parameter.required && parameter.schema == Value::Bool(false))
            && !operation.request_body.as_ref().is_some_and(|body| {
                body.required
                    && request_body_schema(operation)
                        .is_ok_and(|(_, schema)| schema == &Value::Bool(false))
            })
    }) else {
        return Ok(format!(
            "//! Generated SDK smoke tests.\nuse {crate_name}::*;\n\n#[test]\nfn generated_sdk_has_no_sample_invocation() {{\n    assert!(!CONTRACT_DIGEST.is_empty());\n}}\n"
        ));
    };
    let operation_symbols = symbols.operation(operation);
    let args_type = &operation_symbols.args_type;
    let response_type = &operation_symbols.response_type;
    let mut args = String::new();
    args.push_str(&format!("let args = {args_type} {{\n"));
    let fields = operation_field_names(operation);
    for (parameter, field) in operation.parameters.iter().zip(&fields) {
        let ty = type_for_schema(&parameter.schema, None, schemas, symbols)?;
        let value = if parameter.required {
            let value = sample_value(&ty);
            if is_nullable(&parameter.schema, schemas) {
                format!("Field::Value({value})")
            } else {
                value
            }
        } else {
            "Field::Missing".to_string()
        };
        args.push_str(&format!("        {field}: {value},\n"));
    }
    if let Some(body) = &operation.request_body {
        if body.required {
            let body_type = request_body_type(operation, schemas, symbols)?;
            args.push_str(&format!("        body: {},\n", sample_value(&body_type)));
        } else {
            args.push_str("        body: Field::Missing,\n");
        }
    }
    args.push_str("    };\n");
    Ok(format!(
        "//! Generated SDK smoke tests.\nuse {crate_name}::*;\nuse std::future::Future;\nuse std::pin::Pin;\nuse std::task::{{Context, Poll, RawWaker, RawWakerVTable, Waker}};\n\nstruct MockTransport;\n\nimpl Transport for MockTransport {{\n    type Error = ();\n\n    fn call(&self, request: OperationRequest) -> impl Future<Output = Result<OperationResponse, Self::Error>> {{\n        async move {{\n            assert_eq!(request.operation_id, {});\n            let arguments = request.arguments_json().unwrap();\n            assert!(arguments.contains(\"\\\"path\\\"\"));\n            Ok(OperationResponse {{ status: 200, headers: Vec::new(), body: Field::Missing }})\n        }}\n    }}\n}}\n\n#[test]\nfn generated_client_invokes_transport() {{\n    block_on(async {{\n        let client = Client::new(MockTransport);\n        {args}        let response: {response_type} = client.{}(args).await.unwrap();\n        assert_eq!(response.status(), 200);\n    }});\n}}\n\nfn block_on<F: Future>(future: F) -> F::Output {{\n    let waker = unsafe {{ Waker::from_raw(raw_waker()) }};\n    let mut context = Context::from_waker(&waker);\n    let mut future = Box::pin(future);\n    loop {{\n        match Future::poll(Pin::as_mut(&mut future), &mut context) {{\n            Poll::Ready(output) => return output,\n            Poll::Pending => std::thread::yield_now(),\n        }}\n    }}\n}}\n\nfn raw_waker() -> RawWaker {{\n    unsafe fn clone(_: *const ()) -> RawWaker {{ raw_waker() }}\n    unsafe fn wake(_: *const ()) {{}}\n    unsafe fn wake_by_ref(_: *const ()) {{}}\n    unsafe fn drop(_: *const ()) {{}}\n    RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, wake, wake_by_ref, drop))\n}}\n",
        rust_string(&operation.id),
        operation_symbols.method
    ))
}

fn request_body_schema(operation: &Operation) -> ForgeResult<(&str, &Value)> {
    let body = operation
        .request_body
        .as_ref()
        .ok_or_else(|| ForgeError(format!("operation {} has no request body", operation.id)))?;
    if body.content.len() == 1 {
        let (media, schema) = body.content.iter().next().unwrap();
        return Ok((media, schema));
    }
    Err(ForgeError(format!(
        "operation {} requires an explicit media choice",
        operation.id
    )))
}

fn request_body_type(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let (_, schema) = request_body_schema(operation)?;
    type_for_schema(schema, None, schemas, symbols)
}

fn type_for_schema(
    schema: &Value,
    current_type: Option<&str>,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    if schema == &Value::Bool(true) || schema.as_object().is_some_and(|object| object.is_empty()) {
        return Ok("JsonValue".to_string());
    }
    if schema == &Value::Bool(false) {
        return Ok("Never".to_string());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(name) = composition::reference_name(reference) {
            let shape = crate::schema::binding_view(schema, schemas)?;
            if shape.get("oneOf").is_none()
                && shape.get("anyOf").is_none()
                && shape.get("allOf").is_none()
                && (shape.is_boolean()
                    || matches!(
                        shape.get("type").and_then(Value::as_str),
                        Some("string" | "integer" | "number" | "boolean" | "null")
                    ) && string_enum(&shape).is_none()
                    || shape.get("type").is_some_and(Value::is_array))
            {
                return type_for_schema(&shape, current_type, schemas, symbols);
            }
            let ty = symbols.schema_type(&name).to_string();
            if let Some(current) = current_type {
                let mut pending = BTreeSet::from([name]);
                let mut visited = BTreeSet::new();
                while let Some(next) = pending.pop_first() {
                    if !visited.insert(next.clone()) {
                        continue;
                    }
                    if symbols.schema_type(&next) == current {
                        return Ok(format!("Box<{ty}>"));
                    }
                    let definition = schemas
                        .get(&next)
                        .ok_or_else(|| ForgeError(format!("unknown schema {next}")))?;
                    crate::schema::references(definition, &mut pending)?;
                }
            }
            return Ok(ty);
        }
        return Err(ForgeError(format!(
            "unsupported external schema reference {reference}"
        )));
    }
    match schema.get("type") {
        Some(Value::String(kind)) => match kind.as_str() {
            "string" => Ok("String".to_string()),
            "integer" => Ok("i64".to_string()),
            "number" => Ok("f64".to_string()),
            "boolean" => Ok("bool".to_string()),
            "null" => Ok("()".to_string()),
            "array" => {
                let items = schema
                    .get("items")
                    .ok_or_else(|| ForgeError("array schema requires items".to_string()))?;
                Ok(format!(
                    "Vec<{}>",
                    type_for_schema(items, current_type, schemas, symbols)?
                ))
            }
            "object" => Ok("JsonValue".to_string()),
            other => Err(ForgeError(format!("unsupported schema type {other}"))),
        },
        Some(Value::Array(kinds)) => nullable_type(kinds, schema, current_type, schemas, symbols),
        None => Ok("JsonValue".to_string()),
        _ => Err(ForgeError(format!("unsupported schema shape {schema}"))),
    }
}

fn nullable_type(
    kinds: &[Value],
    schema: &Value,
    current_type: Option<&str>,
    schemas: &BTreeMap<String, Value>,
    symbols: &symbols::SdkSymbols,
) -> ForgeResult<String> {
    let non_null = kinds
        .iter()
        .filter_map(Value::as_str)
        .filter(|kind| *kind != "null")
        .collect::<Vec<_>>();
    match non_null.as_slice() {
        ["string"] => Ok("String".to_string()),
        ["boolean"] => Ok("bool".to_string()),
        ["integer"] => Ok("i64".to_string()),
        ["number"] => Ok("f64".to_string()),
        ["object"] => Ok("JsonValue".to_string()),
        ["array"] => {
            let items = schema
                .get("items")
                .ok_or_else(|| ForgeError("array schema requires items".to_string()))?;
            Ok(format!(
                "Vec<{}>",
                type_for_schema(items, current_type, schemas, symbols)?
            ))
        }
        [] if kinds.iter().all(|kind| kind.as_str() == Some("null")) && !kinds.is_empty() => {
            Ok("()".to_string())
        }
        [] => Err(ForgeError("schema type list cannot be empty".to_string())),
        _ => Err(ForgeError(format!(
            "unsupported nullable schema type {kinds:?}"
        ))),
    }
}

fn is_nullable(schema: &Value, schemas: &BTreeMap<String, Value>) -> bool {
    let Ok(schema) = crate::schema::binding_view(schema, schemas) else {
        return false;
    };
    schema
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || schema
            .get("type")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("null")))
}

fn default_expr(value: &Value) -> ForgeResult<String> {
    match value {
        Value::Null => Ok("JsonValue::Null".to_string()),
        Value::Bool(value) => Ok(format!("JsonValue::Bool({value})")),
        Value::String(value) => Ok(format!(
            "JsonValue::String({}.to_string())",
            rust_string(value)
        )),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(format!("JsonValue::Integer({value})"))
            } else if let Some(value) = value.as_u64() {
                Ok(format!("JsonValue::Unsigned({value})"))
            } else {
                Ok(format!(
                    "JsonValue::ExactNumber({}.to_string())",
                    rust_string(&value.to_string())
                ))
            }
        }
        Value::Array(values) => Ok(format!(
            "JsonValue::Array(vec![{}])",
            values
                .iter()
                .map(default_expr)
                .collect::<ForgeResult<Vec<_>>>()?
                .join(",")
        )),
        Value::Object(values) => Ok(format!(
            "JsonValue::Object(vec![{}])",
            values
                .iter()
                .map(|(key, value)| Ok(format!(
                    "({}.to_string(), {})",
                    rust_string(key),
                    default_expr(value)?
                )))
                .collect::<ForgeResult<Vec<_>>>()?
                .join(",")
        )),
    }
}

fn string_enum(schema: &Value) -> Option<Vec<&str>> {
    if schema.get("type").and_then(Value::as_str) != Some("string") {
        return None;
    }
    Some(
        schema
            .get("enum")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect(),
    )
}

fn required_set(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn operation_field_names(operation: &Operation) -> Vec<String> {
    let reserved: &[&str] = if operation.request_body.is_some() {
        &["body"]
    } else {
        &[]
    };
    field_names(
        operation.parameters.iter().map(|p| p.name.as_str()),
        reserved,
    )
}

fn field_names<'a>(
    wire_names: impl IntoIterator<Item = &'a str>,
    reserved: &[&str],
) -> Vec<String> {
    let bases: Vec<String> = wire_names.into_iter().map(snake).collect();
    unique_names(bases, reserved, "_")
}

fn default_method_names(fields: &[String], reserved: &[&str]) -> Vec<String> {
    let bases = fields
        .iter()
        .map(|field| default_method_base(field))
        .collect();
    unique_names(bases, reserved, "_")
}

fn default_method_base(field: &str) -> String {
    let (base, suffix) = field
        .rsplit_once('_')
        .and_then(|(base, suffix)| {
            suffix
                .chars()
                .all(|character| character.is_ascii_digit())
                .then_some((base, suffix))
        })
        .unwrap_or((field, ""));
    let unescaped = base
        .strip_suffix('_')
        .filter(|candidate| RUST_KEYWORDS.contains(candidate))
        .unwrap_or(base);
    if suffix.is_empty() {
        format!("{unescaped}_default")
    } else {
        format!("{unescaped}_{suffix}_default")
    }
}

fn unique_names(bases: Vec<String>, reserved: &[&str], separator: &str) -> Vec<String> {
    let protected: BTreeSet<String> = bases
        .iter()
        .cloned()
        .chain(reserved.iter().map(|name| (*name).to_string()))
        .collect();
    let mut seen: BTreeSet<String> = reserved.iter().map(|name| (*name).to_string()).collect();
    bases
        .into_iter()
        .map(|base| {
            if seen.insert(base.clone()) {
                return base;
            }
            let field = (2_u64..)
                .map(|suffix| format!("{base}{separator}{suffix}"))
                .find(|candidate| !protected.contains(candidate) && !seen.contains(candidate))
                .unwrap();
            seen.insert(field.clone());
            field
        })
        .collect()
}

fn sample_value(ty: &str) -> String {
    if ty == "String" {
        "\"sample\".to_string()".to_string()
    } else if ty == "bool" {
        "true".to_string()
    } else if ty == "i64" {
        "1".to_string()
    } else if ty == "f64" {
        "1.0".to_string()
    } else if ty == "()" {
        "()".to_string()
    } else if ty.starts_with("Vec<") {
        "Vec::new()".to_string()
    } else if ty.starts_with("Box<") {
        "Box::new(JsonValue::Null)".to_string()
    } else {
        "JsonValue::Null".to_string()
    }
}

fn status_variant(status: &str) -> String {
    if status == "default" {
        return "Default".to_string();
    }
    let mut out = String::from("Status");
    for character in status.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_uppercase());
        }
    }
    out
}

fn pascal(value: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if upper {
                out.push(character.to_ascii_uppercase());
                upper = false;
            } else {
                out.push(character);
            }
        } else {
            upper = true;
        }
    }
    if out.is_empty() {
        "Generated".to_string()
    } else if out.as_bytes()[0].is_ascii_digit() || out == "Self" {
        format!("Value{out}")
    } else {
        out
    }
}

fn snake(value: &str) -> String {
    let mut out = String::new();
    let mut previous_was_lower_or_digit = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && previous_was_lower_or_digit && !out.ends_with('_')
            {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        } else {
            if !out.ends_with('_') {
                out.push('_');
            }
            previous_was_lower_or_digit = false;
        }
    }
    let mut out = out.trim_matches('_').to_string();
    if out.is_empty() {
        out.push_str("value");
    }
    if out.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        out.insert(0, '_');
    }
    if RUST_KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

const CLIENT_METHOD_NAMES: &[&str] = &["new", "into_transport"];

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "gen", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

const BASE_TYPES: &str = r#"/// JSON value used at transport boundaries.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    /// A value rejected by a generated composition constraint; JSON encoding fails.
    Invalid,
    /// JSON null.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// JSON floating-point number.
    Number(f64),
    /// JSON number text, checked against the JSON number grammar before encoding.
    ExactNumber(String),
    /// Signed JSON integer, encoded without floating-point conversion.
    Integer(i64),
    /// Unsigned JSON integer, encoded without floating-point conversion.
    Unsigned(u64),
    /// Raw bytes carried by a transport response.
    Bytes(Vec<u8>),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<JsonValue>),
    /// JSON object.
    Object(Vec<(String, JsonValue)>),
}

/// JSON encoding failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonEncodeError;

impl std::fmt::Display for JsonEncodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("json encode error")
    }
}

impl std::error::Error for JsonEncodeError {}

/// Converts a typed SDK value into a JSON value.
pub trait IntoJson {
    /// Convert this value into JSON.
    fn into_json(self) -> JsonValue;
}

impl JsonValue {
    /// Encode this value as JSON text.
    pub fn to_json_string(&self) -> Result<String, JsonEncodeError> {
        let mut out = String::new();
        self.write_json(&mut out)?;
        Ok(out)
    }

    fn write_json(&self, out: &mut String) -> Result<(), JsonEncodeError> {
        match self {
            Self::Invalid => return Err(JsonEncodeError),
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) if value.is_finite() => out.push_str(&value.to_string()),
            Self::Number(_) => return Err(JsonEncodeError),
            Self::ExactNumber(value) if valid_json_number(value) => out.push_str(value),
            Self::ExactNumber(_) => return Err(JsonEncodeError),
            Self::Integer(value) => out.push_str(&value.to_string()),
            Self::Unsigned(value) => out.push_str(&value.to_string()),
            Self::Bytes(_) => return Err(JsonEncodeError),
            Self::String(value) => write_json_string(value, out),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 { out.push(','); }
                    value.write_json(out)?;
                }
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (name, value)) in fields.iter().enumerate() {
                    if index > 0 { out.push(','); }
                    write_json_string(name, out);
                    out.push(':');
                    value.write_json(out)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
}

fn valid_json_number(value: &str) -> bool {
    let mut bytes = value.bytes().peekable();
    if bytes.peek() == Some(&b'-') { bytes.next(); }
    match bytes.next() {
        Some(b'0') => {}
        Some(b'1'..=b'9') => {
            while bytes.peek().is_some_and(u8::is_ascii_digit) { bytes.next(); }
        }
        _ => return false,
    }
    if bytes.peek() == Some(&b'.') {
        bytes.next();
        if !bytes.next().is_some_and(|byte| byte.is_ascii_digit()) { return false; }
        while bytes.peek().is_some_and(u8::is_ascii_digit) { bytes.next(); }
    }
    if bytes.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
        bytes.next();
        if bytes.peek().is_some_and(|byte| matches!(byte, b'+' | b'-')) { bytes.next(); }
        if !bytes.next().is_some_and(|byte| byte.is_ascii_digit()) { return false; }
        while bytes.peek().is_some_and(u8::is_ascii_digit) { bytes.next(); }
    }
    bytes.next().is_none()
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character.is_control() => out.push_str(&format!("\\u{:04x}", character as u32)),
            character => out.push(character),
        }
    }
    out.push('"');
}

impl IntoJson for JsonValue {
    fn into_json(self) -> JsonValue { self }
}

impl IntoJson for String {
    fn into_json(self) -> JsonValue { JsonValue::String(self) }
}

impl IntoJson for bool {
    fn into_json(self) -> JsonValue { JsonValue::Bool(self) }
}

impl IntoJson for i64 {
    fn into_json(self) -> JsonValue { JsonValue::Integer(self) }
}

impl IntoJson for u64 {
    fn into_json(self) -> JsonValue { JsonValue::Unsigned(self) }
}

/// A false schema has no permitted JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Never {}

impl IntoJson for Never {
    fn into_json(self) -> JsonValue { match self {} }
}

impl IntoJson for () {
    fn into_json(self) -> JsonValue { JsonValue::Null }
}

impl<T: IntoJson> IntoJson for Option<T> {
    fn into_json(self) -> JsonValue {
        match self { Some(value) => value.into_json(), None => JsonValue::Null }
    }
}

impl IntoJson for f64 {
    fn into_json(self) -> JsonValue { JsonValue::Number(self) }
}

impl<T: IntoJson> IntoJson for Vec<T> {
    fn into_json(self) -> JsonValue {
        JsonValue::Array(self.into_iter().map(IntoJson::into_json).collect())
    }
}

impl<T: IntoJson> IntoJson for Box<T> {
    fn into_json(self) -> JsonValue { (*self).into_json() }
}

/// Value presence marker that preserves missing, null, explicit value, and default.
#[derive(Clone, Debug, PartialEq)]
pub enum Field<T> {
    /// The field was omitted.
    Missing,
    /// The field was explicitly null.
    Null,
    /// The field has an explicit value.
    Value(T),
    /// The field uses a schema default represented as JSON.
    Default(JsonValue),
}

impl<T> Default for Field<T> {
    fn default() -> Self { Self::Missing }
}

impl<T: IntoJson> Field<T> {
    /// Convert a typed field into a JSON field while preserving presence.
    pub fn into_json_field(self) -> Field<JsonValue> {
        match self {
            Self::Missing => Field::Missing,
            Self::Null => Field::Null,
            Self::Value(value) => Field::Value(value.into_json()),
            Self::Default(value) => Field::Default(value),
        }
    }

    /// Append this field to a JSON object unless it is missing.
    pub fn append_object_field(self, fields: &mut Vec<(String, JsonValue)>, name: &str) {
        match self {
            Self::Missing => {}
            Self::Null => fields.push((name.to_string(), JsonValue::Null)),
            Self::Value(value) => fields.push((name.to_string(), value.into_json())),
            Self::Default(value) => fields.push((name.to_string(), value)),
        }
    }
}

/// Transport-neutral operation request.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationRequest {
    /// Stable operation identity.
    pub operation_id: &'static str,
    /// HTTP method from the operation binding.
    pub method: &'static str,
    /// Path template from the operation binding.
    pub path_template: &'static str,
    /// Path parameters.
    pub path_params: Vec<(&'static str, JsonValue)>,
    /// Query parameters.
    pub query_params: Vec<(&'static str, Field<JsonValue>)>,
    /// Whole-query-string parameters, retaining their contract names.
    pub query_strings: Vec<(&'static str, Field<JsonValue>)>,
    /// Header parameters.
    pub headers: Vec<(&'static str, Field<JsonValue>)>,
    /// Cookie parameters.
    pub cookies: Vec<(&'static str, Field<JsonValue>)>,
    /// JSON or textual request body.
    pub body: Field<JsonValue>,
    /// Raw request bytes, encoded into the body_base64 adapter slot.
    pub body_bytes: Option<Vec<u8>>,
    /// Request media type.
    pub media_type: Field<String>,
}

impl OperationRequest {
    /// Encode arguments using the Forge adapter object shape.
    pub fn arguments_json(&self) -> Result<String, JsonEncodeError> {
        let mut fields = Vec::new();
        fields.push(("path".to_string(), JsonValue::Object(self.path_params.iter().map(|(name, value)| ((*name).to_string(), value.clone())).collect())));
        fields.push(("query".to_string(), optional_fields(&self.query_params)));
        if !self.query_strings.is_empty() {
            fields.push(("querystring".to_string(), optional_fields(&self.query_strings)));
        }
        fields.push(("header".to_string(), optional_fields(&self.headers)));
        fields.push(("cookie".to_string(), optional_fields(&self.cookies)));
        if let Some(bytes) = &self.body_bytes {
            if !matches!(self.body, Field::Missing) { return Err(JsonEncodeError); }
            fields.push(("body_base64".to_string(), JsonValue::String(encode_body_bytes(bytes))));
        }
        match &self.body {
            Field::Missing => {}
            Field::Null => fields.push(("body".to_string(), JsonValue::Null)),
            Field::Value(value) => fields.push(("body".to_string(), value.clone())),
            Field::Default(value) => fields.push(("body".to_string(), value.clone())),
        }
        match &self.media_type {
            Field::Missing => {}
            Field::Null => fields.push(("media_type".to_string(), JsonValue::Null)),
            Field::Value(value) => fields.push(("media_type".to_string(), JsonValue::String(value.clone()))),
            Field::Default(value) => fields.push(("media_type".to_string(), value.clone())),
        }
        JsonValue::Object(fields).to_json_string()
    }
}

fn encode_body_bytes(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        encoded.push(char::from(ALPHABET[(a >> 2) as usize]));
        encoded.push(char::from(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize]));
        encoded.push(if chunk.len() > 1 { char::from(ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize]) } else { '=' });
        encoded.push(if chunk.len() > 2 { char::from(ALPHABET[(c & 63) as usize]) } else { '=' });
    }
    encoded
}

fn optional_fields(fields: &[(&'static str, Field<JsonValue>)]) -> JsonValue {
    JsonValue::Object(fields.iter().filter_map(|(name, value)| match value {
        Field::Missing => None,
        Field::Null => Some(((*name).to_string(), JsonValue::Null)),
        Field::Value(value) => Some(((*name).to_string(), value.clone())),
        Field::Default(value) => Some(((*name).to_string(), value.clone())),
    }).collect())
}

/// Transport-neutral operation response.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Field<JsonValue>,
}

/// Async transport supplied by the SDK consumer.
pub trait Transport {
    /// Transport error type.
    type Error;

    /// Call one operation.
    fn call(&self, request: OperationRequest) -> impl std::future::Future<Output = Result<OperationResponse, Self::Error>>;
}
"#;
