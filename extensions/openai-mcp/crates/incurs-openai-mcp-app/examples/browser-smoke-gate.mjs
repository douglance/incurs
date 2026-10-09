#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

async function main() {
const scriptDir = dirname(fileURLToPath(import.meta.url));
const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index];
  const value = process.argv[index + 1];
  if (!key?.startsWith('--') || value == null) {
    throw new Error('usage: node examples/browser-smoke-gate.mjs [--app-crate PATH --neutral-crate PATH --protocol-crate PATH]');
  }
  args.set(key, value);
}

const appCrate = resolve(args.get('--app-crate') ?? join(scriptDir, '..'));
const neutralCrate = resolve(args.get('--neutral-crate') ?? join(appCrate, '../../../../crates/incurs-mcp-apps'));
const protocolCrate = resolve(args.get('--protocol-crate') ?? join(appCrate, '../incurs-openai-mcp-protocol'));
for (const [name, path] of [['app', appCrate], ['neutral', neutralCrate], ['protocol', protocolCrate]]) {
  if (!existsSync(join(path, 'Cargo.toml'))) {
    throw new Error(`${name} crate is missing Cargo.toml at ${path}`);
  }
}

const tempRoot = await mkProofRoot();
const appCopy = join(tempRoot, 'app');
const neutralCopy = join(tempRoot, 'neutral');
const protocolCopy = join(tempRoot, 'protocol');
const consumer = join(tempRoot, 'consumer');
await copyCrate(appCrate, appCopy);
await copyCrate(neutralCrate, neutralCopy);
await copyCrate(protocolCrate, protocolCopy);
await patchAppManifest(appCopy);
await writeProofWorkspace(tempRoot, consumer);

console.log(`browser-smoke-temp=${tempRoot}`);
console.log(`browser-smoke-app=${appCrate}`);
console.log(`browser-smoke-neutral=${neutralCrate}`);
console.log(`browser-smoke-protocol=${protocolCrate}`);

await run('cargo', [
  'build',
  '--manifest-path',
  join(consumer, 'Cargo.toml'),
  '--target',
  'wasm32-unknown-unknown',
  '--target-dir',
  join(consumer, 'target'),
], consumer);
await requireWasmBindgenVersion(consumer);
await run('wasm-bindgen', [
  join(consumer, 'target/wasm32-unknown-unknown/debug/openai_browser_gate.wasm'),
  '--target',
  'web',
  '--out-dir',
  join(consumer, 'pkg'),
], consumer);
await run('npm', ['ci'], consumer);
if (!process.env.CHROME_PATH) {
  await run('npx', ['playwright', 'install', '--only-shell', 'chromium'], consumer);
}
await run('node', ['run-playwright.mjs'], consumer);

if (!process.env.INCURS_OPENAI_BROWSER_SMOKE_KEEP_TEMP) {
  await rm(tempRoot, { recursive: true, force: true });
}
}

async function mkProofRoot() {
  const prefix = join(tmpdir(), 'incurs-openai-browser-smoke-');
  const root = await import('node:fs/promises').then((fs) => fs.mkdtemp(prefix));
  return root;
}

async function copyCrate(from, to) {
  await cp(from, to, {
    recursive: true,
    filter(source) {
      const parts = source.split('/');
      return !parts.includes('target') && !parts.includes('node_modules') && !parts.includes('pkg') && !parts.includes('.git');
    },
  });
}

async function patchAppManifest(path) {
  const manifestPath = join(path, 'Cargo.toml');
  let manifest = await readFile(manifestPath, 'utf8');
  manifest = manifest.replace(/^incurs-mcp-apps\s*=.*$/m, 'incurs-mcp-apps = { path = "../neutral", version = "0.1.0" }');
  manifest = manifest.replace(/^incurs-openai-mcp-protocol\s*=.*$/m, 'incurs-openai-mcp-protocol = { path = "../protocol", version = "0.1.0" }');
  await writeFile(manifestPath, manifest);
}

async function writeProofWorkspace(root, consumerPath) {
  await writeFile(join(root, 'Cargo.toml'), workspaceManifest);
  await mkdir(join(consumerPath, 'src'), { recursive: true });
  await writeFile(join(consumerPath, 'Cargo.toml'), consumerManifest);
  await writeFile(join(consumerPath, 'src/lib.rs'), rustProofSource);
  await writeFile(join(consumerPath, 'index.html'), indexHtml);
  await writeFile(join(consumerPath, 'package.json'), await readFile(new URL('./browser-smoke-package.json', import.meta.url), 'utf8'));
  await writeFile(join(consumerPath, 'package-lock.json'), await readFile(new URL('./browser-smoke-package-lock.json', import.meta.url), 'utf8'));
  await writeFile(join(consumerPath, 'run-playwright.mjs'), playwrightRunner);
}

