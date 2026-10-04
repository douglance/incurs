//! Request schema failures must stop before the outbound HTTP boundary.
#![cfg(all(feature = "adapters", not(target_arch = "wasm32")))]

use incurs::{
    outbound::{HttpClient, HttpClientError, HttpRequest, HttpResponse},
    tool::{ToolCallOptions, ToolCallOutcome},
};
use incurs_forge::{
    ResolveOptions, adapters::IncursHttpTransport, catalog::compile_cli, resolve_document,
    servers::ServerSelection,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct CountingClient(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl HttpClient for CountingClient {
    async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(HttpResponse::from_bytes(200, vec![], b"{}".to_vec()))
    }
}

#[tokio::test]
async fn tool_rejects_nested_and_selected_media_constraints_before_http() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/request_validation_openapi.json")).unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("validation-proof")).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let cli = compile_cli(
        &contract,
        IncursHttpTransport::new(Arc::new(CountingClient(calls.clone()))),
        &ServerSelection::default(),
    )
    .unwrap();
    let catalog = cli.tool_catalog();
    let valid = json!({
        "query":{"limit":3}, "media_type":"application/json",
        "body":{"profile":{"age":21,"state":"active","name":"Alice","roles":["admin"],
                          "id":9007199254740993_u64,"mode":5,"child":{"age":22,"state":"active"}}}
    });
    let mut cases = Vec::new();
    for (pointer, value) in [
        ("/body/profile/age", json!(17)),
        ("/body/profile/age", json!(21.5)),
        ("/body/profile/state", json!("deleted")),
        ("/body/profile", json!({})),
        ("/body/profile", json!("wrong")),
        ("/body/profile", json!(null)),
        ("/body/profile/name", json!("a")),
        ("/body/profile/roles", json!([])),
        ("/body/profile/roles", json!(["admin", "admin"])),
        ("/body/profile/roles", json!(["unknown"])),
        ("/body/profile/id", json!(9007199254740992_u64)),
        ("/body/profile/mode", json!(10)),
        ("/body/profile/child/age", json!(17)),
        ("/query/limit", json!(0)),
        ("/query/limit", json!("3")),
        ("/media_type", json!("application/vnd.other+json")),
    ] {
        let mut arguments = valid.clone();
        *arguments.pointer_mut(pointer).unwrap() = value;
        cases.push(arguments);
    }
    for pointer in ["/body", "/body/profile"] {
        let mut arguments = valid.clone();
        arguments
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(true));
        cases.push(arguments);
    }
    for arguments in cases {
        let outcome = catalog
            .call(
                "op_submit",
                arguments.as_object().unwrap().clone().into_iter().collect(),
                ToolCallOptions::isolated(),
            )
            .await;
        match outcome {
            ToolCallOutcome::Error { code, message, .. } => {
                assert_eq!(code, "FORGE_VALIDATION", "{message}");
                assert!(message.contains("validation"), "{message}");
            }
            other => panic!("invalid request reached dispatch: {arguments}: {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0, "{arguments}");
    }
    for arguments in [
        valid,
        json!({"media_type":"application/vnd.other+json","body":{"other":true}}),
    ] {
        let outcome = catalog
            .call(
                "op_submit",
                arguments.as_object().unwrap().clone().into_iter().collect(),
                ToolCallOptions::isolated(),
            )
            .await;
        assert!(matches!(outcome, ToolCallOutcome::Ok { .. }), "{outcome:?}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
