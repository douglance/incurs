//! Runtime adapter for MCP servers loaded from an Agent Plugin directory.

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use crate::agent_plugin::loader::AgentPluginStdioMcpServer;
use crate::agent_plugin::loader::{
    AgentPluginMcpServer, AgentPluginMcpTransport, LoadedAgentPlugin,
};
use crate::cli::Cli;
use crate::mcp::McpRemoteOptions;

/// Runtime policy used while connecting a loaded Agent Plugin.
#[derive(Clone)]
pub struct AgentPluginRuntimeOptions {
    /// Base subprocess environment before plugin values and reserved variables are applied.
    pub base_environment: BTreeMap<OsString, OsString>,
    /// Exact MCP protocol standards accepted by the client.
    pub standards: incurs_mcp_protocol::McpStandardSet,
    /// The client every request to an HTTP MCP server goes through.
    ///
    /// When set, Streamable HTTP and legacy SSE servers connect with the
    /// portable [`crate::mcp_client::McpHttpClient`] through it on every
    /// target. When unset, native builds connect with `rmcp`, and wasm32
    /// builds, which have no default client, report each HTTP server as failed
    /// with `HTTP_CLIENT_REQUIRED`.
    pub http_client: Option<crate::outbound::SharedHttpClient>,
}

impl std::fmt::Debug for AgentPluginRuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPluginRuntimeOptions")
            .field("base_environment", &self.base_environment)
            .field("standards", &self.standards)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "dyn HttpClient"),
            )
            .finish()
    }
}

impl Default for AgentPluginRuntimeOptions {
    fn default() -> Self {
        Self {
            base_environment: std::env::vars_os().collect(),
            standards: incurs_mcp_protocol::McpStandardSet::default(),
            http_client: None,
        }
    }
}

/// Connection result for one independently started Agent Plugin MCP server.
#[derive(Debug, Clone)]
pub struct AgentPluginMcpServerStatus {
    /// Server key from `mcp.json`.
    pub name: String,
    /// Configured MCP transport.
    pub transport: AgentPluginMcpTransport,
    /// Number of tools projected when the connection succeeded.
    pub tool_count: usize,
    /// Connection, authentication, or handshake failure when the server was skipped.
    pub error: Option<String>,
    /// Stable machine-readable code for `error`, such as `MCP_UNAUTHORIZED` or
    /// `MCP_STDIO_UNAVAILABLE`, when the failure carries one.
    pub error_code: Option<String>,
}

/// Connected Agent Plugin tool surface and per-server connection results.
#[derive(Clone)]
pub struct ConnectedAgentPlugin {
    /// Namespaced non-CLI invocation surface for every connected MCP server.
    pub catalog: crate::tool::ToolCatalog,
    /// Stable server connection results in `mcp.json` key order.
    pub servers: Vec<AgentPluginMcpServerStatus>,
}

