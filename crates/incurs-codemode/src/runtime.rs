use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::ConnectorDescription;

/// Default number of terminal executions retained by a runtime.
pub const DEFAULT_MAX_EXECUTIONS: usize = 50;
/// Maximum serialized bytes for one durable value.
pub const MAX_DURABLE_VALUE_BYTES: usize = 1_000_000;
/// Default age after which a paused or abandoned running execution expires.
pub const DEFAULT_PAUSED_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
/// Maximum number of ordered lifecycle events retained per execution.
pub const DEFAULT_MAX_EVENTS: usize = 1_000;

static EXECUTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Immutable connector metadata captured when an execution begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    /// Connector schemas, instructions, annotations, and resolved policies.
    pub connectors: Vec<ConnectorDescription>,
    /// Stable SHA-256 fingerprint of the serialized connector set.
    pub fingerprint: String,
}

impl CapabilitySnapshot {
    /// Captures a stable snapshot from resolved connector descriptions.
    pub fn new(connectors: Vec<ConnectorDescription>) -> Result<Self, String> {
        let bytes = serde_json::to_vec(&connectors).map_err(|error| error.to_string())?;
        Ok(Self {
            connectors,
            fingerprint: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

/// Durable reference to a JSON artifact stored outside execution state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Stable artifact identifier.
    pub id: String,
    /// Owning execution.
    pub execution_id: String,
    /// Serialized artifact size.
    pub bytes: usize,
    /// Bounded human-readable preview.
    pub preview: String,
}

/// Storage contract for oversized replay and final values.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Stores one JSON value and returns its durable reference.
    async fn put(&self, execution_id: &str, value: &Value) -> Result<ArtifactRef, String>;
    /// Loads one JSON value when it belongs to the supplied execution.
    async fn get(&self, execution_id: &str, artifact_id: &str) -> Result<Option<Value>, String>;
    /// Deletes artifacts owned by one execution.
    async fn delete_execution(&self, execution_id: &str) -> Result<(), String>;
}

#[derive(Clone)]
struct MemoryArtifact {
    execution_id: String,
    value: Value,
}

/// In-memory artifact store for local use and tests.
#[derive(Default)]
pub struct MemoryArtifactStore {
    values: Mutex<BTreeMap<String, MemoryArtifact>>,
}

#[async_trait]
impl ArtifactStore for MemoryArtifactStore {
    async fn put(&self, execution_id: &str, value: &Value) -> Result<ArtifactRef, String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        let id = format!(
            "{:x}",
            Sha256::digest([execution_id.as_bytes(), &bytes].concat())
        );
        self.values.lock().await.insert(
            id.clone(),
            MemoryArtifact {
                execution_id: execution_id.to_string(),
                value: value.clone(),
            },
        );
        Ok(ArtifactRef {
            id,
            execution_id: execution_id.to_string(),
            bytes: bytes.len(),
            preview: truncate_bytes(String::from_utf8_lossy(&bytes).into_owned(), 512),
        })
    }

    async fn get(&self, execution_id: &str, artifact_id: &str) -> Result<Option<Value>, String> {
        Ok(self
            .values
            .lock()
            .await
            .get(artifact_id)
            .filter(|artifact| artifact.execution_id == execution_id)
            .map(|artifact| artifact.value.clone()))
    }

    async fn delete_execution(&self, execution_id: &str) -> Result<(), String> {
        self.values
            .lock()
            .await
            .retain(|_, artifact| artifact.execution_id != execution_id);
        Ok(())
    }
}

/// Durable execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// A sandbox pass may make progress.
    Running,
    /// A connector action is awaiting approval.
    Paused,
    /// The program completed successfully.
    Completed,
    /// The program or replay failed.
    Error,
    /// A pending action was rejected or expired.
    Rejected,
    /// Applied actions were compensated.
    RolledBack,
    /// The execution was cancelled by its caller.
    Cancelled,
}

impl ExecutionStatus {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Error | Self::Rejected | Self::RolledBack | Self::Cancelled
        )
    }
}

/// Ordered event retained with one execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionEvent {
    /// Execution lifecycle transition.
    Status {
        /// New lifecycle status.
        status: ExecutionStatus,
        /// Event timestamp in Unix milliseconds.
        at: u64,
    },
    /// Incremental structured value.
    Chunk {
        /// Streamed value.
        data: Value,
        /// Event timestamp in Unix milliseconds.
        at: u64,
    },
    /// Runtime log entry.
    Log {
        /// Log severity.
        level: String,
        /// Log message.
        message: String,
        /// Event timestamp in Unix milliseconds.
        at: u64,
    },
    /// Execution progress update.
    Progress {
        /// Human-readable progress message.
        message: String,
        /// Optional completion fraction between 0.0 and 1.0.
        fraction: Option<f64>,
        /// Event timestamp in Unix milliseconds.
        at: u64,
    },
}

/// Durable state of one logged call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogEntryState {
    /// The action is waiting for approval.
    Pending,
    /// The host may be executing the call.
    Executing,
    /// The result was applied and may be replayed.
    Applied,
    /// The action was rejected, compensated, or reset.
    Reverted,
}

