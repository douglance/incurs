use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpCallContext, McpCommandOptions, McpPeer,
    McpPeerRequest,
};
use incurs::mcp::{
    McpHttpBody, McpHttpConfig, McpHttpRequest, McpHttpServer, McpResourceReadResult,
    McpResourceReader,
};
use incurs::output::{CommandResult, Format};
use incurs::tool::{ToolCallOptions, ToolCallOutcome};
use incurs_openai_mcp::{
    IncursOpenAiServer, MentionSearchHandler, OpenAiElicitInput, OpenAiExtensions, OpenAiMentions,
    OpenAiServer, OpenAiServerError, OpenAiSettings, OpenAiSettingsField,
    OpenAiSettingsRegistration, ServerCapabilitiesPatch, ToolCallResult, ToolRegistration,
    ToolRegistrationHandle, openai_html_resource, openai_html_resource_contents,
    openai_ui_resource_metadata,
};
use incurs_openai_mcp_protocol::{
    OPENAI_ELICITATION_METHOD, OPENAI_EXTENSIONS_META_KEY, OPENAI_SETTINGS_CAPABILITY_KEY,
    OPENAI_UI_META_KEY,
};
use serde_json::{Value, json};

const LEGACY_VERSION: &str = "2025-11-25";
const MODERN_VERSION: &str = "2026-07-28";
const SETTINGS_RESOURCE_URI: &str = "ui://openai/settings.html";

#[derive(Clone, Debug, Default)]
struct TestContext;

#[derive(Default)]
struct FakeServer {
    connected: bool,
    fail_second_tool: bool,
    tools: BTreeMap<String, ToolRegistration<TestContext>>,
    removed: Vec<String>,
    capabilities: Vec<ServerCapabilitiesPatch>,
}

impl OpenAiServer<TestContext> for FakeServer {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn register_tool(
        &mut self,
        registration: ToolRegistration<TestContext>,
    ) -> Result<ToolRegistrationHandle, OpenAiServerError> {
        if self.fail_second_tool && self.tools.len() == 1 {
            return Err(OpenAiServerError::Host("duplicate tool".to_string()));
        }
        if self.tools.contains_key(&registration.name) {
            return Err(OpenAiServerError::Host("duplicate tool".to_string()));
        }
        let name = registration.name.clone();
        self.tools.insert(name.clone(), registration);
        Ok(ToolRegistrationHandle { name })
    }

    fn remove_tool(&mut self, handle: &ToolRegistrationHandle) {
        self.tools.remove(&handle.name);
        self.removed.push(handle.name.clone());
    }

    fn register_capabilities(
        &mut self,
        capabilities: ServerCapabilitiesPatch,
    ) -> Result<(), OpenAiServerError> {
        self.capabilities.push(capabilities);
        Ok(())
    }
}

#[test]
fn settings_registers_tools_and_both_capability_maps() {
    let settings = OpenAiSettings::<TestContext>::default();
    let mut server = FakeServer::default();
    let tools = settings
        .register(&mut server, settings_registration(json!("mm")))
        .unwrap();
    assert_eq!(tools.capability.read_tool, "settings.read");
    assert_eq!(tools.capability.update_tool, "settings.update");
    assert!(server.tools.contains_key("settings.read"));
    assert!(server.tools.contains_key("settings.update"));
    assert!(
        server.capabilities[0]
            .extensions
            .contains_key(OPENAI_SETTINGS_CAPABILITY_KEY)
    );
    assert!(
        server.capabilities[0]
            .experimental
            .contains_key(OPENAI_SETTINGS_CAPABILITY_KEY)
    );
    assert_eq!(
        server.tools["settings.read"].annotations["readOnlyHint"],
        Value::Bool(true)
    );
}

#[test]
fn settings_handlers_validate_values() {
    let settings = OpenAiSettings::<TestContext>::default();
    let mut server = FakeServer::default();
    settings
        .register(&mut server, settings_registration(json!("mm")))
        .unwrap();
    let read = call(&server.tools["settings.read"], json!({})).unwrap();
    assert_eq!(read.structured_content["values"], json!({"units":"mm"}));
    let update = call(
        &server.tools["settings.update"],
        json!({"set":{"units":"in"}}),
    )
    .unwrap();
    assert_eq!(update.structured_content["values"], json!({"units":"in"}));
    assert!(call(&server.tools["settings.update"], json!({"set":{}})).is_err());
    assert!(
        call(
            &server.tools["settings.update"],
            json!({"set":{"other":"in"}})
        )
        .is_err()
    );
    assert!(call(&server.tools["settings.update"], json!({"set":{"units":4}})).is_err());
}

#[test]
fn settings_handlers_enforce_pattern_keyword() {
    let settings = OpenAiSettings::<TestContext>::default();
    let mut fields = BTreeMap::new();
    fields.insert(
        "code".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","pattern":"^ok$"}), "Code"),
    );
    let mut server = FakeServer::default();
    settings
        .register(
            &mut server,
            OpenAiSettingsRegistration {
                read_tool: None,
                update_tool: None,
                fields,
                layout: None,
                read: Arc::new(|_| {
                    Box::pin(async { Ok(BTreeMap::from([("code".to_string(), json!("ok"))])) })
                }),
                update: Arc::new(|set, _| Box::pin(async move { Ok(set) })),
            },
        )
        .unwrap();
    assert!(
        call(
            &server.tools["settings.update"],
            json!({"set":{"code":"bad"}})
        )
        .is_err()
    );
    let update = call(
        &server.tools["settings.update"],
        json!({"set":{"code":"ok"}}),
    )
    .unwrap();
    assert_eq!(update.structured_content["values"], json!({"code":"ok"}));
}