/// Fatal failure while preparing a loaded Agent Plugin runtime.
#[derive(Debug, thiserror::Error)]
pub enum AgentPluginRuntimeError {
    /// The client-managed data directory could not be prepared.
    #[error("failed to prepare Agent Plugin data directory: {0}")]
    Io(#[from] std::io::Error),
    /// Connected tools could not be assembled into a catalog.
    #[error("failed to build Agent Plugin tool catalog: {0}")]
    Catalog(#[from] crate::tool::ToolCatalogError),
}

/// Connects every valid MCP binding independently and returns one namespaced tool catalog.
///
/// Native builds can start stdio servers and connect HTTP servers with `rmcp`,
/// or with the portable [`crate::mcp_client::McpHttpClient`] when
/// [`AgentPluginRuntimeOptions::http_client`] is set. wasm32 builds connect
/// HTTP servers with the portable client through that option and report each
/// stdio server as failed with `MCP_STDIO_UNAVAILABLE`. The plugin data directory is created only
/// when a stdio server starts, so an in-memory plugin needs no filesystem.
pub async fn connect_agent_plugin(
    plugin: &LoadedAgentPlugin,
    options: &AgentPluginRuntimeOptions,
) -> Result<ConnectedAgentPlugin, AgentPluginRuntimeError> {
    let remote = McpRemoteOptions {
        standards: options.standards.clone(),
        http_client: options.http_client.clone(),
        ..McpRemoteOptions::default()
    };
    let mut cli = Cli::create(plugin.manifest.name.clone());
    let mut servers = Vec::with_capacity(plugin.mcp_servers.len());
    for (name, server) in &plugin.mcp_servers {
        let transport = transport_kind(server);
        match connect_server(plugin, server, options, &remote).await {
            Ok(commands) => {
                let tool_count = commands.len();
                let mut group = Cli::create(name.clone());
                for (command, def) in commands {
                    group = group.command(command, def);
                }
                cli = cli.group(group);
                servers.push(AgentPluginMcpServerStatus {
                    name: name.clone(),
                    transport,
                    tool_count,
                    error: None,
                    error_code: None,
                });
            }
            Err(error) => servers.push(AgentPluginMcpServerStatus {
                name: name.clone(),
                transport,
                tool_count: 0,
                error_code: match &error {
                    // Coded failures keep their machine-readable code.
                    crate::errors::Error::Incur(error) => Some(error.code.clone()),
                    // Other failures are described by text only.
                    _ => None,
                },
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(ConnectedAgentPlugin {
        catalog: cli.try_tool_catalog()?,
        servers,
    })
}

type Commands = BTreeMap<String, crate::command::CommandDef>;

#[cfg(not(target_arch = "wasm32"))]
fn runtime_error(error: impl std::fmt::Display) -> crate::errors::Error {
    crate::errors::Error::Other(Box::new(std::io::Error::other(error.to_string())))
}

#[cfg(not(target_arch = "wasm32"))]
async fn connect_server(
    plugin: &LoadedAgentPlugin,
    server: &AgentPluginMcpServer,
    options: &AgentPluginRuntimeOptions,
    remote: &McpRemoteOptions,
) -> Result<Commands, crate::errors::Error> {
    match server {
        // A host-supplied client carries every HTTP request.
        AgentPluginMcpServer::StreamableHttp(_) | AgentPluginMcpServer::Sse(_)
            if remote.http_client.is_some() =>
        {
            connect_portable(server, remote).await
        }
        // A local server is spawned directly with exact argv and no shell.
        AgentPluginMcpServer::Stdio(server) => {
            let launch = prepare_stdio(
                &plugin.root,
                &plugin.data_root,
                server,
                &options.base_environment,
            )
            .map_err(runtime_error)?;
            let mut command = tokio::process::Command::new(&launch.executable);
            command
                .args(&launch.args)
                .current_dir(&launch.cwd)
                .env_clear()
                .envs(&launch.environment);
            let transport =
                rmcp::transport::TokioChildProcess::new(command).map_err(runtime_error)?;
            crate::mcp::remote_commands_from_transport(transport, remote).await
        }
        // A current remote server uses Streamable HTTP with configured visible headers.
        AgentPluginMcpServer::StreamableHttp(server) => {
            let headers = configured_headers(&server.headers)
                .map_err(runtime_error)?
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            let config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                server.url.clone(),
            )
            .custom_headers(headers);
            let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
            crate::mcp::remote_commands_from_transport(transport, remote).await
        }
        // A legacy remote server uses the 2024-11-05 HTTP+SSE handshake.
        AgentPluginMcpServer::Sse(server) => {
            let url = url::Url::parse(&server.url).map_err(runtime_error)?;
            let headers = configured_headers(&server.headers).map_err(runtime_error)?;
            let client = crate::outbound::resolve(
                None,
                HTTP_CLIENT_OPTION,
                crate::outbound::Redirects::Refuse,
            )?;
            let transport =
                crate::agent_plugin_sse::LegacySseTransport::connect(url, headers, client)
                    .await
                    .map_err(runtime_error)?;
            crate::mcp::remote_commands_from_transport(transport, remote).await
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn connect_server(
    _plugin: &LoadedAgentPlugin,
    server: &AgentPluginMcpServer,
    _options: &AgentPluginRuntimeOptions,
    remote: &McpRemoteOptions,
) -> Result<Commands, crate::errors::Error> {
    connect_portable(server, remote).await
}

/// Where a host supplies the client for Agent Plugin HTTP servers.
const HTTP_CLIENT_OPTION: &str = "AgentPluginRuntimeOptions::http_client";

/// Connects one server with the portable client, through
/// `remote.http_client` or this target's default.
///
/// A wasm32 host has no processes, so a stdio server fails with a coded
/// `MCP_STDIO_UNAVAILABLE` error instead of starting. A wasm32 host has no
/// default HTTP client, so an HTTP server without one fails with
/// `HTTP_CLIENT_REQUIRED`.
async fn connect_portable(
    server: &AgentPluginMcpServer,
    remote: &McpRemoteOptions,
) -> Result<Commands, crate::errors::Error> {
    use crate::mcp_client::McpHttpClient;

    let with_headers = |headers: &BTreeMap<String, String>| {
        let http_client = crate::outbound::resolve(
            remote.http_client.as_ref(),
            HTTP_CLIENT_OPTION,
            crate::outbound::Redirects::Refuse,
        )?;
        Ok::<_, crate::errors::Error>(McpRemoteOptions {
            headers: headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            http_client: Some(http_client),
            ..remote.clone()
        })
    };
    let client = match server {
        // A local server needs a process, which a wasm32 host cannot start.
        AgentPluginMcpServer::Stdio(_) => {
            return Err(crate::errors::Error::Incur(crate::errors::IncurError {
                message: "stdio MCP servers cannot start on this host".to_string(),
                code: "MCP_STDIO_UNAVAILABLE".to_string(),
                hint: Some(
                    "Serve this MCP server over Streamable HTTP and set its url in mcp.json"
                        .to_string(),
                ),
                retryable: false,
                exit_code: None,
                cause: None,
            }));
        }
        // A current remote server uses Streamable HTTP with configured visible headers.
        AgentPluginMcpServer::StreamableHttp(server) => {
            McpHttpClient::connect(&server.url, &with_headers(&server.headers)?).await?
        }
        // A legacy remote server uses the 2024-11-05 HTTP+SSE handshake.
        AgentPluginMcpServer::Sse(server) => {
            McpHttpClient::connect_legacy_sse(&server.url, &with_headers(&server.headers)?).await?
        }
    };
    crate::mcp::remote_commands_from_client(client).await
}

fn transport_kind(server: &AgentPluginMcpServer) -> AgentPluginMcpTransport {
    match server {
        // Local subprocess binding.
        AgentPluginMcpServer::Stdio(_) => AgentPluginMcpTransport::Stdio,
        // Current remote HTTP binding.
        AgentPluginMcpServer::StreamableHttp(_) => AgentPluginMcpTransport::StreamableHttp,
        // Legacy remote HTTP+SSE binding.
        AgentPluginMcpServer::Sse(_) => AgentPluginMcpTransport::Sse,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct StdioLaunch {
    executable: OsString,
    args: Vec<String>,
    environment: BTreeMap<OsString, OsString>,
    cwd: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare_stdio(
    root: &Path,
    data: &Path,
    server: &AgentPluginStdioMcpServer,
    base_environment: &BTreeMap<OsString, OsString>,
) -> std::io::Result<StdioLaunch> {
    std::fs::create_dir_all(data)?;
    let cwd = if server.resolved_cwd.starts_with(data) {
        std::fs::create_dir_all(&server.resolved_cwd)?;
        let canonical_data = std::fs::canonicalize(data)?;
        let canonical_cwd = std::fs::canonicalize(&server.resolved_cwd)?;
        if !canonical_cwd.starts_with(canonical_data) {
            return Err(std::io::Error::other(
                "stdio cwd escapes the client-managed plugin data directory",
            ));
        }
        canonical_cwd
    } else {
        server.resolved_cwd.clone()
    };
    let root = root.to_string_lossy();
    let data = data.to_string_lossy();
    Ok(StdioLaunch {
        executable: server.resolved_command.as_ref().map_or_else(
            || OsString::from(&server.command),
            |path| path.as_os_str().to_owned(),
        ),
        args: server
            .args
            .iter()
            .map(|arg| expand(arg, &root, &data))
            .collect(),
        environment: resolve_environment(base_environment, &server.env, &root, &data),
        cwd,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn configured_headers(
    configured: &BTreeMap<String, String>,
) -> Result<http::HeaderMap, http::Error> {
    let mut headers = http::HeaderMap::with_capacity(configured.len());
    for (name, value) in configured {
        headers.insert(
            http::HeaderName::from_bytes(name.as_bytes())?,
            http::HeaderValue::from_str(value)?,
        );
    }
    Ok(headers)
}

#[cfg(not(target_arch = "wasm32"))]
fn expand(value: &str, root: &str, data: &str) -> String {
    const PLUGIN_ROOT: &str = "${PLUGIN_ROOT}";
    const PLUGIN_DATA: &str = "${PLUGIN_DATA}";

    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    loop {
        let root_at = rest.find(PLUGIN_ROOT);
        let data_at = rest.find(PLUGIN_DATA);
        let next = match (root_at, data_at) {
            // The root placeholder appears before the data placeholder.
            (Some(root_at), Some(data_at)) if root_at <= data_at => {
                Some((root_at, PLUGIN_ROOT, root))
            }
            // The data placeholder appears before the root placeholder.
            (Some(_), Some(data_at)) => Some((data_at, PLUGIN_DATA, data)),
            // Only the root placeholder remains.
            (Some(root_at), None) => Some((root_at, PLUGIN_ROOT, root)),
            // Only the data placeholder remains.
            (None, Some(data_at)) => Some((data_at, PLUGIN_DATA, data)),
            // No recognized placeholders remain.
            (None, None) => None,
        };
        let Some((at, placeholder, replacement)) = next else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..at]);
        output.push_str(replacement);
        rest = &rest[at + placeholder.len()..];
    }
    output
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_environment(
    base: &BTreeMap<OsString, OsString>,
    configured: &BTreeMap<String, String>,
    root: &str,
    data: &str,
) -> BTreeMap<OsString, OsString> {
    let mut environment = base.clone();
    for (name, value) in configured {
        remove_environment_name(&mut environment, OsStr::new(name));
        environment.insert(
            OsString::from(name),
            OsString::from(expand(value, root, data)),
        );
    }
    for (name, value) in [("PLUGIN_ROOT", root), ("PLUGIN_DATA", data)] {
        remove_environment_name(&mut environment, OsStr::new(name));
        environment.insert(OsString::from(name), OsString::from(value));
    }
    environment
}

#[cfg(not(target_arch = "wasm32"))]
fn remove_environment_name(environment: &mut BTreeMap<OsString, OsString>, name: &OsStr) {
    environment.retain(|candidate, _| !same_environment_name(candidate, name));
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
fn same_environment_name(left: &OsStr, right: &OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
fn same_environment_name(left: &OsStr, right: &OsStr) -> bool {
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_plugin::loader::AgentPluginStdioMcpServer;
    #[cfg(feature = "http")]
    use crate::agent_plugin::loader::{AgentPluginHttpMcpServer, AgentPluginManifest};

    #[cfg(feature = "http")]
    type LegacySseSender =
        tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>;

    #[cfg(feature = "http")]
    #[derive(Clone, Default)]
    struct LegacySseFixture {
        sender: std::sync::Arc<tokio::sync::Mutex<Option<LegacySseSender>>>,
    }

    #[cfg(feature = "http")]
    async fn legacy_sse_get(
        axum::extract::State(state): axum::extract::State<LegacySseFixture>,
    ) -> impl axum::response::IntoResponse {
        use futures::StreamExt;

        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        *state.sender.lock().await = Some(sender);
        let endpoint = futures::stream::once(async {
            Ok(axum::response::sse::Event::default()
                .event("endpoint")
                .data("/messages"))
        });
        axum::response::sse::Sse::new(
            endpoint.chain(tokio_stream::wrappers::ReceiverStream::new(receiver)),
        )
    }

    #[cfg(feature = "http")]
    async fn legacy_sse_post(
        axum::extract::State(state): axum::extract::State<LegacySseFixture>,
        axum::Json(message): axum::Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        let Some(id) = message.get("id").cloned() else {
            return axum::http::StatusCode::ACCEPTED;
        };
        let result = match message.get("method").and_then(serde_json::Value::as_str) {
            // Legacy initialization response.
            Some("initialize") => serde_json::json!({
                "capabilities": { "tools": {} },
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "legacy-fixture", "version": "1.0.0" }
            }),
            // One deterministic tool contract.
            Some("tools/list") => serde_json::json!({
                "tools": [{
                    "description": "Return pong",
                    "inputSchema": { "type": "object", "properties": {} },
                    "name": "ping"
                }]
            }),
            // One deterministic tool result.
            Some("tools/call") => serde_json::json!({
                "content": [{ "type": "text", "text": "{\"message\":\"pong\"}" }],
                "isError": false,
                "structuredContent": { "message": "pong" }
            }),
            // Unsupported requests receive a JSON-RPC method-not-found error.
            _ => {
                let response = serde_json::json!({
                    "error": { "code": -32601, "message": "Method not found" },
                    "id": id,
                    "jsonrpc": "2.0"
                });
                send_legacy_sse_message(&state, response).await;
                return axum::http::StatusCode::ACCEPTED;
            }
        };
        send_legacy_sse_message(
            &state,
            serde_json::json!({ "id": id, "jsonrpc": "2.0", "result": result }),
        )
        .await;
        axum::http::StatusCode::ACCEPTED
    }

    #[cfg(feature = "http")]
    async fn send_legacy_sse_message(state: &LegacySseFixture, message: serde_json::Value) {
        let sender = state.sender.lock().await.clone();
        if let Some(sender) = sender {
            let _ = sender
                .send(Ok(axum::response::sse::Event::default()
                    .event("message")
                    .data(message.to_string())))
                .await;
        }
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "incurs-agent-plugin-runtime-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn placeholder_expansion_is_global_and_non_recursive() {
        let root = "/plugins/${PLUGIN_DATA}/demo";
        let data = "/data/demo";

        assert_eq!(
            expand("${PLUGIN_ROOT}/a:${PLUGIN_DATA}/b:${OTHER}", root, data),
            "/plugins/${PLUGIN_DATA}/demo/a:/data/demo/b:${OTHER}"
        );
    }

    #[test]
    fn configured_environment_overlays_base_before_reserved_values() {
        let base = BTreeMap::from([
            (OsString::from("MODE"), OsString::from("base")),
            (OsString::from("PLUGIN_ROOT"), OsString::from("wrong")),
        ]);
        let configured = BTreeMap::from([
            ("MODE".to_string(), "plugin".to_string()),
            (
                "CONFIG".to_string(),
                "${PLUGIN_ROOT}/config:${UNKNOWN}".to_string(),
            ),
        ]);

        let environment = resolve_environment(&base, &configured, "/plugin", "/data");

        assert_eq!(environment[OsStr::new("MODE")], "plugin");
        assert_eq!(
            environment[OsStr::new("CONFIG")],
            "/plugin/config:${UNKNOWN}"
        );
        assert_eq!(environment[OsStr::new("PLUGIN_ROOT")], "/plugin");
        assert_eq!(environment[OsStr::new("PLUGIN_DATA")], "/data");
    }

    #[test]
    fn stdio_launch_creates_persistent_data_directories_without_removing_contents() {
        let temp = TestDir::new();
        let root = temp.0.join("plugin");
        let data = temp.0.join("data");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("keep.txt"), "keep").unwrap();
        let server = AgentPluginStdioMcpServer {
            command: "demo".to_string(),
            resolved_command: None,
            args: vec!["${PLUGIN_DATA}/input".to_string()],
            resolved_args: vec![data.join("input").to_string_lossy().into_owned()],
            env: BTreeMap::from([("MODE".to_string(), "plugin".to_string())]),
            resolved_env: BTreeMap::from([("MODE".to_string(), "plugin".to_string())]),
            cwd: Some("${PLUGIN_DATA}/work".to_string()),
            resolved_cwd: data.join("work"),
        };

        let launch = prepare_stdio(
            &root,
            &data,
            &server,
            &BTreeMap::from([(OsString::from("MODE"), OsString::from("base"))]),
        )
        .unwrap();

        assert_eq!(launch.executable, OsString::from("demo"));
        assert_eq!(launch.args, server.resolved_args);
        assert_eq!(launch.environment[OsStr::new("MODE")], "plugin");
        assert_eq!(
            launch.environment[OsStr::new("PLUGIN_ROOT")],
            root.as_os_str()
        );
        assert_eq!(
            launch.environment[OsStr::new("PLUGIN_DATA")],
            data.as_os_str()
        );
        assert!(data.join("work").is_dir());
        assert_eq!(
            std::fs::read_to_string(data.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn configured_headers_convert_without_changing_names_or_values() {
        let headers = configured_headers(&BTreeMap::from([
            ("X-Agent-Plugin".to_string(), "demo".to_string()),
            ("Authorization".to_string(), "visible value".to_string()),
        ]))
        .unwrap();

        assert_eq!(headers["x-agent-plugin"], "demo");
        assert_eq!(headers["authorization"], "visible value");
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn streamable_http_connection_and_failure_are_isolated_per_server() {
        struct Ping;

        #[async_trait::async_trait]
        impl crate::command::CommandHandler for Ping {
            async fn run(
                &self,
                _ctx: crate::command::CommandContext,
            ) -> crate::output::CommandResult {
                crate::output::CommandResult::Ok {
                    data: serde_json::json!({ "message": "pong" }),
                    cta: None,
                    exit_code: None,
                }
            }
        }

        let cli = Cli::create("fixture").command(
            "ping",
            crate::command::CommandDef::build("ping", Ping)
                .description("Return pong")
                .done(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app =
            axum::Router::new().route_service("/mcp", crate::mcp::http_service(&cli).unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let temp = TestDir::new();
        let root = temp.0.join("plugin");
        std::fs::create_dir_all(&root).unwrap();
        let plugin = LoadedAgentPlugin {
            root,
            data_root: temp.0.join("data"),
            manifest: AgentPluginManifest {
                schema: crate::agent_plugin::loader::AGENT_PLUGIN_SCHEMA.to_string(),
                name: "fixture".to_string(),
                version: None,
                description: None,
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: Vec::new(),
            },
            skills: Vec::new(),
            mcp_servers: BTreeMap::from([
                (
                    "bad".to_string(),
                    AgentPluginMcpServer::StreamableHttp(AgentPluginHttpMcpServer {
                        url: "http://127.0.0.1:1/mcp".to_string(),
                        headers: BTreeMap::new(),
                    }),
                ),
                (
                    "good".to_string(),
                    AgentPluginMcpServer::StreamableHttp(AgentPluginHttpMcpServer {
                        url: format!("http://{address}/mcp"),
                        headers: BTreeMap::new(),
                    }),
                ),
            ]),
            extensions: BTreeMap::new(),
        };

        let connected = connect_agent_plugin(&plugin, &AgentPluginRuntimeOptions::default())
            .await
            .unwrap();

        assert!(connected.servers[0].error.is_some());
        assert_eq!(connected.servers[1].error, None);
        assert_eq!(connected.servers[1].tool_count, 1);
        assert!(connected.catalog.get("good_ping").is_some());
        let outcome = connected
            .catalog
            .call(
                "good_ping",
                BTreeMap::new(),
                crate::tool::ToolCallOptions::isolated(),
            )
            .await;
        assert!(matches!(outcome, crate::tool::ToolCallOutcome::Ok { .. }));
        server.abort();
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn legacy_sse_connection_lists_and_calls_tools() {
        let state = LegacySseFixture::default();
        let app = axum::Router::new()
            .route("/sse", axum::routing::get(legacy_sse_get))
            .route("/messages", axum::routing::post(legacy_sse_post))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let temp = TestDir::new();
        let root = temp.0.join("plugin");
        std::fs::create_dir_all(&root).unwrap();
        let plugin = LoadedAgentPlugin {
            root,
            data_root: temp.0.join("data"),
            manifest: AgentPluginManifest {
                schema: crate::agent_plugin::loader::AGENT_PLUGIN_SCHEMA.to_string(),
                name: "fixture".to_string(),
                version: None,
                description: None,
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: Vec::new(),
            },
            skills: Vec::new(),
            mcp_servers: BTreeMap::from([(
                "legacy".to_string(),
                AgentPluginMcpServer::Sse(AgentPluginHttpMcpServer {
                    url: format!("http://{address}/sse"),
                    headers: BTreeMap::new(),
                }),
            )]),
            extensions: BTreeMap::new(),
        };
        let options = AgentPluginRuntimeOptions {
            standards: incurs_mcp_protocol::McpStandardSet::legacy_only(),
            ..Default::default()
        };

        let connected = connect_agent_plugin(&plugin, &options).await.unwrap();

        assert_eq!(connected.servers[0].error, None);
        assert!(connected.catalog.get("legacy_ping").is_some());
        let outcome = connected
            .catalog
            .call(
                "legacy_ping",
                BTreeMap::new(),
                crate::tool::ToolCallOptions::isolated(),
            )
            .await;
        assert!(matches!(
            outcome,
            crate::tool::ToolCallOutcome::Ok { data, .. }
                if data == serde_json::json!({ "message": "pong" })
        ));
        server.abort();
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn portable_legacy_sse_matches_the_rmcp_transport() {
        use crate::agent_plugin::loader::AgentPluginHttpMcpServer;
        use crate::mcp_client::tests::{describe, run_command};

        let state = LegacySseFixture::default();
        let app = axum::Router::new()
            .route("/sse", axum::routing::get(legacy_sse_get))
            .route("/messages", axum::routing::post(legacy_sse_post))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let sse = AgentPluginMcpServer::Sse(AgentPluginHttpMcpServer {
            url: format!("http://{address}/sse"),
            headers: BTreeMap::from([("X-Plugin".to_string(), "visible".to_string())]),
        });
        // An in-memory plugin lives under a virtual root that is never created.
        let plugin = LoadedAgentPlugin {
            root: PathBuf::from("/agent-plugin"),
            data_root: PathBuf::from("/agent-plugin/.data"),
            manifest: AgentPluginManifest {
                schema: crate::agent_plugin::loader::AGENT_PLUGIN_SCHEMA.to_string(),
                name: "fixture".to_string(),
                version: None,
                description: None,
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: Vec::new(),
            },
            skills: Vec::new(),
            mcp_servers: BTreeMap::from([("legacy".to_string(), sse.clone())]),
            extensions: BTreeMap::new(),
        };

        for standards in [
            incurs_mcp_protocol::McpStandardSet::legacy_only(),
            incurs_mcp_protocol::McpStandardSet::all(),
        ] {
            let options = AgentPluginRuntimeOptions {
                standards: standards.clone(),
                ..Default::default()
            };
            let remote = McpRemoteOptions {
                standards,
                ..McpRemoteOptions::default()
            };
            // The fixture routes responses to the newest stream, so each client
            // connects and calls before the next one connects.
            let native = connect_server(&plugin, &sse, &options, &remote)
                .await
                .unwrap();
            let native_call = run_command(&native["ping"], serde_json::json!({})).await;
            let portable = connect_portable(&sse, &remote).await.unwrap();
            let portable_call = run_command(&portable["ping"], serde_json::json!({})).await;

            assert_eq!(describe(&portable), describe(&native));
            assert_eq!(portable_call, native_call);
            assert_eq!(native_call["ok"], serde_json::json!({ "message": "pong" }));
        }

        let connected = connect_agent_plugin(&plugin, &AgentPluginRuntimeOptions::default())
            .await
            .unwrap();
        assert_eq!(connected.servers[0].error, None);
        assert!(!std::path::Path::new("/agent-plugin").exists());
        server.abort();
    }

    #[test]
    fn portable_stdio_servers_fail_with_a_coded_error() {
        let stdio = AgentPluginMcpServer::Stdio(AgentPluginStdioMcpServer {
            command: "demo".to_string(),
            resolved_command: None,
            args: Vec::new(),
            resolved_args: Vec::new(),
            env: BTreeMap::new(),
            resolved_env: BTreeMap::new(),
            cwd: None,
            resolved_cwd: PathBuf::from("/agent-plugin"),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let Err(error) = runtime.block_on(connect_portable(&stdio, &McpRemoteOptions::default()))
        else {
            panic!("a stdio server connected on the portable path");
        };
        let crate::errors::Error::Incur(error) = error else {
            panic!("expected a coded error");
        };
        assert_eq!(error.code, "MCP_STDIO_UNAVAILABLE");
        assert!(error.hint.is_some());
    }
}
