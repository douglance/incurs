//! An MCP client that starts nothing until a call genuinely needs it.

use std::sync::Arc;

use async_trait::async_trait;
use incurs_codemode::{McpClient, McpTool};
use incurs_mcp_discovery::{ConfigFingerprint, McpTransport, ServerId};
use serde_json::Value;
use tokio::sync::{OnceCell, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::bridge::IoBridge;
use crate::cache::{CacheEntry, NEGATIVE_TTL_MS, ToolCacheStore, UnreachableRecord, now_ms};
use crate::connect::{Connection, connect, to_mcp_tool, tool_value};
use crate::health::{HealthCode, HealthRegistry, HealthReport, ServerHealth};

/// Bounds on downstream work.
///
/// The per-server cap matters more than the global one: a Code Mode program can
/// trivially issue `Promise.all` over two hundred calls against a single server,
/// and most stdio MCP servers are single-threaded. Without it, a modest program
/// becomes a denial of service against the developer's own tools.
#[derive(Debug, Clone, Copy)]
pub struct ClientLimits {
    /// Concurrent in-flight calls against one server.
    pub per_server_calls: usize,
    /// How long to wait for a connection and handshake.
    pub connect_timeout_ms: u64,
    /// How long to wait for a tool listing.
    pub list_timeout_ms: u64,
    /// How long to wait for one tool call.
    pub call_timeout_ms: u64,
    /// How long a cached schema is served before a refresh is wanted.
    pub cache_ttl_ms: u64,
}

impl Default for ClientLimits {
    fn default() -> Self {
        Self {
            per_server_calls: 8,
            connect_timeout_ms: 30_000,
            list_timeout_ms: 15_000,
            call_timeout_ms: 120_000,
            cache_ttl_ms: crate::cache::DEFAULT_TTL_MS,
        }
    }
}

/// How a connection is produced, so tests can supply one in process.
#[async_trait]
pub trait TransportFactory: Send + Sync {
    /// Opens one connection to the described server.
    async fn connect(&self, transport: &McpTransport) -> Result<Arc<Connection>, HealthReport>;
}

/// Opens real connections.
#[derive(Debug, Default)]
pub struct RealTransportFactory;

#[async_trait]
impl TransportFactory for RealTransportFactory {
    async fn connect(&self, transport: &McpTransport) -> Result<Arc<Connection>, HealthReport> {
        connect(transport).await.map_err(|failure| HealthReport {
            state: failure.state,
            code: Some(failure.code),
            checked_at_ms: now_ms(),
            tool_count: None,
            from_cache: false,
        })
    }
}

/// An MCP client that connects only when a call requires it.
///
/// `list_tools` is answered from memory, then from the on-disk cache, and only
/// then by connecting. That ordering is what makes Composite's startup free:
/// Code Mode resolves every connector's description before running a program,
/// and without a cache that would start every configured server on every run.
pub struct LazyMcpClient {
    id: ServerId,
    fingerprint: ConfigFingerprint,
    display_name: String,
    transport: McpTransport,
    enabled: bool,
    requires_input: bool,
    bridge: IoBridge,
    cache: Option<Arc<ToolCacheStore>>,
    factory: Arc<dyn TransportFactory>,
    health: Arc<HealthRegistry>,
    limits: ClientLimits,
    calls: Semaphore,
    connection: OnceCell<Arc<Connection>>,
    tools: tokio::sync::RwLock<Option<Vec<McpTool>>>,
}

impl std::fmt::Debug for LazyMcpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LazyMcpClient")
            .field("id", &self.id)
            .field("name", &self.display_name)
            .finish_non_exhaustive()
    }
}