/// One entry in the deterministic replay spine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Host-assigned call sequence.
    pub seq: u64,
    /// Connector namespace.
    pub connector: String,
    /// Connector method.
    pub method: String,
    /// Original arguments.
    pub arguments: Value,
    /// Recorded result for non-ephemeral applied calls.
    pub result: Option<Value>,
    /// Whether the call needed approval.
    pub requires_approval: bool,
    /// Whether replay re-executes the call.
    pub ephemeral: bool,
    /// Current durable state.
    pub state: LogEntryState,
}

/// Durable state for one sandbox program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionState {
    /// Stable execution identifier.
    pub id: String,
    /// Original sandbox program.
    pub code: String,
    /// Current lifecycle status.
    pub status: ExecutionStatus,
    /// Ordered deterministic call log.
    pub log: Vec<LogEntry>,
    /// Final successful result.
    pub result: Option<Value>,
    /// Terminal error.
    pub error: Option<String>,
    /// Captured sandbox console output.
    pub logs: Vec<String>,
    /// Connector names available when the run began.
    pub connectors: Vec<String>,
    /// Full immutable capability snapshot for deterministic resume.
    #[serde(default)]
    pub capabilities: Option<CapabilitySnapshot>,
    /// Ordered bounded lifecycle and streaming events.
    #[serde(default)]
    pub events: Vec<ExecutionEvent>,
    /// Creation time in Unix milliseconds.
    pub created_at: u64,
    /// Last state transition in Unix milliseconds.
    pub updated_at: u64,
}

/// A pending connector action shown to an approval UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingAction {
    /// Owning execution.
    pub execution_id: String,
    /// Sequence within the execution.
    pub seq: u64,
    /// Connector namespace.
    pub connector: String,
    /// Connector method.
    pub method: String,
    /// Arguments awaiting approval.
    pub arguments: Value,
}

/// Runtime decision for the next deterministic call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDecision {
    /// Return a recorded result without executing.
    Replay(Value),
    /// Execute the call and report its result at the supplied sequence.
    Execute(u64),
    /// Abort this sandbox pass.
    Pause(u64),
}

/// A saved, addressable sandbox program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    /// Unique snippet name.
    pub name: String,
    /// Human-readable search description.
    pub description: String,
    /// Async JavaScript function source.
    pub code: String,
    /// Save time in Unix milliseconds.
    pub saved_at: u64,
    /// Optional input JSON Schema.
    pub input_schema: Option<Value>,
    /// Connectors required by the saved program.
    pub connectors: Vec<String>,
}

/// Persistent operations required by the portable runtime.
#[async_trait]
pub trait RuntimeStore: Send + Sync {
    /// Loads one execution.
    async fn get_execution(&self, id: &str) -> Result<Option<ExecutionState>, String>;
    /// Replaces one execution.
    async fn put_execution(&self, execution: &ExecutionState) -> Result<(), String>;
    /// Lists every execution.
    async fn list_executions(&self) -> Result<Vec<ExecutionState>, String>;
    /// Deletes one execution and its log.
    async fn delete_execution(&self, id: &str) -> Result<(), String>;
    /// Loads one snippet.
    async fn get_snippet(&self, name: &str) -> Result<Option<Snippet>, String>;
    /// Replaces one snippet.
    async fn put_snippet(&self, snippet: &Snippet) -> Result<(), String>;
    /// Lists snippets.
    async fn list_snippets(&self) -> Result<Vec<Snippet>, String>;
    /// Deletes one snippet.
    async fn delete_snippet(&self, name: &str) -> Result<bool, String>;
}

#[derive(Default)]
struct MemoryState {
    executions: BTreeMap<String, ExecutionState>,
    snippets: BTreeMap<String, Snippet>,
}

/// In-memory runtime store for native use and deterministic tests.
#[derive(Default)]
pub struct MemoryStore {
    state: Mutex<MemoryState>,
}

#[async_trait]
impl RuntimeStore for MemoryStore {
    async fn get_execution(&self, id: &str) -> Result<Option<ExecutionState>, String> {
        Ok(self.state.lock().await.executions.get(id).cloned())
    }

    async fn put_execution(&self, execution: &ExecutionState) -> Result<(), String> {
        self.state
            .lock()
            .await
            .executions
            .insert(execution.id.clone(), execution.clone());
        Ok(())
    }

    async fn list_executions(&self) -> Result<Vec<ExecutionState>, String> {
        Ok(self
            .state
            .lock()
            .await
            .executions
            .values()
            .cloned()
            .collect())
    }

    async fn delete_execution(&self, id: &str) -> Result<(), String> {
        self.state.lock().await.executions.remove(id);
        Ok(())
    }

    async fn get_snippet(&self, name: &str) -> Result<Option<Snippet>, String> {
        Ok(self.state.lock().await.snippets.get(name).cloned())
    }

    async fn put_snippet(&self, snippet: &Snippet) -> Result<(), String> {
        self.state
            .lock()
            .await
            .snippets
            .insert(snippet.name.clone(), snippet.clone());
        Ok(())
    }

    async fn list_snippets(&self) -> Result<Vec<Snippet>, String> {
        Ok(self.state.lock().await.snippets.values().cloned().collect())
    }

    async fn delete_snippet(&self, name: &str) -> Result<bool, String> {
        Ok(self.state.lock().await.snippets.remove(name).is_some())
    }
}

