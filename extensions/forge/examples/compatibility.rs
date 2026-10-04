//! Bounded compatibility audit, not a proof of universal OpenAPI conformance.
//! Exit 1 means at least one valid case failed; successful SDK generation does
//! not mean that generated Rust was built or the HTTP request was executed.
use incurs_forge::{
    ArtifactOptions, ResolveOptions, compile_artifacts, resolve_document,
    runtime::ForgeHttpRequest,
    servers::{ServerSelection, build_operation_request},
};
use serde_json::{Value, json};
use std::process::ExitCode;

struct Case {
    name: &'static str,
    document: Value,
    arguments: Value,
    url: &'static str,
    body: Option<&'static str>,
    media: Option<&'static str>,
}

fn document() -> Value {
    json!({
        "openapi":"3.1.1",
        "info":{"title":"Compatibility probe","version":"1"},
        "servers":[{"url":"https://example.test"}],
        "paths":{"/items":{"get":{
            "operationId":"probe",
            "responses":{"200":{"description":"OK"}}
        }}}
    })
}

fn base(name: &'static str) -> Case {
    Case {
        name,
        document: document(),
        arguments: json!({}),
        url: "https://example.test/items",
        body: None,
        media: None,
    }
}

fn cases() -> Vec<Case> {
    let mut cases = vec![base("openapi-3.1.1")];
    for (name, version) in [("openapi-3.0.3", "3.0.3"), ("openapi-3.2.0", "3.2.0")] {
        let mut case = base(name);
        case.document["openapi"] = json!(version);
        cases.push(case);
    }
    for (name, style, explode, expected) in [
        (
            "query-form-array",
            "form",
            true,
            "https://example.test/items?color=blue&color=black",
        ),
        (
            "query-pipe-array",
            "pipeDelimited",
            false,
            "https://example.test/items?color=blue%7Cblack",
        ),
        (
            "query-space-array",
            "spaceDelimited",
            false,
            "https://example.test/items?color=blue%20black",
        ),
    ] {
        let mut case = base(name);
        case.document["paths"]["/items"]["get"]["parameters"] = json!([{
            "name":"color","in":"query","style":style,"explode":explode,
            "schema":{"type":"array","items":{"type":"string"}}
        }]);
        case.arguments = json!({"query":{"color":["blue","black"]}});
        case.url = expected;
        cases.push(case);
    }
    for (name, style, expected) in [
        ("path-simple", "simple", "https://example.test/items/blue"),
        ("path-label", "label", "https://example.test/items/.blue"),
        (
            "path-matrix",
            "matrix",
            "https://example.test/items/;color=blue",
        ),
    ] {
        let mut case = base(name);
        let mut operation = case.document["paths"]["/items"]["get"].clone();
        operation["parameters"] = json!([{
            "name":"color","in":"path","required":true,"style":style,
            "schema":{"type":"string"}
        }]);
        case.document["paths"] = json!({"/items/{color}":{"get":operation}});
        case.arguments = json!({"path":{"color":"blue"}});
        case.url = expected;
        cases.push(case);
    }
    for (name, media, schema, value, body) in [
        (
            "body-json",
            "application/json",
            json!({"type":"object","properties":{"name":{"type":"string"}}}),
            json!({"name":"blue"}),
            Some(r#"{"name":"blue"}"#),
        ),
        (
            "body-text",
            "text/plain",
            json!({"type":"string"}),
            json!("blue"),
            Some("blue"),
        ),
        (
            "body-form",
            "application/x-www-form-urlencoded",
            json!({"type":"object","properties":{"name":{"type":"string"}}}),
            json!({"name":"blue"}),
            Some("name=blue"),
        ),
        (
            "body-multipart",
            "multipart/form-data",
            json!({"type":"object","properties":{"name":{"type":"string"}}}),
            json!({"name":"blue"}),
            None,
        ),
    ] {
        let mut case = base(name);
        let mut operation = case.document["paths"]["/items"]["get"].clone();
        operation["requestBody"] = json!({"required":true,"content":{media:{"schema":schema}}});
        case.document["paths"]["/items"] = json!({"post":operation});
        case.arguments = json!({"body":value});
        case.body = body;
        case.media = Some(media);
        cases.push(case);
    }
    for (name, schema) in [
        (
            "schema-object",
            json!({"type":"object","properties":{"name":{"type":"string"}}}),
        ),
        ("schema-string-alias", json!({"type":"string"})),
        (
            "schema-one-of",
            json!({"oneOf":[{"type":"string"},{"type":"integer"}]}),
        ),
        (
            "schema-all-of",
            json!({"allOf":[{"type":"object","properties":{"name":{"type":"string"}}},{"type":"object","properties":{"id":{"type":"integer"}}}]}),
        ),
        ("schema-boolean", json!(true)),
    ] {
        let mut case = base(name);
        case.document["components"] = json!({"schemas":{"Payload":schema}});
        case.document["paths"]["/items"]["get"]["responses"]["200"]["content"] =
            json!({"application/json":{"schema":{"$ref":"#/components/schemas/Payload"}}});
        cases.push(case);
    }
    let mut case = base("operation-server-override");
    case.document["paths"]["/items"]["get"]["servers"] =
        json!([{"url":"https://alternate.test/v2"}]);
    case.url = "https://alternate.test/v2/items";
    cases.push(case);
    cases
}

fn request_check(case: &Case, request: &ForgeHttpRequest) -> Result<(), String> {
    let method = if case.media.is_some() { "POST" } else { "GET" };
    if request.method != method || request.url != case.url {
        return Err(format!(
            "expected {method} {}; got {} {}",
            case.url, request.method, request.url
        ));
    }
    if case.media == Some("multipart/form-data") {
        let content_type = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        if !content_type.starts_with("multipart/form-data;") || !content_type.contains("boundary=")
        {
            return Err("missing multipart content type and boundary".into());
        }
        let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
        if !body.contains("name=\"name\"") || !body.contains("\r\n\r\nblue\r\n") {
            return Err("missing multipart name field or value".into());
        }
    } else {
        if request.body.as_deref() != case.body.map(str::as_bytes) {
            return Err(format!("unexpected body: {:?}", request.body));
        }
        let expected = case
            .media
            .map(|media| vec![("Content-Type".to_owned(), media.to_owned())])
            .unwrap_or_default();
        if request.headers != expected {
            return Err(format!(
                "expected headers {expected:?}; got {:?}",
                request.headers
            ));
        }
    }
    Ok(())
}

fn inspect(case: &Case) -> Value {
    let contract = match resolve_document(&case.document, ResolveOptions::new("compatibility")) {
        Ok(contract) => contract,
        Err(error) => {
            return json!({"case":case.name,"resolution":"failed","error":error.to_string(),"ok":false});
        }
    };
    let generation = compile_artifacts(&contract, ArtifactOptions::new("compatibility-sdk"));
    let generation_error = generation.as_ref().err().map(ToString::to_string);
    let binding = build_operation_request(
        &contract.operations[0],
        &contract.schemas,
        &ServerSelection::default(),
        &case.arguments,
    )
    .map_err(|e| e.to_string());
    let request_error = binding
        .and_then(|request| request_check(case, &request))
        .err();
    json!({
        "case":case.name,"resolution":"passed",
        "sdk_generation":if generation_error.is_none() {"passed"} else {"failed"},
        "sdk_error":generation_error,
        "request_check":if request_error.is_none() {"passed"} else {"failed"},
        "request_error":request_error,
        "ok":generation_error.is_none() && request_error.is_none()
    })
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let cases = cases();
    if let [flag, name] = arguments.as_slice()
        && flag == "--document"
    {
        if let Some(case) = cases.iter().find(|case| case.name == name) {
            println!("{}", serde_json::to_string_pretty(&case.document).unwrap());
            return ExitCode::SUCCESS;
        }
        eprintln!("unknown case: {name}");
        return ExitCode::from(2);
    }
    if !arguments.is_empty() {
        eprintln!("usage: compatibility [--document CASE]");
        return ExitCode::from(2);
    }
    let results: Vec<_> = cases.iter().map(inspect).collect();
    let passed = results.iter().filter(|result| result["ok"] == true).count();
    println!("{}", serde_json::to_string_pretty(&json!({
        "scope":"resolution, SDK generation, and selected request semantics; not full conformance",
        "total":results.len(),"passed":passed,"failed":results.len()-passed,"cases":results
    })).unwrap());
    if passed == results.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
