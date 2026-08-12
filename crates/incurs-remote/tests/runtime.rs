use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedContext, TypedResult};
use incurs_remote::{
    ArtifactHandle, CapabilityManifest, RemoteRequestMetadata, RemoteToolCall, RemoteToolControl,
    RemoteToolResult, RemoteToolRuntime, ToolCatalogRemoteRuntime,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize, incurs::Args)]
struct EchoArgs {
    /// Message to echo.
    message: String,
}

#[derive(Deserialize, incurs::Options)]
struct EchoOptions {
    /// Render the message loudly.
    loud: bool,
}

#[derive(JsonSchema, Serialize)]
struct EchoOutput {
    message: String,
    method: String,
    path: String,
}

fn runtime(sender: Option<mpsc::Sender<()>>) -> ToolCatalogRemoteRuntime {
    let command = CommandDef::typed::<EchoArgs, EchoOptions, (), EchoOutput, _, _>(
        "echo",
        move |ctx: TypedContext<EchoArgs, EchoOptions, ()>| {
            let sender = sender.clone();
            async move {
                if let Some(sender) = sender
                    && sender.send(()).await.is_err()
                {}
                let request = ctx.request.unwrap_or_default();
                TypedResult::ok(EchoOutput {
                    message: if ctx.options.loud {
                        ctx.args.message.to_uppercase()
                    } else {
                        ctx.args.message
                    },
                    method: request.method,
                    path: request.path,
                })
            }
        },
    )
    .description("Echo a remote request")
    .done();
    ToolCatalogRemoteRuntime::new(
        Cli::create("fixture")
            .version("1.2.3")
            .command("echo", command)
            .tool_catalog(),
    )
}

fn blocking_runtime(sender: mpsc::Sender<()>) -> ToolCatalogRemoteRuntime {
    let command = CommandDef::typed::<(), (), (), EchoOutput, _, _>("block", move |_| {
        let sender = sender.clone();
        async move {
            sender.send(()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
            TypedResult::ok(EchoOutput {
                message: "done".to_string(),
                method: String::new(),
                path: String::new(),
            })
        }
    })
    .description("Block until cancelled")
    .done();
    ToolCatalogRemoteRuntime::new(
        Cli::create("fixture")
            .command("block", command)
            .tool_catalog(),
    )
}

#[test]
fn manifest_adapts_tool_catalog_definitions() {
    let manifest = runtime(None).manifest();

    assert_eq!(manifest.name, "fixture");
    assert_eq!(manifest.version.as_deref(), Some("1.2.3"));
    assert_eq!(manifest.capabilities.len(), 1);
    assert_eq!(manifest.capabilities[0].id, "echo");
    assert_eq!(
        manifest.capabilities[0].description,
        "Echo a remote request"
    );
    assert_eq!(manifest.capabilities[0].input_schema["type"], "object");
    assert_eq!(
        manifest.capabilities[0].output_schema.as_ref().unwrap()["type"],
        "object"
    );

    let encoded = serde_json::to_value(&manifest).unwrap();
    assert_eq!(encoded["schemaVersion"], "incurs.remote.capabilities.v1");
    assert_eq!(encoded["protocolVersion"], "incurs.remote.v1");
    let decoded: CapabilityManifest = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.name, manifest.name);
    assert_eq!(decoded.capabilities.len(), manifest.capabilities.len());
}

#[tokio::test]
async fn runtime_executes_catalog_tool_with_request_metadata() {
    let mut call = RemoteToolCall::new("call-1", "echo");
    call.agent_id = Some("agent-1".to_string());
    call.device_id = Some("device-1".to_string());
    call.capability_version = Some("1.0.0".to_string());
    call.arguments = json!({
        "message": "hello",
        "loud": true
    });
    call.idempotency_key = Some("idem-1".to_string());
    call.trace_id = Some("trace-1".to_string());
    call.request = RemoteRequestMetadata {
        headers: HashMap::from([("x-trace-id".to_string(), "trace-1".to_string())]),
        method: "remote.call".to_string(),
        path: "/remote/echo".to_string(),
        caller: Some("test".to_string()),
        metadata: BTreeMap::new(),
    };

    let result = runtime(None).call(call, RemoteToolControl::default()).await;

    match result {
        RemoteToolResult::Ok {
            call_id,
            data,
            duration_ms: _,
            artifacts,
            cta,
        } => {
            assert_eq!(call_id, "call-1");
            assert_eq!(
                data,
                json!({
                    "message": "HELLO",
                    "method": "remote.call",
                    "path": "/remote/echo"
                })
            );
            assert!(artifacts.is_empty());
            assert!(cta.is_none());
        }
        other => panic!("expected ok result, got {other:?}"),
    }
}

