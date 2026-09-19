//! Turns configured MCP servers into incurs Code Mode connectors.
//!
//! The same server declared by several agents collapses into one connector, each
//! connector gets a JavaScript namespace that survives servers being added and
//! removed, and one unreachable server never prevents the others from working.

pub mod connector;
pub mod namespace;
pub mod policy;

use std::collections::BTreeMap;
use std::sync::Arc;

use incurs_codemode::{Connector, McpConnector};
use incurs_mcp_client::{HealthRegistry, HealthReport, IoBridge, LazyMcpClient, ToolCacheStore};
use incurs_mcp_discovery::{
    DiscoveredMcpServer, HostPaths, McpDiscoverySource, McpTransport, ServerId, discover,
};

pub use connector::HealthAwareMcpConnector;
pub use namespace::{NamespaceLedger, NamespaceRequest, guard_reserved};
pub use policy::{ApprovalMode, DiscoveredToolPolicyResolver};

/// How the registry is built.
#[derive(Debug, Clone)]
pub struct RegistryOptions {
    /// Approval policy applied to every discovered server.
    pub approval: ApprovalMode,
    /// Namespaces to expose, or empty for all of them.
    pub include: Vec<String>,
    /// Namespaces to withhold.
    pub exclude: Vec<String>,
}

impl Default for RegistryOptions {
    fn default() -> Self {
        Self {
            approval: ApprovalMode::AnnotatedReads,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

/// One logical server, after entries from several hosts are collapsed.
#[derive(Debug, Clone)]
pub struct RegisteredServer {
    /// Stable identity.
    pub id: ServerId,
    /// JavaScript namespace exposed to Code Mode programs.
    pub namespace: String,
    /// Name shown to a person.
    pub display_name: String,
    /// Every host that declared this server.
    pub sources: Vec<McpDiscoverySource>,
    /// Whether every declaring host disabled it.
    pub enabled: bool,
    /// The entry used to reach the server.
    pub server: DiscoveredMcpServer,
}

/// Every configured MCP server on this machine, as Code Mode connectors.
pub struct McpRegistry {
    servers: Vec<RegisteredServer>,
    clients: BTreeMap<String, Arc<LazyMcpClient>>,
    health: Arc<HealthRegistry>,
    options: RegistryOptions,
    diagnostics: Vec<incurs_mcp_discovery::DiscoveryDiagnostic>,
    entry_count: usize,
    self_excluded: usize,
}

/// How many servers may be contacted at once during a refresh.
///
/// Small on purpose: each one is a subprocess or a network connection on the
/// developer's own machine.
const MAX_CONCURRENT_REFRESHES: usize = 8;

/// Returns the current Unix time in milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl McpRegistry {
    /// Discovers every configured server and prepares a client for each.
    ///
    /// Starts nothing: clients are constructed, not connected.
    #[must_use]
    pub fn build(
        paths: &HostPaths,
        bridge: IoBridge,
        cache: Option<Arc<ToolCacheStore>>,
        ledger: &mut NamespaceLedger,
        options: RegistryOptions,
    ) -> Self {
        let mut found = discover(paths);
        let health = HealthRegistry::new();
        // Counted before anything is filtered or collapsed, because that is the
        // number a person comparing this against their own configuration files
        // can actually check.
        let entry_count = found.servers.len();

        // A file that could not be parsed is a finding, not a silence. Dropping
        // it meant a typo in one host's configuration hid every server that host
        // declared, with nothing anywhere saying so.
        for (path, _reason) in std::mem::take(&mut found.unreadable) {
            found
                .diagnostics
                .push(incurs_mcp_discovery::DiscoveryDiagnostic::error(
                    "mcp_config_unreadable",
                    path.display().to_string(),
                    "the file could not be read or parsed, so every server it declares is missing",
                ));
        }

        // A host that registers this very binary as one of its MCP servers puts
        // the process in its own discovery results, and nothing else about the
        // entry distinguishes it from any other stdio server. Left in, the first
        // connection spawns another copy, which discovers itself again. Drop it
        // here so the recursion cannot begin, rather than being noticed once it
        // already has.
        let own_program = std::env::current_exe().ok().and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        });
        let servers: Vec<_> = found.servers.into_iter().filter(|server| {
            let Some(own) = own_program.as_deref() else {
                return true;
            };
            match &server.transport {
                McpTransport::Stdio(stdio) => std::path::Path::new(&stdio.command)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    != Some(own),
                _ => true,
            }
        }).collect();
        let self_excluded = entry_count - servers.len();

        // Collapse entries that describe the same server. The earliest host in
        // precedence order supplies the display name.
        let mut grouped: BTreeMap<String, Vec<DiscoveredMcpServer>> = BTreeMap::new();
        for server in servers {
            grouped
                .entry(server.id.as_str().to_string())
                .or_default()
                .push(server);
        }

        let mut collapsed: Vec<RegisteredServer> = Vec::new();
        for (_, mut entries) in grouped {
            entries.sort_by(|left, right| {
                left.source
                    .discovery
                    .cmp(&right.source.discovery)
                    .then_with(|| left.local_name.cmp(&right.local_name))
            });
            let primary = entries[0].clone();
            collapsed.push(RegisteredServer {
                id: primary.id.clone(),
                namespace: String::new(),
                display_name: primary.local_name.clone(),
                sources: entries.iter().map(|entry| entry.source.discovery).collect(),
                // Disabled everywhere means disabled; enabled anywhere means the
                // developer wants it somewhere.
                enabled: entries.iter().any(|entry| entry.enabled),
                server: primary,
            });
        }

        let requests: Vec<NamespaceRequest> = collapsed
            .iter()
            .map(|server| NamespaceRequest {
                id: server.id.clone(),
                display_name: server.display_name.clone(),
            })
            .collect();
        let namespaces = ledger.assign(&requests, now_ms());

        let mut clients = BTreeMap::new();
        for server in &mut collapsed {
            let namespace = namespaces
                .get(server.id.as_str())
                .cloned()
                .unwrap_or_else(|| guard_reserved(&server.display_name));
            server.namespace = namespace.clone();

            let mut entry = server.server.clone();
            entry.enabled = server.enabled;
            let mut client = LazyMcpClient::new(&entry, bridge.clone(), Arc::clone(&health));
            if let Some(cache) = &cache {
                client = client.with_cache(Arc::clone(cache));
            }
            clients.insert(namespace, Arc::new(client));
        }

        Self {
            servers: collapsed,
            clients,
            health,
            options,
            diagnostics: found.diagnostics,
            entry_count,
            self_excluded,
        }
    }

