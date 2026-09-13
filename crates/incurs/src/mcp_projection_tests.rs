use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::{Value, json};

use super::{McpDiscovery, McpServeOptions, McpToolFilter, http_service, remote_commands};
use crate::cli::Cli;
use crate::command::{CommandDef, McpAnnotations, McpCommandOptions, TypedResult};
use crate::tool::{ToolCallOptions, ToolCallOutcome};

#[derive(serde::Serialize, schemars::JsonSchema)]
struct NaturalData {
    data: Vec<String>,
}

fn fixture(discovery: McpDiscovery) -> Cli {
    let values = CommandDef::typed::<(), (), (), Vec<String>, _, _>("values", |_| async {
        TypedResult::ok(vec!["record".to_string()])
    })
    .mcp(read_only())
    .done();
    let object = CommandDef::typed::<(), (), (), NaturalData, _, _>("object", |_| async {
        TypedResult::ok(NaturalData {
            data: vec!["natural".to_string()],
        })
    })
    .mcp(read_only())
    .done();
    Cli::create("projection-test")
        .mcp(McpServeOptions {
            tools: McpToolFilter {
                discovery,
                ..Default::default()
            },
            ..Default::default()
        })
        .command("values", values)
        .command("object", object)
}

fn read_only() -> McpCommandOptions {
    McpCommandOptions {
        annotations: Some(McpAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn invocation(discovery: McpDiscovery, name: &str) -> CallToolRequestParams {
    match discovery {
        McpDiscovery::Direct => CallToolRequestParams::new(name.to_string()),
        McpDiscovery::Progressive => CallToolRequestParams::new("call_read_tool").with_arguments(
            serde_json::Map::from_iter([
                ("name".to_string(), json!(name)),
                ("arguments".to_string(), json!({})),
            ]),
        ),
    }
}

#[tokio::test]
async fn native_mcp_projects_arrays_and_restores_remote_catalog_contracts() {
    for discovery in [McpDiscovery::Direct, McpDiscovery::Progressive] {
        let cli = fixture(discovery);
        let original = cli
            .tool_catalog()
            .get("values")
            .unwrap()
            .output_schema
            .clone();
        let app = axum::Router::new().nest_service("/mcp", http_service(&cli).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ().serve(StreamableHttpClientTransport::from_uri(url.clone())).await.unwrap();
        let definition: Value = match discovery {
            McpDiscovery::Direct => serde_json::to_value(
                client
                    .list_all_tools()
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|tool| tool.name == "values")
                    .unwrap(),
            )
            .unwrap(),
            McpDiscovery::Progressive => client
                .call_tool(
                    CallToolRequestParams::new("get_tool_details").with_arguments(
                        serde_json::Map::from_iter([("name".to_string(), json!("values"))]),
                    ),
                )
                .await
                .unwrap()
                .structured_content
                .unwrap(),
        };
        assert_eq!(definition["outputSchema"]["type"], "object");
        assert_eq!(
            definition["outputSchema"]["properties"]["data"]["type"],
            "array"
        );
        assert!(definition["_meta"]["io.incurs.outputProjection"].is_object());
        let result = client
            .call_tool(invocation(discovery, "values"))
            .await
            .unwrap();
        assert_eq!(result.structured_content, Some(json!({"data":["record"]})));
        assert!(
            result
                .meta
                .as_ref()
                .unwrap()
                .0
                .contains_key("io.incurs.outputProjection")
        );
        let object = client
            .call_tool(invocation(discovery, "object"))
            .await
            .unwrap();
        assert_eq!(object.structured_content, Some(json!({"data":["natural"]})));
        assert!(
            object
                .meta
                .as_ref()
                .is_none_or(|meta| !meta.0.contains_key("io.incurs.outputProjection"))
        );

        let commands = remote_commands(url).await.unwrap();
        assert_eq!(commands["values"].output_schema, original);
        let mut imported = Cli::create("imported");
        for (name, command) in commands {
            imported = imported.command(name, command);
        }
        for (name, expected) in [
            ("values", json!(["record"])),
            ("object", json!({"data":["natural"]})),
        ] {
            let outcome = imported
                .tool_catalog()
                .call(name, Default::default(), ToolCallOptions::isolated())
                .await;
            assert!(matches!(outcome, ToolCallOutcome::Ok { data, .. } if data == expected));
        }
        server.abort();
    }
}
