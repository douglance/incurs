//! Discovery against a fixture tree of real host configuration shapes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use incurs_mcp_discovery::{
    EnvClass, HostPaths, McpTransport, ProgramToken, ServerId, Severity, discover,
};

/// Counter making each fixture root unique within one test process.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Creates an empty fixture root.
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "incurs-discovery-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create fixture root");
    root
}

/// Writes one fixture file, creating parent directories.
fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(path, contents).expect("write fixture");
}

/// Indexes discovered servers by the name their own config gave them.
fn by_local_name(
    discovery: &incurs_mcp_discovery::Discovery,
) -> BTreeMap<String, &incurs_mcp_discovery::DiscoveredMcpServer> {
    discovery
        .servers
        .iter()
        .map(|server| (server.local_name.clone(), server))
        .collect()
}

#[test]
fn the_same_server_across_hosts_collapses_to_one_identity() {
    let root = temp_root("dedupe");
    // Claude spells it with `-y` and no version; Cursor pins `@latest` and uses
    // a different local name and a different token. All of that is noise.
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{"github":{"type":"stdio","command":"npx",
            "args":["-y","@modelcontextprotocol/server-github"],
            "env":{"GITHUB_PERSONAL_ACCESS_TOKEN":"ghp_AAA"}}}}"#,
    );
    write(
        &root,
        ".cursor/mcp.json",
        r#"{"mcpServers":{"github-mcp":{"command":"npx",
            "args":["@modelcontextprotocol/server-github@latest"],
            "env":{"GITHUB_PERSONAL_ACCESS_TOKEN":"ghp_BBB"}}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    assert_eq!(found.servers.len(), 2, "both entries should be parsed");

    let ids: Vec<&ServerId> = found.servers.iter().map(|server| &server.id).collect();
    assert_eq!(
        ids[0], ids[1],
        "the same server must collapse to one identity"
    );

    assert!(matches!(
        &found.servers[0].transport,
        McpTransport::Stdio(stdio)
            if stdio.program == ProgramToken::Package {
                ecosystem: "npm".to_string(),
                name: "@modelcontextprotocol/server-github".to_string(),
            }
    ));
}

