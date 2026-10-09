use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use incurs::cli::Cli;
use incurs::command::CommandContext;
use incurs::mcp::{McpResourceReadResult, McpResourceReader};
use incurs_openai_mcp::{
    IncursOpenAiServer, OpenAiElicitInput, OpenAiExtensions, OpenAiServer, OpenAiSettingsField,
    OpenAiSettingsRegistration, ToolCallResult, ToolRegistration, openai_html_resource,
    openai_html_resource_contents, openai_ui_resource_metadata,
};
use incurs_openai_mcp_protocol::{
    MentionResourceKind, OpenAIMentionItem, OpenAIMentionResource, OpenAiMentionSearchResult,
};
use serde_json::{Value, json};

const SETTINGS_RESOURCE_URI: &str = "ui://openai/native-settings.html";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    openai_cli().serve().await
}

fn openai_cli() -> Cli {
    let extensions = OpenAiExtensions::<CommandContext>::new();
    let mut server = IncursOpenAiServer::new(Cli::create("openai-stdio-example"));
    extensions
        .settings
        .register(&mut server, settings_registration())
        .expect("settings registration succeeds");
    extensions
        .mentions
        .set_handler(
            &mut server,
            Arc::new(|_, _| {
                Box::pin(async {
                    Ok(OpenAiMentionSearchResult {
                        items: vec![OpenAIMentionItem::Resource(OpenAIMentionResource {
                            kind: MentionResourceKind::Resource,
                            resource_uri: "cad://native-part".to_string(),
                            title: "native-part".to_string(),
                            subtitle: Some("Native stdio fixture".to_string()),
                            icons: None,
                        })],
                    })
                })
            }),
        )
        .expect("mention registration succeeds");
    register_elicitation_tool(&mut server);
    server.resources_mut().resources.push(
        openai_html_resource(
            SETTINGS_RESOURCE_URI,
            "native-settings-ui",
            "Native Settings UI",
            json!({"availableDisplayModes":["inline"],"preferredDisplayMode":"inline"}),
        )
        .expect("resource metadata is valid"),
    );
    server.resources_mut().read = Some(McpResourceReader::new(|request| async move {
        Ok(McpResourceReadResult {
            contents: vec![openai_html_resource_contents(
                request.uri,
                "<main data-openai-native-settings=\"true\">Native settings</main>",
                openai_ui_resource_metadata(
                    json!({"availableDisplayModes":["inline"],"preferredDisplayMode":"inline"}),
                )
                .expect("content metadata is valid"),
            )],
            meta: BTreeMap::new(),
        })
    }));
    server.into_cli()
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
                            Some(json!({"trace":"native"})),
                        )
                        .await?;
                    Ok(ToolCallResult::structured(json!({"answer": answer})))
                })
            }),
        })
        .expect("elicitation registration succeeds");
}

fn settings_registration() -> OpenAiSettingsRegistration<CommandContext> {
    let value = Arc::new(Mutex::new(json!("mm")));
    let mut fields = BTreeMap::new();
    fields.insert(
        "units".to_string(),
        OpenAiSettingsField::new(json!({"type":"string","enum":["mm","in"]}), "Units"),
    );
    OpenAiSettingsRegistration {
        read_tool: None,
        update_tool: None,
        fields,
        layout: None,
        read: {
            let value = Arc::clone(&value);
            Arc::new(move |_| {
                let value = Arc::clone(&value);
                Box::pin(
                    async move { Ok(BTreeMap::from([("units".to_string(), current(&value))])) },
                )
            })
        },
        update: Arc::new(move |set, _| {
            let value = Arc::clone(&value);
            Box::pin(async move {
                *value.lock().expect("settings mutex") = set["units"].clone();
                Ok(BTreeMap::from([("units".to_string(), current(&value))]))
            })
        }),
    }
}

fn current(value: &Arc<Mutex<Value>>) -> Value {
    value.lock().expect("settings mutex").clone()
}
