//! Native adapter proof tests over existing incurs traits.
#![cfg(feature = "adapters")]

use async_trait::async_trait;
use incurs::outbound::{HttpClient, HttpClientError, HttpRequest, HttpResponse};
use incurs_codemode::{
    CodeModeRunOptions, CodeModeService, ExecutionState, ExecutionStatus, SearchOutput,
};
use incurs_forge::adapters::{
    CodeModeOperationAdapter, IncursHttpTransport, OperationCodeBinding, OperationToolBinding,
    RemoteOperationOutcome, RemoteRuntimeAdapter, operation_arguments_json,
};
use incurs_forge::runtime::{ForgeHttpRequest, ForgeTransport};
use incurs_forge::{ApiResponse, Operation};
use incurs_remote::{
    CapabilityManifest, RemoteToolCall, RemoteToolControl, RemoteToolError, RemoteToolResult,
    RemoteToolRuntime,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn outbound_http_adapter_preserves_exchange_shape() {
    let client = Arc::new(RecordingHttpClient::default());
    let transport = IncursHttpTransport::new(client.clone());
    let response = transport
        .exchange(ForgeHttpRequest {
            method: "POST".to_string(),
            url: "https://api.example.test/widgets".to_string(),
            headers: vec![("X-Trace".to_string(), "t1".to_string())],
            body: Some(br#"{"name":"Ada"}"#.to_vec()),
        })
        .await
        .unwrap();

    assert_eq!(
        client.take().unwrap(),
        HttpRequest {
            method: "POST".to_string(),
            url: "https://api.example.test/widgets".to_string(),
            headers: vec![("X-Trace".to_string(), "t1".to_string())],
            body: Some(br#"{"name":"Ada"}"#.to_vec()),
        }
    );
    assert_eq!(response.status, 207);
    assert_eq!(
        response.headers,
        vec![
            ("Set-Cookie".to_string(), "a=1".to_string()),
            ("Set-Cookie".to_string(), "b=2".to_string())
        ]
    );
    assert_eq!(response.body, vec![0, 255, 3]);
}

#[tokio::test]
async fn remote_runtime_adapter_uses_explicit_binding_and_retains_machine_errors_and_control() {
    let runtime = Arc::new(FakeRemoteRuntime::default());
    let adapter = RemoteRuntimeAdapter::new(
        runtime.clone(),
        [OperationToolBinding {
            operation_id: "proof/listWidgets".to_string(),
            capability_id: "widgets.list".to_string(),
        }],
    )
    .unwrap();
    let operation = operation("proof/listWidgets");
    let control = RemoteToolControl {
        cancellation: CancellationToken::new(),
        events: None,
    };
    control.cancellation.cancel();

    let ok = adapter
        .call_operation_with_control(&operation, "call-1", json!({"query":{"limit":2}}), control)
        .await
        .unwrap();
    assert_eq!(
        ok,
        RemoteOperationOutcome::Ok(
            json!({"capability":"widgets.list","arguments":{"query":{"limit":2}},"cancelled":true})
        )
    );

    runtime.fail_next();
    let error = adapter
        .call_operation(&operation, "call-2", json!({}))
        .await
        .unwrap();
    let RemoteOperationOutcome::Error(error) = error else {
        panic!("expected structured error");
    };
    assert_eq!(error["code"], "REMOTE_VALIDATION");
    assert_eq!(error["retryable"], false);
    assert_eq!(error["exit_code"], 64);
}

#[tokio::test]
async fn codemode_adapter_starts_execution_and_forwards_lifecycle_calls() {
    let service = Arc::new(FakeCodeModeService::default());
    let adapter = CodeModeOperationAdapter::new(
        service.clone(),
        [OperationCodeBinding {
            operation_id: "proof/listWidgets".to_string(),
            code: "const request = __FORGE_OPERATION_REQUEST__; return request;".to_string(),
        }],
        CodeModeRunOptions::default(),
    )
    .unwrap();

    let state = adapter
        .execute_operation(&operation("proof/listWidgets"), json!({"path":{"id":"7"}}))
        .await
        .unwrap();
    assert_eq!(state.id, "exec-1");
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert!(service.take_code().unwrap().contains("proof/listWidgets"));
    assert_eq!(adapter.execution("exec-1").await.unwrap().id, "exec-1");
    assert_eq!(adapter.approve("exec-1", 4).await.unwrap().id, "approved-4");
    assert_eq!(adapter.reject("exec-1", 5).await.unwrap().id, "rejected-5");
    assert_eq!(adapter.cancel("exec-1").await.unwrap().id, "cancelled");
    assert_eq!(
        adapter.artifact("exec-1", "artifact-1").await.unwrap(),
        json!({"artifact":"artifact-1"})
    );
}

#[test]
fn generated_arguments_reject_unknown_locations_before_transport() {
    assert!(
        operation_arguments_json(json!({"path":{"id":"7"},"mystery":true}))
            .unwrap_err()
            .0
            .contains("unknown operation argument location")
    );
    assert_eq!(
        operation_arguments_json(json!({"body":null})).unwrap(),
        json!({"body":null})
    );
}

#[derive(Default)]
struct RecordingHttpClient {
    request: Mutex<Option<HttpRequest>>,
}
impl RecordingHttpClient {
    fn take(&self) -> Option<HttpRequest> {
        self.request.lock().unwrap().take()
    }
}

#[async_trait]
impl HttpClient for RecordingHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        *self.request.lock().unwrap() = Some(request);
        Ok(HttpResponse::from_bytes(
            207,
            vec![
                ("Set-Cookie".to_string(), "a=1".to_string()),
                ("Set-Cookie".to_string(), "b=2".to_string()),
            ],
            vec![0, 255, 3],
        ))
    }
}

#[derive(Default)]
struct FakeRemoteRuntime {
    fail: Mutex<bool>,
}
impl FakeRemoteRuntime {
    fn fail_next(&self) {
        *self.fail.lock().unwrap() = true;
    }
}

#[async_trait]
impl RemoteToolRuntime for FakeRemoteRuntime {
    fn manifest(&self) -> CapabilityManifest {
        CapabilityManifest {
            schema_version: "test".to_string(),
            protocol_version: "test".to_string(),
            name: "test".to_string(),
            version: None,
            capabilities: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
    async fn call(&self, call: RemoteToolCall, control: RemoteToolControl) -> RemoteToolResult {
        if std::mem::take(&mut *self.fail.lock().unwrap()) {
            let mut error = RemoteToolError::new("REMOTE_VALIDATION", "bad arguments", Some(false));
            error.exit_code = Some(64);
            return RemoteToolResult::Error {
                call_id: call.call_id,
                error,
                duration_ms: 3,
                artifacts: Vec::new(),
            };
        }
        RemoteToolResult::Ok {
            call_id: call.call_id,
            data: json!({"capability": call.capability, "arguments": call.arguments, "cancelled": control.cancellation.is_cancelled()}),
            duration_ms: 2,
            artifacts: Vec::new(),
            cta: None,
        }
    }
}

#[derive(Default)]
struct FakeCodeModeService {
    code: Mutex<Option<String>>,
}
impl FakeCodeModeService {
    fn take_code(&self) -> Option<String> {
        self.code.lock().unwrap().take()
    }
}

#[async_trait]
impl CodeModeService for FakeCodeModeService {
    async fn search(&self, _query: String) -> Result<SearchOutput, String> {
        Err("unused".to_string())
    }
    async fn execute(
        &self,
        code: String,
        _options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        *self.code.lock().unwrap() = Some(code.clone());
        Ok(state("exec-1", code, ExecutionStatus::Completed))
    }
    async fn execution(&self, execution_id: String) -> Result<ExecutionState, String> {
        Ok(state(
            &execution_id,
            "".to_string(),
            ExecutionStatus::Completed,
        ))
    }
    async fn artifact(&self, _execution_id: String, artifact_id: String) -> Result<Value, String> {
        Ok(json!({"artifact": artifact_id}))
    }
    async fn approve(
        &self,
        _execution_id: String,
        seq: u64,
        _options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        Ok(state(
            &format!("approved-{seq}"),
            "".to_string(),
            ExecutionStatus::Completed,
        ))
    }
    async fn reject(&self, _execution_id: String, seq: u64) -> Result<ExecutionState, String> {
        Ok(state(
            &format!("rejected-{seq}"),
            "".to_string(),
            ExecutionStatus::Rejected,
        ))
    }
    async fn cancel(&self, _execution_id: String) -> Result<ExecutionState, String> {
        Ok(state(
            "cancelled",
            "".to_string(),
            ExecutionStatus::Cancelled,
        ))
    }
}

fn state(id: &str, code: String, status: ExecutionStatus) -> ExecutionState {
    ExecutionState {
        id: id.to_string(),
        code,
        status,
        log: Vec::new(),
        result: Some(json!({"ok": true})),
        error: None,
        logs: Vec::new(),
        connectors: Vec::new(),
        capabilities: None,
        events: Vec::new(),
        created_at: 1,
        updated_at: 2,
    }
}

fn operation(id: &str) -> Operation {
    Operation {
        id: id.to_string(),
        name: "listWidgets".to_string(),
        description: None,
        method: "GET".to_string(),
        path: "/widgets".to_string(),
        servers: Vec::new(),
        parameters: Vec::new(),
        request_body: None,
        responses: BTreeMap::from([(
            "200".to_string(),
            ApiResponse {
                description: "ok".to_string(),
                content: BTreeMap::new(),
                headers: BTreeMap::new(),
            },
        )]),
        security: Value::Array(Vec::new()),
    }
}