#[test]
fn settings_rolls_back_read_tool_when_update_registration_fails() {
    let settings = OpenAiSettings::<TestContext>::default();
    let mut server = FakeServer {
        fail_second_tool: true,
        ..FakeServer::default()
    };
    let error = settings
        .register(&mut server, settings_registration(json!("mm")))
        .unwrap_err();
    assert_eq!(error.to_string(), "duplicate tool");
    assert!(server.tools.is_empty());
    assert_eq!(server.removed, vec!["settings.read"]);
    assert!(server.capabilities.is_empty());
}

#[test]
fn settings_rejects_schema_defaults_and_incomplete_handlers() {
    let settings = OpenAiSettings::<TestContext>::default();
    let mut server = FakeServer::default();
    let mut registration = settings_registration(json!("mm"));
    registration.fields.get_mut("units").unwrap().schema = json!({"type":"string","default":"mm"});
    assert!(settings.register(&mut server, registration).is_err());

    let settings = OpenAiSettings::<TestContext>::default();
    let mut server = FakeServer::default();
    let mut registration = settings_registration(json!("mm"));
    registration.read = Arc::new(|_| Box::pin(async { Ok(BTreeMap::new()) }));
    settings.register(&mut server, registration).unwrap();
    assert!(call(&server.tools["settings.read"], json!({})).is_err());
}

#[test]
fn mentions_register_once_and_replace_handler() {
    let mentions = OpenAiMentions::<TestContext>::default();
    let mut server = FakeServer::default();
    mentions
        .set_handler(&mut server, mention_handler("first"))
        .unwrap();
    mentions
        .set_handler(&mut server, mention_handler("second"))
        .unwrap();
    assert_eq!(server.tools.len(), 1);
    let tool = &server.tools["search_mentions"];
    assert_eq!(
        tool.meta[OPENAI_EXTENSIONS_META_KEY],
        json!({"mentions/search": {}})
    );
    assert_eq!(tool.meta["ui"], json!({"visibility":["app"]}));
    let result = call(tool, json!({"query":""})).unwrap();
    assert_eq!(result.structured_content["items"][0]["title"], "second");
}

#[test]
fn extensions_facade_exposes_helpers() {
    let extensions = OpenAiExtensions::<TestContext>::new();
    let mut server = FakeServer::default();
    extensions
        .settings
        .register(&mut server, settings_registration(json!("mm")))
        .unwrap();
    extensions
        .mentions
        .set_handler(&mut server, mention_handler("part"))
        .unwrap();
    assert!(server.tools.contains_key("settings.read"));
    assert!(server.tools.contains_key("search_mentions"));
    let _ = extensions.elicit_input;
}

#[test]
fn elicitation_builds_openai_request_and_validates_accept_content() {
    let elicit = OpenAiElicitInput;
    let request = elicit.create_request(
        &json!({"extensions":{"openai/elicitation":{"form":{}}}}),
        json!({"mode":"form","message":"Choose units","requestedSchema":{"type":"object","required":["units"],"properties":{"units":{"type":"string"}}}}),
        Some(json!({"timeout":10})),
    ).unwrap();
    assert_eq!(request.method, OPENAI_ELICITATION_METHOD);
    assert_eq!(request.options, Some(json!({"timeout":10})));
    let accepted = elicit
        .validate_response(
            &request,
            json!({"action":"accept","content":{"units":"mm"}}),
        )
        .unwrap();
    assert_eq!(accepted["content"], json!({"units":"mm"}));
    assert!(
        elicit
            .validate_response(&request, json!({"action":"accept","content":{}}))
            .is_err()
    );
}

#[test]
fn elicitation_capability_leaves_must_be_objects() {
    let params = || json!({"mode":"form","requestedSchema":{"type":"object","properties":{},"additionalProperties":false}});
    for capabilities in [
        json!({"extensions":{"openai/elicitation":{"form":null}}}),
        json!({"extensions":{"openai/elicitation":{"form":false}}}),
        json!({"experimental":{"openai/elicitation":{"form":{}}}}),
        json!({"elicitation":null}),
        json!({"elicitation":false}),
        json!({"elicitation":{}}),
    ] {
        assert!(
            OpenAiElicitInput
                .create_request(&capabilities, params(), None)
                .is_err(),
            "accepted falsey capabilities: {capabilities}"
        );
    }
    assert_eq!(
        OpenAiElicitInput
            .create_request(
                &json!({"extensions":{"openai/elicitation":{"form":{}}}}),
                params(),
                None,
            )
            .unwrap()
            .method,
        OPENAI_ELICITATION_METHOD
    );
}

#[test]
fn elicitation_rejects_missing_capability() {
    let elicit = OpenAiElicitInput;
    assert!(
        elicit
            .create_request(
                &json!({}),
                json!({"mode":"form","requestedSchema":{"type":"object","properties":{}}}),
                None
            )
            .is_err()
    );
}

