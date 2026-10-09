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
  await writeFile(join(consumerPath, 'app.html'), appHtml);
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
<title>Independent OpenAI MCP host proof</title>
<h1>Rust MCP App runtime proof</h1>
<iframe id="evil" srcdoc="<script>addEventListener('message',e=>{parent.document.getElementById('app').contentWindow.postMessage(e.data,'*')})</script>" hidden></iframe>
<iframe id="app" src="/app.html" title="Compiled Rust WASM guest"></iframe>
<pre id="proof-status">Running independent host checks...</pre>
<script>
const guest = document.getElementById('app');
window.__hostMessages = [];
window.__hostErrors = [];
window.__forgedResponses = 0;
function requireHost(ok, text) { if (!ok) throw new Error(text); }
function send(message) { guest.contentWindow.postMessage(message, '*'); }
function notify(method, params) { send({jsonrpc:'2.0', method, params}); }
addEventListener('message', async event => {
  if (event.source !== guest.contentWindow) return;
  const message = event.data;
  try {
    requireHost(message && Object.getPrototypeOf(message) === Object.prototype,
      'JSON-RPC payload must be a plain object, received ' + Object.prototype.toString.call(message));
    requireHost(message.jsonrpc === '2.0', 'missing JSON-RPC version');
    window.__hostMessages.push(message);
    const {method, params = {}, id} = message;
    if (id === undefined) {
      requireHost(method === 'ui/notifications/initialized' || method === 'notifications/cancelled',
        'unexpected notification: ' + method);
      return;
    }
    let result = {};
    switch (method) {
      case 'ui/initialize':
        requireHost(params.appInfo.name === 'openai-browser-gate', 'wrong app name');
        document.getElementById('evil').contentWindow.postMessage({
          jsonrpc:'2.0', id, result:{protocolVersion:'spoof', hostInfo:{name:'evil'}, hostCapabilities:{}, hostContext:{}}
        }, '*');
        window.__forgedResponses += 1;
        send({jsonrpc:'2.0', id:'unrelated-id', result:{protocolVersion:'spoof'}});
        await new Promise(resolve => setTimeout(resolve, 25));
        result = {
          protocolVersion:'2025-06-18', hostInfo:{name:'independent-javascript-host', version:'1'},
          hostCapabilities:{experimental:{
            'openai/files':{}, 'openai/message':{}, 'openai/modelContext':{}, 'openai/resource':{}
          }},
          hostContext:{
            'openai/deepLink':{url:'/browser-gate'},
            'openai/modelContext':{content:[],updateId:'initial'},
            'openai/interactionCursor':'default'
          }
        };
        break;
      case 'proof/cursor':
        notify('ui/notifications/host-context-changed', {
          'openai/interactionCursor':'pointer', 'openai/deepLink':{url:'/updated'}
        });
        break;
      case 'openai/files/open':
        requireHost(params.path === '/projects/part.step', 'file path wire mismatch');
        result = {_meta:{trace:'file-open'}};
        break;
      case 'ui/message':
        requireHost(params.role === 'user', 'wrong role');
        requireHost(params._meta['openai/message'].target === 'active', 'missing active target');
        requireHost(params._meta['openai/message'].send === true, 'missing send true');
        if (params.content[0].text === 'cancel' || params.content[0].text === 'dispose') return;
        requireHost(params.content[0].text === 'hello', 'invalid message reached host');
        result = {accepted:true};
        break;
      case 'ui/update-model-context':
        requireHost(params.content[0].text === 'next', 'model context wire mismatch');
        result = {_meta:{'openai/modelContext':{updateId:'next-id'}}};
        break;
      case 'resources/read':
        requireHost(params.uri === 'file://a', 'resource read URI mismatch');
        requireHost(params._meta['openai/resource'].representation === 'text', 'representation override lost');
        requireHost(params.representation === undefined, 'convenience representation leaked');
        result = {contents:[{uri:'file://a', text:'hello', mimeType:'text/plain',
          _meta:{'openai/resource':{etag:'e1',writable:true}}}],_meta:{trace:'resource-read'},hostField:'retained'};
        break;
      case 'resources/subscribe':
        requireHost(params.uri === 'file://watched', 'subscribe URI mismatch');
        notify('notifications/resources/updated', {uri:'file://watched',_meta:{trace:'resource-update'}});
        break;
      case 'resources/unsubscribe':
        requireHost(params.uri === 'file://watched', 'unsubscribe URI mismatch');
        notify('notifications/resources/updated', {uri:'file://ignored'});
        break;
      case 'openai/resources/write':
        if (params.uri === 'file://empty') {
          requireHost(params.blob === '' && params.text === undefined, 'empty blob wire mismatch');
          result = {outcome:'saved',etag:'empty'};
        } else if (params.uri === 'file://a') {
          requireHost(params.text === 'updated' && params.ifMatch === 'e1' && params.blob === undefined,
            'text/ETag wire mismatch');
          result = {outcome:'saved',etag:'e2'};
        } else if (params.uri === 'file://conflict') {
          result = {outcome:'conflict',etag:'current'};
        } else if (params.uri === 'file://large') {
          result = {outcome:'too-large',maxBytes:1};
        } else if (params.uri === 'file://invalid-response') {
          result = {outcome:'saved',etag:123};
        } else throw new Error('invalid write reached host: ' + params.uri);
        break;
      case 'proof/never': return;
      default: throw new Error('unexpected wire method: ' + method);
    }
    send({jsonrpc:'2.0',id,result});
  } catch (error) {
    window.__hostErrors.push(String(error));
    if (message?.id !== undefined) send({jsonrpc:'2.0',id:message.id,error:{code:-32602,message:String(error)}});
  }
});
</script>`;

const appHtml = String.raw`<!doctype html>
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
    await page.waitForFunction(() => typeof document.getElementById('app').contentWindow.__runSmoke === 'function');
    let summary;
    try {
      summary = JSON.parse(await page.evaluate(() => document.getElementById('app').contentWindow.__runSmoke()));
    } catch (error) {
      const errors = await page.evaluate(() => window.__hostErrors);
      throw new Error(String(error) + '\nIndependent host errors: ' + JSON.stringify(errors));
    }
    const host = await page.evaluate(() => ({
      errors: window.__hostErrors, forgedResponses: window.__forgedResponses,
      messages: window.__hostMessages,
    }));
    if (host.errors.length) throw new Error(JSON.stringify(host.errors));
    const counts = {};
    for (const message of host.messages) counts[message.method] = (counts[message.method] ?? 0) + 1;
    for (const [method, expected] of Object.entries({
      'ui/initialize':1, 'ui/notifications/initialized':1, 'proof/cursor':1,
      'openai/files/open':1, 'ui/message':3, 'ui/update-model-context':1,
      'resources/read':1, 'resources/subscribe':1, 'resources/unsubscribe':1,
      'openai/resources/write':5, 'proof/never':1,
    })) {
      if (counts[method] !== expected) throw new Error(JSON.stringify({method,expected,actual:counts[method]}));
    }
    if (host.forgedResponses !== 1) throw new Error('source filter control was not sent');
    if (host.messages.some(message => message.params?.content?.[0]?.text === 'draft')) throw new Error('invalid draft reached host');
    const evidence = { ...summary, wireCounts:counts, rejectedSourceResponses:host.forgedResponses };
    await page.evaluate(value => { document.getElementById('proof-status').textContent = JSON.stringify(value,null,2); }, evidence);
    if (process.env.INCURS_OPENAI_BROWSER_SCREENSHOT) {
      await page.screenshot({path:process.env.INCURS_OPENAI_BROWSER_SCREENSHOT,fullPage:true});
    }
    console.log(JSON.stringify(evidence));

} finally {
  await browser.close();
  server.close();
}
`;

const rustProofSource = String.raw`//! Browser proof for OpenAI MCP App helpers.