/// Turns "this server has no such capability" into an empty list.
///
/// A server that declares no resources answers `resources/list` with JSON-RPC
/// -32601 rather than an empty page. These list methods are offered on every
/// reachable namespace, so a program cannot know in advance which servers have
/// them, and an error would abort a whole composition for asking a reasonable
/// question. Only the list methods are softened: reading a named resource or
/// rendering a named prompt that does not exist is still an error, because the
/// caller asked for something specific.
fn unsupported_is_empty(result: Result<Value, String>) -> Result<Value, String> {
    match result {
        Err(error) if error.contains("-32601") || error.contains("Method not found") => {
            Ok(Value::Array(Vec::new()))
        }
        other => other,
    }
}

impl LazyMcpClient {
    /// Creates a client for one discovered server.
    ///
    /// Performs no I/O.
    #[must_use]
    pub fn new(
        server: &incurs_mcp_discovery::DiscoveredMcpServer,
        bridge: IoBridge,
        health: Arc<HealthRegistry>,
    ) -> Self {
        Self {
            id: server.id.clone(),
            fingerprint: server.fingerprint.clone(),
            display_name: server.local_name.clone(),
            transport: server.transport.clone(),
            enabled: server.enabled,
            requires_input: !server.requires_input.is_empty(),
            bridge,
            cache: None,
            factory: Arc::new(RealTransportFactory),
            health,
            limits: ClientLimits::default(),
            calls: Semaphore::new(ClientLimits::default().per_server_calls),
            connection: OnceCell::new(),
            tools: tokio::sync::RwLock::new(None),
        }
    }