#[tokio::test]
async fn incurs_adapter_lists_and_calls_settings_at_structured_result_level() {
    let settings = OpenAiSettings::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(Cli::create("openai-test"));
    settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), None),
        )
        .unwrap();
    assert!(
        server.mcp_options().capabilities["extensions"]
            .get(OPENAI_SETTINGS_CAPABILITY_KEY)
            .is_some()
    );
    assert!(
        server.mcp_options().capabilities["experimental"]
            .get(OPENAI_SETTINGS_CAPABILITY_KEY)
            .is_some()
    );

    let cli = server.into_cli();
    let catalog = cli.tool_catalog();
    let read = catalog.get("settings.read").unwrap();
    assert_eq!(
        read.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    assert!(read.direct);
    assert_eq!(
        read.output_schema.as_ref().unwrap()["required"],
        json!(["schema", "values"])
    );

    let outcome = catalog
        .call(
            "settings.read",
            BTreeMap::new(),
            ToolCallOptions::isolated(),
        )
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => {
            assert_eq!(data["values"], json!({"units":"mm"}));
            assert!(data.get("structuredContent").is_none());
            assert!(data.get("content").is_none());
        }
        other => panic!("unexpected settings.read outcome: {other:?}"),
    }
}

#[tokio::test]
async fn incurs_adapter_rejects_invalid_settings_before_handler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let settings = OpenAiSettings::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(Cli::create("openai-test"));
    settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), Some(Arc::clone(&calls))),
        )
        .unwrap();
    let catalog = server.into_cli().tool_catalog();
    let outcome = catalog
        .call(
            "settings.update",
            object_args(json!({"set":{"units":4}})),
            ToolCallOptions::isolated(),
        )
        .await;
    match outcome {
        ToolCallOutcome::Error { code, .. } => assert_eq!(code, "OPENAI_EXTENSION_ERROR"),
        other => panic!("unexpected settings.update outcome: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn incurs_adapter_rejects_base_tool_collision_and_rolls_back_settings_read() {
    let base_calls = Arc::new(AtomicUsize::new(0));
    let update_calls = Arc::new(AtomicUsize::new(0));
    let base_cli = Cli::create("openai-test").command(
        "settings.update",
        base_counting_command("base-update", Arc::clone(&base_calls), None),
    );
    let settings = OpenAiSettings::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(base_cli);

    let error = settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), Some(Arc::clone(&update_calls))),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("collides with existing CLI tool: settings.update"),
        "{error}"
    );

    let catalog = server.into_cli().tool_catalog();
    assert!(catalog.get("settings.read").is_none());
    let outcome = catalog
        .call(
            "settings.update",
            BTreeMap::new(),
            ToolCallOptions::isolated(),
        )
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => assert_eq!(data, json!({"source":"base-update"})),
        other => panic!("unexpected base settings.update outcome: {other:?}"),
    }
    assert_eq!(base_calls.load(Ordering::SeqCst), 1);
    assert_eq!(update_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn incurs_adapter_rejects_aliased_base_command_with_openai_tool_name_path() {
    let base_calls = Arc::new(AtomicUsize::new(0));
    let update_calls = Arc::new(AtomicUsize::new(0));
    let base_cli = Cli::create("openai-test").command(
        "settings.update",
        base_counting_command(
            "aliased-base-update",
            Arc::clone(&base_calls),
            Some("existing_update"),
        ),
    );
    let settings = OpenAiSettings::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(base_cli);
    let error = settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), Some(Arc::clone(&update_calls))),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("collides with existing CLI tool: settings.update"),
        "{error}"
    );

    let catalog = server.into_cli().tool_catalog();
    assert!(catalog.get("settings.read").is_none());
    assert!(catalog.get("settings.update").is_none());
    let base = catalog
        .call(
            "existing_update",
            BTreeMap::new(),
            ToolCallOptions::isolated(),
        )
        .await;
    match base {
        ToolCallOutcome::Ok { data, .. } => {
            assert_eq!(data, json!({"source":"aliased-base-update"}))
        }
        other => panic!("unexpected aliased base outcome: {other:?}"),
    }
    assert_eq!(base_calls.load(Ordering::SeqCst), 1);
    assert_eq!(update_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn incurs_adapter_rejects_nested_base_group_with_openai_tool_name_path() {
    let base_calls = Arc::new(AtomicUsize::new(0));
    let update_calls = Arc::new(AtomicUsize::new(0));
    let base_cli = Cli::create("openai-test").group(Cli::create("settings.update").command(
        "child",
        base_counting_command(
            "nested-base-update",
            Arc::clone(&base_calls),
            Some("nested_existing_update"),
        ),
    ));
    let settings = OpenAiSettings::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(base_cli);
    let error = settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), Some(Arc::clone(&update_calls))),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("collides with existing CLI tool: settings.update"),
        "{error}"
    );

    let catalog = server.into_cli().tool_catalog();
    assert!(catalog.get("settings.read").is_none());
    assert!(catalog.get("settings.update").is_none());
    let base = catalog
        .call(
            "nested_existing_update",
            BTreeMap::new(),
            ToolCallOptions::isolated(),
        )
        .await;
    match base {
        ToolCallOutcome::Ok { data, .. } => {
            assert_eq!(data, json!({"source":"nested-base-update"}))
        }
        other => panic!("unexpected nested base outcome: {other:?}"),
    }
    assert_eq!(base_calls.load(Ordering::SeqCst), 1);
    assert_eq!(update_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn incurs_adapter_mentions_uses_toolcatalog_metadata_and_shape() {
    let mentions = OpenAiMentions::<CommandContext>::default();
    let mut server = IncursOpenAiServer::new(Cli::create("openai-test"));
    mentions
        .set_handler(&mut server, command_mention_handler("part"))
        .unwrap();
    let catalog = server.into_cli().tool_catalog();
    let definition = catalog.get("search_mentions").unwrap();
    assert_eq!(
        definition.meta[OPENAI_EXTENSIONS_META_KEY],
        json!({"mentions/search": {}})
    );
    assert_eq!(definition.meta["ui"], json!({"visibility":["app"]}));
    assert_eq!(
        definition.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    let outcome = catalog
        .call(
            "search_mentions",
            object_args(json!({"query":"pa"})),
            ToolCallOptions::isolated(),
        )
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => {
            assert_eq!(data["items"][0]["title"], "part");
            assert!(data.get("structuredContent").is_none());
        }
        other => panic!("unexpected search_mentions outcome: {other:?}"),
    }
}

#[tokio::test]
async fn elicitation_peer_sends_openai_request_and_validates_result() {
    let observed = Arc::new(Mutex::new(None::<McpPeerRequest>));
    let peer = {
        let observed = Arc::clone(&observed);
        McpPeer::new(move |request| {
            *observed.lock().unwrap() = Some(request);
            async { Ok(json!({"action":"accept","content":{"units":"mm"}})) }
        })
    };
    let elicit = OpenAiElicitInput;
    let result = elicit
        .request_peer(
            &peer,
            &json!({"extensions":{"openai/elicitation":{"form":{}}}}),
            json!({"mode":"form","requestedSchema":{"type":"object","required":["units"],"properties":{"units":{"type":"string"}}}}),
            Some(json!({"trace":"t1"})),
        )
        .await
        .unwrap();
    assert_eq!(result["content"], json!({"units":"mm"}));
    let request = observed.lock().unwrap().clone().unwrap();
    assert_eq!(request.method, OPENAI_ELICITATION_METHOD);
    assert_eq!(request.meta, Some(json!({"trace":"t1"})));
    assert_eq!(request.params.unwrap()["mode"], "form");
}

#[tokio::test]
async fn elicitation_context_uses_custom_peer_on_legacy_protocol() {
    let observed = Arc::new(Mutex::new(None::<McpPeerRequest>));
    let peer = {
        let observed = Arc::clone(&observed);
        McpPeer::new(move |request| {
            *observed.lock().unwrap() = Some(request);
            async { Ok(json!({"action":"accept","content":{"units":"in"}})) }
        })
    };
    let context = CommandContext {
        agent: true,
        args: Value::Null,
        env: Value::Null,
        display_name: "openai-test".to_string(),
        globals: Value::Null,
        options: Value::Null,
        request: None,
        mcp: Some(McpCallContext {
            protocol_version: Some(LEGACY_VERSION.to_string()),
            request_meta: None,
            client_capabilities: Some(json!({"extensions":{"openai/elicitation":{"form":{}}}})),
            input_responses: None,
            request_state: None,
            peer: Some(peer),
        }),
        format: Format::Json,
        format_explicit: false,
        name: "openai-test".to_string(),
        vars: Value::Null,
        version: None,
    };
    let result = OpenAiElicitInput
        .request_from_context(
            &context,
            json!({"mode":"form","requestedSchema":{"type":"object","required":["units"],"properties":{"units":{"type":"string"}},"additionalProperties":false}}),
            Some(json!({"trace":"legacy"})),
        )
        .await
        .unwrap();
    assert_eq!(result["content"], json!({"units":"in"}));
    let request = observed.lock().unwrap().clone().unwrap();
    assert_eq!(request.method, OPENAI_ELICITATION_METHOD);
    assert_eq!(request.meta, Some(json!({"trace":"legacy"})));
}

#[tokio::test]
async fn portable_http_initialize_advertises_openai_capabilities() {
    let server = portable_openai_server(None);
    let response = portable_rpc(
        &server,
        1,
        "initialize",
        Some(json!({
            "protocolVersion": LEGACY_VERSION,
            "capabilities": {
                "extensions": {"openai/elicitation": {"form": {}}},
                "experimental": {"openai/elicitation": {"form": {}}}
            },
            "clientInfo": {"name": "openai-test", "version": "1.0.0"}
        })),
    )
    .await;
    let capabilities = &response["result"]["capabilities"];
    for namespace in ["extensions", "experimental"] {
        assert_eq!(
            capabilities[namespace][OPENAI_SETTINGS_CAPABILITY_KEY],
            json!({"readTool":"settings.read","updateTool":"settings.update"})
        );
    }
}

#[tokio::test]
async fn portable_http_lists_direct_openai_tools_under_progressive_discovery() {
    let server = portable_openai_server(None);
    let response = portable_rpc(&server, 2, "tools/list", None).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "search_tools"));
    let read = tool_named(tools, "settings.read");
    let update = tool_named(tools, "settings.update");
    let mentions = tool_named(tools, "search_mentions");
    assert_eq!(read["annotations"]["readOnlyHint"], true);
    assert_eq!(
        read["outputSchema"]["required"],
        json!(["schema", "values"])
    );
    assert_eq!(update["inputSchema"]["required"], json!(["set"]));
    assert_eq!(mentions["annotations"]["readOnlyHint"], true);
    assert_eq!(
        mentions["_meta"][OPENAI_EXTENSIONS_META_KEY],
        json!({"mentions/search": {}})
    );
    assert_eq!(mentions["_meta"]["ui"], json!({"visibility":["app"]}));
}

