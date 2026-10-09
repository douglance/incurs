use std::{cell::RefCell, rc::Rc};

use incurs_mcp_apps::{AppTransport, HostCapabilities, InMemoryTransport, McpApp, RequestOptions};
use incurs_openai_mcp_app::{
    EmptyResult, OPENAI_FILE_OPEN_METHOD, OPENAI_FILES_CAPABILITY_KEY, OPENAI_MESSAGE_KEY,
    OPENAI_MODEL_CONTEXT_KEY, OPENAI_RESOURCE_METADATA_KEY, OpenAiAppExtensions,
    OpenAiMessageMetadata, OpenAiMessageOptions, OpenAiMessageParams, OpenAiMessageTarget,
    OpenAiResourceReadParams, OpenAiResourceRepresentation, OpenAiResourceWriteContent,
    OpenAiResourceWriteOptions,
};
use serde_json::{Map, Value, json};

fn app_with_caps(keys: &[&str]) -> (McpApp<InMemoryTransport>, InMemoryTransport) {
    let mut experimental = Map::new();
    for key in keys {
        experimental.insert((*key).to_string(), json!({}));
    }
    app_with_experimental(experimental)
}

fn app_with_experimental(
    experimental: Map<String, Value>,
) -> (McpApp<InMemoryTransport>, InMemoryTransport) {
    let transport = InMemoryTransport::new();
    let app = McpApp::new(transport.clone());
    app.connect(
        HostCapabilities { experimental },
        json!({
            "openai/deepLink": { "path": ["thread", "hello world"], "query": [["q", "a+b"]] },
            "openai/modelContext": { "content": [{"type":"text","text":"hi"}], "updateId": "u1" }
        }),
        None,
        None,
    );
    (app, transport)
}

#[test]
fn helpers_are_capability_gated_and_deep_links_normalize_legacy_payloads() {
    let (app, _) = app_with_caps(&[OPENAI_MESSAGE_KEY]);
    let openai = OpenAiAppExtensions::new(app);

    assert!(openai.files().is_none());
    assert!(openai.message().is_some());
    assert!(openai.model_context().is_none());
    assert!(openai.resources().is_none());
    assert_eq!(
        openai.deep_link().current().unwrap().url,
        "/thread/hello%20world?q=a%2Bb"
    );
}

#[test]
fn capability_predicates_match_pinned_upstream_helpers() {
    let cases = [
        (None, false, false),
        (Some(Value::Null), false, false),
        (Some(json!(false)), false, true),
        (Some(json!(0)), false, true),
        (Some(json!("")), false, true),
        (Some(json!({})), true, true),
    ];

    for (capability, files_enabled, nullable_enabled) in cases {
        let mut experimental = Map::new();
        if let Some(capability) = capability {
            for key in [
                OPENAI_FILES_CAPABILITY_KEY,
                OPENAI_MESSAGE_KEY,
                OPENAI_MODEL_CONTEXT_KEY,
                OPENAI_RESOURCE_METADATA_KEY,
            ] {
                experimental.insert(key.to_string(), capability.clone());
            }
        }
        let (app, _) = app_with_experimental(experimental);
        let openai = OpenAiAppExtensions::new(app);

        assert_eq!(openai.files().is_some(), files_enabled);
        assert_eq!(openai.message().is_some(), nullable_enabled);
        assert_eq!(openai.model_context().is_some(), nullable_enabled);
        assert_eq!(openai.resources().is_some(), nullable_enabled);
    }
}

