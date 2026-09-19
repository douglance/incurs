use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CodeModeRuntime, Connector, ConnectorDescription, ReplayPolicy, RuntimeError, ToolContext,
    ToolDecision, describe, search,
};

/// Wire-safe result of one connector dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchResponse {
    /// Successful connector or replay result.
    pub result: Option<Value>,
    /// Control signal consumed by the sandbox proxy.
    #[serde(rename = "__codemode_control__")]
    pub control: Option<String>,
    /// Host-side connector or runtime error.
    pub message: Option<String>,
}

/// Wire-safe decision for a local `codemode.step` callback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResponse {
    /// Replay, execute, or pause.
    pub kind: String,
    /// Host-assigned sequence.
    pub seq: u64,
    /// Previously recorded step result.
    pub result: Option<Value>,
}

/// One replay pass's sequenced bridge between a sandbox and Rust connectors.
pub struct DispatchSession {
    runtime: Arc<CodeModeRuntime>,
    execution_id: String,
    context: ToolContext,
    connectors: BTreeMap<String, Arc<dyn Connector>>,
    descriptions: BTreeMap<String, ConnectorDescription>,
    /// Descriptions resolved so far, filled in as namespaces are first used.
    described: tokio::sync::Mutex<BTreeMap<String, ConnectorDescription>>,
    sequence: AtomicU64,
}

impl DispatchSession {
    /// Resolves connector descriptions and creates a fresh replay cursor.
    pub async fn new(
        runtime: Arc<CodeModeRuntime>,
        execution_id: impl Into<String>,
        connectors: Vec<Arc<dyn Connector>>,
    ) -> Result<Self, String> {
        let execution_id = execution_id.into();
        let mut descriptions = Vec::new();
        for connector in &connectors {
            descriptions.push(connector.describe().await?);
        }
        Self::new_with_descriptions(runtime, execution_id, connectors, descriptions).await
    }

    /// Creates a replay cursor using a previously captured capability snapshot.
    pub async fn new_with_descriptions(
        runtime: Arc<CodeModeRuntime>,
        execution_id: impl Into<String>,
        connectors: Vec<Arc<dyn Connector>>,
        snapshot: Vec<ConnectorDescription>,
    ) -> Result<Self, String> {
        let execution_id = execution_id.into();
        Self::new_with_descriptions_and_context(
            runtime,
            ToolContext {
                execution_id,
                control: Default::default(),
                request: None,
            },
            connectors,
            snapshot,
        )
        .await
    }

    /// Creates a replay cursor with an execution-scoped connector context.
    pub async fn new_with_descriptions_and_context(
        runtime: Arc<CodeModeRuntime>,
        context: ToolContext,
        connectors: Vec<Arc<dyn Connector>>,
        snapshot: Vec<ConnectorDescription>,
    ) -> Result<Self, String> {
        // Namespaces are registered by name alone. Describing every connector here
        // meant contacting every configured server before any program ran, so one
        // slow or dead server delayed or failed executions that never mentioned it.
        let mut resolved = BTreeMap::new();
        for connector in connectors {
            let name = connector.name().to_string();
            if resolved.insert(name.clone(), connector).is_some() {
                return Err(format!("Duplicate connector name \"{name}\""));
            }
        }
        let descriptions = snapshot
            .into_iter()
            .map(|description| (description.name.clone(), description))
            .collect();
        Ok(Self {
            runtime,
            execution_id: context.execution_id.clone(),
            context,
            connectors: resolved,
            descriptions,
            described: tokio::sync::Mutex::new(BTreeMap::new()),
            sequence: AtomicU64::new(0),
        })
    }

    /// Returns descriptions used to generate the child Worker bindings.
    pub fn descriptions(&self) -> Vec<ConnectorDescription> {
        self.descriptions.values().cloned().collect()
    }

    /// Returns one connector's description, contacting it at most once per pass.
    ///
    /// This is the whole of the laziness. A namespace the program never calls is
    /// never described, so a server that is slow, unreachable or removed costs
    /// nothing until something actually uses it, and cannot fail an execution that
    /// does not touch it.
    async fn describe_once(&self, name: &str) -> Result<ConnectorDescription, String> {
        // The cache lock is scoped so it is never held across `describe`. A
        // connector may reach back into this session -- a nested apoc invocation
        // does exactly that -- and holding the lock across that await would
        // deadlock the outer program against its own child.
        {
            let cache = self.described.lock().await;
            if let Some(description) = cache.get(name) {
                return Ok(description.clone());
            }
        }
        let connector = self
            .connectors
            .get(name)
            .ok_or_else(|| capability_unavailable(name, ""))?;
        let description = connector.describe().await?;
        self.described
            .lock()
            .await
            .insert(name.to_string(), description.clone());
        Ok(description)
    }

    /// Describes every connector, for the callers that genuinely need all of them.
    ///
    /// Only the in-sandbox `codemode.search` and `codemode.describe` use this: a
    /// program asking what exists is asking to enumerate, so it pays for the
    /// enumeration. Nothing on the ordinary dispatch path calls it.
    async fn described_all(&self) -> Result<Vec<ConnectorDescription>, String> {
        let names: Vec<String> = self.connectors.keys().cloned().collect();
        let mut descriptions = Vec::with_capacity(names.len());
        for name in names {
            descriptions.push(self.describe_once(&name).await?);
        }
        Ok(descriptions)
    }

    /// Executes or replays one connector call under the runtime policy.
    pub async fn call(
        &self,
        connector: &str,
        method: &str,
        arguments: Value,
        now: u64,
    ) -> DispatchResponse {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.call_at(seq, connector, method, arguments, now).await
    }