#[tokio::test]
async fn portable_http_calls_openai_tools_with_top_level_structured_content() {
    let server = portable_openai_server(None);
    let read = portable_rpc(
        &server,
        3,
        "tools/call",
        Some(json!({"name":"settings.read","arguments":{}})),
    )
    .await;
    assert!(read.get("error").is_none(), "unexpected response: {read}");
    let read_result = &read["result"];
    assert_eq!(read_result["content"], json!([]));
    assert_eq!(
        read_result["structuredContent"]["values"],
        json!({"units":"mm"})
    );
    assert!(
        read_result["structuredContent"]
            .get("structuredContent")
            .is_none()
    );
    assert!(read_result["structuredContent"].get("content").is_none());

    let mentions = portable_rpc(
        &server,
        4,
        "tools/call",
        Some(json!({"name":"search_mentions","arguments":{"query":"pa"}})),
    )
    .await;
    assert!(
        mentions.get("error").is_none(),
        "unexpected response: {mentions}"
    );
    assert_eq!(mentions["result"]["content"], json!([]));
    assert_eq!(
        mentions["result"]["structuredContent"]["items"][0]["title"],
        "part"
    );
    assert!(
        mentions["result"]["structuredContent"]
            .get("structuredContent")
            .is_none()
    );
}

#[tokio::test]
async fn portable_http_rejects_invalid_settings_before_update_handler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = portable_openai_server(Some(Arc::clone(&calls)));
    let response = portable_rpc(
        &server,
        5,
        "tools/call",
        Some(json!({"name":"settings.update","arguments":{"set":{"units":4}}})),
    )
    .await;
    assert!(
        response.get("error").is_none(),
        "unexpected response: {response}"
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn portable_http_rejects_settings_constraints_before_update_handler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut server = IncursOpenAiServer::new(Cli::create("openai-test"));
    OpenAiSettings::<CommandContext>::default()
        .register(
            &mut server,
            constrained_command_settings_registration(Arc::clone(&calls)),
        )
        .unwrap();
    let server = McpHttpServer::from_cli(&server.into_cli(), McpHttpConfig::default()).unwrap();

    let invalid_cases = [
        json!({"text":"a"}),
        json!({"text":"abcde"}),
        json!({"lookbehind":"bb"}),
        json!({"lookbehind":"a"}),
        json!({"email":"not-an-email"}),
        json!({"bounded":0}),
        json!({"bounded":5}),
        json!({"half":1.25}),
        json!({"tenth":0.31}),
        json!({"safe_int":1.5}),
        json!({"safe_int":9007199254740992u64}),
        json!({"const_string":"other"}),
        json!({"const_bool":false}),
        json!({"const_number":2}),
        json!({"const_large":9007199254740992u64}),
    ];
    for (index, set) in invalid_cases.into_iter().enumerate() {
        let response = portable_rpc(
            &server,
            51 + index as u64,
            "tools/call",
            Some(json!({"name":"settings.update","arguments":{"set":set}})),
        )
        .await;
        assert_eq!(
            response["result"]["isError"], true,
            "unexpected invalid response for {set}: {response}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "handler called for {set}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let valid_cases = [
        json!({"text":"😀"}),
        json!({"lookbehind":"ab"}),
        json!({"email":"a@example.com"}),
        json!({"bounded":2}),
        json!({"half":2}),
        json!({"tenth":0.3}),
        json!({"safe_int":42}),
        json!({"const_string":"fixed"}),
        json!({"const_bool":true}),
        json!({"const_number":1.0}),
        json!({"const_large":9007199254740993u64}),
    ];
    for (index, set) in valid_cases.into_iter().enumerate() {
        let response = portable_rpc(
            &server,
            80 + index as u64,
            "tools/call",
            Some(json!({"name":"settings.update","arguments":{"set":set}})),
        )
        .await;
        assert_eq!(
            response["result"]["isError"], false,
            "unexpected valid response for {set}: {response}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            index + 1,
            "handler count after {set}"
        );
    }
}

#[tokio::test]
async fn portable_http_preserves_custom_tool_content_blocks() {
    let server = portable_openai_server(None);
    let response = portable_rpc(
        &server,
        53,
        "tools/call",
        Some(json!({"name":"openai.content_result","arguments":{}})),
    )
    .await;
    assert_eq!(
        response["result"]["structuredContent"],
        json!({"ok":true}),
        "unexpected content response: {response}"
    );
    assert_eq!(
        response["result"]["content"],
        json!([{"type":"text","text":"visible content"}])
    );
}

#[tokio::test]
async fn portable_http_lists_and_reads_openai_html_resource() {
    let server = portable_openai_server(None);
    let listed = portable_rpc(&server, 6, "resources/list", None).await;
    let resource = &listed["result"]["resources"][0];
    assert_eq!(resource["uri"], SETTINGS_RESOURCE_URI);
    assert_eq!(resource["mimeType"], "text/html");
    assert_eq!(
        resource["_meta"][OPENAI_UI_META_KEY],
        json!({"availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"inline"})
    );

    let read = portable_rpc(
        &server,
        7,
        "resources/read",
        Some(json!({"uri": SETTINGS_RESOURCE_URI})),
    )
    .await;
    let content = &read["result"]["contents"][0];
    assert_eq!(content["uri"], SETTINGS_RESOURCE_URI);
    assert_eq!(content["mimeType"], "text/html");
    assert!(
        content["text"]
            .as_str()
            .unwrap()
            .contains("data-openai-settings")
    );
    assert_eq!(
        content["_meta"][OPENAI_UI_META_KEY],
        json!({"availableDisplayModes":["inline"],"preferredDisplayMode":"inline"})
    );
}

#[tokio::test]
async fn portable_http_modern_elicitation_uses_mrtr_input_required_and_replay() {
    let server = portable_openai_server(None);
    let discovered = portable_rpc_with_headers(
        &server,
        8,
        "server/discover",
        Some(json!({"_meta": modern_openai_meta()})),
        modern_headers("server/discover", None),
    )
    .await;
    assert!(
        discovered.get("error").is_none(),
        "unexpected discover response: {discovered}"
    );
    assert!(
        discovered["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version == MODERN_VERSION),
        "unexpected discover response: {discovered}"
    );

    let first = portable_rpc_with_headers(
        &server,
        9,
        "tools/call",
        Some(json!({
            "name": "openai.elicit_units",
            "arguments": {},
            "_meta": modern_openai_meta()
        })),
        modern_headers("tools/call", Some("openai.elicit_units")),
    )
    .await;
    assert_eq!(first["result"]["resultType"], "input_required");
    assert_eq!(
        first["result"]["inputRequests"]["openai_elicitation"]["method"],
        OPENAI_ELICITATION_METHOD
    );
    assert_eq!(
        first["result"]["inputRequests"]["openai_elicitation"]["params"]["mode"],
        "form"
    );
    assert_eq!(
        first["result"]["inputRequests"]["openai_elicitation"]["_meta"]["trace"],
        "mrtr"
    );
    let request_state = first["result"]["requestState"].as_str().unwrap();

    let second = portable_rpc_with_headers(
        &server,
        10,
        "tools/call",
        Some(json!({
            "name": "openai.elicit_units",
            "arguments": {},
            "inputResponses": {
                "openai_elicitation": {"action":"accept","content":{"units":"mm"}}
            },
            "requestState": request_state,
            "_meta": modern_openai_meta()
        })),
        modern_headers("tools/call", Some("openai.elicit_units")),
    )
    .await;
    assert_eq!(second["result"]["resultType"], "complete");
    assert_eq!(second["result"]["content"], json!([]));
    assert_eq!(
        second["result"]["structuredContent"]["answer"]["content"],
        json!({"units":"mm"})
    );
}

fn settings_registration(value: Value) -> OpenAiSettingsRegistration<TestContext> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "units".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","enum":["mm","in"]}), "Units"),
    );
    OpenAiSettingsRegistration {
        read_tool: None,
        update_tool: None,
        fields,
        layout: Some(vec![
            json!({"kind":"group","title":"General","items":[{"kind":"property","property":"units"}]}),
        ]),
        read: Arc::new(move |_| {
            let value = value.clone();
            Box::pin(async move { Ok(BTreeMap::from([("units".to_string(), value)])) })
        }),
        update: Arc::new(|set, _| {
            Box::pin(async move {
                Ok(BTreeMap::from([(
                    "units".to_string(),
                    set["units"].clone(),
                )]))
            })
        }),
    }
}

fn command_settings_registration(
    value: Value,
    update_calls: Option<Arc<AtomicUsize>>,
) -> OpenAiSettingsRegistration<CommandContext> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "units".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","enum":["mm","in"]}), "Units"),
    );
    OpenAiSettingsRegistration {
        read_tool: None,
        update_tool: None,
        fields,
        layout: Some(vec![
            json!({"kind":"group","title":"General","items":[{"kind":"property","property":"units"}]}),
        ]),
        read: Arc::new(move |_| {
            let value = value.clone();
            Box::pin(async move { Ok(BTreeMap::from([("units".to_string(), value)])) })
        }),
        update: Arc::new(move |set, _| {
            let update_calls = update_calls.clone();
            Box::pin(async move {
                if let Some(calls) = update_calls {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(BTreeMap::from([(
                    "units".to_string(),
                    set["units"].clone(),
                )]))
            })
        }),
    }
}

fn base_counting_command(
    source: &'static str,
    calls: Arc<AtomicUsize>,
    exposed_name: Option<&str>,
) -> CommandDef {
    CommandDef::build(source, CountingBaseCommand { source, calls })
        .mcp(McpCommandOptions {
            name: exposed_name.map(str::to_string),
            direct: true,
            ..McpCommandOptions::default()
        })
        .done()
}

struct CountingBaseCommand {
    source: &'static str,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl CommandHandler for CountingBaseCommand {
    async fn run(&self, _ctx: CommandContext) -> CommandResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        CommandResult::Ok {
            data: json!({"source": self.source}),
            cta: None,
            exit_code: None,
        }
    }
}

fn constrained_command_settings_registration(
    update_calls: Arc<AtomicUsize>,
) -> OpenAiSettingsRegistration<CommandContext> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "text".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","minLength":2,"maxLength":4}), "Text"),
    );
    fields.insert(
        "lookbehind".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","pattern":"(?<=a)b"}), "Lookbehind"),
    );
    fields.insert(
        "email".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","format":"email"}), "Email"),
    );
    fields.insert(
        "bounded".to_string(),
        OpenAiSettingsField::new(
            json!({"type":"number","exclusiveMinimum":0,"exclusiveMaximum":5}),
            "Bounded",
        ),
    );
    fields.insert(
        "half".to_string(),
        OpenAiSettingsField::new(json!({"type":"number","multipleOf":0.5}), "Half"),
    );
    fields.insert(
        "tenth".to_string(),
        OpenAiSettingsField::new(json!({"type":"number","multipleOf":0.1}), "Tenth"),
    );
    fields.insert(
        "safe_int".to_string(),
        OpenAiSettingsField::new(
            json!({"type":"integer","minimum":-9007199254740991i64,"maximum":9007199254740991i64}),
            "Safe integer",
        ),
    );
    fields.insert(
        "const_string".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","const":"fixed"}), "Const string"),
    );
    fields.insert(
        "const_bool".to_string(),
        OpenAiSettingsField::new(json!({"type":"boolean","const":true}), "Const bool"),
    );
    fields.insert(
        "const_number".to_string(),
        OpenAiSettingsField::new(json!({"type":"number","const":1}), "Const number"),
    );
    fields.insert(
        "const_large".to_string(),
        OpenAiSettingsField::new(
            json!({"type":"integer","const":9007199254740993u64}),
            "Const large integer",
        ),
    );
    OpenAiSettingsRegistration {
        read_tool: None,
        update_tool: None,
        fields,
        layout: None,
        read: Arc::new(|_| {
            Box::pin(async {
                Ok(BTreeMap::from([
                    ("text".to_string(), json!("😀")),
                    ("lookbehind".to_string(), json!("ab")),
                    ("email".to_string(), json!("a@example.com")),
                    ("bounded".to_string(), json!(2)),
                    ("half".to_string(), json!(2)),
                    ("tenth".to_string(), json!(0.3)),
                    ("safe_int".to_string(), json!(42)),
                    ("const_string".to_string(), json!("fixed")),
                    ("const_bool".to_string(), json!(true)),
                    ("const_number".to_string(), json!(1)),
                    ("const_large".to_string(), json!(9007199254740993u64)),
                ]))
            })
        }),
        update: Arc::new(move |set, _| {
            let update_calls = Arc::clone(&update_calls);
            Box::pin(async move {
                update_calls.fetch_add(1, Ordering::SeqCst);
                let mut values = BTreeMap::from([
                    ("text".to_string(), json!("😀")),
                    ("lookbehind".to_string(), json!("ab")),
                    ("email".to_string(), json!("a@example.com")),
                    ("bounded".to_string(), json!(2)),
                    ("half".to_string(), json!(2)),
                    ("tenth".to_string(), json!(0.3)),
                    ("safe_int".to_string(), json!(42)),
                    ("const_string".to_string(), json!("fixed")),
                    ("const_bool".to_string(), json!(true)),
                    ("const_number".to_string(), json!(1)),
                    ("const_large".to_string(), json!(9007199254740993u64)),
                ]);
                values.extend(set);
                Ok(values)
            })
        }),
    }
}

