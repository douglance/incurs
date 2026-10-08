//! Verify the remote adapter invokes the existing ToolCatalog implementation.
#![cfg(feature = "adapters")]
use incurs::{
    cli::Cli,
    command::{CommandDef, TypedResult},
};
use incurs_openapi::{
    ResolveOptions,
    adapters::{OperationToolBinding, RemoteOperationOutcome, RemoteRuntimeAdapter},
    resolve_document,
};
use incurs_remote::ToolCatalogRemoteRuntime;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn invokes_real_tool_catalog_with_an_explicit_operation_binding() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let command = CommandDef::typed::<(), (), (), Value, _, _>("ping", move |_| {
        let calls = handler_calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            TypedResult::ok(json!({"source":"real ToolCatalog","answer":42}))
        }
    })
    .description("A local adapter proof")
    .done();
    let runtime =
        ToolCatalogRemoteRuntime::new(Cli::create("proof").command("ping", command).tool_catalog());
    let adapter = RemoteRuntimeAdapter::new(
        Arc::new(runtime),
        [OperationToolBinding {
            operation_id: "worker-proof/listWidgets".into(),
            capability_id: "ping".into(),
        }],
    )
    .unwrap();
    let document = serde_json::from_str(include_str!(
        "../../openapi-workers/fixtures/proof-openapi.json"
    ))
    .unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("worker-proof")).unwrap();
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == "listWidgets")
        .unwrap();
    let result = adapter
        .call_operation(operation, "real-catalog-proof", json!({}))
        .await
        .unwrap();
    assert_eq!(
        result,
        RemoteOperationOutcome::Ok(json!({"source":"real ToolCatalog","answer":42}))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