#[test]
fn servers_differing_only_by_an_identity_env_value_stay_distinct() {
    // This shape is real: one memory server per dataset, identical argv. A
    // scheme hashing only command and args would merge two different servers.
    let root = temp_root("identity-env");
    write(
        &root,
        ".cursor/mcp.json",
        r#"{"mcpServers":{
            "short-term":{"command":"npx","args":["-y","@modelcontextprotocol/server-memory"],
                "env":{"MEMORY_FILE_PATH":"/data/short/memory.json"}},
            "long-term":{"command":"npx","args":["-y","@modelcontextprotocol/server-memory"],
                "env":{"MEMORY_FILE_PATH":"/data/long/memory.json"}}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    assert_ne!(
        servers["short-term"].id, servers["long-term"].id,
        "an identity-bearing env value must separate two servers"
    );
}

#[test]
fn rotating_a_credential_keeps_identity_but_changes_the_fingerprint() {
    let build = |token: &str| {
        let root = temp_root("rotate");
        write(
            &root,
            ".claude.json",
            &format!(
                r#"{{"mcpServers":{{"gh":{{"command":"gh-mcp","env":{{"GITHUB_TOKEN":"{token}"}}}}}}}}"#
            ),
        );
        let found = discover(&HostPaths::rooted(&root));
        let server = &found.servers[0];
        (server.id.clone(), server.fingerprint.clone())
    };

    let (id_a, fingerprint_a) = build("ghp_AAA");
    let (id_b, fingerprint_b) = build("ghp_BBB");

    assert_eq!(id_a, id_b, "a rotated credential is the same server");
    assert_ne!(
        fingerprint_a, fingerprint_b,
        "a rotated credential can change scopes, so the schema cache must invalidate"
    );
}

#[test]
fn a_non_string_env_value_does_not_cost_the_rest_of_the_file() {
    // Real configuration stores numbers here. A `BTreeMap<String, String>`
    // would fail the whole document and silently drop every other server.
    let root = temp_root("numeric-env");
    write(
        &root,
        ".cursor/mcp.json",
        r#"{"mcpServers":{
            "taskmaster":{"command":"taskmaster","env":{"MAX_TOKENS":64000,"TEMPERATURE":0.2}},
            "other":{"command":"other-server"}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    assert_eq!(servers.len(), 2, "both servers must survive");

    let McpTransport::Stdio(stdio) = &servers["taskmaster"].transport else {
        panic!("expected a stdio transport");
    };
    assert_eq!(stdio.env["MAX_TOKENS"].expose(), "64000");
    assert!(
        found
            .diagnostics
            .iter()
            .any(|d| d.code == "mcp_env_value_coerced" && d.severity == Severity::Warning),
        "the coercion should be reported"
    );
}

#[test]
fn a_jsonc_document_with_comments_and_a_trailing_comma_parses() {
    let root = temp_root("jsonc");
    write(
        &root,
        "Library/Application Support/Code/User/mcp.json",
        r#"{
            // VS Code documents this file as JSONC, and real files use it.
            "servers": {
                /* block comment */
                "docs": { "command": "docs-server", "args": ["--stdio"] },
            },
        }"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    assert!(
        found.unreadable.is_empty(),
        "JSONC must parse, got {:?}",
        found.unreadable
    );
    assert_eq!(found.servers.len(), 1);
    assert_eq!(found.servers[0].local_name, "docs");
}

#[test]
fn a_double_slash_inside_a_string_is_not_a_comment() {
    let root = temp_root("url-in-jsonc");
    write(
        &root,
        ".gemini/settings.json",
        r#"{"mcpServers":{"api":{"url":"https://example.com/mcp"}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    assert!(found.unreadable.is_empty(), "{:?}", found.unreadable);
    let McpTransport::StreamableHttp(http) = &found.servers[0].transport else {
        panic!("expected an http transport");
    };
    assert_eq!(http.canonical_url, "https://example.com/mcp");
}

#[test]
fn a_malformed_entry_does_not_hide_its_healthy_neighbours() {
    let root = temp_root("malformed");
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{
            "good":{"command":"a"},
            "broken":{"nothing":true},
            "alsogood":{"command":"b"}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    assert_eq!(servers.len(), 2, "the two valid entries must survive");
    assert!(servers.contains_key("good") && servers.contains_key("alsogood"));
    assert!(
        found
            .diagnostics
            .iter()
            .any(|d| d.code == "mcp_missing_command" && d.path.ends_with("/broken"))
    );
}

#[test]
fn codex_toml_is_read_and_a_disabled_entry_is_marked() {
    let root = temp_root("codex");
    write(
        &root,
        ".codex/config.toml",
        r#"
[mcp_servers.apoc]
command = "/opt/bin/apoc"
args = ["--mcp"]

[mcp_servers.remote]
url = "https://example.dev/mcp"

[mcp_servers.off]
command = "./thing"
enabled = false
"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    assert_eq!(servers.len(), 3);
    assert!(servers["apoc"].enabled);
    assert!(!servers["off"].enabled, "enabled = false must be honoured");
    assert!(matches!(
        servers["remote"].transport,
        McpTransport::StreamableHttp(_)
    ));
    // An absolute command is a path, not a bare name. Host configs use these,
    // and rejecting them would drop the server.
    let McpTransport::Stdio(stdio) = &servers["apoc"].transport else {
        panic!("expected stdio");
    };
    assert_eq!(
        stdio.program,
        ProgramToken::Path {
            value: PathBuf::from("/opt/bin/apoc")
        }
    );
}

#[test]
fn amp_uses_a_flat_dotted_map_key() {
    let root = temp_root("amp");
    write(
        &root,
        ".config/amp/settings.json",
        r#"{"amp.mcpServers":{"fql":{"command":"npx","args":["fql","--mcp"]}},
            "amp.other":true}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    assert_eq!(found.servers.len(), 1);
    assert_eq!(found.servers[0].local_name, "fql");
}

#[test]
fn a_claude_project_map_is_scoped_to_its_project() {
    let root = temp_root("claude-projects");
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{"global":{"command":"g"}},
            "projects":{"/work/repo":{"mcpServers":{"local":{"command":"l"}}}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    assert_eq!(servers.len(), 2);
    assert_eq!(
        servers["global"].source.scope,
        incurs_mcp_discovery::ConfigScope::User
    );
    assert_eq!(
        servers["local"].source.scope,
        incurs_mcp_discovery::ConfigScope::Project {
            root: PathBuf::from("/work/repo")
        }
    );
    assert_eq!(
        servers["local"].source.pointer,
        "/projects/~1work~1repo/mcpServers/local"
    );
}

#[test]
fn an_unresolvable_prompt_placeholder_is_reported_rather_than_substituted() {
    let root = temp_root("placeholder");
    write(
        &root,
        ".vscode/mcp.json",
        r#"{"servers":{"azure":{"command":"az-mcp","env":{"AZURE_PAT":"${input:azure-pat}"}}}}"#,
    );

    let mut paths = HostPaths::rooted(&root);
    paths.project = Some(root.clone());
    let found = discover(&paths);
    let servers = by_local_name(&found);
    assert_eq!(
        servers["azure"].requires_input,
        vec!["input:azure-pat".to_string()],
        "a prompt placeholder must be reported, not silently substituted"
    );
    assert!(
        found
            .diagnostics
            .iter()
            .any(|d| d.code == "mcp_unresolved_placeholder")
    );
}

#[test]
fn an_env_placeholder_is_expanded_from_the_supplied_environment() {
    let root = temp_root("env-placeholder");
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{"s":{"command":"s","env":{"REGION":"${env:MY_REGION}"}}}}"#,
    );

    let mut paths = HostPaths::rooted(&root);
    paths
        .env
        .insert("MY_REGION".to_string(), "eu-west-1".to_string());
    let found = discover(&paths);
    let McpTransport::Stdio(stdio) = &found.servers[0].transport else {
        panic!("expected stdio");
    };
    assert_eq!(stdio.env["REGION"].expose(), "eu-west-1");
    assert!(found.servers[0].requires_input.is_empty());
}

#[test]
fn two_hosts_with_different_tokens_for_one_endpoint_collapse() {
    let root = temp_root("http-auth");
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{"api":{"type":"http","url":"https://Example.com:443/mcp",
            "headers":{"Authorization":"Bearer AAA"}}}}"#,
    );
    write(
        &root,
        ".cursor/mcp.json",
        r#"{"mcpServers":{"the-api":{"url":"https://example.com/mcp",
            "headers":{"Authorization":"Bearer BBB"}}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    assert_eq!(found.servers.len(), 2);
    assert_eq!(
        found.servers[0].id, found.servers[1].id,
        "default port, host case, and the credential itself are all noise"
    );
}

#[test]
fn an_authenticated_endpoint_is_not_the_same_server_as_an_anonymous_one() {
    let root = temp_root("http-anon");
    write(
        &root,
        ".claude.json",
        r#"{"mcpServers":{"a":{"url":"https://example.com/mcp","headers":{"Authorization":"Bearer X"}},
            "b":{"url":"https://example.com/mcp"}}}"#,
    );

    let found = discover(&HostPaths::rooted(&root));
    let servers = by_local_name(&found);
    // An anonymous connection lists a different tool set, so merging these
    // would poison the schema cache for one of them.
    assert_ne!(servers["a"].id, servers["b"].id);
}

#[test]
fn a_credential_never_reaches_identity_or_any_rendered_form() {
    const CANARY: &str = "CANARY_TOKEN_9f3a";
    let root = temp_root("canary");
    write(
        &root,
        ".claude.json",
        &format!(r#"{{"mcpServers":{{"s":{{"command":"s","env":{{"API_TOKEN":"{CANARY}"}}}}}}}}"#),
    );

    let found = discover(&HostPaths::rooted(&root));
    let server = &found.servers[0];

    assert!(
        !server.id.as_str().contains(CANARY) && !server.fingerprint.as_str().contains(CANARY),
        "a credential must not appear in a digest"
    );
    // Debug is the format a log or a panic reaches for.
    let rendered = format!("{:?}", server.transport);
    assert!(
        !rendered.contains(CANARY),
        "a credential must not survive Debug formatting: {rendered}"
    );

    let McpTransport::Stdio(stdio) = &server.transport else {
        panic!("expected stdio");
    };
    assert_eq!(stdio.env["API_TOKEN"].class(), EnvClass::Secret);
    assert_eq!(stdio.env["API_TOKEN"].expose(), CANARY);
}
