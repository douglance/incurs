use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use incurs::command::RequestContext;
use incurs::tool::{ToolCallControl, ToolEvent, ToolEventSink};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    ArtifactStore, CapabilitySnapshot, Clock, CodeExecutor, CodeModeRuntime, Connector,
    DispatchRequest, DispatchSession, ExecutionEvent, ExecutionHost, ExecutionState,
    ExecutionStatus, RuntimeStore, SearchOutput, SystemClock, ToolContext,
};

/// Transport context inherited by one Code Mode execution pass.
#[derive(Clone, Default)]
pub struct CodeModeRunOptions {
    /// Cooperative cancellation signal for the active pass.
    pub cancellation: CancellationToken,
    /// Request metadata inherited by connector calls.
    pub request: Option<RequestContext>,
}

struct RuntimeEventSink {
    runtime: Arc<CodeModeRuntime>,
    execution_id: String,
    clock: Arc<dyn Clock>,
}

#[async_trait]
impl ToolEventSink for RuntimeEventSink {
    async fn emit(&self, event: ToolEvent) {
        let at = self.clock.now_ms();
        let event = match event {
            ToolEvent::Progress { message, fraction } => ExecutionEvent::Progress {
                message,
                fraction,
                at,
            },
            ToolEvent::Log { level, message } => ExecutionEvent::Log { level, message, at },
            ToolEvent::Chunk { data } => ExecutionEvent::Chunk { data, at },
        };
        let _ = self.runtime.event(&self.execution_id, event).await;
    }
}

/// Generic Code Mode lifecycle over interchangeable execution and storage adapters.
pub struct CodeMode {
    runtime: Arc<CodeModeRuntime>,
    executor: Box<dyn CodeExecutor>,
    connectors: Vec<Arc<dyn Connector>>,
    clock: Arc<dyn Clock>,
    contexts: Mutex<HashMap<String, ToolContext>>,
}

impl CodeMode {
    /// Creates a native Code Mode runtime using the system clock.
    pub fn new(
        store: Arc<dyn RuntimeStore>,
        executor: impl CodeExecutor + 'static,
        connectors: Vec<Arc<dyn Connector>>,
    ) -> Self {
        Self::with_clock(store, executor, connectors, SystemClock)
    }

    /// Creates Code Mode with an explicit oversized-value artifact store.
    pub fn with_artifact_store(
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        executor: impl CodeExecutor + 'static,
        connectors: Vec<Arc<dyn Connector>>,
    ) -> Self {
        Self {
            runtime: Arc::new(CodeModeRuntime::with_artifacts(store, artifacts)),
            executor: Box::new(executor),
            connectors,
            clock: Arc::new(SystemClock),
            contexts: Mutex::new(HashMap::new()),
        }
    }

    /// Creates Code Mode with an explicit platform clock.
    pub fn with_clock(
        store: Arc<dyn RuntimeStore>,
        executor: impl CodeExecutor + 'static,
        connectors: Vec<Arc<dyn Connector>>,
        clock: impl Clock + 'static,
    ) -> Self {
        Self {
            runtime: Arc::new(CodeModeRuntime::new(store)),
            executor: Box::new(executor),
            connectors,
            clock: Arc::new(clock),
            contexts: Mutex::new(HashMap::new()),
        }
    }