    /// Returns how many configuration entries were parsed, before collapsing.
    ///
    /// Distinct from `servers().len()`, which counts servers after entries from
    /// several hosts are merged. Reporting the merged count as the parsed count
    /// hid the collapse entirely.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// Returns how many entries were dropped as this process itself.
    #[must_use]
    pub fn self_excluded(&self) -> usize {
        self.self_excluded
    }

    /// Returns every registered server.
    #[must_use]
    pub fn servers(&self) -> &[RegisteredServer] {
        &self.servers
    }

    /// Returns the shared health registry.
    #[must_use]
    pub fn health(&self) -> Arc<HealthRegistry> {
        Arc::clone(&self.health)
    }

    /// Returns discovery findings about individual configuration entries.
    #[must_use]
    pub fn diagnostics(&self) -> &[incurs_mcp_discovery::DiscoveryDiagnostic] {
        &self.diagnostics
    }

    /// Returns cached tool counts for every server, starting nothing.
    #[must_use]
    pub fn cached_tool_counts(&self) -> BTreeMap<String, Option<usize>> {
        self.clients
            .iter()
            .map(|(namespace, client)| (namespace.clone(), client.cached_tool_count()))
            .collect()
    }

    /// Returns why each namespace is unusable, where that is already known.
    ///
    /// Starts nothing: a refusal is decided from the configuration alone, which
    /// is what lets a diagnostic explain a dead entry without launching it.
    #[must_use]
    pub fn refusals(&self) -> BTreeMap<String, Option<HealthReport>> {
        self.clients
            .iter()
            .map(|(namespace, client)| (namespace.clone(), client.refusal()))
            .collect()
    }

