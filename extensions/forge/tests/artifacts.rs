//! Artifact compiler tests using independent resolved-contract expectations.

use incurs_forge::{
    ApiResponse, Artifact, ArtifactOptions, ArtifactSet, Operation, Parameter, RequestBody,
    ResolvedOpenApi, compile_artifacts, publish_artifacts,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[test]
fn compiles_deterministic_artifacts_and_searchable_docs() {
    let mut contract = contract();
    contract.title = "Widget <Admin>".to_string();
    contract.operations[0].responses.insert(
        "<script>".to_string(),
        ApiResponse {
            description: "bad".to_string(),
            content: BTreeMap::new(),
            headers: BTreeMap::new(),
        },
    );
    let set =
        compile_artifacts(&contract, ArtifactOptions::new("incurs-forge-widget-sdk")).unwrap();
    let paths = set.paths().collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "artifact-manifest.json",
            "contract.json",
            "docs/index.html",
            "docs/search.json",
            "sdk/Cargo.lock",
            "sdk/Cargo.toml",
            "sdk/README.md",
            "sdk/src/lib.rs",
            "sdk/src/response-schemas.json",
            "sdk/tests/smoke.rs",
        ]
    );
    assert_eq!(set.contract_digest.len(), 64);
    let docs = text(&set, "docs/index.html");
    assert!(docs.contains("Widget &lt;Admin&gt;"));
    assert!(!docs.contains("Widget <Admin>"));
    assert!(docs.contains("type=\"search\""));
    assert!(docs.contains("Parameters, body, and responses"));
    assert!(docs.contains("includeArchived"));
    assert!(docs.contains("request_body"));
    assert!(docs.contains("../contract.json"));
    assert!(docs.contains("&lt;script&gt;"));
    assert!(!docs.contains("Responses: <script>"));
    assert!(!docs.contains("\"<script>\""));
    let search = text(&set, "docs/search.json");
    assert!(search.contains("listWidgets"));
    assert!(search.contains(&set.contract_digest));
}

#[test]
fn publication_requires_new_safe_target() {
    let set = compile_artifacts(&contract(), ArtifactOptions::default()).unwrap();
    let target = temp_path("publish");
    let _ = fs::remove_dir_all(&target);
    let report = publish_artifacts(&set, &target).unwrap();
    assert_eq!(report.files.len(), set.artifacts.len());
    assert!(target.join("sdk/src/lib.rs").is_file());
    let existing = publish_artifacts(&set, &target).unwrap_err();
    assert!(
        existing
            .to_string()
            .contains("publish target already exists")
    );
    let unsafe_set = ArtifactSet {
        contract_digest: "x".to_string(),
        artifacts: vec![Artifact {
            path: "../escape".to_string(),
            bytes: Vec::new(),
        }],
    };
    let unsafe_target = temp_path("unsafe");
    let _ = fs::remove_dir_all(&unsafe_target);
    let error = publish_artifacts(&unsafe_set, &unsafe_target).unwrap_err();
    assert!(error.to_string().contains("invalid artifact path"));
    assert!(!unsafe_target.exists());
}

fn text(set: &ArtifactSet, path: &str) -> String {
    String::from_utf8(set.get(path).unwrap().bytes.clone()).unwrap()
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "incurs-forge-artifacts-{name}-{}",
        std::process::id()
    ))
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
