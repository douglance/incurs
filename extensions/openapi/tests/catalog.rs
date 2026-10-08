//! Automatic import behavior across discovery, CLI, control, and raw HTTP results.
#![cfg(feature = "adapters")]
use incurs::{
    cli::Runtime,
    outbound::{HttpClient, HttpClientError, HttpRequest, HttpResponse},
    tool::{ToolCallOptions, ToolCallOutcome},
};
use incurs_openapi::{
    ResolveOptions, ResolvedOpenApi,
    adapters::IncursHttpTransport,
    catalog::{compile_cli, tool_name},
    resolve_document,
    servers::ServerSelection,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;

fn contract() -> ResolvedOpenApi {
    resolve_document(&json!({
        "openapi":"3.1.1","info":{"title":"Catalog proof","version":"1"},
        "servers":[{"url":"https://example.test/v1"}],
        "components":{"schemas":{"Node":{"type":"object","properties":{
            "next":{"$ref":"#/components/schemas/Node"},
            "data":{"type":"object","default":{"$ref":"#/components/schemas/Node"}}
        }}}},
        "paths":{
            "/items":{"get":{"operationId":"list","responses":{"200":{"description":"OK"}}}},
            "/items/{id}":{"post":{
                "operationId":"write","description":"Write an item",
                "parameters":[{"in":"path","name":"id","required":true,"schema":{"type":"string"}}],
                "requestBody":{"required":true,"content":{
                    "application/json":{"schema":{"type":"object","properties":{"note":{"type":["string","null"]},"node":{"$ref":"#/components/schemas/Node"}}}},
                    "text/plain":{"schema":{"type":"string"}}
                }},
                "responses":{"204":{"description":"Written"}}
            }}
        }
    }), ResolveOptions::new("catalog-proof")).unwrap()
}

#[derive(Default)]
struct RecordingClient(Mutex<Vec<HttpRequest>>);
#[async_trait::async_trait]
impl HttpClient for RecordingClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        self.0.lock().unwrap().push(request);
        Ok(HttpResponse::from_bytes(
            429,
            vec![
                ("Set-Cookie".into(), "a=1".into()),
                ("Set-Cookie".into(), "b=2".into()),
            ],
            vec![0, 255, 1],
        ))
    }
}

