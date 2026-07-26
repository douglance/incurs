use async_trait::async_trait;
use serde_json::Value;

use crate::{CodeModeRunOptions, ExecutionState, SearchOutput};

/// Send-safe lifecycle boundary used by transports and native actor handles.
#[async_trait]
pub trait CodeModeService: Send + Sync {
    /// Searches current connector methods and saved snippets.
    async fn search(&self, query: String) -> Result<SearchOutput, String>;

    /// Starts an execution and returns its durable running state.
    async fn execute(
        &self,
        code: String,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String>;

    /// Returns one bounded execution snapshot by identifier.
    ///
    /// Oversized values remain artifact references so transports can return the
    /// snapshot before retrieving a selected artifact.
    async fn execution(&self, execution_id: String) -> Result<ExecutionState, String>;

    /// Returns one artifact owned by an execution.
    async fn artifact(&self, execution_id: String, artifact_id: String) -> Result<Value, String>;

    /// Approves a pending action and drives the next replay pass.
    async fn approve(
        &self,
        execution_id: String,
        seq: u64,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String>;

    /// Rejects a pending action.
    async fn reject(&self, execution_id: String, seq: u64) -> Result<ExecutionState, String>;

    /// Cancels a running or paused execution.
    async fn cancel(&self, execution_id: String) -> Result<ExecutionState, String>;
}