#![deny(missing_docs)]

use std::{cell::Cell, rc::Rc};

use futures::channel::oneshot;
use incurs_mcp_apps::{
    AppError, AppResult, AppTransport, InitializeParams, McpApp, RequestCancellation, RequestOptions,
    browser::BrowserPostMessageTransport,
};
use incurs_openai_mcp_app::{
    OpenAiAppExtensions, OpenAiMessageMetadata, OpenAiMessageOptions, OpenAiMessageParams,
    OpenAiMessageTarget, OpenAiResourceReadParams, OpenAiResourceReadMetadata,
    OpenAiResourceReadPreference, OpenAiResourceRepresentation, OpenAiResourceWriteContent,
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
    let transport = BrowserPostMessageTransport::new()?;
    let app = McpApp::new(transport.clone());
    let openai = OpenAiAppExtensions::new(app.clone());
    require(openai.files().is_none(), "files enabled before initialize")?;
    require(openai.message().is_none(), "message enabled before initialize")?;
    app.initialize(
        InitializeParams::new("openai-browser-gate", "1.0.0", "2025-06-18"),
        short_options(),
    ).await?;
    require(openai.files().is_some(), "files helper missing")?;
    require(openai.message().is_some(), "message helper missing")?;
    require(openai.model_context().is_some(), "model context helper missing")?;
    require(openai.resources().is_some(), "resources helper missing")?;
    require(openai.deep_link().current().is_some_and(|link| link.url == "/browser-gate"), "deep link mismatch")?;
    let cursor = openai.install_interaction_cursor_style()?;
    require_stylesheet()?;
    require_cursor("default")?;
    transport.request("proof/cursor", json!({}), short_options()).await?;
    require_cursor("pointer")?;
    require(openai.deep_link().current().is_some_and(|link| link.url == "/updated"), "deep link update missing")?;

    let files = openai.files().unwrap();
    let opened = files.open("/projects/part.step").await?;
    require(serde_json::to_value(opened).unwrap()["_meta"]["trace"] == "file-open", "file metadata lost")?;
    require(files.open("").await.is_err(), "empty file path reached transport")?;
    prove_message_defaults_send_false_and_cancel(&transport, &openai).await?;
    prove_model_context(&openai).await?;
    prove_empty_blob_write(&openai).await?;
    prove_resources(&openai).await?;

    let timeout = transport.request("proof/never", json!({}), RequestOptions {
        timeout_ms: Some(25), ..RequestOptions::default()
    }).await.expect_err("unanswered request must time out");
    require(matches!(timeout, AppError::Timeout { .. }), "wrong timeout error")?;
    require(transport.pending_request_count() == 0, "timeout left pending request")?;

    let (sender, receiver) = oneshot::channel();
    let message = openai.message().unwrap();
    wasm_bindgen_futures::spawn_local(async move {
        let result = message.send(
            OpenAiMessageParams::user(vec![json!({"type":"text","text":"dispose"})]),
            short_options(),
        ).await;
        let _ = sender.send(result);
    });
    wait_for_browser(0).await?;
    require(transport.pending_request_count() == 1, "dispose request not pending")?;
    cursor.dispose();
    transport.dispose();
    let disposed = receiver.await.map_err(|_| AppError::Transport("dispose task dropped".into()))?
        .expect_err("dispose must reject pending work");
    require(matches!(disposed, AppError::Disposed), "wrong dispose error")?;
    require(transport.pending_request_count() == 0, "dispose left pending request")?;
    require(files.open("/after-dispose").await.is_err(), "disposed transport accepted work")?;
    Ok(json!({
        "browserGate": true, "independentHost": true, "parentWindow": true,
        "uiHandshake": true, "sourceAndIdFiltering": true, "files": true,
        "deepLinkUpdates": true, "messageDefaults": true, "sendFalseRejectedBeforePost": true,
        "requestCancellation": true, "requestTimeout": true, "disposePending": true,
        "modelContext": true, "resources": true, "resourceNotifications": true,
        "emptyBlob": true, "cursorStyle": true, "metadataPreservation": true
    }))
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
        AppError::Cancelled { method } if method == "ui/message" => {}
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

async fn prove_resources(
    openai: &OpenAiAppExtensions<BrowserPostMessageTransport>,
) -> AppResult<()> {
    let resources = openai.resources().unwrap();
    let read = resources.read(OpenAiResourceReadParams {
        uri: "file://a".into(),
        meta: Some(OpenAiResourceReadMetadata { openai_resource: Some(OpenAiResourceReadPreference {
            representation: Some(OpenAiResourceRepresentation::Blob),
        }), ..OpenAiResourceReadMetadata::default() }),
        representation: Some(OpenAiResourceRepresentation::Text),
    }, short_options()).await?;
    require(read.contents[0].text.as_deref() == Some("hello"), "resource text mismatch")?;
    let value = serde_json::to_value(&read).unwrap();
    require(value["_meta"]["trace"] == "resource-read" && value["hostField"] == "retained", "read result fields lost")?;
    require(read.contents[0].openai_metadata.as_ref().is_some_and(|meta| meta.etag.as_deref() == Some("e1") && meta.writable == Some(true)), "resource metadata mismatch")?;
    let seen = Rc::new(Cell::new(0));
    let seen_handler = seen.clone();
    let registration = resources.add_update_handler(move |notification| {
        if notification.method == "notifications/resources/updated" && notification.params.uri == "file://watched" && notification.params.meta.as_ref().is_some_and(|meta| meta.get("trace") == Some(&json!("resource-update"))) {
            seen_handler.set(seen_handler.get() + 1);
        } else {
            seen_handler.set(100);
        }
    });
    resources.subscribe("file://watched", short_options()).await?;
    require(seen.get() == 1, "resource update callback missing")?;
    registration.dispose();
    resources.unsubscribe("file://watched", short_options()).await?;
    require(seen.get() == 1, "removed resource handler called")?;

    for (uri, expected, content, if_match) in [
        ("file://a", "saved", "updated", Some("e1".to_string())),
        ("file://conflict", "conflict", "stale", Some("old".to_string())),
        ("file://large", "too-large", "large", None),
    ] {
        let result = resources.write(uri, OpenAiResourceWriteOptions {
            if_match, content: OpenAiResourceWriteContent::Text(content.into()),
        }, short_options()).await?;
        require(serde_json::to_value(result).unwrap()["outcome"] == expected, "write outcome mismatch")?;
    }
    require(resources.write("file://invalid-response", OpenAiResourceWriteOptions {
        if_match: None, content: OpenAiResourceWriteContent::Text("bad".into()),
    }, short_options()).await.is_err(), "invalid write response accepted")?;
    require(resources.write("file://invalid-blob", OpenAiResourceWriteOptions {
        if_match: None, content: OpenAiResourceWriteContent::Blob("!".into()),
    }, short_options()).await.is_err(), "invalid blob reached host")?;
    Ok(())
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
        timeout_ms: Some(2000),
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