#[test]
fn discovery_preserves_nested_schemas_and_has_no_empty_combinators() {
    let cli = compile_cli(
        &contract(),
        IncursHttpTransport::new(Arc::new(RecordingClient::default())),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    let list = &catalog.get("op_list").unwrap().input_schema;
    assert!(list.get("allOf").is_none(), "{list}");
    assert_eq!(list["properties"], json!({}));
    assert_eq!(list["$defs"], json!({}));
    let schema = &catalog.get("op_write").unwrap().input_schema;
    assert_eq!(schema["required"], json!(["path"]));
    assert_eq!(schema["$defs"].as_object().unwrap().len(), 1);
    assert_eq!(schema["x-openapi-operation-id"], "catalog-proof/write");
    assert_eq!(
        schema["properties"]["path"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        schema["$defs"]["Node"]["properties"]["next"]["$ref"],
        "#/$defs/Node"
    );
    assert_eq!(
        schema["$defs"]["Node"]["properties"]["data"]["default"]["$ref"],
        "#/components/schemas/Node"
    );
    assert_eq!(
        schema["properties"]["media_type"]["enum"],
        json!(["application/json", "text/plain"])
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[tokio::test]
async fn imported_tools_preserve_string_bodies_and_non_success_binary_responses() {
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &contract(),
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    let outcome = catalog
        .call(
            "op_write",
            json!({
                "path":{"id":"a/b"},"body":"123","media_type":"text/plain"
            })
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect(),
            ToolCallOptions::isolated(),
        )
        .await;
    let ToolCallOutcome::Ok { data, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(
        data,
        json!({"status":429,"headers":[["Set-Cookie","a=1"],["Set-Cookie","b=2"]],"body":[0,255,1]})
    );
    let calls = client.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].url, "https://example.test/v1/items/a%2Fb");
    assert_eq!(calls[0].body.as_deref(), Some(b"123".as_slice()));
}

#[tokio::test]
async fn cli_json_options_reach_the_shared_binding_without_guessing_body_types() {
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &contract(),
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    let mut output = Vec::new();
    let exit = cli
        .run_to(
            vec![
                "op_write",
                "--path",
                r#"{"id":"a/b"}"#,
                "--body-json",
                r#"{"note":null}"#,
                "--media-type",
                "application/json",
                "--format",
                "json",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            &mut output,
            Runtime::new("proof", Default::default(), false),
        )
        .await
        .unwrap();
    assert_eq!(exit.unwrap_or(0), 0, "{}", String::from_utf8_lossy(&output));
    let calls = client.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].url, "https://example.test/v1/items/a%2Fb");
    assert_eq!(
        calls[0].body.as_deref(),
        Some(br#"{"note":null}"#.as_slice())
    );
    let output: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        output,
        json!({"status":429,"headers":[["Set-Cookie","a=1"],["Set-Cookie","b=2"]],"body":[0,255,1]})
    );
}

#[tokio::test]
async fn malformed_and_conflicting_arguments_never_reach_http() {
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &contract(),
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    for arguments in [
        json!({"path":{},"body":{},"media_type":"application/json"}),
        json!({"path":{"id":"x"},"body_json":"{","media_type":"application/json"}),
        json!({"path":{"id":"x"},"body":{},"body_json":"{}","media_type":"application/json"}),
        json!({"path":{"id":"x"},"body":{},"media_type":"application/json","unknown":true}),
        json!({"path":{"id":"x"}}),
    ] {
        let result = catalog
            .call(
                "op_write",
                arguments.as_object().unwrap().clone().into_iter().collect(),
                ToolCallOptions::isolated(),
            )
            .await;
        assert!(
            matches!(result, ToolCallOutcome::Error { .. }),
            "{result:?}"
        );
    }
    assert!(client.0.lock().unwrap().is_empty());
}

struct CancelClient {
    cancellation: CancellationToken,
    dropped: Arc<AtomicBool>,
}
struct DropSignal(Arc<AtomicBool>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl HttpClient for CancelClient {
    async fn send(&self, _: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        let _signal = DropSignal(self.dropped.clone());
        self.cancellation.cancel();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_drops_an_active_imported_http_future() {
    let cancellation = CancellationToken::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let transport = IncursHttpTransport::new(Arc::new(CancelClient {
        cancellation: cancellation.clone(),
        dropped: dropped.clone(),
    }));
    let cli = compile_cli(&contract(), transport, &ServerSelection::default()).unwrap();
    let mut options = ToolCallOptions::isolated();
    options.control.cancellation = cancellation;
    let result = cli
        .tool_catalog()
        .call("op_list", Default::default(), options)
        .await;
    let ToolCallOutcome::Error { message, .. } = result else {
        panic!("{result:?}")
    };
    assert!(message.to_lowercase().contains("cancel"), "{message}");
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn tool_names_are_safe_and_duplicates_do_not_replace_commands() {
    let mut contract = contract();
    let operation = &mut contract.operations[0];
    operation.name = "a b".into();
    assert_eq!(tool_name(operation), "op_a_20b");
    operation.name = "a_20b".into();
    assert_eq!(tool_name(operation), "op_a_5f20b");
    operation.name = "雪".repeat(100);
    let name = tool_name(operation);
    assert!(
        name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    contract.operations.push(contract.operations[0].clone());
    assert!(
        compile_cli(
            &contract,
            IncursHttpTransport::new(Arc::new(RecordingClient::default())),
            &ServerSelection::default()
        )
        .is_err()
    );
}

#[tokio::test]
async fn omitted_schema_remains_unconstrained_in_discovery_and_invocation() {
    let contract = resolve_document(
        &json!({
            "openapi":"3.1.1","info":{"title":"Unconstrained body","version":"1"},
            "servers":[{"url":"https://example.test"}],
            "paths":{"/any":{"post":{"operationId":"any",
                "requestBody":{"required":true,"content":{"application/json":{}}},
                "responses":{"200":{"description":"OK"}}
            }}}
        }),
        ResolveOptions::new("any"),
    )
    .unwrap();
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &contract,
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    assert!(client.0.lock().unwrap().is_empty());
    let catalog = cli.tool_catalog();
    assert_eq!(
        catalog.get("op_any").unwrap().input_schema["properties"]["body"],
        json!({})
    );
    for (body, wire) in [
        (json!(null), "null"),
        (json!(true), "true"),
        (json!(9007199254740993_i64), "9007199254740993"),
        (json!("123"), "\"123\""),
        (json!([]), "[]"),
        (json!({"a":1}), "{\"a\":1}"),
    ] {
        let result = catalog
            .call(
                "op_any",
                std::collections::BTreeMap::from([("body".into(), body)]),
                ToolCallOptions::isolated(),
            )
            .await;
        assert!(matches!(result, ToolCallOutcome::Ok { .. }), "{result:?}");
        assert_eq!(
            client.0.lock().unwrap().last().unwrap().body.as_deref(),
            Some(wire.as_bytes())
        );
    }
}

#[test]
fn exclusive_bounds_follow_the_source_openapi_version() {
    let legacy = json!({"type":"integer","minimum":0,"exclusiveMinimum":true,"maximum":10,"exclusiveMaximum":false});
    let modern = json!({"type":"number","exclusiveMinimum":1.25,"exclusiveMaximum":10.5});
    for (version, schema, expected) in [
        (
            "3.0.4",
            legacy.clone(),
            json!({"type":"integer","exclusiveMinimum":0,"maximum":10}),
        ),
        ("3.1.1", modern.clone(), modern),
    ] {
        let mut contract = contract();
        contract.openapi_version = version.into();
        let operation = contract
            .operations
            .iter_mut()
            .find(|op| op.name == "list")
            .unwrap();
        operation.parameters.push(incurs_openapi::Parameter {
            content: None,
            name: "limit".into(),
            location: "query".into(),
            required: false,
            schema,
            style: "form".into(),
            explode: true,
        });
        let cli = compile_cli(
            &contract,
            IncursHttpTransport::new(Arc::new(RecordingClient::default())),
            &ServerSelection::default(),
        )
        .unwrap();
        assert_eq!(
            cli.tool_catalog().get("op_list").unwrap().input_schema["properties"]["query"]["properties"]
                ["limit"],
            expected
        );
    }
    let mut contract = contract();
    contract.openapi_version = "3.1.1".into();
    contract
        .operations
        .iter_mut()
        .find(|op| op.name == "list")
        .unwrap()
        .parameters
        .push(incurs_openapi::Parameter {
            content: None,
            name: "limit".into(),
            location: "query".into(),
            required: false,
            schema: legacy,
            style: "form".into(),
            explode: true,
        });
    assert!(
        compile_cli(
            &contract,
            IncursHttpTransport::new(Arc::new(RecordingClient::default())),
            &ServerSelection::default()
        )
        .is_err(),
        "OpenAPI 3.1 must not silently reinterpret legacy boolean bounds"
    );
}

fn media_contract() -> ResolvedOpenApi {
    resolve_document(
        &serde_json::from_str(include_str!("fixtures/media_openapi.json")).unwrap(),
        ResolveOptions::new("media-proof"),
    )
    .unwrap()
}

#[test]
fn binary_media_discovery_uses_an_exclusive_base64_slot() {
    let cli = compile_cli(
        &media_contract(),
        IncursHttpTransport::new(Arc::new(RecordingClient::default())),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    let schema = &catalog.get("op_uploadBinary").unwrap().input_schema;
    assert_eq!(schema["properties"]["body_base64"]["type"], "string");
    assert_eq!(
        schema["properties"]["body_base64"]["contentEncoding"],
        "base64"
    );
    assert!(schema["properties"].get("body").is_none());
    assert!(schema["properties"].get("body_json").is_none());
    assert_eq!(schema["$defs"], json!({}));
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(schema)
        .unwrap();
    for valid in [
        json!({}),
        json!({"body_base64":""}),
        json!({"body_base64":"AP+ADQo="}),
    ] {
        assert!(validator.is_valid(&valid), "{valid}");
    }
    for invalid in [
        json!({"body":"text"}),
        json!({"body_json":"\"text\""}),
        json!({"body_base64":null}),
        json!({"body_base64":"","body":"text"}),
    ] {
        assert!(!validator.is_valid(&invalid), "{invalid}");
    }
    let mixed = &catalog.get("op_sendMixed").unwrap().input_schema;
    assert_eq!(
        mixed["properties"]["media_type"]["enum"],
        json!([
            "application/json",
            "application/octet-stream",
            "application/x-forbidden+json",
            "multipart/form-data"
        ])
    );
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(mixed)
        .unwrap();
    for valid in [
        json!({"body_base64":"","media_type":"application/octet-stream"}),
        json!({"body":null,"media_type":"application/json"}),
    ] {
        assert!(validator.is_valid(&valid), "{valid}");
    }
    for invalid in [
        json!({}),
        json!({"body_base64":""}),
        json!({"body":null,"media_type":"application/octet-stream"}),
        json!({"body_base64":"","media_type":"application/json"}),
        json!({"body":{},"body_base64":"","media_type":"multipart/form-data"}),
    ] {
        assert!(!validator.is_valid(&invalid), "{invalid}");
    }
    let config = &catalog.get("op_sendConfig").unwrap().input_schema;
    assert_eq!(
        config["properties"]["media_type"]["enum"],
        json!(["application/json", "text/plain;charset=UTF-8"])
    );
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(config)
        .unwrap();
    assert!(validator.is_valid(&json!({"body":{"name":"x"},"media_type":"application/json"})));
    assert!(
        !validator.is_valid(&json!({"body":{"name":"x"},"media_type":"text/plain;charset=UTF-8"}))
    );
}

#[tokio::test]
async fn tool_and_cli_binary_uploads_decode_exact_bytes() {
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &media_contract(),
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    let outcome = cli
        .tool_catalog()
        .call(
            "op_uploadBinary",
            json!({"body_base64":"AP+ADQo="})
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
            ToolCallOptions::isolated(),
        )
        .await;
    assert!(matches!(outcome, ToolCallOutcome::Ok { .. }), "{outcome:?}");
    let mut output = Vec::new();
    let exit = cli
        .run_to(
            vec![
                "op_uploadBinary",
                "--body-base64",
                "AP+ADQo=",
                "--format",
                "json",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            &mut output,
            Runtime::new("media-proof", Default::default(), false),
        )
        .await
        .unwrap();
    assert_eq!(exit.unwrap_or(0), 0, "{}", String::from_utf8_lossy(&output));
    let calls = client.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for call in calls.iter() {
        assert_eq!(call.url, "https://example.test/binary");
        assert_eq!(call.body.as_deref(), Some([0, 255, 128, 13, 10].as_slice()));
        assert_eq!(
            call.headers,
            vec![("Content-Type".into(), "application/octet-stream".into())]
        );
    }
}

#[tokio::test]
async fn invalid_binary_slots_and_frames_never_reach_http() {
    let client = Arc::new(RecordingClient::default());
    let cli = compile_cli(
        &media_contract(),
        IncursHttpTransport::new(client.clone()),
        &ServerSelection::default(),
    )
    .unwrap();
    for (tool, arguments) in [
        ("op_uploadBinary", json!({"body_base64":"!"})),
        (
            "op_uploadBinary",
            json!({"body_base64":"","body_json":"null"}),
        ),
        ("op_uploadBinary", json!({"body_base64":"","body":"text"})),
        ("op_uploadNdjson", json!({"body_base64":"MQ=="})),
        ("op_uploadRecords", json!({"body":[1,2]})),
        (
            "op_sendConfig",
            json!({"body":{"name":"x"},"media_type":"text/plain;charset=UTF-8"}),
        ),
    ] {
        let result = cli
            .tool_catalog()
            .call(
                tool,
                arguments.as_object().unwrap().clone().into_iter().collect(),
                ToolCallOptions::isolated(),
            )
            .await;
        assert!(
            matches!(result, ToolCallOutcome::Error { .. }),
            "{result:?}"
        );
    }
    assert!(client.0.lock().unwrap().is_empty());
}

#[test]
fn binary_text_alternative_discovery_keeps_every_source_constraint() {
    let contract = resolve_document(
        &json!({
            "openapi":"3.1.0", "info":{"title":"Binary text constraints","version":"1"},
            "servers":[{"url":"https://example.test"}],
            "paths":{"/bytes":{"post":{
                "operationId":"storeBytes",
                "requestBody":{"required":true,"content":{"application/octet-stream":{"schema":{
                    "allOf":[{"type":"string","minLength":5},{"type":"string","minLength":1}]
                }}}},
                "responses":{"204":{"description":"accepted"}}
            }}}
        }),
        ResolveOptions::new("media-constraints"),
    )
    .unwrap();
    let cli = compile_cli(
        &contract,
        IncursHttpTransport::new(Arc::new(RecordingClient::default())),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    let schema = &catalog.get("op_storeBytes").unwrap().input_schema;
    let validator = jsonschema::draft202012::options()
        .offline()
        .build(schema)
        .unwrap();
    assert!(validator.is_valid(&json!({"body":"valid"})));
    assert!(!validator.is_valid(&json!({"body":"x"})));
    assert!(!validator.is_valid(&json!({"body":""})));
}
