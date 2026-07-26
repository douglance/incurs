use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ConnectorDescription, DispatchResponse, DispatchSession, StepResponse};

/// Result returned by a Code Mode program executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteResult {
    /// Successful program result.
    pub result: Option<Value>,
    /// Program or sandbox error.
    pub error: Option<String>,
    /// Captured console output.
    pub logs: Vec<String>,
}

/// Host request emitted by an executing Code Mode program.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchRequest {
    /// Execute or replay one connector call.
    Call {
        /// Durable execution identifier.
        execution_id: String,
        /// Program-assigned deterministic sequence.
        seq: u64,
        /// Connector namespace.
        connector: String,
        /// Connector method.
        method: String,
        /// JSON-safe tagged arguments.
        arguments: Value,
    },
    /// Decide whether a local `codemode.step` callback should run.
    BeginStep {
        /// Durable execution identifier.
        execution_id: String,
        /// Program-assigned deterministic sequence.
        seq: u64,
        /// Stable step name.
        name: String,
    },
    /// Record a completed local `codemode.step` callback.
    RecordStep {
        /// Durable execution identifier.
        execution_id: String,
        /// Program-assigned deterministic sequence.
        seq: u64,
        /// JSON-safe tagged callback result.
        result: Value,
    },
}

/// Executes model-generated JavaScript for one replay pass.
///
/// Executors provide isolation and host bridging. The durable lifecycle,
/// connector policy, approvals, replay, and rollback remain in [`crate::CodeMode`].
#[async_trait(?Send)]
pub trait CodeExecutor {
    /// Executes one pass and returns its result or captured program error.
    async fn execute(
        &self,
        code: &str,
        connectors: &[ConnectorDescription],
        execution_id: &str,
        host: Arc<ExecutionHost>,
    ) -> Result<ExecuteResult, String>;
}

/// Supplies monotonic-enough Unix millisecond timestamps to Code Mode.
pub trait Clock: Send + Sync {
    /// Returns the current Unix timestamp in milliseconds.
    fn now_ms(&self) -> u64;
}

/// Native system clock used by default.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Execution-scoped bridge from a local engine into durable dispatch.
pub struct ExecutionHost {
    session: Arc<DispatchSession>,
    clock: Arc<dyn Clock>,
}

impl ExecutionHost {
    pub(crate) fn new(session: Arc<DispatchSession>, clock: Arc<dyn Clock>) -> Self {
        Self { session, clock }
    }

    /// Executes or replays one connector call.
    pub async fn call(
        &self,
        seq: u64,
        connector: &str,
        method: &str,
        arguments: Value,
    ) -> DispatchResponse {
        self.session
            .call_at(seq, connector, method, arguments, self.clock.now_ms())
            .await
    }

    /// Begins a durable local callback step.
    pub async fn begin_step(&self, seq: u64, name: &str) -> Result<StepResponse, String> {
        self.session
            .begin_step_at(seq, name, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())
    }

    /// Records a completed durable local callback step.
    pub async fn record_step(&self, seq: u64, result: Value) -> Result<(), String> {
        self.session
            .record_step(seq, result, self.clock.now_ms())
            .await
            .map_err(|error| error.to_string())
    }
}
