//! Generate the standalone Forge proof SDK and documentation artifacts.

use incurs_forge::{
    ApiResponse, ArtifactOptions, Operation, Parameter, RequestBody, ResolvedOpenApi,
    compile_artifacts, publish_artifacts,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate <new-target-directory>")?;
    let artifacts =
        compile_artifacts(&contract(), ArtifactOptions::new("incurs-forge-widget-sdk"))?;
    let report = publish_artifacts(&artifacts, target)?;
    println!(
        "published {} files to {}",
        report.files.len(),
        report.target.display()
    );
    Ok(())
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
