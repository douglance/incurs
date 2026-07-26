#![cfg(target_arch = "wasm32")]

use std::cell::OnceCell;
use std::sync::Arc;

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::CommandResult;
use incurs::tool::ToolCallOptions;
use incurs_codemode::{
    CodeMode, Connector, ConnectorDescription, DEFAULT_PAUSED_TTL_MS, DispatchRequest,
    IncurConnector, ToolContext,
};
use incurs_codemode_cloudflare::{
    CloudflareClock, DurableSqlStore, DynamicWorkerExecutor, DynamicWorkerOptions, WorkerLoader,
};
use serde::Deserialize;
use worker::{
    Context, DurableObject, Env, Request, Response, Result, State, durable_object, event,
    wasm_bindgen,
};

struct Sum;

#[async_trait::async_trait]
impl CommandHandler for Sum {
    async fn run(&self, context: CommandContext) -> CommandResult {
        let left = context.options["left"].as_i64().unwrap_or_default();
        let right = context.options["right"].as_i64().unwrap_or_default();
        CommandResult::Ok {
            data: serde_json::json!({ "sum": left + right }),
            cta: None,
        }
    }
}

struct Save;

#[async_trait::async_trait]
impl CommandHandler for Save {
    async fn run(&self, context: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: serde_json::json!({ "saved": context.options["value"] }),
            cta: None,
        }
    }
}

struct MathConnector {
    inner: IncurConnector,
}

#[async_trait::async_trait]
impl Connector for MathConnector {
    async fn describe(&self) -> std::result::Result<ConnectorDescription, String> {
        self.inner.describe().await
    }

    async fn execute(
        &self,
        method: &str,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> std::result::Result<serde_json::Value, String> {
        self.inner.execute(method, arguments, context).await
    }

    async fn revert(
        &self,
        method: &str,
        _arguments: serde_json::Value,
        _result: serde_json::Value,
        _context: &ToolContext,
    ) -> std::result::Result<bool, String> {
        Ok(method == "save")
    }
}

#[derive(incurs::Options, Deserialize)]
#[allow(dead_code)]
struct SumOptions {
    /// Left operand.
    left: i64,
    /// Right operand.
    right: i64,
}

#[derive(incurs::Options, Deserialize)]
#[allow(dead_code)]
struct SaveOptions {
    /// Value to persist.
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest {
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecisionRequest {
    execution_id: String,
    seq: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionRequest {
    execution_id: String,
}

#[event(fetch)]
async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    let namespace = env.durable_object("CODEMODE")?;
    namespace
        .id_from_name("default")?
        .get_stub()?
        .fetch_with_request(request)
        .await
}

/// Durable host for Code Mode execution history and snippets.
#[durable_object]
pub struct CodeModeExample {
    state: State,
    env: Env,
    runtime: OnceCell<CodeMode>,
}

impl DurableObject for CodeModeExample {
    fn new(state: State, env: Env) -> Self {
        Self {
            state,
            env,
            runtime: OnceCell::new(),
        }
    }

    async fn fetch(&self, mut request: Request) -> Result<Response> {
        let runtime = self.runtime()?;
        let path = request.path();
        if path == "/__incurs_codemode_dispatch" {
            let input = request.json::<DispatchRequest>().await?;
            return Response::from_json(
                &runtime
                    .dispatch(input)
                    .await
                    .map_err(worker::Error::RustError)?,
            );
        }
        let response = match path.as_str() {
            "/execute" => {
                let input = request.json::<ExecuteRequest>().await?;
                runtime.execute(&input.code).await
            }
            "/approve" => {
                let input = request.json::<DecisionRequest>().await?;
                runtime.approve(&input.execution_id, input.seq).await
            }
            "/reject" => {
                let input = request.json::<DecisionRequest>().await?;
                runtime.reject(&input.execution_id, input.seq).await
            }
            "/rollback" => {
                let input = request.json::<ExecutionRequest>().await?;
                runtime.rollback(&input.execution_id).await
            }
            "/pending" => {
                return Response::from_json(
                    &runtime
                        .runtime()
                        .pending(None)
                        .await
                        .map_err(|error| worker::Error::RustError(error.to_string()))?,
                );
            }
            "/expire" => {
                return Response::from_json(
                    &runtime
                        .expire(DEFAULT_PAUSED_TTL_MS)
                        .await
                        .map_err(worker::Error::RustError)?,
                );
            }
            _ => return Response::error("Not found", 404),
        }
        .map_err(worker::Error::RustError)?;
        Response::from_json(&response)
    }
}

impl CodeModeExample {
    fn runtime(&self) -> Result<&CodeMode> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(runtime);
        }
        let loader = self.env.get_binding::<WorkerLoader>("LOADER")?;
        let dispatcher = self
            .env
            .durable_object("CODEMODE")?
            .get_by_name("default")?;
        let store = Arc::new(DurableSqlStore::new(self.state.storage().sql())?);
        let artifacts = store.clone();
        let read_only = McpCommandOptions {
            annotations: Some(McpAnnotations {
                read_only_hint: Some(true),
                ..McpAnnotations::default()
            }),
            ..McpCommandOptions::default()
        };
        let catalog = Cli::create("math")
            .command(
                "sum",
                CommandDef::build("sum", Sum)
                    .description("Add two integers.")
                    .options::<SumOptions>()
                    .mcp(read_only)
                    .done(),
            )
            .command(
                "save",
                CommandDef::build("save", Save)
                    .description("Persist a value.")
                    .options::<SaveOptions>()
                    .done(),
            )
            .tool_catalog();
        self.runtime
            .set(CodeMode::with_clock_and_artifact_store(
                store,
                artifacts,
                DynamicWorkerExecutor::new(loader, dispatcher, DynamicWorkerOptions::default()),
                vec![Arc::new(MathConnector {
                    inner: IncurConnector::new(catalog)
                        .with_call_options(ToolCallOptions::isolated()),
                })],
                CloudflareClock,
            ))
            .map_err(|_| {
                worker::Error::RustError("Code Mode runtime already initialized".into())
            })?;
        Ok(self.runtime.get().unwrap())
    }
}