fn mention_handler(title: &'static str) -> MentionSearchHandler<TestContext> {
    Arc::new(move |_, _| {
        Box::pin(async move {
            Ok(incurs_openai_mcp_protocol::OpenAiMentionSearchResult {
                items: vec![incurs_openai_mcp_protocol::OpenAIMentionItem::Resource(
                    incurs_openai_mcp_protocol::OpenAIMentionResource {
                        kind: incurs_openai_mcp_protocol::MentionResourceKind::Resource,
                        resource_uri: "cad://part".to_string(),
                        title: title.to_string(),
                        subtitle: None,
                        icons: None,
                    },
                )],
            })
        })
    })
}

fn command_mention_handler(title: &'static str) -> MentionSearchHandler<CommandContext> {
    Arc::new(move |_, _| {
        Box::pin(async move {
            Ok(incurs_openai_mcp_protocol::OpenAiMentionSearchResult {
                items: vec![incurs_openai_mcp_protocol::OpenAIMentionItem::Resource(
                    incurs_openai_mcp_protocol::OpenAIMentionResource {
                        kind: incurs_openai_mcp_protocol::MentionResourceKind::Resource,
                        resource_uri: "cad://part".to_string(),
                        title: title.to_string(),
                        subtitle: None,
                        icons: None,
                    },
                )],
            })
        })
    })
}