    /// Returns each namespace's recorded failure, where one is held on disk.
    ///
    /// Starts nothing.
    #[must_use]
    pub fn cached_failures(&self) -> BTreeMap<String, Option<HealthReport>> {
        self.clients
            .iter()
            .map(|(namespace, client)| (namespace.clone(), client.cached_failure()))
            .collect()
    }

    /// Returns the client behind one namespace.
    #[must_use]
    pub fn client(&self, namespace: &str) -> Option<Arc<LazyMcpClient>> {
        self.clients.get(namespace).map(Arc::clone)
    }

    /// Returns whether a namespace is exposed under the current options.
    ///
    /// Public because connectors that are not discovered servers — the local
    /// command namespace, in particular — are appended by the caller and must
    /// answer to the same `--include` and `--exclude` as everything else.
    #[must_use]
    pub fn exposes(&self, namespace: &str) -> bool {
        if self.options.exclude.iter().any(|name| name == namespace) {
            return false;
        }
        self.options.include.is_empty() || self.options.include.iter().any(|n| n == namespace)
    }

    /// Builds the connector set Code Mode runs against.
    ///
    /// Performs no I/O: each connector resolves its tools lazily, and normally
    /// from cache.
    #[must_use]
    pub fn connectors(&self) -> Vec<Arc<dyn Connector>> {
        let policy: Arc<dyn incurs_codemode::ToolPolicyResolver> =
            Arc::new(DiscoveredToolPolicyResolver::new(self.options.approval));
        self.servers
            .iter()
            .filter(|server| self.exposes(&server.namespace))
            .filter_map(|server| {
                let client = self.clients.get(&server.namespace)?;
                let inner = McpConnector::new(
                    server.namespace.clone(),
                    Arc::clone(client) as Arc<dyn incurs_codemode::McpClient>,
                )
                .with_policy_resolver(Arc::clone(&policy));
                Some(Arc::new(HealthAwareMcpConnector::new(
                    server.id.clone(),
                    server.namespace.clone(),
                    inner,
                    Arc::clone(&self.health),
                    Arc::clone(client),
                )) as Arc<dyn Connector>)
            })
            .collect()
    }

    /// Contacts every exposed server and refreshes its cached schema.
    ///
    /// This is the one operation that deliberately starts servers, so it is also
    /// the one that has to be bounded. Serially, a machine with thirty servers
    /// and a thirty-second connect timeout takes a quarter of an hour in the
    /// worst case, almost all of it spent waiting on servers that will never
    /// answer. Running them concurrently under a small permit count keeps the
    /// wall clock near the slowest server rather than the sum of all of them,
    /// without spawning thirty subprocesses at once.
    pub async fn refresh(&self) -> BTreeMap<String, Result<usize, HealthReport>> {
        let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REFRESHES));
        let mut tasks = tokio::task::JoinSet::new();
        for server in &self.servers {
            if !self.exposes(&server.namespace) || !server.enabled {
                continue;
            }
            let Some(client) = self.clients.get(&server.namespace) else {
                continue;
            };
            let namespace = server.namespace.clone();
            let client = Arc::clone(client);
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                let _permit = permits.acquire().await;
                (namespace, client.refresh().await)
            });
        }
        let mut out = BTreeMap::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok((namespace, outcome)) = joined {
                out.insert(namespace, outcome);
            }
        }
        out
    }

    /// Closes every open connection.
    pub async fn shutdown(&self) {
        for client in self.clients.values() {
            client.shutdown().await;
        }
    }
}
