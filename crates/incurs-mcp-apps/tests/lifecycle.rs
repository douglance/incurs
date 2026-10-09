use std::{cell::Cell, rc::Rc};

use incurs_mcp_apps::{
    AppError, AppTransport, HOST_CONTEXT_CHANGED_METHOD, HostCapabilities, INITIALIZE_METHOD,
    INITIALIZED_NOTIFICATION_METHOD, InMemoryTransport, McpApp, RequestOptions,
    SEND_MESSAGE_METHOD, TOOL_INPUT_NOTIFICATION_METHOD, TOOL_RESULT_NOTIFICATION_METHOD,
};
use serde_json::{Map, Value, json};

#[tokio::test]
async fn lifecycle_keeps_host_state_and_correlates_requests() {
    let transport = InMemoryTransport::new();
    transport.handle(
        SEND_MESSAGE_METHOD,
        incurs_mcp_apps::value_handler(|params| Ok(json!({ "echo": params }))),
    );
    let app = McpApp::new(transport.clone());
    let mut experimental = Map::new();
    experimental.insert("openai/message".to_string(), json!({}));
    app.connect(
        HostCapabilities { experimental },
        json!({ "theme": "dark" }),
        Some(json!({ "file": { "name": "a.txt", "resourceUri": "file://a" } })),
        Some(json!({ "ok": true })),
    );

    let changed = Rc::new(Cell::new(false));
    let changed_clone = changed.clone();
    let registration = app.add_host_context_listener(move |context| {
        changed_clone.set(context["theme"] == "light");
    });
    app.replace_host_context(json!({ "theme": "light" }));
    registration.dispose();

    let result: Value = app
        .send_message(
            json!({ "role": "user" }),
            RequestOptions {
                timeout_ms: Some(42),
                ..RequestOptions::default()
            },
        )
        .await
        .expect("message request succeeds");

    assert!(changed.get());
    assert_eq!(result["echo"]["role"], "user");
    let requests = transport.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].id, 1);
    assert_eq!(requests[0].method, "ui/message");
    assert_eq!(requests[0].timeout.unwrap().as_millis(), 42);
}

#[tokio::test]
async fn initialize_uses_ui_wire_methods_and_standard_notifications_update_state() {
    let transport = InMemoryTransport::new();
    transport.handle(
        INITIALIZE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            assert_eq!(params["appInfo"]["name"], "demo");
            Ok(json!({
                "protocolVersion": "2025-06-18",
                "hostInfo": { "name": "host" },
                "hostCapabilities": { "experimental": { "openai/message": {} } },
                "hostContext": { "theme": "dark", "count": 1 },
                "toolInput": { "initial": true },
                "toolResult": { "status": "ready" }
            }))
        }),
    );
    let app = McpApp::new(transport.clone());
    app.initialize(
        incurs_mcp_apps::InitializeParams::new("demo", "1.0.0", "2025-06-18"),
        RequestOptions::default(),
    )
    .await
    .expect("initialize succeeds");

    transport.emit(
        HOST_CONTEXT_CHANGED_METHOD,
        json!({ "count": 2, "extra": true }),
    );
    transport.emit(TOOL_INPUT_NOTIFICATION_METHOD, json!({ "changed": true }));
    transport.emit(TOOL_RESULT_NOTIFICATION_METHOD, json!({ "status": "done" }));

    let snapshot = app.snapshot();
    assert_eq!(snapshot.host_context["theme"], "dark");
    assert_eq!(snapshot.host_context["count"], 2);
    assert_eq!(snapshot.host_context["extra"], true);
    assert_eq!(snapshot.tool_input, Some(json!({ "changed": true })));
    assert_eq!(snapshot.tool_result, Some(json!({ "status": "done" })));

    let requests = transport.recorded_requests();
    assert_eq!(requests[0].method, "ui/initialize");
    let notifications = transport.recorded_notifications();
    assert_eq!(notifications[0].method, INITIALIZED_NOTIFICATION_METHOD);
}

#[tokio::test]
async fn dispose_rejects_later_work() {
    let transport = InMemoryTransport::new();
    let app = McpApp::new(transport);
    app.dispose();

    let error = app
        .request::<Value>("anything", json!({}), RequestOptions::default())
        .await
        .expect_err("disposed transport rejects requests");
    assert_eq!(error, AppError::Disposed);
}
