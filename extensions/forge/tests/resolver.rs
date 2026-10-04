//! Resolver integration tests using independent expectations.

use incurs_forge::{Overlay, ResolveOptions, resolve_document};
use serde_json::{Value, json};

fn load(value: &str) -> Value {
    serde_json::from_str(value).unwrap()
}

// Resolve only reference edges in the test, independent of production binding helpers.
fn target<'a>(contract: &'a incurs_forge::ResolvedOpenApi, mut value: &'a Value) -> &'a Value {
    let mut seen = std::collections::BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        assert!(seen.insert(reference), "unproductive reference cycle");
        let name = reference
            .strip_prefix("#/components/schemas/")
            .unwrap()
            .replace("~1", "/")
            .replace("~0", "~");
        value = &contract.schemas[&name];
    }
    value
}

fn options() -> ResolveOptions {
    let mut options = ResolveOptions::new("worker-proof");
    options.documents.insert(
        "common.json".to_string(),
        load(include_str!("../fixtures/common.json")),
    );
    options
}

#[test]
fn resolves_openapi_contract_shape() {
    let root = load(include_str!("../fixtures/proof-openapi.json"));
    let resolved = resolve_document(&root, options()).unwrap();
    assert_eq!(
        resolved.server_url.as_deref(),
        Some("https://api.example.test")
    );
    assert_eq!(resolved.operations.len(), 2);
    let get = resolved
        .operations
        .iter()
        .find(|operation| operation.name == "get_pet")
        .unwrap();
    assert_eq!(get.id, "worker-proof/get_pet");
    assert_eq!(get.security, json!([{ "pet_key": ["read"] }]));
    let trace = get
        .parameters
        .iter()
        .filter(|parameter| parameter.name == "trace" && parameter.location == "header")
        .collect::<Vec<_>>();
    assert_eq!(trace.len(), 1);
    assert!(trace[0].required);
    let include = get
        .parameters
        .iter()
        .find(|parameter| parameter.name == "include")
        .unwrap();
    assert_eq!(include.style, "pipeDelimited");
    assert!(!include.explode);
    assert!(get.responses["200"].headers.contains_key("etag"));
    assert!(
        target(&resolved, &get.responses["404"].content["application/json"])
            .pointer("/properties/message")
            .is_some()
    );
    assert_eq!(
        get.responses["default"].content["application/octet-stream"]
            .get("format")
            .and_then(Value::as_str),
        Some("binary")
    );
    let post = resolved
        .operations
        .iter()
        .find(|operation| operation.name == "create_pet")
        .unwrap();
    assert_eq!(post.security, json!([{ "global_key": [] }]));
    assert!(!post.request_body.as_ref().unwrap().required);
    assert_eq!(
        resolved.schemas["Pet"]
            .pointer("/properties/friend/$ref")
            .and_then(Value::as_str),
        Some("#/components/schemas/Pet")
    );
}

#[test]
fn overlay_changes_presentation_before_resolution() {
    let root = load(include_str!("../fixtures/proof-openapi.json"));
    let mut overlaid_options = options();
    overlaid_options.overlays.push(Overlay {
        pointer: "/paths/~1pets~1{id}/get/summary".to_string(),
        value: Value::String("Read a pet by id".to_string()),
    });
    let overlaid = resolve_document(&root, overlaid_options).unwrap();
    let get = overlaid
        .operations
        .iter()
        .find(|operation| operation.name == "get_pet")
        .unwrap();
    assert_eq!(get.description.as_deref(), Some("Read a pet by id"));
}