#[tokio::test]
async fn outbound_file_and_message_validation_rejects_before_transport() {
    let (app, transport) = app_with_caps(&[OPENAI_FILES_CAPABILITY_KEY, OPENAI_MESSAGE_KEY]);
    transport.handle(
        OPENAI_FILE_OPEN_METHOD,
        incurs_mcp_apps::value_handler(|_| Ok(json!({}))),
    );
    transport.handle(
        incurs_mcp_apps::SEND_MESSAGE_METHOD,
        incurs_mcp_apps::value_handler(|_| Ok(json!({}))),
    );
    let openai = OpenAiAppExtensions::new(app);

    let file_error = openai.files().unwrap().open("").await.unwrap_err();
    assert!(file_error.to_string().contains("path"));

    let content_error = openai
        .message()
        .unwrap()
        .send(
            OpenAiMessageParams::user(vec![json!({ "type": "unsupported" })]),
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(content_error.to_string().contains("content[0].type"));

    let mut send_false = OpenAiMessageParams::user(vec![json!({ "type": "text", "text": "hi" })]);
    send_false.meta = Some(OpenAiMessageMetadata {
        openai_message: Some(OpenAiMessageOptions {
            target: OpenAiMessageTarget::Active,
            send: false,
        }),
    });
    let send_error = openai
        .message()
        .unwrap()
        .send(send_false, RequestOptions::default())
        .await
        .unwrap_err();
    assert!(send_error.to_string().contains("send"));

    assert!(transport.recorded_requests().is_empty());
}

#[tokio::test]
async fn file_and_message_helpers_preserve_openai_wire_methods_and_defaults() {
    let (app, transport) = app_with_caps(&["openai/files", OPENAI_MESSAGE_KEY]);
    transport.handle(
        OPENAI_FILE_OPEN_METHOD,
        incurs_mcp_apps::value_handler(|_| Ok(json!({}))),
    );
    transport.handle(
        incurs_mcp_apps::SEND_MESSAGE_METHOD,
        incurs_mcp_apps::value_handler(|params| Ok(json!({ "params": params }))),
    );
    let openai = OpenAiAppExtensions::new(app);

    let opened: EmptyResult = openai.files().unwrap().open("/tmp/a.txt").await.unwrap();
    assert_eq!(opened, EmptyResult {});
    let sent = openai
        .message()
        .unwrap()
        .send(
            OpenAiMessageParams::user(vec![json!({ "type": "text", "text": "hi" })]),
            RequestOptions::default(),
        )
        .await
        .unwrap();

    let requests = transport.recorded_requests();
    assert_eq!(requests[0].method, "openai/files/open");
    assert_eq!(requests[0].params, json!({ "path": "/tmp/a.txt" }));
    assert_eq!(requests[1].method, "ui/message");
    assert_eq!(
        sent["params"]["_meta"][OPENAI_MESSAGE_KEY]["target"],
        "active"
    );
    assert_eq!(sent["params"]["_meta"][OPENAI_MESSAGE_KEY]["send"], true);
}

#[tokio::test]
async fn model_context_update_ids_clear_state_and_remount_state_are_preserved() {
    let (app, transport) = app_with_caps(&[OPENAI_MODEL_CONTEXT_KEY]);
    transport.handle(
        incurs_mcp_apps::MODEL_CONTEXT_UPDATE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            assert_eq!(params["content"][0]["text"], "next");
            Ok(json!({ "_meta": { "openai/modelContext": { "updateId": "u2" } } }))
        }),
    );
    let openai = OpenAiAppExtensions::new(app.clone());
    let model_context = openai.model_context().unwrap();

    assert_eq!(
        model_context.current().unwrap().unwrap().update_id,
        "u1".to_string()
    );
    app.replace_host_context(json!({ "openai/modelContext": null }));
    assert_eq!(model_context.current(), Some(None));
    app.replace_host_context(json!({
        "openai/modelContext": { "content": [], "updateId": "remounted" }
    }));
    assert_eq!(
        model_context.current().unwrap().unwrap().update_id,
        "remounted".to_string()
    );

    let result = model_context
        .update(
            json!({ "content": [{ "type": "text", "text": "next" }] }),
            RequestOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.update_id, "u2");
    assert_eq!(
        transport.recorded_requests()[0].method,
        "ui/update-model-context"
    );
}