fn call(
    registration: &ToolRegistration<TestContext>,
    params: Value,
) -> Result<ToolCallResult, OpenAiServerError> {
    block_on((registration.handler)(params, TestContext))
}

fn object_args(value: Value) -> BTreeMap<String, Value> {
    value.as_object().unwrap().clone().into_iter().collect()
}

fn portable_openai_server(update_calls: Option<Arc<AtomicUsize>>) -> McpHttpServer {
    let extensions = OpenAiExtensions::<CommandContext>::new();
    let mut server = IncursOpenAiServer::new(Cli::create("openai-test"));
    extensions
        .settings
        .register(
            &mut server,
            command_settings_registration(json!("mm"), update_calls),
        )
        .unwrap();
    extensions
        .mentions
        .set_handler(&mut server, command_mention_handler("part"))
        .unwrap();
    register_elicitation_tool(&mut server);
    register_content_tool(&mut server);
    register_marker_collision_tool(&mut server);
    let resource = openai_html_resource(
        SETTINGS_RESOURCE_URI,
        "settings-ui",
        "Settings UI",
        json!({"availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"inline"}),
    )
    .unwrap();
    server.resources_mut().resources.push(resource);
    server.resources_mut().read = Some(McpResourceReader::new(|request| async move {
        Ok(McpResourceReadResult {
            contents: vec![openai_html_resource_contents(
                request.uri,
                "<main data-openai-settings=\"true\">Settings</main>",
                openai_ui_resource_metadata(
                    json!({"availableDisplayModes":["inline"],"preferredDisplayMode":"inline"}),
                )
                .unwrap(),
            )],
            meta: BTreeMap::new(),
        })
    }));
    let cli = server.into_cli();
    McpHttpServer::from_cli(&cli, McpHttpConfig::default()).unwrap()
}

fn register_content_tool(server: &mut IncursOpenAiServer) {
    server
        .register_tool(ToolRegistration {
            name: "openai.content_result".to_string(),
            description: "Return visible content.".to_string(),
            title: Some("Content result".to_string()),
            icons: Vec::new(),
            annotations: BTreeMap::new(),
            meta: BTreeMap::new(),
            result_meta: BTreeMap::new(),
            direct: true,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            output_schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),
            handler: Arc::new(|_, _| {
                Box::pin(async move {
                    Ok(ToolCallResult {
                        content: vec![json!({"type":"text","text":"visible content"})],
                        structured_content: json!({"ok":true}),
                    })
                })
            }),
        })
        .unwrap();
}

fn register_marker_collision_tool(server: &mut IncursOpenAiServer) {
    server
        .register_tool(ToolRegistration {
            name: "openai.marker_collision".to_string(),
            description: "Return marker-shaped user content.".to_string(),
            title: Some("Marker collision".to_string()),
            icons: Vec::new(),
            annotations: BTreeMap::new(),
            meta: BTreeMap::new(),
            result_meta: BTreeMap::new(),
            direct: true,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            output_schema: json!({"type":"object","properties":{},"additionalProperties":true}),
            handler: Arc::new(|_, _| {
                Box::pin(async move {
                    Ok(ToolCallResult::structured(json!({
                        "__openaiToolResult": true,
                        "content": "user value",
                        "structuredContent": {"user": 1}
                    })))
                })
            }),
        })
        .unwrap();
}

#[tokio::test]
async fn portable_http_preserves_user_structured_content_marker_collision() {
    let server = portable_openai_server(None);
    let response = portable_rpc(
        &server,
        54,
        "tools/call",
        Some(json!({"name":"openai.marker_collision","arguments":{}})),
    )
    .await;
    assert_eq!(response["result"]["content"], json!([]));
    assert_eq!(
        response["result"]["structuredContent"],
        json!({"__openaiToolResult":true,"content":"user value","structuredContent":{"user":1}})
    );
}

fn register_elicitation_tool(server: &mut IncursOpenAiServer) {
    server
        .register_tool(ToolRegistration {
            name: "openai.elicit_units".to_string(),
            description: "Elicit measurement units.".to_string(),
            title: Some("Elicit units".to_string()),
            icons: Vec::new(),
            annotations: BTreeMap::new(),
            meta: BTreeMap::new(),
            result_meta: BTreeMap::new(),
            direct: true,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            output_schema: json!({"type":"object","properties":{"answer":{"type":"object"}},"required":["answer"],"additionalProperties":false}),
            handler: Arc::new(|_, context| {
                Box::pin(async move {
                    let answer = OpenAiElicitInput
                        .request_from_context(
                            &context,
                            json!({
                                "mode":"form",
                                "message":"Choose units",
                                "requestedSchema":{
                                    "type":"object",
                                    "required":["units"],
                                    "properties":{"units":{"type":"string","enum":["mm","in"]}},
                                    "additionalProperties":false
                                }
                            }),
                            Some(json!({"trace":"mrtr"})),
                        )
                        .await?;
                    Ok(ToolCallResult::structured(json!({"answer": answer})))
                })
            }),
        })
        .unwrap();
}

async fn portable_rpc(
    server: &McpHttpServer,
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Value {
    portable_rpc_with_headers(server, id, method, params, legacy_headers()).await
}

async fn portable_rpc_with_headers(
    server: &McpHttpServer,
    id: u64,
    method: &str,
    params: Option<Value>,
    headers: Vec<(String, String)>,
) -> Value {
    let response = server
        .handle(McpHttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers,
            body: rpc(id, method, params),
        })
        .await;
    assert_eq!(response.status, 200);
    let mut messages = portable_messages(response.body).await;
    assert_eq!(messages.len(), 1, "unexpected messages: {messages:?}");
    messages.remove(0)
}

async fn portable_messages(body: McpHttpBody) -> Vec<Value> {
    match body {
        McpHttpBody::Empty => Vec::new(),
        McpHttpBody::Full(bytes) => vec![serde_json::from_slice(&bytes).unwrap()],
        McpHttpBody::EventStream(mut stream) => {
            let mut text = String::new();
            while let Some(chunk) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
                text.push_str(&chunk);
            }
            text.split("\n\n")
                .filter_map(|event| {
                    event.lines().find_map(|line| {
                        line.strip_prefix("data: ")
                            .map(|data| serde_json::from_str(data).unwrap())
                    })
                })
                .collect()
        }
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

fn modern_headers(method: &str, name: Option<&str>) -> Vec<(String, String)> {
    let mut headers = legacy_headers();
    for (_, value) in headers
        .iter_mut()
        .filter(|(key, _)| key == "mcp-protocol-version")
    {
        *value = MODERN_VERSION.to_string();
    }
    headers.push(("mcp-method".to_string(), method.to_string()));
    if let Some(name) = name {
        headers.push(("mcp-name".to_string(), name.to_string()));
    }
    headers
}

fn modern_openai_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": {"openai/elicitation": {"form": {}}}
        },
        "io.modelcontextprotocol/clientInfo": {"name": "openai-test", "version": "1.0.0"}
    })
}

fn rpc(id: u64, method: &str, params: Option<Value>) -> Vec<u8> {
    let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
    if let Some(params) = params {
        message["params"] = params;
    }
    serde_json::to_vec(&message).unwrap()
}

fn tool_named<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("missing tool {name}: {tools:?}"))
}

fn block_on<T>(mut future: Pin<Box<dyn Future<Output = T> + Send + 'static>>) -> T {
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn noop_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}
