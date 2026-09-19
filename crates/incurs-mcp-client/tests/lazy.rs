//! Lazy startup, caching, partial failure, and cancellation.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use incurs_codemode::McpClient;
use incurs_mcp_client::testing::{FixtureBehavior, FixtureFactory, FixtureServer};
use incurs_mcp_client::{
    HealthCode, HealthRegistry, IoBridge, LazyMcpClient, ServerHealth, ToolCacheStore,
};
use incurs_mcp_discovery::{HostPaths, discover};
use serde_json::json;

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Creates an isolated directory for one test.
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "incurs-client-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create root");
    root
}

/// Builds one discovered server from a minimal fixture config.
fn discovered(root: &std::path::Path, name: &str) -> incurs_mcp_discovery::DiscoveredMcpServer {
    let config = root.join(".claude.json");
    std::fs::write(
        &config,
        format!(r#"{{"mcpServers":{{"{name}":{{"command":"/nonexistent/{name}"}}}}}}"#),
    )
    .expect("write config");
    let found = discover(&HostPaths::rooted(root));
    found.servers.into_iter().next().expect("one server")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_warm_cache_answers_without_connecting() {
    let root = temp_root("warm-cache");
    let server = discovered(&root, "issues");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));
    let fixture = FixtureServer::new("issues", &["list", "get"]);

    // First pass: the cache is cold, so the server is contacted once.
    let cold = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert_eq!(cold.list_tools().await.expect("tools").len(), 2);
    assert_eq!(fixture.connect_count(), 1);

    // Second pass, fresh client over the same cache: nothing may be started.
    let warm = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    let tools = warm.list_tools().await.expect("tools");

    assert_eq!(tools.len(), 2, "the cache must answer in full");
    assert_eq!(
        fixture.connect_count(),
        1,
        "a warm cache must not open a connection"
    );
    assert!(!warm.is_connected());
}

