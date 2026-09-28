//! Every incurs feature, served from one Cloudflare Worker.
//!
//! Each route exercises one feature so a smoke test can prove it runs on
//! Workers rather than only compiling for wasm32.

#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Extension;
use axum::routing::{get, post};
use axum::{Json, Router};
use incurs::agent_plugin::loader::{
    AGENT_PLUGIN_MCP_SCHEMA, AgentPluginFiles, AgentPluginLoadOptions, load_agent_plugin_from_files,
};
use incurs::agent_plugin_runtime::{AgentPluginRuntimeOptions, connect_agent_plugin};
use incurs::cli::{Cli, Runtime};
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::http::{RouterOptions, build_cli_router_with};
use incurs::mcp::{McpHttpConfig, McpRemoteOptions};
use incurs::openapi::{FetchFn, GenerateOptions, OpenApiSource};
use incurs::outbound::SharedHttpClient;
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs_mcp_cloudflare::WorkersHttpClient;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_service::Service;
use worker::{Context, Env, HttpRequest, event, send::SendFuture};

/// Worker variables forwarded to commands as their environment.
const FORWARDED_VARS: &[&str] = &["GREETING_STYLE", "OPENAPI_BASE", "SELF_URL"];

struct Greet;

#[async_trait::async_trait]
impl CommandHandler for Greet {
    async fn run(&self, context: CommandContext) -> CommandResult {
        let name = context.args["name"].as_str().unwrap_or("world");
        let style = context.env["style"].as_str().unwrap_or("plain");
        CommandResult::Ok {
            data: json!({ "message": format!("hello {name}"), "style": style }),
            cta: None,
            exit_code: None,
        }
    }
}

fn field(name: &'static str, field_type: FieldType, env_name: Option<&'static str>) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.to_string(),
        description: None,
        field_type,
        required: false,
        default: None,
        alias: None,
        deprecated: false,
        env_name,
    }
}

/// Calls the Workers fetch API; generated OpenAPI commands use it for every
/// operation.
fn workers_fetch() -> FetchFn {
    Arc::new(|url, method, headers, body| {
        Box::pin(SendFuture::new(async move {
            let result = async {
                let mut init = worker::RequestInit::new();
                init.with_method(worker::Method::from(method));
                let request_headers = worker::Headers::new();
                for (name, value) in headers {
                    request_headers.set(&name, &value)?;
                }
                init.with_headers(request_headers);
                if let Some(body) = body {
                    init.with_body(Some(body.into()));
                }
                let request = worker::Request::new_with_init(&url, &init)?;
                let mut response = worker::Fetch::Request(request).send().await?;
                response.json::<Value>().await
            }
            .await;
            result.unwrap_or_else(|error| json!({ "fetchError": error.to_string() }))
        }))
    })
}

/// The client incurs sends its own outbound HTTP through: remote MCP, plugin
/// MCP servers, and the OpenAPI spec download.
fn outbound() -> SharedHttpClient {
    Arc::new(WorkersHttpClient::new())
}

/// The commands every route serves: a local command and an OpenAPI group.
async fn build_cli(vars: &HashMap<String, String>) -> Result<Cli, String> {
    let mut greet = CommandDef::build("greet", Greet)
        .description("Greet someone")
        .done();
    greet.args_fields = vec![field("name", FieldType::String, None)];
    greet.env_fields = vec![field("style", FieldType::String, Some("GREETING_STYLE"))];
    let cli = Cli::create("worker-demo")
        .version("1.0.0")
        .description("incurs running inside a Cloudflare Worker")
        .command("greet", greet);
    let Some(base) = vars.get("OPENAPI_BASE") else {
        return Ok(cli);
    };
    let options = GenerateOptions {
        base_path: Some(base.clone()),
        ..GenerateOptions::default()
    };
    // Downloading the spec by URL goes through `WorkersHttpClient`; the
    // generated operations go through `workers_fetch`.
    SendFuture::new(cli.openapi_source_group(
        "pets",
        OpenApiSource::Url {
            url: format!("{base}/openapi.json"),
            client: outbound(),
        },
        workers_fetch(),
        options,
        Some("Pet store operations".to_string()),
    ))
    .await
    .map_err(|error| error.to_string())
}