#[test]
fn preserves_operation_id_spelling_and_resolves_local_operation_refs() {
    let root = json!({
        "openapi": "3.1.0",
        "info": { "title": "Refs", "version": "1" },
        "components": { "schemas": { "Widget": { "type": "object", "properties": { "id": { "type": "string" } } } } },
        "paths": {
            "/widgets/{id}": {
                "parameters": [{ "$ref": "#/components/parameters/WidgetId" }],
                "get": {
                    "operationId": "listWidgets",
                    "responses": { "200": { "$ref": "#/components/responses/WidgetResponse" } }
                },
                "patch": {
                    "operationId": "updateWidget",
                    "requestBody": { "$ref": "#/components/requestBodies/WidgetBody" },
                    "responses": { "200": { "$ref": "#/components/responses/WidgetResponse" } }
                }
            }
        },
        "components": {
            "parameters": { "WidgetId": { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } } },
            "requestBodies": { "WidgetBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Widget" } } } } },
            "responses": { "WidgetResponse": { "description": "ok", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Widget" } } } } },
            "schemas": { "Widget": { "type": "object", "properties": { "id": { "type": "string" } } } }
        }
    });
    let resolved = resolve_document(&root, ResolveOptions::new("worker-proof")).unwrap();
    assert!(
        resolved
            .operations
            .iter()
            .any(|operation| operation.name == "listWidgets"
                && operation.id == "worker-proof/listWidgets")
    );
    let update = resolved
        .operations
        .iter()
        .find(|operation| operation.name == "updateWidget")
        .unwrap();
    assert!(update.request_body.as_ref().unwrap().required);
    assert_eq!(
        target(
            &resolved,
            &update.responses["200"].content["application/json"]
        )
        .pointer("/properties/id/type")
        .and_then(Value::as_str),
        Some("string")
    );
}

#[test]
fn rejects_missing_external_ref_document() {
    let root = json!({
        "openapi": "3.1.0",
        "info": { "title": "Bad", "version": "1" },
        "paths": { "/bad": { "get": { "operationId": "bad", "responses": { "200": { "description": "bad", "content": { "application/json": { "schema": { "$ref": "missing.json#/Thing" } } } } } } } }
    });
    let error = resolve_document(&root, ResolveOptions::new("worker-proof")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("external ref document not supplied: missing.json")
    );
}

#[test]
fn rejects_duplicate_operation_identity() {
    let mut root = load(include_str!("../fixtures/proof-openapi.json"));
    if let Some(value) = root.pointer_mut("/paths/~1pets/post/operationId") {
        *value = Value::String("get_pet".to_string());
    }
    let error = resolve_document(&root, options()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate operation id worker-proof/get_pet")
    );
}

#[test]
fn rejects_unsupported_openapi_version() {
    let root = json!({
        "openapi": "2.0",
        "info": { "title": "Bad", "version": "1" },
        "paths": {}
    });
    let error = resolve_document(&root, ResolveOptions::new("worker-proof")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported openapi version 2.0")
    );
}

#[test]
fn resolves_external_refs_relative_to_their_source_document() {
    let root = json!({
        "openapi": "3.1.0",
        "info": { "title": "Relative", "version": "1" },
        "paths": { "/outer": { "get": { "operationId": "getOuter", "responses": { "200": { "description": "ok", "content": { "application/json": { "schema": { "$ref": "common.json#/components/schemas/Outer" } } } } } } } }
    });
    let mut options = ResolveOptions::new("worker-proof");
    options.document_uri = "apis/root.json".to_string();
    options.documents.insert("apis/common.json".to_string(), json!({
        "components": { "schemas": { "Outer": { "type": "object", "properties": { "inner": { "$ref": "inner.json#/components/schemas/Inner" } } } } }
    }));
    options.documents.insert("apis/inner.json".to_string(), json!({
        "components": { "schemas": { "Inner": { "type": "object", "properties": { "name": { "type": "string" } } } } }
    }));
    let resolved = resolve_document(&root, options).unwrap();
    let operation = resolved
        .operations
        .iter()
        .find(|operation| operation.name == "getOuter")
        .unwrap();
    assert_eq!(
        target(
            &resolved,
            &target(
                &resolved,
                &operation.responses["200"].content["application/json"]
            )["properties"]["inner"]
        )
        .pointer("/properties/name/type")
        .and_then(Value::as_str),
        Some("string")
    );
}

#[test]
fn rejects_malformed_content_node() {
    let root = json!({
        "openapi": "3.1.0",
        "info": { "title": "Bad", "version": "1" },
        "paths": { "/bad": { "get": { "operationId": "bad", "responses": { "200": { "description": "bad", "content": [] } } } } }
    });
    let error = resolve_document(&root, ResolveOptions::new("worker-proof")).unwrap_err();
    assert!(error.to_string().contains("content must be an object"));
}