/// Runtime state and persistence failures.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The requested execution does not exist.
    #[error("Execution \"{0}\" not found")]
    MissingExecution(String),
    /// The execution cannot make the requested transition.
    #[error("{0}")]
    InvalidState(String),
    /// A value cannot be recorded without corrupting replay.
    #[error("{0}")]
    DurableValue(String),
    /// The configured store failed.
    #[error("Runtime store failed: {0}")]
    Store(String),
}

/// Portable deterministic replay and approval state machine.
pub struct CodeModeRuntime {
    store: Arc<dyn RuntimeStore>,
    artifacts: Arc<dyn ArtifactStore>,
    gate: Mutex<()>,
}

impl CodeModeRuntime {
    /// Creates a runtime over a persistence implementation.
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self::with_artifacts(store, Arc::new(MemoryArtifactStore::default()))
    }

    /// Creates a runtime with an explicit oversized-value store.
    pub fn with_artifacts(store: Arc<dyn RuntimeStore>, artifacts: Arc<dyn ArtifactStore>) -> Self {
        Self {
            store,
            artifacts,
            gate: Mutex::new(()),
        }
    }

    /// Starts a fresh execution and prunes old terminal history.
    pub async fn begin(
        &self,
        code: impl Into<String>,
        connectors: Vec<String>,
        now: u64,
    ) -> Result<String, RuntimeError> {
        self.begin_inner(code.into(), connectors, None, now).await
    }

    /// Creates a durable execution with an immutable capability snapshot.
    pub async fn begin_with_capabilities(
        &self,
        code: &str,
        capabilities: CapabilitySnapshot,
        now: u64,
    ) -> Result<String, RuntimeError> {
        let connectors = capabilities
            .connectors
            .iter()
            .map(|connector| connector.name.clone())
            .collect();
        self.begin_inner(code.to_string(), connectors, Some(capabilities), now)
            .await
    }

    async fn begin_inner(
        &self,
        code: String,
        connectors: Vec<String>,
        capabilities: Option<CapabilitySnapshot>,
        now: u64,
    ) -> Result<String, RuntimeError> {
        let _guard = self.gate.lock().await;
        ensure_size("The execution code", &Value::String(code.clone()))?;
        let sequence = EXECUTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("exec_{now:016}_{sequence:016x}");
        self.store
            .put_execution(&ExecutionState {
                id: id.clone(),
                code,
                status: ExecutionStatus::Running,
                log: Vec::new(),
                result: None,
                error: None,
                logs: Vec::new(),
                connectors,
                capabilities,
                events: vec![ExecutionEvent::Status {
                    status: ExecutionStatus::Running,
                    at: now,
                }],
                created_at: now,
                updated_at: now,
            })
            .await
            .map_err(RuntimeError::Store)?;
        self.prune_locked(DEFAULT_MAX_EXECUTIONS, Some(&id)).await?;
        Ok(id)
    }

    /// Moves a paused execution back to running for a replay pass.
    pub async fn resume(&self, id: &str, now: u64) -> Result<ExecutionState, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        if execution.status != ExecutionStatus::Paused {
            return Err(RuntimeError::InvalidState(format!(
                "Execution \"{id}\" is {:?}, not paused",
                execution.status
            )));
        }
        execution.status = ExecutionStatus::Running;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::Running,
                at: now,
            },
        );
        execution.updated_at = now;
        self.save(&execution).await?;
        Ok(execution)
    }

    /// Cancels a running or paused execution.
    pub async fn cancel(&self, id: &str, now: u64) -> Result<bool, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        if execution.status.terminal() {
            return Ok(false);
        }
        execution.status = ExecutionStatus::Cancelled;
        execution.error = Some("Execution cancelled".to_string());
        execution.updated_at = now;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::Cancelled,
                at: now,
            },
        );
        self.save(&execution).await?;
        Ok(true)
    }

    /// Appends one bounded execution event.
    pub async fn event(&self, id: &str, event: ExecutionEvent) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        push_event(&mut execution, event);
        self.save(&execution).await
    }

    /// Loads an artifact value by owning execution and artifact identifiers.
    pub async fn artifact(
        &self,
        execution_id: &str,
        artifact_id: &str,
    ) -> Result<Option<Value>, RuntimeError> {
        self.artifacts
            .get(execution_id, artifact_id)
            .await
            .map_err(RuntimeError::Store)
    }

    async fn spill(&self, execution_id: &str, value: Value) -> Result<Value, RuntimeError> {
        if serialized_size(&value) <= MAX_DURABLE_VALUE_BYTES {
            return Ok(value);
        }
        let artifact = self
            .artifacts
            .put(execution_id, &value)
            .await
            .map_err(RuntimeError::Store)?;
        Ok(serde_json::json!({ "$artifact": artifact }))
    }

    async fn rehydrate(&self, execution_id: &str, value: Value) -> Result<Value, RuntimeError> {
        let Some(artifact) = artifact_ref(&value)? else {
            return Ok(value);
        };
        self.artifacts
            .get(execution_id, &artifact.id)
            .await
            .map_err(RuntimeError::Store)?
            .ok_or_else(|| {
                RuntimeError::Store(format!("Artifact \"{}\" is unavailable", artifact.id))
            })
    }

    async fn rehydrate_execution(
        &self,
        mut execution: ExecutionState,
    ) -> Result<ExecutionState, RuntimeError> {
        for entry in &mut execution.log {
            entry.arguments = self
                .rehydrate(&execution.id, entry.arguments.clone())
                .await?;
            if let Some(result) = entry.result.take() {
                entry.result = Some(self.rehydrate(&execution.id, result).await?);
            }
        }
        if let Some(result) = execution.result.take() {
            execution.result = Some(self.rehydrate(&execution.id, result).await?);
        }
        Ok(execution)
    }

    async fn rehydrate_entry(
        &self,
        execution_id: &str,
        mut entry: LogEntry,
    ) -> Result<LogEntry, RuntimeError> {
        entry.arguments = self.rehydrate(execution_id, entry.arguments).await?;
        if let Some(result) = entry.result.take() {
            entry.result = Some(self.rehydrate(execution_id, result).await?);
        }
        Ok(entry)
    }

    async fn rehydrate_pending(
        &self,
        execution_id: &str,
        mut action: PendingAction,
    ) -> Result<PendingAction, RuntimeError> {
        action.arguments = self.rehydrate(execution_id, action.arguments).await?;
        Ok(action)
    }

    /// Decides whether the next call replays, executes, or pauses.
    #[allow(clippy::too_many_arguments)]
    pub async fn decide(
        &self,
        id: &str,
        seq: u64,
        connector: &str,
        method: &str,
        arguments: Value,
        requires_approval: bool,
        ephemeral: bool,
        now: u64,
    ) -> Result<ToolDecision, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        if execution.status != ExecutionStatus::Running {
            return Ok(ToolDecision::Pause(seq));
        }
        if requires_approval && ephemeral {
            return Err(RuntimeError::InvalidState(format!(
                "{connector}.{method} cannot require approval and re-execute on replay"
            )));
        }
        let arguments = self.spill(id, arguments).await?;
        if let Some(entry) = execution.log.iter_mut().find(|entry| entry.seq == seq) {
            if entry.connector != connector
                || entry.method != method
                || stable_json(&entry.arguments) != stable_json(&arguments)
            {
                execution.status = ExecutionStatus::Error;
                execution.error = Some(format!(
                    "Replay diverged at step {seq}: expected {}.{}, got {connector}.{method}. \
                     Code must be deterministic up to tool calls and steps.",
                    entry.connector, entry.method
                ));
                execution.updated_at = now;
                self.save(&execution).await?;
                return Ok(ToolDecision::Pause(seq));
            }
            return match entry.state {
                LogEntryState::Applied if !entry.ephemeral => Ok(ToolDecision::Replay(
                    self.rehydrate(id, entry.result.clone().unwrap_or(Value::Null))
                        .await?,
                )),
                LogEntryState::Pending | LogEntryState::Executing | LogEntryState::Applied => {
                    entry.state = LogEntryState::Executing;
                    execution.updated_at = now;
                    self.save(&execution).await?;
                    Ok(ToolDecision::Execute(seq))
                }
                LogEntryState::Reverted => {
                    entry.state = if requires_approval {
                        LogEntryState::Pending
                    } else {
                        LogEntryState::Executing
                    };
                    entry.arguments = arguments;
                    entry.result = None;
                    entry.requires_approval = requires_approval;
                    entry.ephemeral = ephemeral;
                    if requires_approval {
                        execution.status = ExecutionStatus::Paused;
                        push_event(
                            &mut execution,
                            ExecutionEvent::Status {
                                status: ExecutionStatus::Paused,
                                at: now,
                            },
                        );
                    }
                    execution.updated_at = now;
                    self.save(&execution).await?;
                    Ok(if requires_approval {
                        ToolDecision::Pause(seq)
                    } else {
                        ToolDecision::Execute(seq)
                    })
                }
            };
        }
        execution.log.push(LogEntry {
            seq,
            connector: connector.to_string(),
            method: method.to_string(),
            arguments,
            result: None,
            requires_approval,
            ephemeral,
            state: if requires_approval {
                LogEntryState::Pending
            } else {
                LogEntryState::Executing
            },
        });
        if requires_approval {
            execution.status = ExecutionStatus::Paused;
            push_event(
                &mut execution,
                ExecutionEvent::Status {
                    status: ExecutionStatus::Paused,
                    at: now,
                },
            );
        }
        execution.updated_at = now;
        self.save(&execution).await?;
        Ok(if requires_approval {
            ToolDecision::Pause(seq)
        } else {
            ToolDecision::Execute(seq)
        })
    }

    /// Records a completed connector call for deterministic replay.
    pub async fn record_result(
        &self,
        id: &str,
        seq: u64,
        result: Value,
        now: u64,
    ) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        let result = self.spill(id, result).await?;
        if execution.status != ExecutionStatus::Running {
            return Err(RuntimeError::InvalidState(format!(
                "Execution \"{id}\" is {:?}, not running",
                execution.status
            )));
        }
        let Some(entry) = execution.log.iter_mut().find(|entry| entry.seq == seq) else {
            return Err(RuntimeError::InvalidState(format!(
                "No log entry at step {seq}"
            )));
        };
        if entry.state != LogEntryState::Executing {
            return Err(RuntimeError::InvalidState(format!(
                "Log entry at step {seq} is {:?}, not executing",
                entry.state
            )));
        }
        if !entry.ephemeral {
            entry.result = Some(result);
        }
        entry.state = LogEntryState::Applied;
        execution.updated_at = now;
        self.save(&execution).await
    }

    /// Marks an execution completed and records bounded audit output.
    pub async fn complete(
        &self,
        id: &str,
        result: Value,
        logs: Vec<String>,
        now: u64,
    ) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        let result = self.spill(id, result).await?;
        execution.status = ExecutionStatus::Completed;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::Completed,
                at: now,
            },
        );
        execution.result = Some(result);
        execution.logs = bounded_logs(logs);
        execution.updated_at = now;
        self.save(&execution).await
    }

    /// Marks an execution failed.
    pub async fn fail(
        &self,
        id: &str,
        error: impl Into<String>,
        logs: Vec<String>,
        now: u64,
    ) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        execution.status = ExecutionStatus::Error;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::Error,
                at: now,
            },
        );
        execution.error = Some(truncate_bytes(error.into(), MAX_DURABLE_VALUE_BYTES));
        execution.logs = bounded_logs(logs);
        execution.updated_at = now;
        self.save(&execution).await
    }

    /// Approves a pending action without racing a stale second approval.
    pub async fn approve(&self, id: &str, seq: u64, now: u64) -> Result<bool, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        if execution.status != ExecutionStatus::Paused {
            return Ok(false);
        }
        let Some(entry) = execution
            .log
            .iter()
            .find(|entry| entry.seq == seq && entry.state == LogEntryState::Pending)
        else {
            return Ok(false);
        };
        let _ = entry;
        execution.status = ExecutionStatus::Running;
        execution.updated_at = now;
        self.save(&execution).await?;
        Ok(true)
    }

    /// Rejects a pending action and terminates its execution.
    pub async fn reject(&self, id: &str, seq: u64, now: u64) -> Result<bool, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        let Some(entry) = execution
            .log
            .iter_mut()
            .find(|entry| entry.seq == seq && entry.state == LogEntryState::Pending)
        else {
            return Ok(false);
        };
        entry.state = LogEntryState::Reverted;
        let error = format!(
            "Action {}.{} rejected by user",
            entry.connector, entry.method
        );
        execution.status = ExecutionStatus::Rejected;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::Rejected,
                at: now,
            },
        );
        execution.error = Some(error);
        execution.updated_at = now;
        self.save(&execution).await?;
        Ok(true)
    }

    /// Lists actionable approvals, optionally scoped to one execution.
    pub async fn pending(&self, id: Option<&str>) -> Result<Vec<PendingAction>, RuntimeError> {
        let executions = if let Some(id) = id {
            vec![self.require(id).await?]
        } else {
            self.store
                .list_executions()
                .await
                .map_err(RuntimeError::Store)?
        };
        let mut result = Vec::new();
        for execution in executions
            .into_iter()
            .filter(|execution| execution.status == ExecutionStatus::Paused)
        {
            for entry in execution
                .log
                .into_iter()
                .filter(|entry| entry.state == LogEntryState::Pending)
            {
                let action = PendingAction {
                    execution_id: execution.id.clone(),
                    seq: entry.seq,
                    connector: entry.connector,
                    method: entry.method,
                    arguments: entry.arguments,
                };
                result.push(self.rehydrate_pending(&execution.id, action).await?);
            }
        }
        result.sort_by(|left, right| {
            right
                .execution_id
                .cmp(&left.execution_id)
                .then_with(|| left.seq.cmp(&right.seq))
        });
        Ok(result)
    }

    /// Returns applied connector actions in reverse order for compensation.
    pub async fn actions_to_revert(&self, id: &str) -> Result<Vec<LogEntry>, RuntimeError> {
        let execution = self.require(id).await?;
        let mut actions = Vec::new();
        for entry in execution.log.into_iter().filter(|entry| {
            entry.state == LogEntryState::Applied && !entry.ephemeral && entry.connector != "__step"
        }) {
            actions.push(self.rehydrate_entry(&execution.id, entry).await?);
        }
        actions.sort_by_key(|entry| std::cmp::Reverse(entry.seq));
        Ok(actions)
    }

    /// Marks one compensated action reverted.
    pub async fn mark_reverted(&self, id: &str, seq: u64, now: u64) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        if let Some(entry) = execution.log.iter_mut().find(|entry| entry.seq == seq) {
            entry.state = LogEntryState::Reverted;
            execution.updated_at = now;
            self.save(&execution).await?;
        }
        Ok(())
    }

    /// Marks an execution rolled back after compensation completes.
    pub async fn finish_rollback(&self, id: &str, now: u64) -> Result<(), RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut execution = self.require(id).await?;
        execution.status = ExecutionStatus::RolledBack;
        push_event(
            &mut execution,
            ExecutionEvent::Status {
                status: ExecutionStatus::RolledBack,
                at: now,
            },
        );
        execution.updated_at = now;
        self.save(&execution).await
    }

    /// Expires abandoned paused and running executions.
    pub async fn expire(&self, now: u64, max_age_ms: u64) -> Result<Vec<String>, RuntimeError> {
        let _guard = self.gate.lock().await;
        let mut expired = Vec::new();
        for mut execution in self
            .store
            .list_executions()
            .await
            .map_err(RuntimeError::Store)?
        {
            if !matches!(
                execution.status,
                ExecutionStatus::Paused | ExecutionStatus::Running
            ) || now.saturating_sub(execution.updated_at) <= max_age_ms
            {
                continue;
            }
            execution.status = if execution.status == ExecutionStatus::Paused {
                ExecutionStatus::Rejected
            } else {
                ExecutionStatus::Error
            };
            execution.error = Some(if execution.status == ExecutionStatus::Rejected {
                "Expired awaiting approval".to_string()
            } else {
                "Expired while running - the host never completed the pass".to_string()
            });
            for entry in &mut execution.log {
                if entry.state == LogEntryState::Pending {
                    entry.state = LogEntryState::Reverted;
                }
            }
            execution.updated_at = now;
            self.save(&execution).await?;
            expired.push(execution.id);
        }
        Ok(expired)
    }

    /// Loads one execution.
    pub async fn execution(&self, id: &str) -> Result<Option<ExecutionState>, RuntimeError> {
        let Some(execution) = self
            .store
            .get_execution(id)
            .await
            .map_err(RuntimeError::Store)?
        else {
            return Ok(None);
        };
        self.rehydrate_execution(execution).await.map(Some)
    }

    /// Loads one bounded execution snapshot with artifact references intact.
    pub async fn execution_snapshot(
        &self,
        id: &str,
    ) -> Result<Option<ExecutionState>, RuntimeError> {
        self.store
            .get_execution(id)
            .await
            .map_err(RuntimeError::Store)
    }

    /// Lists executions newest first.
    pub async fn executions(&self) -> Result<Vec<ExecutionState>, RuntimeError> {
        let mut executions = self
            .store
            .list_executions()
            .await
            .map_err(RuntimeError::Store)?;
        executions.sort_by_key(|execution| {
            (
                std::cmp::Reverse(execution.created_at),
                std::cmp::Reverse(execution.id.clone()),
            )
        });
        Ok(executions)
    }

    /// Prunes terminal history while retaining every live execution.
    pub async fn prune(&self, keep: usize) -> Result<usize, RuntimeError> {
        let _guard = self.gate.lock().await;
        self.prune_locked(keep, None).await
    }

    /// Saves a working execution as a reusable snippet.
    pub async fn save_snippet(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        execution_id: &str,
        input_schema: Option<Value>,
        now: u64,
    ) -> Result<Snippet, RuntimeError> {
        let execution = self.require(execution_id).await?;
        if execution.status != ExecutionStatus::Completed {
            return Err(RuntimeError::InvalidState(
                "Only completed executions can be saved as snippets".to_string(),
            ));
        }
        let snippet = Snippet {
            name: name.into(),
            description: description.into(),
            code: execution.code,
            saved_at: now,
            input_schema,
            connectors: execution.connectors,
        };
        self.store
            .put_snippet(&snippet)
            .await
            .map_err(RuntimeError::Store)?;
        Ok(snippet)
    }

    /// Lists snippets in stable name order.
    pub async fn snippets(&self) -> Result<Vec<Snippet>, RuntimeError> {
        let mut snippets = self
            .store
            .list_snippets()
            .await
            .map_err(RuntimeError::Store)?;
        snippets.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(snippets)
    }

    /// Deletes one snippet.
    pub async fn delete_snippet(&self, name: &str) -> Result<bool, RuntimeError> {
        self.store
            .delete_snippet(name)
            .await
            .map_err(RuntimeError::Store)
    }

    async fn require(&self, id: &str) -> Result<ExecutionState, RuntimeError> {
        self.store
            .get_execution(id)
            .await
            .map_err(RuntimeError::Store)?
            .ok_or_else(|| RuntimeError::MissingExecution(id.to_string()))
    }

    async fn save(&self, execution: &ExecutionState) -> Result<(), RuntimeError> {
        self.store
            .put_execution(execution)
            .await
            .map_err(RuntimeError::Store)
    }

    async fn prune_locked(
        &self,
        keep: usize,
        excluding: Option<&str>,
    ) -> Result<usize, RuntimeError> {
        let mut terminal = self
            .store
            .list_executions()
            .await
            .map_err(RuntimeError::Store)?
            .into_iter()
            .filter(|execution| {
                execution.status.terminal()
                    && excluding.is_none_or(|id| execution.id.as_str() != id)
            })
            .collect::<Vec<_>>();
        terminal.sort_by_key(|execution| std::cmp::Reverse(execution.created_at));
        let remove = terminal.len().saturating_sub(keep);
        for execution in terminal.into_iter().rev().take(remove) {
            self.store
                .delete_execution(&execution.id)
                .await
                .map_err(RuntimeError::Store)?;
            self.artifacts
                .delete_execution(&execution.id)
                .await
                .map_err(RuntimeError::Store)?;
        }
        Ok(remove)
    }
}