fn self_mcp_url(vars: &HashMap<String, String>) -> Option<String> {
    vars.get("SELF_URL").map(|base| format!("{base}/api/mcp"))
}

#[derive(Deserialize)]
struct RunRequest {
    argv: Vec<String>,
}

/// Runs one command line through the same path as the native CLI. The `self`
/// group holds this Worker's own MCP tools, fetched over MCP from `/api/mcp`.
#[worker::send]
async fn run(
    Extension(vars): Extension<Arc<HashMap<String, String>>>,
    Json(input): Json<RunRequest>,
) -> Json<Value> {
    let mut cli = match build_cli(&vars).await {
        Ok(cli) => cli,
        Err(error) => return Json(json!({ "error": error })),
    };
    if let Some(url) = self_mcp_url(&vars) {
        cli = match cli
            .remote_mcp_with(
                "self",
                url,
                None,
                &McpRemoteOptions {
                    http_client: Some(outbound()),
                    ..McpRemoteOptions::default()
                },
            )
            .await
        {
            Ok(cli) => cli,
            Err(error) => return Json(json!({ "error": error.to_string() })),
        };
    }
    let mut output = Vec::new();
    let result = cli
        .run_to(
            input.argv,
            &mut output,
            Runtime::new("worker-demo", vars.as_ref().clone(), false),
        )
        .await;
    Json(json!({
        "exitCode": result.as_ref().ok().cloned().flatten().unwrap_or(0),
        "error": result.err().map(|error| error.to_string()),
        "output": String::from_utf8_lossy(&output),
    }))
}

/// Loads the bundled plugin package from memory and connects its MCP servers.
#[worker::send]
async fn plugin(Extension(vars): Extension<Arc<HashMap<String, String>>>) -> Json<Value> {
    let mut files: AgentPluginFiles = [
        ("plugin.json", include_str!("../plugin/plugin.json")),
        (
            "skills/greeting/SKILL.md",
            include_str!("../plugin/skills/greeting/SKILL.md"),
        ),
    ]
    .into_iter()
    .map(|(path, text)| (path, text.as_bytes().to_vec()))
    .collect();
    if let Some(url) = self_mcp_url(&vars) {
        let mcp = json!({
            "$schema": AGENT_PLUGIN_MCP_SCHEMA,
            "mcpServers": {
                "self": { "type": "streamable-http", "url": url },
                "local": { "type": "stdio", "command": "./bin/server" },
            },
        });
        files.insert("mcp.json", mcp.to_string().into_bytes());
        files.insert("bin/server", Vec::new());
    }
    let report = load_agent_plugin_from_files(&files, &AgentPluginLoadOptions::default());
    let Some(loaded) = report.plugin.as_ref() else {
        return Json(
            json!({ "diagnostics": report.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>() }),
        );
    };
    let options = AgentPluginRuntimeOptions {
        http_client: Some(outbound()),
        ..AgentPluginRuntimeOptions::default()
    };
    let servers = match connect_agent_plugin(loaded, &options).await {
        Ok(connected) => connected
            .servers
            .iter()
            .map(|server| {
                json!({
                    "name": server.name,
                    "toolCount": server.tool_count,
                    "errorCode": server.error_code,
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => vec![json!({ "error": error.to_string() })],
    };
    Json(json!({
        "name": loaded.manifest.name,
        "skills": loaded.skills.iter().map(|skill| skill.name.clone()).collect::<Vec<_>>(),
        "diagnostics": report.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        "servers": servers,
    }))
}

#[event(fetch)]
async fn fetch(
    request: HttpRequest,
    env: Env,
    _context: Context,
) -> worker::Result<axum::http::Response<axum::body::Body>> {
    std::panic::set_hook(Box::new(|info| worker::console_error!("{info}")));
    let vars: HashMap<String, String> = FORWARDED_VARS
        .iter()
        .filter_map(|name| {
            env.var(name)
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect();
    let cli = build_cli(&vars).await.map_err(worker::Error::RustError)?;
    let api = build_cli_router_with(
        &cli,
        RouterOptions {
            env: Some(vars.clone()),
            mcp: McpHttpConfig::default(),
        },
    )
    .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let mut router = Router::new()
        .route("/run", post(run))
        .route("/plugin", get(plugin))
        .nest("/api", api)
        .layer(Extension(Arc::new(vars)));
    Ok(router.call(request).await?)
}