    /// Creates Code Mode with explicit platform clock and artifact storage.
    pub fn with_clock_and_artifact_store(
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        executor: impl CodeExecutor + 'static,
        connectors: Vec<Arc<dyn Connector>>,
        clock: impl Clock + 'static,
    ) -> Self {
        Self {
            runtime: Arc::new(CodeModeRuntime::with_artifacts(store, artifacts)),
            executor: Box::new(executor),
            connectors,
            clock: Arc::new(clock),
            contexts: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the portable runtime for approvals, history, and snippets.
    pub fn runtime(&self) -> Arc<CodeModeRuntime> {
        Arc::clone(&self.runtime)
    }

    /// Returns model-facing JavaScript declarations and connector guidance.
    pub async fn instructions(&self) -> Result<String, String> {
        let mut descriptions = Vec::new();
        for connector in &self.connectors {
            descriptions.push(connector.describe().await?);
        }
        let mut sections = descriptions
            .iter()
            .filter_map(|connector| {
                connector
                    .instructions
                    .as_ref()
                    .map(|instructions| format!("## {}\n\n{instructions}", connector.name))
            })
            .collect::<Vec<_>>();
        sections.extend(descriptions.iter().map(crate::generate_types));
        Ok(sections.join("\n\n"))
    }

    /// Searches current connector methods and saved snippets.
    pub async fn search(&self, query: &str) -> Result<SearchOutput, String> {
        let mut descriptions = Vec::new();
        for connector in &self.connectors {
            descriptions.push(connector.describe().await?);
        }
        let snippets = self
            .runtime
            .snippets()
            .await
            .map_err(|error| error.to_string())?;
        Ok(crate::search(query, &descriptions, &snippets))
    }

    /// Returns one execution by identifier.
    pub async fn execution(&self, execution_id: &str) -> Result<ExecutionState, String> {
        self.require(execution_id).await
    }

    /// Returns one bounded execution snapshot with artifact references intact.
    pub async fn execution_snapshot(&self, execution_id: &str) -> Result<ExecutionState, String> {
        self.runtime
            .execution_snapshot(execution_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Execution \"{execution_id}\" not found"))
    }

    /// Returns one oversized value owned by an execution.
    pub async fn artifact(&self, execution_id: &str, artifact_id: &str) -> Result<Value, String> {
        self.runtime
            .artifact(execution_id, artifact_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!("Artifact \"{artifact_id}\" not found for execution \"{execution_id}\"")
            })
    }

    /// Returns ordered retained events for one execution.
    pub async fn events(&self, execution_id: &str) -> Result<Vec<ExecutionEvent>, String> {
        Ok(self.require(execution_id).await?.events)
    }

