//! Native adapter proof for existing incurs runtime boundaries.
//!
//! The portable compiler and HTTP binder stay dependency-light. This module is
//! available behind the `adapters` feature for hosts that want to bridge a
//! generated Forge SDK request into existing incurs runtime traits.

use crate::runtime::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport};
use crate::{ForgeError, ForgeResult, Operation};
use incurs::outbound::{HttpRequest, SharedHttpClient};
use incurs_codemode::{CodeModeRunOptions, CodeModeService, ExecutionState};
use incurs_remote::{RemoteToolCall, RemoteToolControl, RemoteToolResult, RemoteToolRuntime};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A Forge HTTP transport backed by incurs outbound HTTP.
#[derive(Clone)]
pub struct IncursHttpTransport {
    client: SharedHttpClient,
}

impl IncursHttpTransport {
    /// Creates an adapter over an existing incurs outbound HTTP client.
    pub fn new(client: SharedHttpClient) -> Self {
        Self { client }
    }
}

impl ForgeTransport for IncursHttpTransport {
    async fn exchange(&self, request: ForgeHttpRequest) -> ForgeResult<ForgeHttpResponse> {
        let response = self
            .client
            .send(HttpRequest {
                method: request.method,
                url: request.url,
                headers: request.headers,
                body: request.body,
            })
            .await
            .map_err(|error| ForgeError(format!("incurs http exchange failed: {error}")))?;
        let status = response.status;
        let headers = response.headers.clone();
        let body = response
            .bytes()
            .await
            .map_err(|error| ForgeError(format!("incurs http body failed: {error}")))?;
        Ok(ForgeHttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Explicit mapping from a Forge operation id to a remote capability id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationToolBinding {
    /// Stable Forge operation id.
    pub operation_id: String,
    /// Canonical incurs capability id.
    pub capability_id: String,
}

/// Structured outcome from an incurs remote runtime call.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteOperationOutcome {
    /// The bound capability returned structured data.
    Ok(Value),
    /// The bound capability returned a machine-readable error.
    Error(Value),
}

/// Executes explicitly bound Forge operations through an incurs remote runtime.
#[derive(Clone)]
pub struct RemoteRuntimeAdapter<R: ?Sized> {
    runtime: Arc<R>,
    bindings: BTreeMap<String, String>,
}

impl<R> RemoteRuntimeAdapter<R>
where
    R: RemoteToolRuntime + ?Sized,
{
    /// Creates an adapter with exact operation-to-capability bindings.
    pub fn new(
        runtime: Arc<R>,
        bindings: impl IntoIterator<Item = OperationToolBinding>,
    ) -> ForgeResult<Self> {
        let mut out = BTreeMap::new();
        for binding in bindings {
            if out
                .insert(binding.operation_id.clone(), binding.capability_id)
                .is_some()
            {
                return Err(ForgeError(format!(
                    "duplicate operation binding: {}",
                    binding.operation_id
                )));
            }
        }
        Ok(Self {
            runtime,
            bindings: out,
        })
    }

    /// Calls the bound capability with default remote control.
    pub async fn call_operation(
        &self,
        operation: &Operation,
        call_id: impl Into<String>,
        arguments: Value,
    ) -> ForgeResult<RemoteOperationOutcome> {
        self.call_operation_with_control(
            operation,
            call_id,
            arguments,
            RemoteToolControl::default(),
        )
        .await
    }

    /// Calls the bound capability with caller-supplied cancellation and event control.
    pub async fn call_operation_with_control(
        &self,
        operation: &Operation,
        call_id: impl Into<String>,
        arguments: Value,
        control: RemoteToolControl,
    ) -> ForgeResult<RemoteOperationOutcome> {
        let capability = self.bindings.get(&operation.id).ok_or_else(|| {
            ForgeError(format!("operation has no remote binding: {}", operation.id))
        })?;
        let mut call = RemoteToolCall::new(call_id, capability.clone());
        call.arguments = operation_arguments_json(arguments)?;
        match self.runtime.call(call, control).await {
            RemoteToolResult::Ok { data, .. } => Ok(RemoteOperationOutcome::Ok(data)),
            RemoteToolResult::Error { error, .. } => serde_json::to_value(error)
                .map(RemoteOperationOutcome::Error)
                .map_err(|error| ForgeError(error.to_string())),
        }
    }
}

/// Code Mode program strategy for one Forge operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationCodeBinding {
    /// Stable Forge operation id.
    pub operation_id: String,
    /// JavaScript program submitted to Code Mode for this operation.
    pub code: String,
}

/// Runs explicitly bound Forge operations through Code Mode lifecycle methods.
#[derive(Clone)]
pub struct CodeModeOperationAdapter<S: ?Sized> {
    service: Arc<S>,
    bindings: BTreeMap<String, String>,
    options: CodeModeRunOptions,
}

impl<S> CodeModeOperationAdapter<S>
where
    S: CodeModeService + ?Sized,
{
    /// Creates an adapter with explicit operation-to-program bindings.
    pub fn new(
        service: Arc<S>,
        bindings: impl IntoIterator<Item = OperationCodeBinding>,
        options: CodeModeRunOptions,
    ) -> ForgeResult<Self> {
        let mut out = BTreeMap::new();
        for binding in bindings {
            if out
                .insert(binding.operation_id.clone(), binding.code)
                .is_some()
            {
                return Err(ForgeError(format!(
                    "duplicate operation binding: {}",
                    binding.operation_id
                )));
            }
        }
        Ok(Self {
            service,
            bindings: out,
            options,
        })
    }

    /// Returns the underlying Code Mode service for lifecycle operations not wrapped here.
    pub fn service(&self) -> &Arc<S> {
        &self.service
    }

    /// Starts Code Mode execution for the bound operation and returns its durable lifecycle state.
    pub async fn execute_operation(
        &self,
        operation: &Operation,
        arguments: Value,
    ) -> ForgeResult<ExecutionState> {
        let template = self.bindings.get(&operation.id).ok_or_else(|| {
            ForgeError(format!(
                "operation has no Code Mode binding: {}",
                operation.id
            ))
        })?;
        let code = render_code_mode_program(template, &operation.id, arguments)?;
        self.service
            .execute(code, self.options.clone())
            .await
            .map_err(ForgeError)
    }

    /// Reads one durable execution state.
    pub async fn execution(&self, execution_id: impl Into<String>) -> ForgeResult<ExecutionState> {
        self.service
            .execution(execution_id.into())
            .await
            .map_err(ForgeError)
    }

    /// Reads one artifact owned by an execution.
    pub async fn artifact(
        &self,
        execution_id: impl Into<String>,
        artifact_id: impl Into<String>,
    ) -> ForgeResult<Value> {
        self.service
            .artifact(execution_id.into(), artifact_id.into())
            .await
            .map_err(ForgeError)
    }

    /// Approves one pending action and continues lifecycle replay.
    pub async fn approve(
        &self,
        execution_id: impl Into<String>,
        seq: u64,
    ) -> ForgeResult<ExecutionState> {
        self.service
            .approve(execution_id.into(), seq, self.options.clone())
            .await
            .map_err(ForgeError)
    }

    /// Rejects one pending action.
    pub async fn reject(
        &self,
        execution_id: impl Into<String>,
        seq: u64,
    ) -> ForgeResult<ExecutionState> {
        self.service
            .reject(execution_id.into(), seq)
            .await
            .map_err(ForgeError)
    }

    /// Cancels one running or paused execution.
    pub async fn cancel(&self, execution_id: impl Into<String>) -> ForgeResult<ExecutionState> {
        self.service
            .cancel(execution_id.into())
            .await
            .map_err(ForgeError)
    }
}

fn render_code_mode_program(
    template: &str,
    operation_id: &str,
    arguments: Value,
) -> ForgeResult<String> {
    let payload =
        json!({ "operation_id": operation_id, "arguments": operation_arguments_json(arguments)? });
    let payload = serde_json::to_string(&payload).map_err(|error| ForgeError(error.to_string()))?;
    Ok(template.replace("__FORGE_OPERATION_REQUEST__", &payload))
}

/// Converts generated SDK arguments into a JSON object accepted by adapters.
pub fn operation_arguments_json(value: Value) -> ForgeResult<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| ForgeError("operation arguments must be a JSON object".to_string()))?;
    for key in object.keys() {
        if ![
            "path",
            "query",
            "querystring",
            "header",
            "cookie",
            "body",
            "media_type",
        ]
        .contains(&key.as_str())
        {
            return Err(ForgeError(format!(
                "unknown operation argument location: {key}"
            )));
        }
    }
    Ok(Value::Object(object.clone()))
}

/// Builds arguments from optional location maps for tests and simple hosts.
pub fn operation_arguments_from_locations(
    locations: BTreeMap<String, Value>,
) -> ForgeResult<Value> {
    operation_arguments_json(Value::Object(locations.into_iter().collect::<Map<_, _>>()))
}
