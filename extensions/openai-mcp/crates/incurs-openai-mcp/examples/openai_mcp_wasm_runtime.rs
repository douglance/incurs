use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use incurs::cli::Cli;
use incurs::command::CommandContext;
use incurs::mcp::{McpHttpBody, McpHttpConfig, McpHttpRequest, McpHttpServer};
use incurs::tool::EnvironmentSource;
use incurs_openai_mcp::{
    IncursOpenAiServer, OpenAiSettings, OpenAiSettingsField, OpenAiSettingsRegistration,
};
use serde_json::{Value, json};

const LEGACY_VERSION: &str = "2025-11-25";
static UPDATE_CALLS: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn openai_mcp_server_wasm_smoke() -> u32 {
    match run_smoke() {
        Ok(()) => 0,
        Err(code) => code,
    }
}

fn run_smoke() -> Result<(), u32> {
    UPDATE_CALLS.store(0, Ordering::SeqCst);
    let server = build_server()?;

    let initialized = portable_rpc(
        &server,
        1,
        "initialize",
        Some(json!({
            "protocolVersion": LEGACY_VERSION,
            "capabilities": {
                "extensions": {"openai/elicitation": {"form": {}}},
                "experimental": {"openai/elicitation": {"form": {}}}
            },
            "clientInfo": {"name": "openai-wasm-smoke", "version": "1.0.0"}
        })),
    )?;
    ensure(
        initialized["result"]["capabilities"]["extensions"]["openai/settings"]
            == json!({"readTool":"settings.read","updateTool":"settings.update"}),
        10,
    )?;

    let discovered = portable_rpc(
        &server,
        2,
        "server/discover",
        Some(json!({"_meta": openai_meta()})),
    )?;
    ensure(discovered["result"]["resultType"] == "complete", 11)?;
    ensure(
        discovered["result"]["capabilities"]["experimental"]["openai/settings"]
            == json!({"readTool":"settings.read","updateTool":"settings.update"}),
        12,
    )?;

    let listed = portable_rpc(&server, 3, "tools/list", None)?;
    let tools = listed["result"]["tools"].as_array().ok_or(20_u32)?;
    ensure(has_tool(tools, "settings.read"), 21)?;
    ensure(has_tool(tools, "settings.update"), 22)?;

    let read = portable_rpc(
        &server,
        4,
        "tools/call",
        Some(json!({"name":"settings.read","arguments":{}})),
    )?;
    ensure(read["result"]["content"] == json!([]), 30)?;
    ensure(
        read["result"]["structuredContent"]["values"] == json!({"text":"😀","code":"ok"}),
        31,
    )?;

    let invalid = portable_rpc(
        &server,
        5,
        "tools/call",
        Some(json!({"name":"settings.update","arguments":{"set":{"text":"a"}}})),
    )?;
    ensure(invalid["result"]["isError"] == true, 40)?;
    ensure(UPDATE_CALLS.load(Ordering::SeqCst) == 0, 41)?;

    let valid = portable_rpc(
        &server,
        6,
        "tools/call",
        Some(json!({"name":"settings.update","arguments":{"set":{"text":"😀","code":"ok"}}})),
    )?;
    ensure(valid["result"]["isError"] == false, 50)?;
    ensure(
        valid["result"]["structuredContent"]["values"] == json!({"text":"😀","code":"ok"}),
        51,
    )?;
    ensure(UPDATE_CALLS.load(Ordering::SeqCst) == 1, 52)?;

    Ok(())
}

fn build_server() -> Result<McpHttpServer, u32> {
    let mut openai = IncursOpenAiServer::new(Cli::create("openai-wasm-smoke"));
    OpenAiSettings::<CommandContext>::default()
        .register(&mut openai, settings_registration())
        .map_err(|_| 60_u32)?;
    let cli = openai.into_cli();
    let config = McpHttpConfig {
        environment: EnvironmentSource::Empty,
        ..Default::default()
    };
    McpHttpServer::from_cli(&cli, config).map_err(|_| 61_u32)
}

fn settings_registration() -> OpenAiSettingsRegistration<CommandContext> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "text".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","minLength":2}), "Text"),
    );
    fields.insert(
        "code".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","pattern":"^ok$"}), "Code"),
    );
    OpenAiSettingsRegistration {
        read_tool: None,
        update_tool: None,
        fields,
        layout: None,
        read: Arc::new(|_| Box::pin(async { Ok(default_values()) })),
        update: Arc::new(|set, _| {
            Box::pin(async move {
                UPDATE_CALLS.fetch_add(1, Ordering::SeqCst);
                let mut values = default_values();
                values.extend(set);
                Ok(values)
            })
        }),
    }
}

fn default_values() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("text".to_string(), json!("😀")),
        ("code".to_string(), json!("ok")),
    ])
}

fn portable_rpc(
    server: &McpHttpServer,
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Result<Value, u32> {
    let response = poll_ready(server.handle(McpHttpRequest {
        method: "POST".to_string(),
        path: "/mcp".to_string(),
        headers: legacy_headers(),
        body: rpc(id, method, params),
    }))?;
    ensure(response.status == 200, 70)?;
    let mut messages = portable_messages(response.body)?;
    ensure(messages.len() == 1, 71)?;
    Ok(messages.remove(0))
}

fn portable_messages(body: McpHttpBody) -> Result<Vec<Value>, u32> {
    match body {
        McpHttpBody::Empty => Ok(Vec::new()),
        McpHttpBody::Full(bytes) => serde_json::from_slice(&bytes)
            .map(|value| vec![value])
            .map_err(|_| 80_u32),
        McpHttpBody::EventStream(mut stream) => {
            let mut text = String::new();
            let mut context = Context::from_waker(Waker::noop());
            loop {
                match stream.as_mut().poll_next(&mut context) {
                    Poll::Ready(value) => match value {
                        Some(chunk) => text.push_str(&chunk),
                        None => break,
                    },
                    Poll::Pending => return Err(91),
                }
            }
            text.split("\n\n")
                .filter_map(|event| {
                    event.lines().find_map(|line| {
                        line.strip_prefix("data: ")
                            .map(|data| serde_json::from_str(data).map_err(|_| 81_u32))
                    })
                })
                .collect()
        }
    }
}

fn poll_ready<F: Future>(future: F) -> Result<F::Output, u32> {
    let mut future = Box::pin(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => Ok(value),
        Poll::Pending => Err(90),
    }
}

fn legacy_headers() -> Vec<(String, String)> {
    vec![
        ("host".to_string(), "localhost".to_string()),
        (
            "accept".to_string(),
            "application/json, text/event-stream".to_string(),
        ),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "mcp-protocol-version".to_string(),
            LEGACY_VERSION.to_string(),
        ),
    ]
}

fn openai_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": LEGACY_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": {"openai/elicitation": {"form": {}}},
            "experimental": {"openai/elicitation": {"form": {}}}
        },
        "io.modelcontextprotocol/clientInfo": {"name": "openai-wasm-smoke", "version": "1.0.0"}
    })
}

fn rpc(id: u64, method: &str, params: Option<Value>) -> Vec<u8> {
    let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
    if let Some(params) = params {
        message["params"] = params;
    }
    serde_json::to_vec(&message).unwrap_or_default()
}

fn has_tool(tools: &[Value], name: &str) -> bool {
    tools.iter().any(|tool| tool["name"] == name)
}

fn ensure(condition: bool, code: u32) -> Result<(), u32> {
    if condition { Ok(()) } else { Err(code) }
}

fn main() {}