    /// Attaches a persistent schema cache.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<ToolCacheStore>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Overrides how connections are opened.
    #[must_use]
    pub fn with_transport_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = factory;
        self
    }

    /// Overrides the concurrency and timeout bounds.
    #[must_use]
    pub fn with_limits(mut self, limits: ClientLimits) -> Self {
        self.calls = Semaphore::new(limits.per_server_calls);
        self.limits = limits;
        self
    }

    /// Returns this server's identity.
    #[must_use]
    pub fn id(&self) -> &ServerId {
        &self.id
    }

    /// Returns the name this server has in its own configuration.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns whether a connection is currently open.
    ///
    /// Used by tests to assert that a server was never started.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.initialized()
    }

    /// Returns what the cache knows, without opening a connection.
    ///
    /// This is what lets a diagnostic report tool counts while keeping its
    /// promise that inspecting configuration never starts anything.
    #[must_use]
    pub fn cached_tool_count(&self) -> Option<usize> {
        self.cache
            .as_ref()?
            .load(&self.id, &self.fingerprint)
            .map(|entry| entry.tools.len())
    }

    /// Returns the recorded reason this server was unreachable, if one is held.
    ///
    /// Read from the cache without connecting, so a diagnostic can distinguish
    /// "cached, and empty because it is broken" from "cached, and genuinely has
    /// no tools" — which a tool count of zero cannot do on its own.
    #[must_use]
    pub fn cached_failure(&self) -> Option<HealthReport> {
        let entry = self.cache.as_ref()?.load(&self.id, &self.fingerprint)?;
        let failure = entry.unreachable?;
        Some(HealthReport {
            state: failure.state,
            code: Some(failure.code),
            checked_at_ms: failure.checked_at_ms,
            tool_count: None,
            from_cache: true,
        })
    }

    /// Lists the server's resources as JSON.
    ///
    /// # Errors
    /// Returns an error when the server cannot be reached or declares no
    /// resource capability.
    pub async fn resources(&self) -> Result<Value, String> {
        let result = self
            .bridged(|connection| async move {
                let list = connection.list_resources().await?;
                serde_json::to_value(list).map_err(|error| error.to_string())
            })
            .await;
        Ok(unsupported_is_empty(result)?)
    }

    /// Reads one resource by URI.
    ///
    /// # Errors
    /// Returns an error when the server cannot be reached or the URI is unknown.
    pub async fn read_resource(&self, uri: &str) -> Result<Value, String> {
        let uri = uri.to_string();
        self.bridged(move |connection| async move {
            let value = connection.read_resource(&uri).await?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        })
        .await
    }

    /// Lists the server's prompts as JSON.
    ///
    /// # Errors
    /// Returns an error when the server cannot be reached or declares no prompt
    /// capability.
    pub async fn prompts(&self) -> Result<Value, String> {
        let result = self
            .bridged(|connection| async move {
                let list = connection.list_prompts().await?;
                serde_json::to_value(list).map_err(|error| error.to_string())
            })
            .await;
        Ok(unsupported_is_empty(result)?)
    }

    /// Renders one prompt with arguments.
    ///
    /// # Errors
    /// Returns an error when the server cannot be reached or the prompt is
    /// unknown.
    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let name = name.to_string();
        let arguments = match arguments {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            _ => return Err(format!("arguments to prompt {name} must be an object")),
        };
        self.bridged(move |connection| async move {
            let value = connection.get_prompt(&name, arguments).await?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        })
        .await
    }

    /// Runs one connection-bearing operation under the same limits as a tool call.
    ///
    /// Connecting, the call permit, the timeout, and the bridged runtime are all
    /// identical to `call_tool_cancellable`; only the request differs. Sharing
    /// the path means a resource read cannot bypass a limit a tool call obeys.
    async fn bridged<F, Fut>(&self, operation: F) -> Result<Value, String>
    where
        F: FnOnce(Arc<Connection>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Value, String>> + Send,
    {
        let _permit = self
            .calls
            .acquire()
            .await
            .map_err(|_| "the MCP client is shutting down".to_string())?;
        let connection = self.connected().await.map_err(|report| {
            format!("{} is unavailable: {}", self.display_name, report.summary())
        })?;
        let timeout = self.limits.call_timeout_ms;
        self.bridge
            .run(async move {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(timeout),
                    operation(connection),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("the request timed out".to_string()),
                }
            })
            .await
            .map_err(|error| error.to_string())?
    }

    /// Records a state and returns it.
    fn record(&self, report: HealthReport) -> HealthReport {
        self.health.record(&self.id, report);
        report
    }

    /// Returns the reason this server cannot be used, if there is one.
    ///
    /// Checked before any connection attempt, so a disabled, incomplete, or
    /// unspeakable entry never causes a process to start. Public because it is
    /// also how a diagnostic reports the reason without contacting anything.
    #[must_use]
    pub fn refusal(&self) -> Option<HealthReport> {
        let code = if !self.enabled {
            HealthCode::ConfigDisabled
        } else if self.requires_input {
            HealthCode::ConfigNeedsInput
        } else if matches!(self.transport, McpTransport::Sse(_)) {
            // rmcp removed its SSE client transport, and the streamable-HTTP
            // client cannot stand in for it: the two speak different protocols,
            // so attempting one against the other produced a namespace that was
            // permanently and silently empty. Refusing names the reason instead.
            HealthCode::TransportUnsupported
        } else {
            return None;
        };
        Some(HealthReport {
            state: ServerHealth::InvalidConfiguration,
            code: Some(code),
            checked_at_ms: now_ms(),
            tool_count: None,
            from_cache: false,
        })
    }

    /// Returns the live connection, opening it on first use.
    async fn connected(&self) -> Result<Arc<Connection>, HealthReport> {
        if let Some(refusal) = self.refusal() {
            return Err(self.record(refusal));
        }
        self.connection
            .get_or_try_init(|| async {
                let transport = self.transport.clone();
                let factory = Arc::clone(&self.factory);
                let timeout = self.limits.connect_timeout_ms;
                // Built and driven on the bridged runtime: the caller's thread
                // may have no I/O driver at all.
                let opened = self
                    .bridge
                    .run(async move {
                        tokio::time::timeout(
                            std::time::Duration::from_millis(timeout),
                            factory.connect(&transport),
                        )
                        .await
                    })
                    .await;
                match opened {
                    Ok(Ok(Ok(connection))) => Ok(connection),
                    Ok(Ok(Err(report))) => Err(self.record(report)),
                    Ok(Err(_elapsed)) => Err(self.record(HealthReport {
                        state: ServerHealth::Unavailable,
                        code: Some(HealthCode::ConnectFailed),
                        checked_at_ms: now_ms(),
                        tool_count: None,
                        from_cache: false,
                    })),
                    Err(_stopped) => Err(self.record(HealthReport {
                        state: ServerHealth::Unavailable,
                        code: Some(HealthCode::ConnectFailed),
                        checked_at_ms: now_ms(),
                        tool_count: None,
                        from_cache: false,
                    })),
                }
            })
            .await
            .cloned()
    }

    /// Persists a failure so the next process does not repeat the wait.
    ///
    /// Recorded against the same fingerprint as a successful entry, so fixing
    /// the configuration invalidates the failure for free. Any schema already
    /// cached is carried forward: a server that answered yesterday and is down
    /// today still has usable tools, and throwing them away would turn one
    /// unreachable server into an empty namespace for no reason.
    fn remember_failure(&self, report: &HealthReport) {
        let Some(cache) = &self.cache else { return };
        let Some(code) = report.code else { return };
        let existing = cache.load(&self.id, &self.fingerprint);
        let _ = cache.store(&CacheEntry {
            cache_schema_version: crate::cache::CACHE_SCHEMA_VERSION,
            server_id: self.id.as_str().to_string(),
            config_fingerprint: self.fingerprint.as_str().to_string(),
            instructions: existing
                .as_ref()
                .and_then(|entry| entry.instructions.clone()),
            protocol_version: existing
                .as_ref()
                .and_then(|entry| entry.protocol_version.clone()),
            fetched_at_ms: existing
                .as_ref()
                .map_or(report.checked_at_ms, |entry| entry.fetched_at_ms),
            tools: existing.map(|entry| entry.tools).unwrap_or_default(),
            unreachable: Some(UnreachableRecord {
                state: report.state,
                code,
                checked_at_ms: report.checked_at_ms,
            }),
        });
    }

    /// Fetches the tool schema from the server and persists it.
    async fn fetch_tools(&self) -> Result<Vec<McpTool>, HealthReport> {
        let connection = self.connected().await.inspect_err(|report| {
            self.remember_failure(report);
        })?;
        let listing = {
            let connection = Arc::clone(&connection);
            let timeout = self.limits.list_timeout_ms;
            self.bridge
                .run(async move {
                    tokio::time::timeout(
                        std::time::Duration::from_millis(timeout),
                        connection.list_tools(),
                    )
                    .await
                })
                .await
        };
        let tools = match listing {
            Ok(Ok(Ok(tools))) => tools,
            _ => {
                let report = self.record(HealthReport {
                    state: ServerHealth::SchemaRefreshFailed,
                    code: Some(HealthCode::ToolsListFailed),
                    checked_at_ms: now_ms(),
                    tool_count: None,
                    from_cache: false,
                });
                self.remember_failure(&report);
                return Err(report);
            }
        };
        let tools: Vec<McpTool> = tools.into_iter().map(to_mcp_tool).collect();

        if let Some(cache) = &self.cache {
            let _ = cache.store(&CacheEntry {
                cache_schema_version: crate::cache::CACHE_SCHEMA_VERSION,
                server_id: self.id.as_str().to_string(),
                config_fingerprint: self.fingerprint.as_str().to_string(),
                instructions: connection.instructions(),
                protocol_version: connection.protocol_version(),
                fetched_at_ms: now_ms(),
                tools: tools.clone(),
                unreachable: None,
            });
        }
        self.record(HealthReport {
            state: ServerHealth::Healthy,
            code: None,
            checked_at_ms: now_ms(),
            tool_count: Some(tools.len()),
            from_cache: false,
        });
        Ok(tools)
    }

    /// Warms the cache by contacting the server.
    ///
    /// This is the one entry point that deliberately starts a server, used by an
    /// explicit refresh rather than by ordinary capability listing.
    ///
    /// # Errors
    /// Returns the recorded health state when the server cannot be reached.
    pub async fn refresh(&self) -> Result<usize, HealthReport> {
        let tools = self.fetch_tools().await?;
        let count = tools.len();
        *self.tools.write().await = Some(tools);
        Ok(count)
    }

    /// Closes the connection if one is open.
    pub async fn shutdown(&self) {
        if let Some(connection) = self.connection.get() {
            connection.shutdown();
        }
    }
}

