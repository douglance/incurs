#![cfg(target_arch = "wasm32")]

use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::Arc;

use futures_util::StreamExt;
use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::CommandResult;
use incurs::tool::ToolCallOptions;
use incurs_codemode::{
    CodeMode, CodeModeRunOptions, Connector, ConnectorDescription, DEFAULT_PAUSED_TTL_MS,
    DispatchRequest, ExecutionState, IncurConnector, SearchOutput, ToolContext,
};
use incurs_codemode_cloudflare::{
    CloudflareClock, DurableSqlStore, DynamicWorkerExecutor, DynamicWorkerOptions, McpHttpOptions,
    McpHttpRequest, WorkerLoader, drive_with_terminal_failure, handle_mcp_request,
    persist_terminal_failure, tenant_key,
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
            exit_code: None,
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
            exit_code: None,
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
    let preflight = request.path() == "/mcp" && request.method().as_ref() == "OPTIONS";
    let tenant = if preflight {
        "mcp-preflight".to_string()
    } else {
        match request_tenant(&request, &env) {
            Ok(tenant) => tenant,
            Err(response) => return Ok(response),
        }
    };
    let namespace = env.durable_object("CODEMODE")?;
    namespace
        .id_from_name(&tenant)?
        .get_stub()?
        .fetch_with_request(request)
        .await
}

/// Durable host for Code Mode execution history and snippets.
#[durable_object]
pub struct CodeModeExample {
    state: State,
    env: Env,
    runtime: OnceCell<Rc<CodeMode>>,
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
        if path == "/mcp" {
            let method = request.method().as_ref().to_string();
            let headers = request.headers().entries().collect();
            let mut allowed_origins = request
                .url()
                .ok()
                .map(|url| vec![url.origin().ascii_serialization()])
                .unwrap_or_default();
            if let Ok(origins) = self.env.var("MCP_ALLOWED_ORIGINS") {
                allowed_origins.extend(
                    origins
                        .to_string()
                        .split(',')
                        .map(str::trim)
                        .filter(|origin| !origin.is_empty())
                        .map(ToString::to_string),
                );
            }
            let options = McpHttpOptions {
                allowed_origins,
                ..McpHttpOptions::default()
            };
            let body = if method.eq_ignore_ascii_case("POST") {
                Some(read_bounded_body(&mut request, options.max_body_bytes).await?)
            } else {
                None
            };
            let input = McpHttpRequest {
                method,
                path: request.path(),
                headers,
                body,
            };
            let response = handle_mcp_request(
                &WorkerCodeModeService {
                    runtime: Rc::clone(runtime),
                    state: &self.state,
                },
                input,
                &options,
            )
            .await;
            let mut output = match response.body {
                Some(body) => Response::from_json(&body)?.with_status(response.status),
                None => Response::empty()?.with_status(response.status),
            };
            for (name, value) in response.headers {
                output.headers_mut().set(&name, &value)?;
            }
            return Ok(output);
        }
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

async fn read_bounded_body(request: &mut Request, limit: usize) -> Result<String> {
    let mut stream = request.stream()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > limit {
            return Ok("\0".repeat(limit + 1));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map_err(|_| worker::Error::RustError("MCP request body must be UTF-8".into()))
}

impl CodeModeExample {
    fn runtime(&self) -> Result<&Rc<CodeMode>> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(runtime);
        }
        let loader = self.env.get_binding::<WorkerLoader>("LOADER")?;
        let dispatcher = self
            .env
            .durable_object("CODEMODE")?
            .id_from_string(&self.state.id().to_string())?
            .get_stub()?;
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
            .set(Rc::new(CodeMode::with_clock_and_artifact_store(
                store,
                artifacts,
                DynamicWorkerExecutor::new(loader, dispatcher, DynamicWorkerOptions::default()),
                vec![Arc::new(MathConnector {
                    inner: IncurConnector::new(catalog)
                        .with_call_options(ToolCallOptions::isolated()),
                })],
                CloudflareClock,
            )))
            .map_err(|_| {
                worker::Error::RustError("Code Mode runtime already initialized".into())
            })?;
        Ok(self.runtime.get().unwrap())
    }
}

struct WorkerCodeModeService<'a> {
    runtime: Rc<CodeMode>,
    state: &'a State,
}

#[async_trait::async_trait(?Send)]
impl incurs_codemode_cloudflare::WorkerCodeModeService for WorkerCodeModeService<'_> {
    async fn search(&self, query: String) -> std::result::Result<SearchOutput, String> {
        self.runtime.search(&query).await
    }

    async fn execute(
        &self,
        code: String,
        options: CodeModeRunOptions,
    ) -> std::result::Result<ExecutionState, String> {
        let state = self.runtime.start(&code).await?;
        let execution_id = state.id.clone();
        let runtime = Rc::clone(&self.runtime);
        self.state.wait_until(async move {
            let _ = drive_with_terminal_failure(&runtime, &execution_id, options, &CloudflareClock)
                .await;
        });
        Ok(state)
    }

    async fn execution(&self, execution_id: String) -> std::result::Result<ExecutionState, String> {
        self.runtime.execution_snapshot(&execution_id).await
    }

    async fn artifact(
        &self,
        execution_id: String,
        artifact_id: String,
    ) -> std::result::Result<serde_json::Value, String> {
        self.runtime.artifact(&execution_id, &artifact_id).await
    }

    async fn approve(
        &self,
        execution_id: String,
        seq: u64,
        options: CodeModeRunOptions,
    ) -> std::result::Result<ExecutionState, String> {
        if let Err(error) = self.runtime.approve_with(&execution_id, seq, options).await {
            let _ =
                persist_terminal_failure(&self.runtime, &execution_id, &error, &CloudflareClock)
                    .await;
            return Err(error);
        }
        self.runtime.execution_snapshot(&execution_id).await
    }

    async fn reject(
        &self,
        execution_id: String,
        seq: u64,
    ) -> std::result::Result<ExecutionState, String> {
        self.runtime.reject(&execution_id, seq).await?;
        self.runtime.execution_snapshot(&execution_id).await
    }

    async fn cancel(&self, execution_id: String) -> std::result::Result<ExecutionState, String> {
        self.runtime.cancel(&execution_id).await?;
        self.runtime.execution_snapshot(&execution_id).await
    }
}

fn request_tenant(request: &Request, env: &Env) -> std::result::Result<String, Response> {
    let local = request
        .url()
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1"));
    let configured = env
        .var("MCP_AUTH_TOKEN")
        .ok()
        .map(|value| value.to_string());
    let Some(expected) = configured else {
        if local {
            return Ok("local-default".to_string());
        }
        return Err(Response::error(
            "Remote access is disabled until MCP_AUTH_TOKEN is configured",
            503,
        )
        .unwrap());
    };
    let supplied = request
        .headers()
        .get("authorization")
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .split_once(' ')
                .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
                .map(|(_, token)| token.to_string())
        });
    if !supplied.is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes())) {
        let mut response = Response::error("Unauthorized", 401).unwrap();
        let _ = response
            .headers_mut()
            .set("www-authenticate", "Bearer realm=\"incurs-codemode\"");
        return Err(response);
    }
    Ok(format!("tenant-{}", tenant_key(&expected)))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}