#[tokio::test]
async fn cancellation_prevents_catalog_invocation() {
    let (sender, mut receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = runtime(Some(sender))
        .call(
            RemoteToolCall::new("call-2", "echo"),
            RemoteToolControl {
                cancellation,
                events: None,
            },
        )
        .await;

    match result {
        RemoteToolResult::Error { call_id, error, .. } => {
            assert_eq!(call_id, "call-2");
            assert_eq!(error.code, "CANCELLED");
        }
        other => panic!("expected cancellation error, got {other:?}"),
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn expired_deadline_prevents_catalog_invocation() {
    let (sender, mut receiver) = mpsc::channel(1);
    let mut call = RemoteToolCall::new("call-3", "echo");
    call.deadline_ms = Some(1);

    let result = runtime(Some(sender))
        .call(call, RemoteToolControl::default())
        .await;

    match result {
        RemoteToolResult::Error { call_id, error, .. } => {
            assert_eq!(call_id, "call-3");
            assert_eq!(error.code, "DEADLINE_EXCEEDED");
            assert_eq!(error.retryable, Some(true));
        }
        other => panic!("expected deadline error, got {other:?}"),
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn active_deadline_cancels_catalog_invocation() {
    let (sender, mut receiver) = mpsc::channel(1);
    let mut call = RemoteToolCall::new("call-5", "block");
    call.deadline_ms = Some(now_unix_ms() + 20);
    let runtime = blocking_runtime(sender);

    let result = runtime.call(call, RemoteToolControl::default()).await;

    receiver.try_recv().unwrap();
    match result {
        RemoteToolResult::Error {
            call_id,
            error,
            duration_ms,
            ..
        } => {
            assert_eq!(call_id, "call-5");
            assert_eq!(error.code, "DEADLINE_EXCEEDED");
            assert!(duration_ms < 1_000);
        }
        other => panic!("expected deadline error, got {other:?}"),
    }
}

#[tokio::test]
async fn caller_cancellation_still_cancels_active_catalog_invocation() {
    let (sender, mut receiver) = mpsc::channel(1);
    let cancellation = CancellationToken::new();
    let runtime = blocking_runtime(sender);
    let control = RemoteToolControl {
        cancellation: cancellation.clone(),
        events: None,
    };

    let task = tokio::spawn(async move {
        runtime
            .call(RemoteToolCall::new("call-6", "block"), control)
            .await
    });
    receiver.recv().await.unwrap();
    cancellation.cancel();
    let result = task.await.unwrap();

    match result {
        RemoteToolResult::Error { call_id, error, .. } => {
            assert_eq!(call_id, "call-6");
            assert_eq!(error.code, "CANCELLED");
        }
        other => panic!("expected cancellation error, got {other:?}"),
    }
}

#[test]
fn remote_call_and_result_serialize_artifact_handles() {
    let artifact = ArtifactHandle {
        id: "artifact-1".to_string(),
        media_type: "application/json".to_string(),
        name: Some("payload.json".to_string()),
        uri: Some("incurs-artifact://artifact-1".to_string()),
        metadata: BTreeMap::from([("sha256".to_string(), "abc123".to_string())]),
    };
    let mut call = RemoteToolCall::new("call-4", "echo");
    call.artifacts.push(artifact.clone());

    let encoded = serde_json::to_value(&call).unwrap();
    assert_eq!(encoded["call_id"], "call-4");
    assert_eq!(encoded["arguments"], json!({}));
    assert_eq!(encoded["artifacts"][0]["id"], "artifact-1");
    let decoded: RemoteToolCall = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, call);

    let result = RemoteToolResult::Ok {
        call_id: call.call_id,
        data: Value::Null,
        duration_ms: 7,
        artifacts: vec![artifact],
        cta: None,
    };
    let encoded = serde_json::to_value(&result).unwrap();
    assert_eq!(encoded["status"], "ok");
    assert_eq!(encoded["duration_ms"], 7);
    assert_eq!(encoded["artifacts"][0]["media_type"], "application/json");
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
