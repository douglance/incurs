//! End-to-end composition across several MCP servers in one Code Mode program.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use incurs_codemode::{CodeMode, Connector, MemoryStore};
use incurs_mcp_client::testing::{FixtureFactory, FixtureServer};
use incurs_mcp_client::{HealthRegistry, IoBridge, LazyMcpClient};
use incurs_mcp_discovery::{HostPaths, discover};
use incurs_mcp_registry::{
    ApprovalMode, DiscoveredToolPolicyResolver, HealthAwareMcpConnector, NamespaceLedger,
    NamespaceRequest,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "incurs-compose-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create root");
    root
}

/// Builds connectors over in-process fixtures, one per named server.
fn connectors(
    root: &std::path::Path,
    servers: &[(&str, FixtureServer)],
) -> Vec<Arc<dyn Connector>> {
    let entries = servers
        .iter()
        .map(|(name, _)| format!(r#""{name}":{{"command":"/bin/{name}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        root.join(".claude.json"),
        format!(r#"{{"mcpServers":{{{entries}}}}}"#),
    )
    .expect("write config");

    let discovered = discover(&HostPaths::rooted(root)).servers;
    let health = HealthRegistry::new();
    let policy: Arc<dyn incurs_codemode::ToolPolicyResolver> = Arc::new(
        DiscoveredToolPolicyResolver::new(ApprovalMode::AnnotatedReads),
    );

    let mut ledger = NamespaceLedger::default();
    let requests: Vec<NamespaceRequest> = discovered
        .iter()
        .map(|server| NamespaceRequest {
            id: server.id.clone(),
            display_name: server.local_name.clone(),
        })
        .collect();
    let namespaces = ledger.assign(&requests, 0);

    discovered
        .iter()
        .map(|server| {
            let fixture = servers
                .iter()
                .find(|(name, _)| *name == server.local_name)
                .map(|(_, fixture)| fixture.clone())
                .expect("fixture for server");
            let namespace = namespaces[server.id.as_str()].clone();
            let client = Arc::new(
                LazyMcpClient::new(server, IoBridge::current(), Arc::clone(&health))
                    .with_transport_factory(FixtureFactory::new(fixture)),
            );
            let inner = incurs_codemode::McpConnector::new(
                namespace.clone(),
                Arc::clone(&client) as Arc<dyn incurs_codemode::McpClient>,
            )
            .with_policy_resolver(Arc::clone(&policy));
            Arc::new(HealthAwareMcpConnector::new(
                server.id.clone(),
                namespace,
                inner,
                Arc::clone(&health),
                client,
            )) as Arc<dyn Connector>
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_program_composes_three_servers_and_returns_only_its_own_result() {
    let root = temp_root("three-servers");
    let issues = FixtureServer::new("issues", &["search"]);
    let tickets = FixtureServer::new("tickets", &["list"]);
    let notifications = FixtureServer::new("notifications", &["send"]);

    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        incurs_codemode_local::LocalExecutor::default(),
        connectors(
            &root,
            &[
                ("issues", issues.clone()),
                ("tickets", tickets.clone()),
                ("notifications", notifications.clone()),
            ],
        ),
    );

    // One search interface reaches every server, so the model never needs to
    // know which server implements a capability before looking for it. Search
    // scores by token coverage rather than meaning, so each intent is its own
    // query, which is also how an agent uses it.
    for (query, expected) in [
        ("search", "issues"),
        ("list", "tickets"),
        ("send", "notifications"),
    ] {
        let found = codemode.search(query).await.expect("search");
        assert!(
            found
                .results
                .iter()
                .any(|result| result.connector == expected),
            "searching {query:?} did not reach {expected}"
        );
        assert!(
            found
                .results
                .iter()
                .any(|result| result.types.contains("declare const")),
            "search must return declarations the model can write against"
        );
    }

    let state = codemode
        .execute(
            r"async () => {
                const [found, known] = await Promise.all([
                    issues.search({ value: 'bug' }),
                    tickets.list({ value: 'bug' }),
                ]);
                const missing = found.value !== known.value ? [found.value] : [];
                if (missing.length > 0) {
                    await notifications.send({ value: missing.join(',') });
                }
                return { checked: 2, missing: missing.length };
            }",
        )
        .await
        .expect("execute");

    assert_eq!(
        state.status,
        incurs_codemode::ExecutionStatus::Completed,
        "program failed: {:?}",
        state.error
    );
    assert_eq!(
        state.result.expect("result")["checked"],
        serde_json::json!(2)
    );
    // One execution, three servers, all read tools running without a pause.
    assert_eq!(issues.call_count(), 1);
    assert_eq!(tickets.call_count(), 1);
    assert_eq!(notifications.call_count(), 0, "nothing was missing");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_unavailable_server_does_not_disable_the_healthy_ones() {
    let root = temp_root("partial-failure");
    let healthy = FixtureServer::new("healthy", &["ping"]);
    let broken = FixtureServer::new("broken", &["ping"])
        .with_behavior(incurs_mcp_client::testing::FixtureBehavior::RefuseConnection);

    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        incurs_codemode_local::LocalExecutor::default(),
        connectors(
            &root,
            &[("healthy", healthy.clone()), ("broken", broken.clone())],
        ),
    );

    // Search must still work: it is the model's only way to learn what exists.
    let found = codemode.search("ping").await.expect("search must not fail");
    assert!(
        found
            .results
            .iter()
            .any(|result| result.connector == "healthy"),
        "the healthy server must remain searchable"
    );

    let state = codemode
        .execute("async () => (await healthy.ping({ value: 'ok' })).value")
        .await
        .expect("execute");
    assert_eq!(state.status, incurs_codemode::ExecutionStatus::Completed);
    assert_eq!(state.result.expect("result"), serde_json::json!("ok"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unavailable_namespace_tells_the_model_why_it_is_empty() {
    let root = temp_root("explains");
    let broken = FixtureServer::new("broken", &["ping"])
        .with_behavior(incurs_mcp_client::testing::FixtureBehavior::RefuseConnection);
    let built = connectors(&root, &[("broken", broken)]);
    let description = built[0].describe().await.expect("describe");

    assert!(description.tools.is_empty());
    let instructions = description.instructions.expect("an explanation");
    assert!(
        instructions.contains("UNAVAILABLE"),
        "an empty namespace must say why: {instructions}"
    );
    assert!(
        !instructions.contains("/bin/broken"),
        "the explanation must not leak the configured command: {instructions}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_program_reaches_tools_resources_and_prompts_of_the_same_server() {
    let root = temp_root("all-three-kinds");
    let docs = FixtureServer::new("docs", &["search"])
        .with_resources(&["file:///readme.md", "file:///guide.md"])
        .with_prompts(&["summarize"]);

    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        incurs_codemode_local::LocalExecutor::default(),
        connectors(&root, &[("docs", docs.clone())]),
    );

    // MCP servers expose tools, resources, and prompts. A composition layer that
    // reaches only the first is not composing the server, it is composing a
    // third of it, so all three are asserted from inside one program.
    let state = codemode
        .execute(
            r"async () => {
                const [tool, resources, prompts] = await Promise.all([
                    docs.search({ value: 'anything' }),
                    docs.mcp_resources(),
                    docs.mcp_prompts(),
                ]);
                const read = await docs.mcp_read_resource({ uri: resources[0].uri });
                const rendered = await docs.mcp_get_prompt({ name: prompts[0].name });
                return {
                    tool_ran: tool !== null && tool !== undefined,
                    resource_uris: resources.map(r => r.uri),
                    prompt_names: prompts.map(p => p.name),
                    read_text: read.contents[0].text,
                    prompt_description: rendered.description,
                };
            }",
        )
        .await
        .expect("execute");

    assert_eq!(state.status, incurs_codemode::ExecutionStatus::Completed);
    let result = state.result.expect("a result");

    assert_eq!(result["tool_ran"], true, "the tool call must still work");
    assert_eq!(
        result["resource_uris"],
        serde_json::json!(["file:///readme.md", "file:///guide.md"]),
        "every resource the server lists must reach the program"
    );
    assert_eq!(
        result["prompt_names"],
        serde_json::json!(["summarize"]),
        "every prompt the server lists must reach the program"
    );
    assert_eq!(
        result["read_text"], "contents of file:///readme.md",
        "reading a resource must return the server's own content"
    );
    assert_eq!(
        result["prompt_description"], "rendered summarize",
        "rendering a prompt must return the server's own rendering"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_tool_named_like_a_synthetic_method_keeps_its_own_tool() {
    let root = temp_root("name-collision");
    // The namespace is flat, so a server that already publishes `mcp_resources`
    // must keep it. Shadowing the server's own tool would break that server to
    // add a convenience.
    let odd = FixtureServer::new("odd", &["mcp_resources"]).with_resources(&["file:///x"]);

    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        incurs_codemode_local::LocalExecutor::default(),
        connectors(&root, &[("odd", odd.clone())]),
    );

    let state = codemode
        .execute(
            r"async () => {
                // The server's own echo tool returns its arguments; the synthetic
                // one would return an array of resources.
                return await odd.mcp_resources({ marker: 'the servers own tool' });
            }",
        )
        .await
        .expect("execute");

    assert_eq!(state.status, incurs_codemode::ExecutionStatus::Completed);
    assert_eq!(
        state.result.expect("a result")["marker"],
        "the servers own tool",
        "the server's own tool must win over the synthetic method"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tools_only_server_answers_the_resource_methods_with_empty_lists() {
    let root = temp_root("tools-only");
    // Most MCP servers implement tools and nothing else, and answer
    // `resources/list` with method-not-found. These methods are offered on every
    // reachable namespace, so a program cannot know which servers have them; an
    // error here would abort a whole composition for asking a fair question.
    let plain = FixtureServer::new("plain", &["search"]).without_resources_or_prompts();

    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        incurs_codemode_local::LocalExecutor::default(),
        connectors(&root, &[("plain", plain.clone())]),
    );

    let state = codemode
        .execute(
            r"async () => {
                const [resources, prompts, tool] = await Promise.all([
                    plain.mcp_resources(),
                    plain.mcp_prompts(),
                    plain.search({ value: 'still works' }),
                ]);
                return {
                    resources,
                    prompts,
                    tool_still_works: tool.value === 'still works',
                };
            }",
        )
        .await
        .expect("execute");

    assert_eq!(state.status, incurs_codemode::ExecutionStatus::Completed);
    let result = state.result.expect("a result");
    assert_eq!(
        result["resources"],
        serde_json::json!([]),
        "a tools-only server must report no resources rather than failing"
    );
    assert_eq!(
        result["prompts"],
        serde_json::json!([]),
        "a tools-only server must report no prompts rather than failing"
    );
    assert_eq!(
        result["tool_still_works"], true,
        "softening the list methods must not affect tool calls"
    );
}