fn ensure_size(what: &str, value: &Value) -> Result<(), RuntimeError> {
    let size = serialized_size(value);
    if size > MAX_DURABLE_VALUE_BYTES {
        Err(RuntimeError::DurableValue(format!(
            "{what} is too large to record durably ({size} bytes > \
             {MAX_DURABLE_VALUE_BYTES} byte limit). Write large data to a file or workspace \
             instead and pass or return a small reference."
        )))
    } else {
        Ok(())
    }
}

fn serialized_size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |value| value.len())
}

fn stable_json(value: &Value) -> String {
    match value {
        Value::Object(values) => {
            let mut values = values.iter().collect::<Vec<_>>();
            values.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                values
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        stable_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(stable_json).collect::<Vec<_>>().join(",")
        ),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn bounded_logs(logs: Vec<String>) -> Vec<String> {
    let mut size = 2;
    logs.into_iter()
        .take_while(|log| {
            size += log.len() + 3;
            size <= MAX_DURABLE_VALUE_BYTES
        })
        .collect()
}

fn push_event(execution: &mut ExecutionState, event: ExecutionEvent) {
    execution.events.push(event);
    if execution.events.len() > DEFAULT_MAX_EVENTS {
        execution
            .events
            .drain(..execution.events.len() - DEFAULT_MAX_EVENTS);
    }
}

fn artifact_ref(value: &Value) -> Result<Option<ArtifactRef>, RuntimeError> {
    let Value::Object(values) = value else {
        return Ok(None);
    };
    if values.len() != 1 {
        return Ok(None);
    }
    let Some(artifact) = values.get("$artifact") else {
        return Ok(None);
    };
    serde_json::from_value::<ArtifactRef>(artifact.clone())
        .map(Some)
        .map_err(|error| RuntimeError::Store(error.to_string()))
}

fn truncate_bytes(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn runtime() -> CodeModeRuntime {
        CodeModeRuntime::new(Arc::new(MemoryStore::default()))
    }

    fn runtime_parts() -> (CodeModeRuntime, Arc<MemoryStore>, Arc<MemoryArtifactStore>) {
        let store = Arc::new(MemoryStore::default());
        let artifacts = Arc::new(MemoryArtifactStore::default());
        (
            CodeModeRuntime::with_artifacts(store.clone(), artifacts.clone()),
            store,
            artifacts,
        )
    }

    fn artifact_id(value: &Value) -> String {
        serde_json::from_value::<ArtifactRef>(value["$artifact"].clone())
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn pauses_approves_and_replays_without_reapplying() {
        let runtime = runtime();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        assert_eq!(
            runtime
                .decide(&id, 0, "db", "write", json!({"id": 1}), true, false, 2)
                .await
                .unwrap(),
            ToolDecision::Pause(0)
        );
        assert!(runtime.approve(&id, 0, 3).await.unwrap());
        assert_eq!(
            runtime
                .decide(&id, 0, "db", "write", json!({"id": 1}), true, false, 4)
                .await
                .unwrap(),
            ToolDecision::Execute(0)
        );
        runtime
            .record_result(&id, 0, json!({"ok": true}), 5)
            .await
            .unwrap();
        assert_eq!(
            runtime
                .decide(&id, 0, "db", "write", json!({"id": 1}), true, false, 6)
                .await
                .unwrap(),
            ToolDecision::Replay(json!({"ok": true}))
        );
    }

    #[tokio::test]
    async fn rejects_results_for_pending_approval_entries() {
        let runtime = runtime();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        runtime
            .decide(&id, 0, "db", "write", json!({"id": 1}), true, false, 2)
            .await
            .unwrap();

        let error = runtime
            .record_result(&id, 0, json!({"forged": true}), 3)
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::InvalidState(_)));
        let execution = runtime.execution(&id).await.unwrap().unwrap();
        assert_eq!(execution.status, ExecutionStatus::Paused);
        assert_eq!(execution.log[0].state, LogEntryState::Pending);
    }

    #[tokio::test]
    async fn rollback_targets_only_logged_connector_actions() {
        let runtime = runtime();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        runtime
            .decide(&id, 0, "db", "read", json!({}), false, true, 2)
            .await
            .unwrap();
        runtime.record_result(&id, 0, json!(1), 3).await.unwrap();
        runtime
            .decide(&id, 1, "db", "write", json!({}), false, false, 4)
            .await
            .unwrap();
        runtime.record_result(&id, 1, json!(2), 5).await.unwrap();

        let actions = runtime.actions_to_revert(&id).await.unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].seq, 1);
    }

    #[tokio::test]
    async fn divergence_is_terminal() {
        let runtime = runtime();
        let id = runtime.begin("async () => {}", vec![], 1).await.unwrap();
        runtime
            .decide(&id, 0, "db", "read", json!({"id": 1}), false, false, 2)
            .await
            .unwrap();
        runtime.record_result(&id, 0, json!(1), 3).await.unwrap();
        assert_eq!(
            runtime
                .decide(&id, 0, "db", "read", json!({"id": 2}), false, false, 4)
                .await
                .unwrap(),
            ToolDecision::Pause(0)
        );
        assert_eq!(
            runtime.execution(&id).await.unwrap().unwrap().status,
            ExecutionStatus::Error
        );
    }

    #[tokio::test]
    async fn spills_and_rehydrates_oversized_replay_values() {
        let (runtime, store, _) = runtime_parts();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        runtime
            .decide(&id, 0, "db", "read", json!({}), false, false, 2)
            .await
            .unwrap();
        let value = json!("x".repeat(MAX_DURABLE_VALUE_BYTES + 1));
        runtime
            .record_result(&id, 0, value.clone(), 3)
            .await
            .unwrap();
        let raw = store.get_execution(&id).await.unwrap().unwrap();
        assert!(raw.log[0].result.as_ref().unwrap()["$artifact"].is_object());
        let execution = runtime.execution(&id).await.unwrap().unwrap();
        assert_eq!(execution.log[0].result, Some(value.clone()));
        assert_eq!(
            runtime
                .decide(&id, 0, "db", "read", json!({}), false, false, 4)
                .await
                .unwrap(),
            ToolDecision::Replay(value)
        );
    }

    #[tokio::test]
    async fn spills_and_rehydrates_oversized_arguments() {
        let (runtime, store, _) = runtime_parts();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        let arguments = json!({"payload": "x".repeat(MAX_DURABLE_VALUE_BYTES + 1)});

        assert_eq!(
            runtime
                .decide(&id, 0, "db", "write", arguments.clone(), false, false, 2)
                .await
                .unwrap(),
            ToolDecision::Execute(0)
        );

        let raw = store.get_execution(&id).await.unwrap().unwrap();
        assert!(raw.log[0].arguments["$artifact"].is_object());
        let execution = runtime.execution(&id).await.unwrap().unwrap();
        assert_eq!(execution.log[0].arguments, arguments);
    }

    #[tokio::test]
    async fn pending_actions_rehydrate_oversized_arguments() {
        let runtime = runtime();
        let id = runtime
            .begin("async () => {}", vec!["db".into()], 1)
            .await
            .unwrap();
        let arguments = json!({"payload": "x".repeat(MAX_DURABLE_VALUE_BYTES + 1)});

        assert_eq!(
            runtime
                .decide(&id, 0, "db", "write", arguments.clone(), true, false, 2)
                .await
                .unwrap(),
            ToolDecision::Pause(0)
        );

        let pending = runtime.pending(Some(&id)).await.unwrap();
        assert_eq!(pending[0].arguments, arguments);
    }

    #[tokio::test]
    async fn retrieves_oversized_final_result_by_execution_and_artifact_ids() {
        let (runtime, store, _) = runtime_parts();
        let id = runtime.begin("async () => {}", vec![], 1).await.unwrap();
        let value = json!("x".repeat(MAX_DURABLE_VALUE_BYTES + 1));

        runtime
            .complete(&id, value.clone(), Vec::new(), 2)
            .await
            .unwrap();

        let raw = store.get_execution(&id).await.unwrap().unwrap();
        let artifact_id = artifact_id(raw.result.as_ref().unwrap());
        assert_eq!(
            runtime.artifact(&id, &artifact_id).await.unwrap(),
            Some(value.clone())
        );
        assert_eq!(
            runtime.execution(&id).await.unwrap().unwrap().result,
            Some(value)
        );
    }

    #[tokio::test]
    async fn denies_artifact_lookup_for_wrong_execution_owner() {
        let (runtime, store, _) = runtime_parts();
        let owner = runtime.begin("async () => {}", vec![], 1).await.unwrap();
        let other = runtime.begin("async () => {}", vec![], 2).await.unwrap();
        let value = json!("x".repeat(MAX_DURABLE_VALUE_BYTES + 1));
        runtime
            .complete(&owner, value, Vec::new(), 3)
            .await
            .unwrap();
        let raw = store.get_execution(&owner).await.unwrap().unwrap();
        let artifact_id = artifact_id(raw.result.as_ref().unwrap());

        assert_eq!(runtime.artifact(&other, &artifact_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn missing_artifact_fails_rehydrated_execution_lookup() {
        let (runtime, _, artifacts) = runtime_parts();
        let id = runtime.begin("async () => {}", vec![], 1).await.unwrap();
        let value = json!("x".repeat(MAX_DURABLE_VALUE_BYTES + 1));
        runtime.complete(&id, value, Vec::new(), 2).await.unwrap();

        artifacts.delete_execution(&id).await.unwrap();
        let error = runtime.execution(&id).await.unwrap_err();

        assert!(matches!(error, RuntimeError::Store(message) if message.contains("unavailable")));
    }

    #[tokio::test]
    async fn prune_deletes_artifacts_for_removed_executions() {
        let (runtime, store, _) = runtime_parts();
        let id = runtime.begin("async () => {}", vec![], 1).await.unwrap();
        let value = json!("x".repeat(MAX_DURABLE_VALUE_BYTES + 1));
        runtime
            .complete(&id, value.clone(), Vec::new(), 2)
            .await
            .unwrap();
        let raw = store.get_execution(&id).await.unwrap().unwrap();
        let artifact_id = artifact_id(raw.result.as_ref().unwrap());
        assert_eq!(
            runtime.artifact(&id, &artifact_id).await.unwrap(),
            Some(value)
        );

        assert_eq!(runtime.prune(0).await.unwrap(), 1);

        assert_eq!(runtime.artifact(&id, &artifact_id).await.unwrap(), None);
        assert!(runtime.execution(&id).await.unwrap().is_none());
    }
}