    /// Executes or replays one connector call at an explicit sandbox sequence.
    pub async fn call_at(
        &self,
        seq: u64,
        connector: &str,
        method: &str,
        arguments: Value,
        now: u64,
    ) -> DispatchResponse {
        match self
            .call_inner(seq, connector, method, arguments, now)
            .await
        {
            Ok(response) => response,
            Err(error) => DispatchResponse {
                result: None,
                control: Some("error".to_string()),
                message: Some(error),
            },
        }
    }

    /// Begins a deterministic local step.
    pub async fn begin_step(&self, name: &str, now: u64) -> Result<StepResponse, RuntimeError> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.begin_step_at(seq, name, now).await
    }

    /// Begins a deterministic local step at an explicit sandbox sequence.
    pub async fn begin_step_at(
        &self,
        seq: u64,
        name: &str,
        now: u64,
    ) -> Result<StepResponse, RuntimeError> {
        Ok(
            match self
                .runtime
                .decide(
                    &self.execution_id,
                    seq,
                    "__step",
                    name,
                    Value::Null,
                    false,
                    false,
                    now,
                )
                .await?
            {
                ToolDecision::Replay(result) => StepResponse {
                    kind: "replay".to_string(),
                    seq,
                    result: Some(result),
                },
                ToolDecision::Execute(seq) => StepResponse {
                    kind: "execute".to_string(),
                    seq,
                    result: None,
                },
                ToolDecision::Pause(seq) => StepResponse {
                    kind: "pause".to_string(),
                    seq,
                    result: None,
                },
            },
        )
    }

    /// Records a local step result for replay.
    pub async fn record_step(&self, seq: u64, result: Value, now: u64) -> Result<(), RuntimeError> {
        self.runtime
            .record_result(&self.execution_id, seq, result, now)
            .await
    }

    /// Notifies connectors that the current sandbox pass ended.
    pub async fn pass_ended(&self, status: &str) {
        for connector in self.connectors.values() {
            connector.pass_ended(&self.execution_id, status).await;
        }
    }

    /// Notifies connectors that the execution reached a terminal state.
    pub async fn execution_ended(&self, status: &str) {
        for connector in self.connectors.values() {
            connector.execution_ended(&self.execution_id, status).await;
        }
    }

    async fn call_inner(
        &self,
        seq: u64,
        connector_name: &str,
        method: &str,
        arguments: Value,
        now: u64,
    ) -> Result<DispatchResponse, String> {
        if connector_name == "codemode" {
            return self.call_builtin(seq, method, arguments, now).await;
        }
        let connector = self
            .connectors
            .get(connector_name)
            .ok_or_else(|| capability_unavailable(connector_name, method))?;
        let description = self.describe_once(connector_name).await?;
        let tool = description
            .tools
            .iter()
            .find(|tool| tool.name == method)
            .ok_or_else(|| format!("Tool \"{method}\" not found on {connector_name}"))?;
        match self
            .runtime
            .decide(
                &self.execution_id,
                seq,
                connector_name,
                method,
                arguments.clone(),
                tool.policy.requires_approval,
                tool.policy.replay == ReplayPolicy::Reexecute,
                now,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            ToolDecision::Replay(result) => Ok(DispatchResponse {
                result: Some(result),
                control: None,
                message: None,
            }),
            ToolDecision::Pause(_) => Ok(DispatchResponse {
                result: None,
                control: Some("pause".to_string()),
                message: None,
            }),
            ToolDecision::Execute(seq) => {
                let result = connector.execute(method, arguments, &self.context).await?;
                self.runtime
                    .record_result(&self.execution_id, seq, result.clone(), now)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(DispatchResponse {
                    result: Some(result),
                    control: None,
                    message: None,
                })
            }
        }
    }

    async fn call_builtin(
        &self,
        seq: u64,
        method: &str,
        arguments: Value,
        now: u64,
    ) -> Result<DispatchResponse, String> {
        match self
            .runtime
            .decide(
                &self.execution_id,
                seq,
                "codemode",
                method,
                arguments.clone(),
                false,
                true,
                now,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            ToolDecision::Pause(_) => Ok(DispatchResponse {
                result: None,
                control: Some("pause".to_string()),
                message: None,
            }),
            ToolDecision::Replay(result) => Ok(DispatchResponse {
                result: Some(result),
                control: None,
                message: None,
            }),
            ToolDecision::Execute(seq) => {
                let connectors = self.described_all().await?;
                let snippets = self
                    .runtime
                    .snippets()
                    .await
                    .map_err(|error| error.to_string())?;
                let value = match method {
                    "search" => serde_json::to_value(search(
                        arguments
                            .get("query")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        &connectors,
                        &snippets,
                    )),
                    "describe" => serde_json::to_value(describe(
                        arguments
                            .get("target")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        &connectors,
                        &snippets,
                    )),
                    _ => return Err(format!("Tool \"{method}\" not found on codemode")),
                }
                .map_err(|error| error.to_string())?;
                self.runtime
                    .record_result(&self.execution_id, seq, value.clone(), now)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(DispatchResponse {
                    result: Some(value),
                    control: None,
                    message: None,
                })
            }
        }
    }
}

fn capability_unavailable(connector: &str, method: &str) -> String {
    serde_json::json!({
        "code": "CAPABILITY_UNAVAILABLE",
        "message": format!(
            "Capability {connector}.{method} existed when the execution began but is unavailable"
        ),
        "connector": connector,
        "method": method,
    })
    .to_string()
}
