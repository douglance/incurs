//! Independent response schema graph and source-version controls.
use incurs_forge::{ArtifactOptions, ResolveOptions, compile_artifacts, resolve_document};
use serde_json::{Value, json};

fn graph(document: &Value) -> Value {
    let contract = resolve_document(document, ResolveOptions::new("response-graph")).unwrap();
    let artifacts =
        compile_artifacts(&contract, ArtifactOptions::new("response-graph-sdk")).unwrap();
    serde_json::from_slice(
        &artifacts
            .get("sdk/src/response-schemas.json")
            .unwrap()
            .bytes,
    )
    .unwrap()
}

fn document(version: &str, schema: Value) -> Value {
    json!({
        "openapi":version,"info":{"title":"Response graph","version":"1"},
        "paths":{"/value":{"get":{"operationId":"readValue","responses":{"200":{
            "description":"value","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Shared"}}}
        }}}}},
        "components":{"schemas":{"Shared":schema}}
    })
}

#[test]
fn repeated_response_references_keep_one_shared_definition() {
    let marker = "shared-response-definition-marker";
    let mut source = document(
        "3.1.0",
        json!({
            "description":marker,"type":"object","required":["value"],
            "properties":{"value":{"type":"string","minLength":2},"next":{"$ref":"#/components/schemas/Shared"}},
            "additionalProperties":false
        }),
    );
    let paths = source["paths"].as_object_mut().unwrap();
    let template = paths["/value"].clone();
    for index in 0..128 {
        let mut operation = template.clone();
        operation["get"]["operationId"] = json!(format!("read{index}"));
        paths.insert(format!("/value/{index}"), operation);
    }
    let schema = graph(&source);
    let text = serde_json::to_string(&schema).unwrap();
    assert_eq!(
        text.matches(marker).count(),
        1,
        "named response schema was copied"
    );
    assert!(
        text.len() < 6000,
        "repeated response graph grew to {} bytes",
        text.len()
    );
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(&json!({"#/$defs/Shared":{"value":"ok","next":{"value":"ok"}}})));
    assert!(!validator.is_valid(&json!({"#/$defs/Shared":{"value":"ok","next":{"value":"x"}}})));
}

#[test]
fn response_context_retains_readonly_and_omits_writeonly_required_names() {
    let source = document(
        "3.1.0",
        json!({
            "type":"object","required":["stamp","secret"],
            "properties":{"stamp":{"type":"string","readOnly":true},"secret":{"type":"string","writeOnly":true}}
        }),
    );
    let schema = graph(&source);
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(&json!({"#/$defs/Shared":{"stamp":"server"}})));
    assert!(!validator.is_valid(&json!({"#/$defs/Shared":{}})));
    assert!(!validator.is_valid(&json!({"#/$defs/Shared":{"stamp":"server","secret":false}})));
}

#[test]
fn response_nullable_follows_the_source_openapi_version() {
    for (version, accepts_null) in [("3.0.3", true), ("3.1.0", false), ("3.2.0", false)] {
        let source = document(
            version,
            json!({"type":"string","nullable":true,"minLength":2}),
        );
        let schema = graph(&source);
        let validator = jsonschema::draft202012::options()
            .offline()
            .build(&schema)
            .unwrap();
        assert_eq!(
            validator.is_valid(&json!({"#/$defs/Shared":null})),
            accepts_null,
            "{version}"
        );
        assert!(
            validator.is_valid(&json!({"#/$defs/Shared":"ok"})),
            "{version}"
        );
        assert!(
            !validator.is_valid(&json!({"#/$defs/Shared":"x"})),
            "{version}"
        );
    }
}