#[async_trait]
impl McpClient for LazyMcpClient {
    /// Lists the server's tools, preferring memory then cache over a connection.
    ///
    /// A failure yields an empty list rather than an error. That is both cheaper
    /// and more accurate than failing: a server that cannot be reached genuinely
    /// exposes no tools right now, so its namespace renders as `{}` and a
    /// program touching it gets a clean `TypeError` inside the sandbox instead
    /// of taking down every other connector in the same execution. The reason is
    /// not lost — it is recorded in the health registry and surfaced through the
    /// connector's model-facing instructions.
    async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        if let Some(tools) = self.tools.read().await.as_ref() {
            return Ok(tools.clone());
        }
        if self.refusal().is_some() {
            return Ok(Vec::new());
        }
        if let Some(cache) = &self.cache
            && let Some(entry) = cache.load(&self.id, &self.fingerprint)
        {
            // A recent failure is answered from disk. This is the one thing that
            // keeps an unattended run from paying every unreachable server's
            // connect timeout again on every single execution.
            if let Some(failure) = &entry.unreachable
                && failure.is_fresh(now_ms(), NEGATIVE_TTL_MS)
            {
                self.record(HealthReport {
                    state: failure.state,
                    code: Some(failure.code),
                    checked_at_ms: failure.checked_at_ms,
                    tool_count: None,
                    from_cache: true,
                });
                return Ok(entry.tools);
            }
            let stale = entry.is_stale(now_ms(), self.limits.cache_ttl_ms);
            self.record(HealthReport {
                state: ServerHealth::Healthy,
                code: None,
                checked_at_ms: entry.fetched_at_ms,
                tool_count: Some(entry.tools.len()),
                from_cache: true,
            });
            let tools = entry.tools;
            if !stale {
                *self.tools.write().await = Some(tools.clone());
            }
            return Ok(tools);
        }
        match self.fetch_tools().await {
            Ok(tools) => {
                *self.tools.write().await = Some(tools.clone());
                Ok(tools)
            }
            // Deliberately not memoized: a server that recovers must be able to
            // report its tools without restarting the whole process.
            Err(_) => Ok(Vec::new()),
        }
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.call_tool_cancellable(name, arguments, &CancellationToken::new())
            .await
    }

    /// Calls one tool, abandoning the call when the execution is cancelled.
    async fn call_tool_cancellable(
        &self,
        name: &str,
        arguments: Value,
        cancellation: &CancellationToken,
    ) -> Result<Value, String> {
        let _permit = self
            .calls
            .acquire()
            .await
            .map_err(|_| "the MCP client is shutting down".to_string())?;

        let connection = self.connected().await.map_err(|report| {
            format!("{} is unavailable: {}", self.display_name, report.summary())
        })?;

        let arguments = match arguments {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            _ => return Err(format!("arguments to {name} must be an object")),
        };

        let name_owned = name.to_string();
        let timeout = self.limits.call_timeout_ms;
        let cancellation = cancellation.clone();
        self.bridge
            .run(async move {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err("Call cancelled".to_string()),
                    result = tokio::time::timeout(
                        std::time::Duration::from_millis(timeout),
                        connection.call_tool(&name_owned, arguments),
                    ) => match result {
                        Ok(Ok(value)) => tool_value(value),
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err(format!("{name_owned} timed out")),
                    },
                }
            })
            .await
            .map_err(|error| error.to_string())?
    }
}