async function requireWasmBindgenVersion(cwd) {
  const output = await commandOutput('wasm-bindgen', ['--version'], cwd);
  if (!/wasm-bindgen 0\.2\.126\b/.test(output)) {
    throw new Error(`wasm-bindgen 0.2.126 is required, found: ${output.trim()}`);
  }
}

async function commandOutput(command, argv, cwd) {
  return await new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, argv, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    child.stdout.on('data', (chunk) => {
      output += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      output += chunk.toString();
    });
    child.on('error', rejectRun);
    child.on('exit', (code) => {
      if (code === 0) {
        resolveRun(output);
      } else {
        rejectRun(new Error(`${command} ${argv.join(' ')} exited ${code}: ${output}`));
      }
    });
  });
}

async function run(command, argv, cwd) {
  await new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, argv, { cwd, stdio: 'inherit' });
    child.on('error', rejectRun);
    child.on('exit', (code) => {
      if (code === 0) {
        resolveRun();
      } else {
        rejectRun(new Error(`${command} ${argv.join(' ')} exited ${code}`));
      }
    });
  });
}

const workspaceManifest = String.raw`[workspace]
members = ["app", "neutral", "protocol", "consumer"]
resolver = "2"

[workspace.package]
edition = "2024"
rust-version = "1.88"
license = "MIT AND Apache-2.0"
repository = "https://github.com/douglance/incurs"

[workspace.dependencies]
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "=1.0.151"
thiserror = "2"

[patch.crates-io]
incurs-mcp-apps = { path = "neutral" }
incurs-openai-mcp-protocol = { path = "protocol" }
`;

const consumerManifest = String.raw`[package]
name = "openai-browser-gate"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
futures = "0.3"
incurs-mcp-apps = { path = "../neutral" }
incurs-openai-mcp-app = { path = "../app" }
js-sys = "=0.3.103"
serde_json = "1"
wasm-bindgen = "=0.2.126"
wasm-bindgen-futures = "=0.4.76"
web-sys = { version = "=0.3.103", features = ["CssStyleDeclaration", "Document", "Element", "HtmlElement", "Node", "Window"] }
`;

const indexHtml = String.raw`<!doctype html>
<meta charset="utf-8">
<title>OpenAI MCP App Browser Gate</title>
<script type="module">
  import init, { run_smoke } from './pkg/openai_browser_gate.js';
  window.__runSmoke = async () => {
    await init();
    return await run_smoke();
  };
</script>
`;

const packageJson = String.raw`{"type":"module","dependencies":{"@playwright/test":"^1.56.1"}}
`;

const playwrightRunner = String.raw`import { chromium } from '@playwright/test';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, resolve } from 'node:path';

const root = resolve(import.meta.dirname);
const types = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.wasm', 'application/wasm'],
]);
const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');
  const path = url.pathname === '/' ? '/index.html' : url.pathname;
  try {
    const body = await readFile(join(root, path));
    response.writeHead(200, { 'content-type': types.get(extname(path)) ?? 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end('not found');
  }
});
await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen));
const { port } = server.address();
const executablePath = process.env.CHROME_PATH;
const browser = await chromium.launch({
  headless: true,
  ...(executablePath ? { executablePath } : {}),
});
try {
  const page = await browser.newPage();
  await page.goto('http://127.0.0.1:' + port + '/');
  await page.waitForFunction(() => typeof window.__runSmoke === 'function');
  const summary = await page.evaluate(() => window.__runSmoke());
  console.log(summary);
} finally {
  await browser.close();
  server.close();
}
`;

