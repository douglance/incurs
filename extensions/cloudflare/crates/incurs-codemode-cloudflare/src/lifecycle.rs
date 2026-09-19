use incurs_codemode::{Clock, CodeMode, CodeModeRunOptions, ExecutionState, ExecutionStatus};

/// Drives one execution pass and persists a terminal error when setup fails.
pub async fn drive_with_terminal_failure(
    runtime: &CodeMode,
    execution_id: &str,
    options: CodeModeRunOptions,
    clock: &dyn Clock,
) -> Result<ExecutionState, String> {
    match runtime.drive_with(execution_id, options).await {
        Ok(state) => Ok(state),
        Err(error) => {
            let _ = persist_terminal_failure(runtime, execution_id, &error, clock).await;
            Err(error)
        }
    }
}

/// Persists a drive failure unless the execution already reached a terminal state.
pub async fn persist_terminal_failure(
    runtime: &CodeMode,
    execution_id: &str,
    error: &str,
    clock: &dyn Clock,
) -> Result<ExecutionState, String> {
    let state = runtime.execution_snapshot(execution_id).await?;
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
    runtime
        .runtime()
        .fail(
            execution_id,
            format!("Code Mode drive failed: {error}"),
            Vec::new(),
            clock.now_ms(),
        )
        .await
        .map_err(|error| error.to_string())?;
    runtime.execution_snapshot(execution_id).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use incurs_codemode::{
        CodeExecutor, Connector, ConnectorDescription, ExecuteResult, ExecutionHost, MemoryStore,
        ToolContext,
    };
    use serde_json::Value;

    use super::*;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_ms(&self) -> u64 {
            42
        }
    }

    struct FlakyConnector {
        descriptions: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Connector for FlakyConnector {
        fn name(&self) -> &str {
            "test"
        }

        async fn describe(&self) -> Result<ConnectorDescription, String> {
            if self.descriptions.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ConnectorDescription {
                    name: "test".to_string(),
                    instructions: None,
                    tools: Vec::new(),
                })
            } else {
                Err("connector setup failed".to_string())
            }
        }

        async fn execute(
            &self,
            _method: &str,
            _arguments: Value,
            _context: &ToolContext,
        ) -> Result<Value, String> {
            Err("not called".to_string())
        }
    }

    struct UnusedExecutor;

    #[async_trait::async_trait(?Send)]
    impl CodeExecutor for UnusedExecutor {
        async fn execute(
            &self,
            _code: &str,
            _connectors: &[ConnectorDescription],
            _execution_id: &str,
            _host: Arc<ExecutionHost>,
        ) -> Result<ExecuteResult, String> {
            Err("not called".to_string())
        }
    }

    #[tokio::test]
    async fn drive_setup_errors_become_terminal_execution_errors() {
        // Two connectors claiming one namespace. This used to be driven by a
        // connector whose second `describe` failed, because session construction
        // described every connector up front; namespaces now bind lazily, so a
        // describe failure surfaces at the call that needs it rather than at setup.
        // A duplicate namespace is still a genuine setup failure -- the bindings
        // cannot be emitted at all -- so it exercises the same path.
        let runtime = CodeMode::with_clock(
            Arc::new(MemoryStore::default()),
            UnusedExecutor,
            vec![
                Arc::new(FlakyConnector {
                    descriptions: AtomicUsize::new(0),
                }),
                Arc::new(FlakyConnector {
                    descriptions: AtomicUsize::new(0),
                }),
            ],
            FixedClock,
        );
        let running = runtime.start("1 + 1").await.unwrap();
        let result = drive_with_terminal_failure(
            &runtime,
            &running.id,
            CodeModeRunOptions::default(),
            &FixedClock,
        )
        .await;
        assert_eq!(result.unwrap_err(), "Duplicate connector name \"test\"");
        let failed = runtime.execution_snapshot(&running.id).await.unwrap();
        assert_eq!(failed.status, ExecutionStatus::Error);
        assert_eq!(
            failed.error.as_deref(),
            Some("Code Mode drive failed: Duplicate connector name \"test\"")
        );
    }
}