#[tokio::test(flavor = "multi_thread")]
async fn listing_capabilities_never_starts_a_server_that_is_not_called() {
    let root = temp_root("unused");
    let used = discovered(&root, "used");
    let unused_root = temp_root("unused-b");
    let unused = discovered(&unused_root, "unused");

    let used_fixture = FixtureServer::new("used", &["ping"]);
    let unused_fixture = FixtureServer::new("unused", &["ping"]);

    let used_client = LazyMcpClient::new(&used, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(used_fixture.clone()));
    let unused_client = LazyMcpClient::new(&unused, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(unused_fixture.clone()));

    // Both are listed; only one is called.
    let _ = used_client.list_tools().await.expect("tools");
    let _ = unused_client.list_tools().await.expect("tools");
    let value = used_client
        .call_tool("ping", json!({"value": "x"}))
        .await
        .expect("call");

    assert_eq!(value, json!({"value": "x"}));
    assert_eq!(used_fixture.call_count(), 1);
    assert_eq!(
        unused_fixture.call_count(),
        0,
        "a server nothing called must serve no calls"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_yields_no_tools_instead_of_an_error() {
    let root = temp_root("unreachable");
    let server = discovered(&root, "broken");
    let health = HealthRegistry::new();
    let client = LazyMcpClient::new(&server, IoBridge::current(), Arc::clone(&health))
        .with_transport_factory(FixtureFactory::new(
            FixtureServer::new("broken", &["x"]).with_behavior(FixtureBehavior::RefuseConnection),
        ));

    // Empty rather than Err: one dead server must not fail the whole
    // connector set, and it genuinely exposes no tools right now.
    let tools = client.list_tools().await.expect("must not error");
    assert!(tools.is_empty());

    let report = health.get(&server.id).expect("health recorded");
    assert_eq!(report.state, ServerHealth::Unavailable);
    assert!(
        !report.summary().is_empty(),
        "the reason must be reportable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_server_is_not_permanently_empty() {
    let root = temp_root("recovery");
    let server = discovered(&root, "flaky");
    let failing =
        FixtureServer::new("flaky", &["ping"]).with_behavior(FixtureBehavior::RefuseConnection);
    let client = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(failing));

    assert!(client.list_tools().await.expect("first").is_empty());

    // A new client over a working fixture stands in for the server recovering.
    // The point is that the empty list was never memoized as the answer.
    let working = FixtureServer::new("flaky", &["ping"]);
    let recovered = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(working));
    assert_eq!(recovered.list_tools().await.expect("second").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_server_is_never_started() {
    let root = temp_root("disabled");
    std::fs::create_dir_all(root.join(".codex")).expect("codex dir");
    std::fs::write(
        root.join(".codex/config.toml"),
        "[mcp_servers.off]\ncommand = \"off\"\nenabled = false\n",
    )
    .expect("write");
    let server = discover(&HostPaths::rooted(&root))
        .servers
        .into_iter()
        .next()
        .expect("one server");
    let fixture = FixtureServer::new("off", &["ping"]);
    let client = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(fixture.clone()));

    assert!(client.list_tools().await.expect("tools").is_empty());
    assert_eq!(
        fixture.connect_count(),
        0,
        "a server disabled in its own config must never be contacted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_ends_a_call_the_server_never_answers() {
    let root = temp_root("cancel");
    let server = discovered(&root, "slow");
    let fixture = FixtureServer::new("slow", &["wait"]).with_behavior(FixtureBehavior::Hang);
    let client = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(fixture.clone()));

    let cancellation = tokio_util::sync::CancellationToken::new();
    let trigger = cancellation.clone();
    let probe = fixture.clone();
    tokio::spawn(async move {
        while probe.call_count() == 0 {
            tokio::task::yield_now().await;
        }
        trigger.cancel();
    });

    let result = client
        .call_tool_cancellable("wait", json!({}), &cancellation)
        .await;

    assert_eq!(result, Err("Call cancelled".to_string()));
    assert_eq!(fixture.call_count(), 1, "the call must really have started");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_configuration_invalidates_the_cached_schema() {
    let root = temp_root("invalidate");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));
    let config = root.join(".claude.json");

    let write_config = |token: &str| {
        std::fs::write(
            &config,
            format!(r#"{{"mcpServers":{{"s":{{"command":"s","env":{{"DATASET":"{token}"}}}}}}}}"#),
        )
        .expect("write");
        discover(&HostPaths::rooted(&root))
            .servers
            .into_iter()
            .next()
            .expect("one server")
    };

    let first = write_config("alpha");
    let fixture = FixtureServer::new("s", &["a", "b"]);
    let client = LazyMcpClient::new(&first, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert_eq!(client.list_tools().await.expect("tools").len(), 2);
    assert_eq!(fixture.connect_count(), 1);

    // A different dataset is a different server, so its cache entry must not
    // be reused under the first server's identity.
    let second = write_config("beta");
    assert_ne!(first.id, second.id);
    let refreshed = LazyMcpClient::new(&second, IoBridge::current(), HealthRegistry::new())
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert_eq!(refreshed.list_tools().await.expect("tools").len(), 2);
    assert_eq!(
        fixture.connect_count(),
        2,
        "a changed configuration must force a refresh"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_connection_is_not_retried_by_the_next_process() {
    let root = temp_root("negative-cache");
    let server = discovered(&root, "broken");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));
    let fixture =
        FixtureServer::new("broken", &["list"]).with_behavior(FixtureBehavior::RefuseConnection);

    let first = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert!(first.list_tools().await.expect("empty list").is_empty());
    assert_eq!(fixture.attempt_count(), 1, "the first run must try once");

    // A second process over the same cache. This is the whole point: an
    // unattended run must not pay every dead server's connect timeout again.
    let health = HealthRegistry::new();
    let second = LazyMcpClient::new(&server, IoBridge::current(), Arc::clone(&health))
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert!(second.list_tools().await.expect("empty list").is_empty());
    assert_eq!(
        fixture.attempt_count(),
        1,
        "a recorded failure must suppress the second attempt"
    );

    // Suppressing the attempt is only half of it. An empty tool list that reads
    // as `Healthy` is worse than the retry it saved: a health check would call a
    // dead server available, and the model would be told an empty namespace is
    // fine. The recorded reason has to survive the round trip.
    let report = health.get(second.id()).expect("a health report");
    assert_ne!(
        report.state,
        ServerHealth::Healthy,
        "a server answered from a recorded failure must not read as healthy"
    );
    assert_eq!(report.code, Some(HealthCode::SpawnNotFound));
    assert!(
        report.from_cache,
        "the answer came from disk, not the server"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_refresh_ignores_a_recorded_failure() {
    let root = temp_root("negative-refresh");
    let server = discovered(&root, "broken");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));
    let fixture =
        FixtureServer::new("broken", &["list"]).with_behavior(FixtureBehavior::RefuseConnection);

    let first = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    let _ = first.list_tools().await;
    assert_eq!(fixture.attempt_count(), 1);

    let second = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    assert!(second.refresh().await.is_err());
    assert_eq!(
        fixture.attempt_count(),
        2,
        "an explicit refresh is the escape hatch and must always contact the server"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_configuration_discards_a_recorded_failure() {
    let root = temp_root("negative-fingerprint");
    let server = discovered(&root, "broken");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));
    let fixture =
        FixtureServer::new("broken", &["list"]).with_behavior(FixtureBehavior::RefuseConnection);

    let first = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    let _ = first.list_tools().await;
    assert_eq!(fixture.attempt_count(), 1);

    // Rewriting the entry changes its fingerprint, which is what a developer
    // fixing the configuration actually does. The failure must not outlive it.
    let edited = temp_root("negative-fingerprint-edited");
    std::fs::write(
        edited.join(".claude.json"),
        r#"{"mcpServers":{"broken":{"command":"/nonexistent/broken","args":["--now-fixed"]}}}"#,
    )
    .expect("write config");
    let changed = discover(&HostPaths::rooted(&edited))
        .servers
        .into_iter()
        .next()
        .expect("one server");

    let second = LazyMcpClient::new(&changed, IoBridge::current(), HealthRegistry::new())
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(fixture.clone()));
    let _ = second.list_tools().await;
    assert_eq!(
        fixture.attempt_count(),
        2,
        "a configuration change must un-suppress the server"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_schema_survives_a_later_failure() {
    let root = temp_root("negative-preserves");
    let server = discovered(&root, "flaky");
    let cache = Arc::new(ToolCacheStore::new(root.join("cache")));

    let healthy = FixtureServer::new("flaky", &["list", "get"]);
    let warm = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(healthy));
    assert_eq!(warm.list_tools().await.expect("tools").len(), 2);

    // The server goes away. A refresh fails and records that, but the schema it
    // already published is still the best answer anyone has.
    let down =
        FixtureServer::new("flaky", &["list"]).with_behavior(FixtureBehavior::RefuseConnection);
    let failing = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(Arc::clone(&cache))
        .with_transport_factory(FixtureFactory::new(down.clone()));
    assert!(failing.refresh().await.is_err());

    let after = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_cache(cache)
        .with_transport_factory(FixtureFactory::new(down));
    assert_eq!(
        after.list_tools().await.expect("tools").len(),
        2,
        "recording a failure must not throw away a usable cached schema"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_sse_entry_is_refused_by_name_and_never_contacted() {
    let root = temp_root("sse-refused");
    std::fs::write(
        root.join(".claude.json"),
        r#"{"mcpServers":{"legacy":{"type":"sse","url":"http://127.0.0.1:1/sse"}}}"#,
    )
    .expect("write config");
    let server = discover(&HostPaths::rooted(&root))
        .servers
        .into_iter()
        .next()
        .expect("one server");

    let health = HealthRegistry::new();
    let client = LazyMcpClient::new(&server, IoBridge::current(), Arc::clone(&health));

    // Empty on its own is what the old behaviour produced too. The point of the
    // change is that the reason survives, so the namespace can explain itself
    // rather than leaving the model to invent methods for an empty object.
    assert!(client.list_tools().await.expect("no tools").is_empty());
    let refusal = client.refusal().expect("an sse entry is refused");
    assert_eq!(refusal.code, Some(HealthCode::TransportUnsupported));
    assert_eq!(refusal.state, ServerHealth::InvalidConfiguration);
    assert!(
        refusal.summary().contains("transport"),
        "the reason a person reads must name the transport: {}",
        refusal.summary()
    );
    assert!(
        health
            .get(client.id())
            .is_none_or(|report| report.state != ServerHealth::Healthy),
        "a refused entry must never be recorded healthy"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_closes_an_open_connection() {
    let root = temp_root("shutdown");
    let server = discovered(&root, "closable");
    let fixture = FixtureServer::new("closable", &["list"]);
    let client = LazyMcpClient::new(&server, IoBridge::current(), HealthRegistry::new())
        .with_transport_factory(FixtureFactory::new(fixture.clone()));

    assert_eq!(client.list_tools().await.expect("tools").len(), 1);
    // Give the spawned server task its turn to register as open.
    for _ in 0..100 {
        if fixture.open_connections() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.open_connections(), 1, "the connection must be open");

    client.shutdown().await;

    // The close is observed from the server's side on purpose. Asserting from
    // the client would pass against the previous implementation, which returned
    // without ever cancelling anything.
    let mut closed = false;
    for _ in 0..200 {
        if fixture.open_connections() == 0 {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(closed, "shutdown must actually reach the server");
}