const rustProofSource = String.raw`//! Browser proof for OpenAI MCP App helpers.

#![deny(missing_docs)]

use std::{cell::Cell, rc::Rc};

use futures::{FutureExt, channel::oneshot};
use incurs_mcp_apps::{
    AppError, AppResult, AppTransport, HOST_CONTEXT_CHANGED_METHOD, INITIALIZE_METHOD,
    InitializeParams, MODEL_CONTEXT_UPDATE_METHOD, McpApp, RequestCancellation, RequestOptions,
    SEND_MESSAGE_METHOD, browser::BrowserPostMessageTransport,
};
use incurs_openai_mcp_app::{
    OPENAI_MCP_APP_RESOURCE_WRITE_METHOD, OpenAiAppExtensions, OpenAiMessageMetadata,
    OpenAiMessageOptions, OpenAiMessageParams, OpenAiMessageTarget, OpenAiResourceWriteContent,
    OpenAiResourceWriteOptions,
};
use js_sys::Promise;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;

/// Runs the browser proof and returns a JSON summary string.
#[wasm_bindgen]
pub async fn run_smoke() -> Result<String, JsValue> {
    run_smoke_inner()
        .await
        .map(|value| value.to_string())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

async fn run_smoke_inner() -> AppResult<Value> {
    let window = web_sys::window().ok_or_else(|| AppError::Transport("missing window".into()))?;
    let transport = BrowserPostMessageTransport::new_for_window(
        window.clone(),
        window.clone(),
        window.clone(),
    )?;
    install_handlers(&transport);

    let app = McpApp::new(transport.clone());
    let openai = OpenAiAppExtensions::new(app.clone());
    require(openai.files().is_none(), "files enabled before initialize")?;
    require(openai.message().is_none(), "message enabled before initialize")?;

    app.initialize(
        InitializeParams::new("openai-browser-gate", "1.0.0", "2025-06-18"),
        short_options(),
    )
    .await?;
    require(openai.files().is_some(), "files helper missing")?;
    require(openai.message().is_some(), "message helper missing")?;
    require(openai.model_context().is_some(), "model context helper missing")?;
    require(openai.resources().is_some(), "resources helper missing")?;
    require(
        openai
            .deep_link()
            .current()
            .is_some_and(|link| link.url == "/browser-gate"),
        "deep link mismatch",
    )?;

    let cursor = openai.install_interaction_cursor_style()?;
    require_stylesheet()?;
    require_cursor("default")?;
    transport.notify(
        HOST_CONTEXT_CHANGED_METHOD,
        json!({ "openai/interactionCursor": "pointer" }),
    )?;
    wait_for_browser(0).await?;
    require_cursor("pointer")?;

    prove_message_defaults_send_false_and_cancel(&transport, &openai).await?;
    prove_model_context(&openai).await?;
    prove_empty_blob_write(&openai).await?;

    cursor.dispose();
    transport.dispose();
    Ok(json!({
        "browserGate": true,
        "uiHandshake": true,
        "messageDefaults": true,
        "sendFalseRejectedBeforePost": true,
        "requestCancellation": true,
        "modelContext": true,
        "emptyBlob": true,
        "cursorStyle": true
    }))
}

fn install_handlers(transport: &BrowserPostMessageTransport) {
    transport.handle(
        INITIALIZE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            require(params["appInfo"]["name"] == "openai-browser-gate", "wrong app name")?;
            Ok(json!({
                "protocolVersion": "2025-06-18",
                "hostInfo": { "name": "browser-gate-host" },
                "hostCapabilities": { "experimental": {
                    "openai/files": {},
                    "openai/message": {},
                    "openai/modelContext": {},
                    "openai/resource": {}
                }},
                "hostContext": {
                    "openai/deepLink": { "url": "/browser-gate" },
                    "openai/modelContext": { "content": [], "updateId": "initial" },
                    "openai/interactionCursor": "default"
                }
            }))
        }),
    );

    let message_count = Rc::new(Cell::new(0));
    transport.handle(SEND_MESSAGE_METHOD, {
        let message_count = message_count.clone();
        Rc::new(move |params| {
            let index = message_count.get();
            message_count.set(index + 1);
            if index == 0 {
                async move {
                    require(params["content"][0]["text"] == "hello", "wrong message text")?;
                    require(params["_meta"]["openai/message"]["target"] == "active", "missing active target")?;
                    require(params["_meta"]["openai/message"]["send"] == true, "missing send true")?;
                    Ok(json!({ "accepted": true }))
                }
                .boxed_local()
            } else {
                async move { futures::future::pending::<AppResult<Value>>().await }.boxed_local()
            }
        })
    });

    transport.handle(MODEL_CONTEXT_UPDATE_METHOD, incurs_mcp_apps::value_handler(|params| {
        require(params["content"][0]["text"] == "next", "wrong model context update")?;
        Ok(json!({ "_meta": { "openai/modelContext": { "updateId": "next-id" } } }))
    }));

    transport.handle(OPENAI_MCP_APP_RESOURCE_WRITE_METHOD, incurs_mcp_apps::value_handler(|params| {
        require(params["blob"] == "", "empty blob was not sent literally")?;
        require(params.get("text").is_none(), "blob write included text")?;
        Ok(json!({ "outcome": "saved", "etag": "empty" }))
    }));
}

async fn prove_message_defaults_send_false_and_cancel(
    transport: &BrowserPostMessageTransport,
    openai: &OpenAiAppExtensions<BrowserPostMessageTransport>,
) -> AppResult<()> {
    let message = openai.message().unwrap();
    let response = message
        .send(
            OpenAiMessageParams::user(vec![json!({ "type": "text", "text": "hello" })]),
            short_options(),
        )
        .await?;
    require(response["accepted"] == true, "message response mismatch")?;

    let mut invalid = OpenAiMessageParams::user(vec![json!({ "type": "text", "text": "draft" })]);
    invalid.meta = Some(OpenAiMessageMetadata {
        openai_message: Some(OpenAiMessageOptions {
            target: OpenAiMessageTarget::Active,
            send: false,
        }),
    });
    let error = message
        .send(invalid, short_options())
        .await
        .expect_err("send:false should be rejected before posting");
    require(error.to_string().contains("send"), "send:false error did not name send")?;
    require(transport.pending_request_count() == 0, "send:false left a pending request")?;

    let cancellation = RequestCancellation::new();
    let cancellation_for_request = cancellation.clone();
    let (sender, receiver) = oneshot::channel();
    let pending_message = message.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = pending_message
            .send(
                OpenAiMessageParams::user(vec![json!({ "type": "text", "text": "cancel" })]),
                RequestOptions {
                    timeout_ms: Some(500),
                    cancellation: Some(cancellation_for_request),
                },
            )
            .await;
        let _ = sender.send(result);
    });
    wait_for_browser(0).await?;
    require(transport.pending_request_count() == 1, "pending request was not tracked")?;
    cancellation.cancel();
    let cancelled = receiver
        .await
        .map_err(|_| AppError::Transport("cancelled task dropped".into()))?
        .expect_err("cancelled request succeeded");
    match cancelled {
        AppError::Cancelled { method } if method == SEND_MESSAGE_METHOD => {}
        other => return Err(AppError::validation("cancel", format!("unexpected error: {other}"))),
    }
    wait_for_browser(0).await?;
    require(transport.pending_request_count() == 0, "cancelled request remained pending")
}

async fn prove_model_context(
    openai: &OpenAiAppExtensions<BrowserPostMessageTransport>,
) -> AppResult<()> {
    let model = openai.model_context().unwrap();
    require(
        model.current().and_then(|state| state).is_some_and(|state| state.update_id == "initial"),
        "initial model context missing",
    )?;
    let result = model
        .update(json!({ "content": [{ "type": "text", "text": "next" }] }), short_options())
        .await?
        .ok_or_else(|| AppError::validation("modelContext", "missing updateId"))?;
    require(result.update_id == "next-id", "model context updateId mismatch")
}

async fn prove_empty_blob_write(
    openai: &OpenAiAppExtensions<BrowserPostMessageTransport>,
) -> AppResult<()> {
    let result = openai
        .resources()
        .unwrap()
        .write(
            "file://empty",
            OpenAiResourceWriteOptions {
                if_match: None,
                content: OpenAiResourceWriteContent::Blob(String::new()),
            },
            short_options(),
        )
        .await?;
    require(
        matches!(result, incurs_openai_mcp_app::OpenAiResourceWriteResult::Saved { .. }),
        "empty blob write failed",
    )
}

fn require_stylesheet() -> AppResult<()> {
    let text = document_root()?
        .owner_document()
        .ok_or_else(|| AppError::Transport("missing document".into()))?
        .get_element_by_id("openai-mcp-app-styles")
        .ok_or_else(|| AppError::validation("styles", "missing style element"))?
        .text_content()
        .unwrap_or_default();
    require(text.contains(".cursor-interaction"), "stylesheet content missing")
}

fn require_cursor(expected: &'static str) -> AppResult<()> {
    let actual = document_root()?
        .style()
        .get_property_value("--cursor-interaction")
        .map_err(|error| AppError::Transport(format!("style read failed: {error:?}")))?;
    require(actual == expected, "cursor value mismatch")
}

fn document_root() -> AppResult<web_sys::HtmlElement> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .ok_or_else(|| AppError::Transport("missing document element".into()))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| AppError::Transport("document element is not an HtmlElement".into()))
}

fn short_options() -> RequestOptions {
    RequestOptions {
        timeout_ms: Some(500),
        ..RequestOptions::default()
    }
}

fn require(condition: bool, message: &'static str) -> AppResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AppError::validation("browser gate", message))
    }
}

async fn wait_for_browser(milliseconds: i32) -> AppResult<()> {
    let promise = Promise::new(&mut |resolve, _reject| {
        let _ = web_sys::window()
            .expect("window exists")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds);
    });
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|error| AppError::Transport(format!("timer failed: {error:?}")))
}
`;

await main();
