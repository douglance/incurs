#![cfg(target_arch = "wasm32")]

use std::{cell::Cell, rc::Rc};

use futures::{FutureExt, channel::oneshot};
use incurs_mcp_apps::{
    AppError, AppTransport, HOST_CONTEXT_CHANGED_METHOD, INITIALIZE_METHOD,
    INITIALIZED_NOTIFICATION_METHOD, InitializeParams, McpApp, RequestOptions,
    TOOL_INPUT_NOTIFICATION_METHOD, TOOL_RESULT_NOTIFICATION_METHOD,
    browser::BrowserPostMessageTransport,
};
use js_sys::Promise;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test(async)]
async fn browser_post_message_correlates_initialize_and_standard_notifications() {
    let window = web_sys::window().expect("window exists");
    let transport =
        BrowserPostMessageTransport::new_for_window(window.clone(), window.clone(), window.clone())
            .expect("transport installs");
    transport.handle(
        INITIALIZE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            assert_eq!(params["appInfo"]["name"], "browser-demo");
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
    let context_changes = Rc::new(Cell::new(0));
    let context_changes_clone = context_changes.clone();
    let context_registration = app.add_host_context_listener(move |_| {
        context_changes_clone.set(context_changes_clone.get() + 1);
    });

    app.initialize(
        InitializeParams::new("browser-demo", "1.0.0", "2025-06-18"),
        RequestOptions {
            timeout_ms: Some(500),
        },
    )
    .await
    .expect("initialize resolves through postMessage response correlation");
    wait_for_browser(0).await;
    assert!(initialized.get());
    assert_eq!(app.host_context()["theme"], "dark");

    transport
        .notify(
            HOST_CONTEXT_CHANGED_METHOD,
            json!({ "theme": "light", "extra": true }),
        )
        .expect("host context notification posts");
    transport
        .notify(TOOL_INPUT_NOTIFICATION_METHOD, json!({ "changed": true }))
        .expect("tool input notification posts");
    transport
        .notify(TOOL_RESULT_NOTIFICATION_METHOD, json!({ "status": "done" }))
        .expect("tool result notification posts");
    wait_for_browser(0).await;

    let snapshot = app.snapshot();
    assert_eq!(snapshot.host_context["theme"], "light");
    assert_eq!(snapshot.host_context["openai/interactionCursor"], "pointer");
    assert_eq!(snapshot.host_context["extra"], true);
    assert_eq!(snapshot.tool_input, Some(json!({ "changed": true })));
    assert_eq!(snapshot.tool_result, Some(json!({ "status": "done" })));

    let changes_before_dispose = context_changes.get();
    context_registration.dispose();
    transport
        .notify(HOST_CONTEXT_CHANGED_METHOD, json!({ "theme": "dark" }))
        .expect("host context notification posts after listener cleanup");
    wait_for_browser(0).await;
    assert_eq!(context_changes.get(), changes_before_dispose);

    initialized_registration.dispose();
    transport.dispose();
}

#[wasm_bindgen_test(async)]
async fn browser_post_message_times_out_disposes_pending_and_filters_wrong_sources() {
    let window = web_sys::window().expect("window exists");

    let timeout_transport =
        BrowserPostMessageTransport::new_for_window(window.clone(), window.clone(), window.clone())
            .expect("transport installs");
    timeout_transport.handle(
        "pending",
        Rc::new(|_| {
            async move { futures::future::pending::<incurs_mcp_apps::AppResult<Value>>().await }
                .boxed_local()
        }),
    );
    let timeout_error = timeout_transport
        .request(
            "pending",
            json!({}),
            RequestOptions {
                timeout_ms: Some(25),
                ..RequestOptions::default()
            },
        )
        .await
        .expect_err("pending request times out");
    assert_eq!(
        timeout_error,
        AppError::Timeout {
            method: "pending".to_string()
        }
    );
    timeout_transport.dispose();

    let dispose_transport =
        BrowserPostMessageTransport::new_for_window(window.clone(), window.clone(), window.clone())
            .expect("transport installs");
    dispose_transport.handle(
        "dispose-pending",
        Rc::new(|_| {
            async move { futures::future::pending::<incurs_mcp_apps::AppResult<Value>>().await }
                .boxed_local()
        }),
    );
    let (sender, receiver) = oneshot::channel();
    let dispose_request_transport = dispose_transport.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = dispose_request_transport
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
    wait_for_browser(0).await;
    dispose_transport.dispose();
    let dispose_error = receiver
        .await
        .expect("request task returns")
        .expect_err("dispose rejects pending request");
    assert_eq!(dispose_error, AppError::Disposed);

    let document = window.document().expect("document exists");
    let iframe = document
        .create_element("iframe")
        .expect("iframe element")
        .dyn_into::<web_sys::HtmlIFrameElement>()
        .expect("iframe type");
    document
        .body()
        .expect("document body")
        .append_child(&iframe)
        .expect("append iframe");
    let wrong_source = iframe.content_window().expect("iframe content window");
    let wrong_source_transport =
        BrowserPostMessageTransport::new_for_window(window.clone(), window.clone(), wrong_source)
            .expect("transport installs");
    wrong_source_transport.handle(
        "echo",
        incurs_mcp_apps::value_handler(|_| Ok(json!({ "ignored": false }))),
    );
    let wrong_source_error = wrong_source_transport
        .request(
            "echo",
            json!({}),
            RequestOptions {
                timeout_ms: Some(25),
                ..RequestOptions::default()
            },
        )
        .await
        .expect_err("messages from a different source are ignored until timeout");
    assert_eq!(
        wrong_source_error,
        AppError::Timeout {
            method: "echo".to_string()
        }
    );
    wrong_source_transport.dispose();
    iframe.remove();
}

async fn wait_for_browser(milliseconds: i32) {
    let promise = Promise::new(&mut |resolve, _reject| {
        web_sys::window()
            .expect("window exists")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds)
            .expect("timer installs");
    });
    let _ = JsFuture::from(promise).await.expect("timer resolves");
}