    /// Cancels a running or paused execution.
    pub async fn cancel(&self, execution_id: &str) -> Result<ExecutionState, String> {
        if let Some(context) = self.contexts.lock().await.get(execution_id) {
            context.control.cancellation.cancel();
        }
        self.runtime
            .cancel(execution_id, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?;
        self.require(execution_id).await
    }

    /// Creates a durable running execution without driving its first pass.
    pub async fn start(&self, code: &str) -> Result<ExecutionState, String> {
        let mut descriptions = Vec::new();
        for connector in &self.connectors {
            descriptions.push(connector.describe().await?);
        }
        let capabilities = CapabilitySnapshot::new(descriptions)?;
        let id = self
            .runtime
            .begin_with_capabilities(code, capabilities, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?;
        self.require(&id).await
    }

    /// Starts and drives a new execution until it completes, fails, or pauses.
    pub async fn execute(&self, code: &str) -> Result<ExecutionState, String> {
        self.execute_with(code, CodeModeRunOptions::default()).await
    }

    /// Starts and drives a new execution with transport request context.
    pub async fn execute_with(
        &self,
        code: &str,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        let state = self.start(code).await?;
        self.drive_with(&state.id, options).await
    }

    /// Resumes a paused execution and drives the next replay pass.
    pub async fn resume(&self, execution_id: &str) -> Result<ExecutionState, String> {
        self.resume_with(execution_id, CodeModeRunOptions::default())
            .await
    }

    /// Resumes a paused execution with transport request context.
    pub async fn resume_with(
        &self,
        execution_id: &str,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        self.runtime
            .resume(execution_id, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?;
        self.drive_with(execution_id, options).await
    }

    /// Approves a pending action and resumes only if this call won the race.
    pub async fn approve(&self, execution_id: &str, seq: u64) -> Result<ExecutionState, String> {
        self.approve_with(execution_id, seq, CodeModeRunOptions::default())
            .await
    }

    /// Approves a pending action and resumes with transport request context.
    pub async fn approve_with(
        &self,
        execution_id: &str,
        seq: u64,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        if !self
            .runtime
            .approve(execution_id, seq, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?
        {
            return self.require(execution_id).await;
        }
        self.drive_with(execution_id, options).await
    }

    /// Rejects a pending action and notifies connector lifecycle hooks.
    pub async fn reject(&self, execution_id: &str, seq: u64) -> Result<ExecutionState, String> {
        if self
            .runtime
            .reject(execution_id, seq, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?
        {
            self.notify_execution_end(execution_id, "rejected").await;
        }
        self.require(execution_id).await
    }

    /// Compensates applied connector actions in reverse order.
    pub async fn rollback(&self, execution_id: &str) -> Result<ExecutionState, String> {
        for action in self
            .runtime
            .actions_to_revert(execution_id)
            .await
            .map_err(|error| error.to_string())?
        {
            let connector = self
                .connector(&action.connector)
                .await?
                .ok_or_else(|| format!("Connector \"{}\" not found", action.connector))?;
            if !connector
                .revert(
                    &action.method,
                    action.arguments,
                    action.result.unwrap_or(Value::Null),
                    &ToolContext {
                        execution_id: execution_id.to_string(),
                        control: Default::default(),
                        request: None,
                    },
                )
                .await?
            {
                return Err(format!(
                    "{}.{} did not compensate step {}",
                    action.connector, action.method, action.seq
                ));
            }
            self.runtime
                .mark_reverted(execution_id, action.seq, self.clock.now_ms())
                .await
                .map_err(|error| error.to_string())?;
        }
        self.runtime
            .finish_rollback(execution_id, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())?;
        self.notify_execution_end(execution_id, "rolled_back").await;
        self.require(execution_id).await
    }

    /// Expires stale live executions and releases connector resources.
    pub async fn expire(&self, max_age_ms: u64) -> Result<Vec<String>, String> {
        let ids = self
            .runtime
            .expire(self.clock.now_ms(), max_age_ms)
            .await
            .map_err(|error| error.to_string())?;
        for id in &ids {
            let status = self.require(id).await?.status;
            self.notify_execution_end(
                id,
                if status == ExecutionStatus::Rejected {
                    "rejected"
                } else {
                    "error"
                },
            )
            .await;
        }
        Ok(ids)
    }

    /// Handles one program-to-host dispatch request.
    pub async fn dispatch(&self, request: DispatchRequest) -> Result<Value, String> {
        match request {
            DispatchRequest::Call {
                execution_id,
                seq,
                connector,
                method,
                arguments,
            } => {
                let session = self.session(&execution_id).await?;
                serde_json::to_value(
                    session
                        .call_at(seq, &connector, &method, arguments, self.clock.now_ms())
                        .await,
                )
                .map_err(|error| error.to_string())
            }
            DispatchRequest::BeginStep {
                execution_id,
                seq,
                name,
            } => {
                let session = self.session(&execution_id).await?;
                serde_json::to_value(
                    session
                        .begin_step_at(seq, &name, self.clock.now_ms())
                        .await
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())
            }
            DispatchRequest::RecordStep {
                execution_id,
                seq,
                result,
            } => {
                self.runtime
                    .record_result(&execution_id, seq, result, self.clock.now_ms())
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(serde_json::json!({ "ok": true }))
            }
        }
    }

    /// Drives one running execution pass with transport request context.
    pub async fn drive_with(
        &self,
        execution_id: &str,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        let state = self.require(execution_id).await?;
        if matches!(
            state.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Error
                | ExecutionStatus::Rejected
                | ExecutionStatus::RolledBack
                | ExecutionStatus::Cancelled
        ) {
            return Ok(state);
        }
        let context = self.context(execution_id, options);
        let cancellation = context.control.cancellation.clone();
        self.contexts
            .lock()
            .await
            .insert(execution_id.to_string(), context.clone());
        let session = match self.session_with_context(execution_id, context).await {
            Ok(session) => session,
            Err(error) => {
                self.contexts.lock().await.remove(execution_id);
                return Err(error);
            }
        };
        let descriptions = session.descriptions();
        let execution = self.executor.execute(
            &state.code,
            &descriptions,
            execution_id,
            Arc::new(ExecutionHost::new(
                Arc::clone(&session),
                Arc::clone(&self.clock),
            )),
        );
        tokio::pin!(execution);
        let response = tokio::select! {
            _ = cancellation.cancelled() => None,
            response = &mut execution => Some(response),
        };
        self.contexts.lock().await.remove(execution_id);
        let Some(response) = response else {
            self.runtime
                .cancel(execution_id, self.clock.now_ms())
                .await
                .map_err(|error| error.to_string())?;
            session.pass_ended("cancelled").await;
            session.execution_ended("cancelled").await;
            return self.require(execution_id).await;
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.runtime
                    .fail(execution_id, error, Vec::new(), self.clock.now_ms())
                    .await
                    .map_err(|error| error.to_string())?;
                session.pass_ended("error").await;
                session.execution_ended("error").await;
                return self.require(execution_id).await;
            }
        };
        let current = self.require(execution_id).await?;
        if current.status == ExecutionStatus::Paused {
            session.pass_ended("paused").await;
            return Ok(current);
        }
        if current.status == ExecutionStatus::Error {
            session.pass_ended("error").await;
            session.execution_ended("error").await;
            return Ok(current);
        }
        if let Some(error) = response.error {
            self.runtime
                .fail(execution_id, error, response.logs, self.clock.now_ms())
                .await
                .map_err(|error| error.to_string())?;
            session.pass_ended("error").await;
            session.execution_ended("error").await;
        } else {
            self.runtime
                .complete(
                    execution_id,
                    response.result.unwrap_or(Value::Null),
                    response.logs,
                    self.clock.now_ms(),
                )
                .await
                .map_err(|error| error.to_string())?;
            session.pass_ended("completed").await;
            session.execution_ended("completed").await;
        }
        self.require(execution_id).await
    }

    async fn session(&self, execution_id: &str) -> Result<Arc<DispatchSession>, String> {
        let context = self
            .contexts
            .lock()
            .await
            .get(execution_id)
            .cloned()
            .unwrap_or_else(|| self.context(execution_id, CodeModeRunOptions::default()));
        self.session_with_context(execution_id, context).await
    }

    async fn session_with_context(
        &self,
        execution_id: &str,
        context: ToolContext,
    ) -> Result<Arc<DispatchSession>, String> {
        let state = self.require(execution_id).await?;
        let descriptions = if let Some(capabilities) = state.capabilities {
            capabilities.connectors
        } else {
            let mut descriptions = Vec::new();
            for connector in &self.connectors {
                descriptions.push(connector.describe().await?);
            }
            descriptions
        };
        Ok(Arc::new(
            DispatchSession::new_with_descriptions_and_context(
                Arc::clone(&self.runtime),
                context,
                self.connectors.clone(),
                descriptions,
            )
            .await?,
        ))
    }

    fn context(&self, execution_id: &str, options: CodeModeRunOptions) -> ToolContext {
        ToolContext {
            execution_id: execution_id.to_string(),
            control: ToolCallControl {
                cancellation: options.cancellation,
                events: Some(Arc::new(RuntimeEventSink {
                    runtime: Arc::clone(&self.runtime),
                    execution_id: execution_id.to_string(),
                    clock: Arc::clone(&self.clock),
                })),
            },
            request: options.request,
        }
    }

    async fn require(&self, execution_id: &str) -> Result<ExecutionState, String> {
        self.runtime
            .execution(execution_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Execution \"{execution_id}\" not found"))
    }

    async fn connector(&self, name: &str) -> Result<Option<&Arc<dyn Connector>>, String> {
        for connector in &self.connectors {
            if connector.describe().await?.name == name {
                return Ok(Some(connector));
            }
        }
        Ok(None)
    }

    async fn notify_execution_end(&self, execution_id: &str, status: &str) {
        for connector in &self.connectors {
            connector.execution_ended(execution_id, status).await;
        }
    }
}
