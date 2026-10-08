//! Generated SDK tests that inspect the emitted crate contract.

use incurs_openapi::{
    ApiResponse, ArtifactOptions, Operation, Parameter, RequestBody, ResolvedOpenApi,
    compile_artifacts,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[test]
fn generated_sdk_uses_async_transport_and_preserves_presence() {
    let set = compile_artifacts(
        &contract(),
        ArtifactOptions::new("incurs-openapi-widget-sdk"),
    )
    .unwrap();
    let manifest = String::from_utf8(set.get("sdk/Cargo.toml").unwrap().bytes.clone()).unwrap();
    assert!(manifest.contains("[workspace]"));
    let sdk = String::from_utf8(set.get("sdk/src/lib.rs").unwrap().bytes.clone()).unwrap();
    assert!(sdk.contains("fn call(&self, request: OperationRequest) -> impl std::future::Future"));
    assert!(!sdk.contains("Future<Output = Result<OperationResponse, Self::Error>> + Send"));
    assert!(sdk.contains("/// Transport-neutral generated SDK client."));
    assert!(sdk.contains("pub async fn list_widgets"));
    assert!(sdk.contains("pub async fn update_widget"));
    assert!(sdk.contains("pub include_archived: Field<bool>"));
    assert!(sdk.contains("pub fn include_archived_default() -> Field<bool>"));
    assert!(sdk.contains("Field::Default(JsonValue::Bool(false))"));
    assert!(sdk.contains("pub description: Field<String>"));
    assert!(sdk.contains("pub parent: Field<Box<Widget>>"));
    assert!(sdk.contains("pub fn arguments_json(&self)"));
    assert!(sdk.contains("pub media_type: Field<String>"));
    assert!(sdk.contains("Bytes(Vec<u8>)"));
    let smoke = String::from_utf8(set.get("sdk/tests/smoke.rs").unwrap().bytes.clone()).unwrap();
    assert!(smoke.starts_with("//! Generated SDK smoke tests."));
    assert!(smoke.contains("block_on(async"));
    assert!(smoke.contains("request.arguments_json().unwrap()"));
}

#[test]
fn unsupported_schema_shapes_fail_before_generation() {
    let mut contract = contract();
    contract
        .schemas
        .get_mut("Widget")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("mystery".to_string(), json!({ "type": "mystery" }));
    let error = compile_artifacts(&contract, ArtifactOptions::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported schema type mystery")
    );
}

fn contract() -> ResolvedOpenApi {
    let mut schemas = BTreeMap::new();
    schemas.insert(
        "Widget".to_string(),
        json!({
            "type": "object",
            "required": ["id", "name", "active"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "active": { "type": "boolean" },
                "description": { "type": "string", "nullable": true },
                "parent": { "$ref": "#/components/schemas/Widget", "nullable": true }
            }
        }),
    );
    schemas.insert(
        "WidgetPatch".to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"] },
                "active": { "type": "boolean", "default": false }
            }
        }),
    );
    ResolvedOpenApi {
        openapi_version: "3.1.0".into(),
        json_schema_dialect: None,
        namespace: "worker-proof".to_string(),
        title: "Widgets".to_string(),
        server_url: Some("https://api.example.test".to_string()),
        operations: vec![list_widgets(), update_widget()],
        schemas,
    }
}

fn list_widgets() -> Operation {
    Operation {
        id: "worker-proof/listWidgets".to_string(),
        name: "listWidgets".to_string(),
        description: Some("List widgets".to_string()),
        method: "GET".to_string(),
        path: "/widgets".to_string(),
        servers: Vec::new(),
        parameters: vec![Parameter {
            content: None,
            name: "includeArchived".to_string(),
            location: "query".to_string(),
            required: false,
            schema: json!({ "type": "boolean", "default": false }),
            style: "form".to_string(),
            explode: true,
        }],
        request_body: None,
        responses: responses(
            "200",
            json!({ "type": "array", "items": { "$ref": "#/components/schemas/Widget" } }),
        ),
        security: Value::Array(Vec::new()),
    }
}

fn update_widget() -> Operation {
    Operation {
        id: "worker-proof/updateWidget".to_string(),
        name: "updateWidget".to_string(),
        description: Some("Update widget".to_string()),
        method: "PATCH".to_string(),
        path: "/widgets/{id}".to_string(),
        servers: Vec::new(),
        parameters: vec![Parameter {
            content: None,
            name: "id".to_string(),
            location: "path".to_string(),
            required: true,
            schema: json!({ "type": "string" }),
            style: "simple".to_string(),
            explode: false,
        }],
        request_body: Some(RequestBody {
            encoding: BTreeMap::new(),
            required: true,
            content: map1(
                "application/json",
                json!({ "$ref": "#/components/schemas/WidgetPatch" }),
            ),
        }),
        responses: responses("200", json!({ "$ref": "#/components/schemas/Widget" })),
        security: Value::Array(Vec::new()),
    }
}

fn responses(status: &str, schema: Value) -> BTreeMap<String, ApiResponse> {
    let mut responses = BTreeMap::new();
    responses.insert(
        status.to_string(),
        ApiResponse {
            description: "ok".to_string(),
            content: map1("application/json", schema),
            headers: BTreeMap::new(),
        },
    );
    responses
}

fn map1(key: &str, value: Value) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    map.insert(key.to_string(), value);
    map
}
