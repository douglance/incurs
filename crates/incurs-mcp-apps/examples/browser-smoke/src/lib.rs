//! Browser-executed smoke proof for the MCP Apps `postMessage` transport.

#![deny(missing_docs)]

use std::{cell::Cell, rc::Rc};

use futures::{FutureExt, channel::oneshot};
use incurs_mcp_apps::{
    AppError, AppResult, AppTransport, HOST_CONTEXT_CHANGED_METHOD, INITIALIZE_METHOD,
    INITIALIZED_NOTIFICATION_METHOD, InitializeParams, McpApp, RequestOptions,
    TOOL_INPUT_NOTIFICATION_METHOD, TOOL_RESULT_NOTIFICATION_METHOD,
    browser::BrowserPostMessageTransport,
};
use js_sys::Promise;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;

/// Runs a real browser `postMessage` lifecycle smoke test and returns a JSON summary string.
#[wasm_bindgen]
pub async fn run_smoke() -> Result<String, JsValue> {
    let summary = run_smoke_inner()
        .await
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    Ok(summary.to_string())
}

async fn run_smoke_inner() -> AppResult<Value> {
    run_initialize_and_notifications().await?;
    run_timeout().await?;
    run_dispose_pending().await?;
    run_wrong_source_filter().await?;
    Ok(json!({
        "initialize": true,
        "notifications": true,
        "timeout": true,
        "dispose": true,
        "sourceFilter": true
    }))
}

async fn run_initialize_and_notifications() -> AppResult<()> {
    let window = browser_window()?;
    let transport = BrowserPostMessageTransport::new_for_window(
        window.clone(),
        window.clone(),
        window.clone(),
    )?;
    transport.handle(
        INITIALIZE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            if params["appInfo"]["name"] != "browser-smoke" {
                return Err(AppError::validation("appInfo.name", "unexpected app name"));
            }
            Ok(json!({
                "protocolVersion": "2025-06-18",
                "hostInfo": { "name": "browser-host" },
                "hostCapabilities": { "experimental": { "openai/message": {} } },
                "hostContext": { "theme": "dark", "openai/interactionCursor": "pointer" },
                "toolInput": { "initial": true },
                "toolResult": { "status": "ready" }
            }))
        }),
    );
    let initialized = Rc::new(Cell::new(false));
    let initialized_clone = initialized.clone();
    let initialized_registration = transport.on(
        INITIALIZED_NOTIFICATION_METHOD,
        Rc::new(move |_| initialized_clone.set(true)),
    );
    let app = McpApp::new(transport.clone());
    app.initialize(
        InitializeParams::new("browser-smoke", "1.0.0", "2025-06-18"),
        RequestOptions {
            timeout_ms: Some(500),
        },
    )
    .await?;
    wait_for_browser(0).await?;
    require(
        initialized.get(),
        "initialized notification was not observed",
    )?;
    require(
        app.host_context()["theme"] == "dark",
        "initial host context missing",
    )?;

    transport.notify(
        HOST_CONTEXT_CHANGED_METHOD,
        json!({ "theme": "light", "extra": true }),
    )?;
    transport.notify(TOOL_INPUT_NOTIFICATION_METHOD, json!({ "changed": true }))?;
    transport.notify(TOOL_RESULT_NOTIFICATION_METHOD, json!({ "status": "done" }))?;
    wait_for_browser(0).await?;
    let snapshot = app.snapshot();
    require(
        snapshot.host_context["theme"] == "light",
        "host context did not merge",
    )?;
    require(
        snapshot.host_context["openai/interactionCursor"] == "pointer",
        "host context merge dropped existing keys",
    )?;
    require(
        snapshot.tool_input == Some(json!({ "changed": true })),
        "tool input not updated",
    )?;
    require(
        snapshot.tool_result == Some(json!({ "status": "done" })),
        "tool result not updated",
    )?;
    initialized_registration.dispose();
    transport.dispose();
    Ok(())
}

async fn run_timeout() -> AppResult<()> {
    let window = browser_window()?;
    let transport = BrowserPostMessageTransport::new_for_window(
        window.clone(),
        window.clone(),
        window.clone(),
    )?;
    transport.handle(
        "pending",
        Rc::new(|_| {
            async move { futures::future::pending::<incurs_mcp_apps::AppResult<Value>>().await }
                .boxed_local()
        }),
    );
    let error = transport
        .request(
            "pending",
            json!({}),
            RequestOptions {
                timeout_ms: Some(25),
                ..RequestOptions::default()
            },
        )
        .await
        .expect_err("pending request must time out");
    require(
        error
            == AppError::Timeout {
                method: "pending".to_string(),
            },
        "pending request returned the wrong error",
    )?;
    transport.dispose();
    Ok(())
}

async fn run_dispose_pending() -> AppResult<()> {
    let window = browser_window()?;
    let transport = BrowserPostMessageTransport::new_for_window(
        window.clone(),
        window.clone(),
        window.clone(),
    )?;
    transport.handle(
        "dispose-pending",
        Rc::new(|_| {
            async move { futures::future::pending::<incurs_mcp_apps::AppResult<Value>>().await }
                .boxed_local()
        }),
    );
    let (sender, receiver) = oneshot::channel();
    let request_transport = transport.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = request_transport
            .request(
                "dispose-pending",
                json!({}),
                RequestOptions {
                    timeout_ms: Some(500),
                    ..RequestOptions::default()
                },
            )
            .await;
        let _ = sender.send(result);
    });
    wait_for_browser(0).await?;
    transport.dispose();
    let error = receiver
        .await
        .map_err(|_| AppError::Transport("dispose request task dropped".into()))?
        .expect_err("disposed pending request must reject");
    require(
        error == AppError::Disposed,
        "pending request was not rejected on dispose",
    )
}

async fn run_wrong_source_filter() -> AppResult<()> {
    let window = browser_window()?;
    let document = window
        .document()
        .ok_or_else(|| AppError::Transport("missing document".into()))?;
    let iframe = document
        .create_element("iframe")
        .map_err(|error| AppError::Transport(format!("iframe create failed: {error:?}")))?
        .dyn_into::<web_sys::HtmlIFrameElement>()
        .map_err(|_| AppError::Transport("iframe cast failed".into()))?;
    document
        .body()
        .ok_or_else(|| AppError::Transport("missing document body".into()))?
        .append_child(&iframe)
        .map_err(|error| AppError::Transport(format!("iframe append failed: {error:?}")))?;
    let wrong_source = iframe
        .content_window()
        .ok_or_else(|| AppError::Transport("missing iframe window".into()))?;
    let transport =
        BrowserPostMessageTransport::new_for_window(window.clone(), window.clone(), wrong_source)?;
    transport.handle(
        "echo",
        incurs_mcp_apps::value_handler(|_| Ok(json!({ "ignored": false }))),
    );
    let error = transport
        .request(
            "echo",
            json!({}),
            RequestOptions {
                timeout_ms: Some(25),
                ..RequestOptions::default()
            },
        )
        .await
        .expect_err("wrong-source request should be ignored until timeout");
    require(
        error
            == AppError::Timeout {
                method: "echo".to_string(),
            },
        "wrong-source filter did not ignore the message",
    )?;
    transport.dispose();
    iframe.remove();
    Ok(())
}

fn browser_window() -> AppResult<web_sys::Window> {
    web_sys::window().ok_or_else(|| AppError::Transport("missing window".into()))
}

fn require(condition: bool, message: &'static str) -> AppResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AppError::validation("browser smoke", message))
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