#[tokio::test]
async fn resources_merge_representation_parse_metadata_write_and_unsubscribe_handlers() {
    let (app, transport) = app_with_caps(&[OPENAI_RESOURCE_METADATA_KEY, OPENAI_MODEL_CONTEXT_KEY]);
    transport.handle(
        incurs_mcp_apps::RESOURCE_READ_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            assert_eq!(
                params["_meta"][OPENAI_RESOURCE_METADATA_KEY]["representation"],
                "text"
            );
            Ok(json!({ "contents": [{ "uri": "file://a", "text": "hello", "_meta": { "openai/resource": { "etag": "e1", "writable": true } } }] }))
        }),
    );
    transport.handle(
        incurs_openai_mcp_app::OPENAI_MCP_APP_RESOURCE_WRITE_METHOD,
        incurs_mcp_apps::value_handler(|params| {
            assert_eq!(params["ifMatch"], "e1");
            assert_eq!(params["text"], "hello");
            Ok(json!({ "outcome": "saved", "etag": "e2" }))
        }),
    );
    transport.handle(
        "resources/subscribe",
        incurs_mcp_apps::value_handler(|_| Ok(json!({}))),
    );
    transport.handle(
        "resources/unsubscribe",
        incurs_mcp_apps::value_handler(|_| Ok(json!({}))),
    );
    let openai = OpenAiAppExtensions::new(app);
    let resources = openai.resources().unwrap();

    let read = resources
        .read(
            OpenAiResourceReadParams {
                uri: "file://a".into(),
                meta: None,
                representation: Some(OpenAiResourceRepresentation::Text),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        read.contents[0]
            .openai_metadata
            .as_ref()
            .unwrap()
            .etag
            .as_deref(),
        Some("e1")
    );
    let write = resources
        .write(
            "file://a",
            OpenAiResourceWriteOptions {
                if_match: Some("e1".into()),
                content: OpenAiResourceWriteContent::Text("hello".into()),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(write).unwrap(),
        json!({ "outcome": "saved", "etag": "e2" })
    );
    resources
        .subscribe("file://a", RequestOptions::default())
        .await
        .unwrap();
    resources
        .unsubscribe("file://a", RequestOptions::default())
        .await
        .unwrap();

    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let seen_clone = seen.clone();
    let registration = resources.add_update_handler(move |notification| {
        seen_clone.borrow_mut().push(notification.params.uri);
    });
    transport.emit(
        "notifications/resources/updated",
        json!({ "uri": "file://a" }),
    );
    registration.dispose();
    transport.emit(
        "notifications/resources/updated",
        json!({ "uri": "file://b" }),
    );
    assert_eq!(&*seen.borrow(), &["file://a".to_string()]);
}

#[tokio::test]
async fn resource_write_validation_rejects_invalid_blob_before_transport() {
    let (app, transport) = app_with_caps(&[OPENAI_RESOURCE_METADATA_KEY]);
    transport.handle(
        incurs_openai_mcp_app::OPENAI_MCP_APP_RESOURCE_WRITE_METHOD,
        incurs_mcp_apps::value_handler(|_| Ok(json!({ "outcome": "saved", "etag": "e2" }))),
    );
    let error = OpenAiAppExtensions::new(app)
        .resources()
        .unwrap()
        .write(
            "file://a",
            OpenAiResourceWriteOptions {
                if_match: None,
                content: OpenAiResourceWriteContent::Blob("not base64".into()),
            },
            RequestOptions::default(),
        )
        .await
        .expect_err("invalid base64 is rejected");
    assert!(error.to_string().contains("base64"));
    assert!(transport.recorded_requests().is_empty());
}

#[tokio::test]
async fn response_validation_rejects_invalid_openai_metadata_and_write_results() {
    let (app, transport) = app_with_caps(&[OPENAI_RESOURCE_METADATA_KEY, OPENAI_MODEL_CONTEXT_KEY]);
    transport.handle(
        incurs_mcp_apps::RESOURCE_READ_METHOD,
        incurs_mcp_apps::value_handler(|_| {
            Ok(json!({ "contents": [{ "uri": "file://a", "text": "hello", "_meta": { "openai/resource": { "etag": null } } }] }))
        }),
    );
    transport.handle(
        incurs_openai_mcp_app::OPENAI_MCP_APP_RESOURCE_WRITE_METHOD,
        incurs_mcp_apps::value_handler(|_| Ok(json!({ "outcome": "saved" }))),
    );
    transport.handle(
        incurs_mcp_apps::MODEL_CONTEXT_UPDATE_METHOD,
        incurs_mcp_apps::value_handler(|_| {
            Ok(json!({ "_meta": { "openai/modelContext": { "updateId": "" } } }))
        }),
    );
    let openai = OpenAiAppExtensions::new(app);

    let read_error = openai
        .resources()
        .unwrap()
        .read(
            OpenAiResourceReadParams {
                uri: "file://a".into(),
                meta: None,
                representation: None,
            },
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        read_error
            .to_string()
            .contains("OpenAIResourceContentMetadataSchema")
    );

    let write_error = openai
        .resources()
        .unwrap()
        .write(
            "file://a",
            OpenAiResourceWriteOptions {
                if_match: None,
                content: OpenAiResourceWriteContent::Text("hello".into()),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        write_error
            .to_string()
            .contains("OpenAIResourceWriteResultSchema")
    );

    let model_error = openai
        .model_context()
        .unwrap()
        .update(json!({ "content": [] }), RequestOptions::default())
        .await
        .unwrap_err();
    assert!(
        model_error
            .to_string()
            .contains("OpenAIModelContextMetadataSchema")
    );
}

#[tokio::test]
async fn resource_write_numeric_limits_retain_host_number_values() {
    for max_bytes in [json!(1.0), json!(1.5), json!(-1.0)] {
        let (app, transport) = app_with_caps(&["openai/resource"]);
        let reported = max_bytes.clone();
        transport.handle(
            "openai/resources/write",
            incurs_mcp_apps::value_handler(move |_| {
                Ok(json!({"outcome":"too-large","maxBytes":reported.clone()}))
            }),
        );
        let result = OpenAiAppExtensions::new(app)
            .resources()
            .unwrap()
            .write(
                "file://large",
                OpenAiResourceWriteOptions {
                    if_match: None,
                    content: OpenAiResourceWriteContent::Text("payload".into()),
                },
                RequestOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(result).unwrap()["maxBytes"], max_bytes);
    }
}
