//! Loader for portable Agent Plugins 1.0 directories.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Canonical Agent Plugins 1.0 manifest schema identifier.
pub const AGENT_PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// Canonical Agent Plugins 1.0 MCP schema identifier.
pub const AGENT_PLUGIN_MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// Official Agent Plugins 1.0 manifest JSON Schema bundled for offline loading.
pub const AGENT_PLUGIN_SCHEMA_JSON: &str = include_str!("schemas/plugin.schema.json");

/// Official Agent Plugins 1.0 MCP JSON Schema bundled for offline loading.
pub const AGENT_PLUGIN_MCP_SCHEMA_JSON: &str = include_str!("schemas/mcp.schema.json");

/// Options that control local Agent Plugins loading.
#[derive(Debug, Clone)]
pub struct AgentPluginLoadOptions {
    /// Filesystem-resolved client-managed writable data root for this plugin instance.
    pub plugin_data_root: PathBuf,
    /// MCP transports this client implementation can connect to.
    pub supported_mcp_transports: BTreeSet<AgentPluginMcpTransport>,
}

impl Default for AgentPluginLoadOptions {
    fn default() -> Self {
        Self {
            plugin_data_root: std::env::temp_dir().join("incurs-agent-plugin-data"),
            supported_mcp_transports: AgentPluginMcpTransport::all().into_iter().collect(),
        }
    }
}

/// Result of attempting to load one Agent Plugin directory.
#[derive(Debug, Clone)]
pub struct AgentPluginLoadReport {
    /// Loaded plugin data, or `None` when the manifest failed fatally.
    pub plugin: Option<LoadedAgentPlugin>,
    /// Structured diagnostics collected during manifest and component loading.
    pub diagnostics: Vec<AgentPluginDiagnostic>,
}

/// Loaded portable Agent Plugin package.
#[derive(Debug, Clone)]
pub struct LoadedAgentPlugin {
    /// Filesystem-resolved plugin root.
    pub root: PathBuf,
    /// Filesystem-normalized client-managed writable data root.
    pub data_root: PathBuf,
    /// Parsed root plugin manifest.
    pub manifest: AgentPluginManifest,
    /// Valid Agent Skills discovered from immediate children of `skills/`.
    pub skills: Vec<LoadedAgentPluginSkill>,
    /// Valid MCP server bindings discovered from root `mcp.json`.
    pub mcp_servers: BTreeMap<String, AgentPluginMcpServer>,
    /// Raw object-valued manifest extensions keyed by namespace.
    pub extensions: BTreeMap<String, Value>,
}

/// Parsed root `plugin.json` manifest.
#[derive(Debug, Clone)]
pub struct AgentPluginManifest {
    /// Canonical manifest schema identifier.
    pub schema: String,
    /// Stable Agent Plugins package name.
    pub name: String,
    /// Optional plugin version string.
    pub version: Option<String>,
    /// Optional plugin description.
    pub description: Option<String>,
    /// Optional plugin author metadata.
    pub author: Option<AgentPluginAuthor>,
    /// Optional plugin homepage string.
    pub homepage: Option<String>,
    /// Optional plugin repository string.
    pub repository: Option<String>,
    /// Optional plugin license string.
    pub license: Option<String>,
    /// Optional plugin search keywords.
    pub keywords: Vec<String>,
}

/// Parsed plugin author metadata.
#[derive(Debug, Clone)]
pub struct AgentPluginAuthor {
    /// Optional author name.
    pub name: Option<String>,
    /// Optional author email string.
    pub email: Option<String>,
    /// Optional author URL string.
    pub url: Option<String>,
}

/// Loaded Agent Skill prompt artifact.
#[derive(Debug, Clone)]
pub struct LoadedAgentPluginSkill {
    /// Skill directory name and frontmatter name.
    pub name: String,
    /// Skill description from frontmatter.
    pub description: String,
    /// Optional skill license string.
    pub license: Option<String>,
    /// Optional skill compatibility note.
    pub compatibility: Option<String>,
    /// Optional experimental allowed-tools selector string.
    pub allowed_tools: Option<String>,
    /// String-valued Agent Skill metadata map.
    pub metadata: BTreeMap<String, String>,
    /// Markdown body after frontmatter.
    pub body: String,
    /// Filesystem-resolved `SKILL.md` path.
    pub path: PathBuf,
    /// Filesystem-resolved skill root directory.
    pub root: PathBuf,
}

/// MCP transport kind used by Agent Plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentPluginMcpTransport {
    /// Local subprocess stdio transport.
    Stdio,
    /// Current Streamable HTTP transport.
    StreamableHttp,
    /// Legacy HTTP+SSE transport.
    Sse,
}

impl AgentPluginMcpTransport {
    /// Returns every Agent Plugins MCP transport kind.
    pub fn all() -> [Self; 3] {
        [Self::Stdio, Self::StreamableHttp, Self::Sse]
    }
}

/// Valid MCP server binding from `mcp.json`.
#[derive(Debug, Clone)]
pub enum AgentPluginMcpServer {
    /// Local stdio MCP server.
    Stdio(AgentPluginStdioMcpServer),
    /// Remote Streamable HTTP MCP server.
    StreamableHttp(AgentPluginHttpMcpServer),
    /// Remote legacy HTTP+SSE MCP server.
    Sse(AgentPluginHttpMcpServer),
}

/// Valid stdio MCP server binding.
#[derive(Debug, Clone)]
pub struct AgentPluginStdioMcpServer {
    /// Configured command token before placeholder expansion.
    pub command: String,
    /// Filesystem-resolved command path when `command` is plugin-relative.
    pub resolved_command: Option<PathBuf>,
    /// Configured argument strings before placeholder expansion.
    pub args: Vec<String>,
    /// Argument strings after single-pass plugin placeholder expansion.
    pub resolved_args: Vec<String>,
    /// Configured environment overlay before placeholder expansion.
    pub env: BTreeMap<String, String>,
    /// Environment overlay after single-pass plugin placeholder expansion.
    pub resolved_env: BTreeMap<String, String>,
    /// Configured working directory string, or `None` when omitted.
    pub cwd: Option<String>,
    /// Filesystem-normalized working directory after placeholder expansion.
    pub resolved_cwd: PathBuf,
}

/// Valid remote HTTP MCP server binding.
#[derive(Debug, Clone)]
pub struct AgentPluginHttpMcpServer {
    /// Absolute HTTP or HTTPS MCP endpoint URL.
    pub url: String,
    /// Fixed visible header values keyed by original header name.
    pub headers: BTreeMap<String, String>,
}

/// Severity for Agent Plugins loader diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPluginDiagnosticSeverity {
    /// Informational diagnostic that does not affect loading.
    Info,
    /// Nonfatal diagnostic that ignored a field, extension, or component.
    Warning,
    /// Fatal diagnostic for a manifest or component failure boundary.
    Error,
}

/// Structured diagnostic emitted while loading an Agent Plugin.
#[derive(Debug, Clone)]
pub struct AgentPluginDiagnostic {
    /// Severity of the loader finding.
    pub severity: AgentPluginDiagnosticSeverity,
    /// Stable machine-readable diagnostic code.
    pub code: &'static str,
    /// Plugin-relative or document-relative path for the finding.
    pub path: String,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// Loads a portable Agent Plugins 1.0 directory from disk without fetching schemas.
pub fn load_agent_plugin(
    root: impl AsRef<Path>,
    options: &AgentPluginLoadOptions,
) -> AgentPluginLoadReport {
    let mut diagnostics = Vec::new();
    let root = match fs::canonicalize(root.as_ref()) {
        Ok(root) if root.is_dir() => root,
        Ok(path) => {
            diagnostics.push(error(
                "plugin_root_kind",
                ".",
                format!("plugin root is not a directory: {}", path.display()),
            ));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
        Err(source) => {
            diagnostics.push(error("plugin_root_missing", ".", source.to_string()));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let plugin_json = root.join("plugin.json");
    let plugin_json = match canonical_file(&root, &plugin_json) {
        Ok(path) => path,
        Err(message) => {
            diagnostics.push(error("manifest_path_invalid", "plugin.json", message));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let manifest_text = match fs::read_to_string(&plugin_json) {
        Ok(text) => text,
        Err(source) => {
            diagnostics.push(error(
                "manifest_read_failed",
                "plugin.json",
                source.to_string(),
            ));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let manifest_json = match serde_json::from_str::<Value>(&manifest_text) {
        Ok(value) => value,
        Err(source) => {
            diagnostics.push(error(
                "manifest_json_invalid",
                "plugin.json",
                source.to_string(),
            ));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let (manifest, extensions) = match parse_manifest(&manifest_json, &mut diagnostics) {
        Some(manifest) => manifest,
        None => {
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let skills = load_skills(&root, &mut diagnostics);
    let data_root = normalize_absolute(&options.plugin_data_root);
    let mcp_servers = load_mcp_servers(&root, options, &manifest.schema, &mut diagnostics);
    let plugin = LoadedAgentPlugin {
        root,
        data_root,
        manifest,
        skills,
        mcp_servers,
        extensions,
    };
    AgentPluginLoadReport {
        plugin: Some(plugin),
        diagnostics,
    }
}

fn parse_manifest(
    value: &Value,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Option<(AgentPluginManifest, BTreeMap<String, Value>)> {
    let Some(object) = value.as_object() else {
        diagnostics.push(error(
            "manifest_not_object",
            "plugin.json",
            "manifest must be a JSON object",
        ));
        return None;
    };
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "$schema"
                | "name"
                | "version"
                | "description"
                | "author"
                | "homepage"
                | "repository"
                | "license"
                | "keywords"
                | "extensions"
        ) {
            diagnostics.push(warning(
                "manifest_unknown_field",
                format!("plugin.json/{key}"),
                "unknown manifest field ignored",
            ));
        }
    }
    let schema = required_string(object, "$schema", "plugin.json/$schema", diagnostics)?;
    if schema != AGENT_PLUGIN_SCHEMA {
        diagnostics.push(error(
            "manifest_schema_unsupported",
            "plugin.json/$schema",
            format!("unsupported schema: {schema}"),
        ));
        return None;
    }
    let name = required_string(object, "name", "plugin.json/name", diagnostics)?;
    if !is_plugin_name(&name) {
        diagnostics.push(error(
            "manifest_name_invalid",
            "plugin.json/name",
            format!("invalid plugin name: {name}"),
        ));
        return None;
    }
    let version = optional_string(object, "version", "plugin.json/version", diagnostics)?;
    let description = optional_string(
        object,
        "description",
        "plugin.json/description",
        diagnostics,
    )?;
    let homepage = optional_string(object, "homepage", "plugin.json/homepage", diagnostics)?;
    let repository = optional_string(object, "repository", "plugin.json/repository", diagnostics)?;
    let license = optional_string(object, "license", "plugin.json/license", diagnostics)?;
    let keywords = match object.get("keywords") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut values = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let Some(item) = item.as_str() else {
                    diagnostics.push(error(
                        "manifest_keywords_invalid",
                        format!("plugin.json/keywords/{index}"),
                        "keyword must be a string",
                    ));
                    return None;
                };
                values.push(item.to_string());
            }
            values
        }
        Some(_) => {
            diagnostics.push(error(
                "manifest_keywords_invalid",
                "plugin.json/keywords",
                "keywords must be an array of strings",
            ));
            return None;
        }
    };
    let author = parse_author(object.get("author"), diagnostics)?;
    let extensions = parse_extensions(object.get("extensions"), diagnostics);
    Some((
        AgentPluginManifest {
            schema,
            name,
            version,
            description,
            author,
            homepage,
            repository,
            license,
            keywords,
        },
        extensions,
    ))
}

fn parse_author(
    value: Option<&Value>,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Option<Option<AgentPluginAuthor>> {
    let Some(value) = value else {
        return Some(None);
    };
    let Some(object) = value.as_object() else {
        diagnostics.push(error(
            "manifest_author_invalid",
            "plugin.json/author",
            "author must be an object",
        ));
        return None;
    };
    for key in object.keys() {
        if !matches!(key.as_str(), "name" | "email" | "url") {
            diagnostics.push(error(
                "manifest_author_invalid",
                format!("plugin.json/author/{key}"),
                "author has an unknown field",
            ));
            return None;
        }
    }
    Some(Some(AgentPluginAuthor {
        name: optional_string(object, "name", "plugin.json/author/name", diagnostics)?,
        email: optional_string(object, "email", "plugin.json/author/email", diagnostics)?,
        url: optional_string(object, "url", "plugin.json/author/url", diagnostics)?,
    }))
}

fn parse_extensions(
    value: Option<&Value>,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> BTreeMap<String, Value> {
    let Some(value) = value else {
        return BTreeMap::new();
    };
    let Some(object) = value.as_object() else {
        diagnostics.push(warning(
            "manifest_extensions_ignored",
            "plugin.json/extensions",
            "non-object extensions field ignored",
        ));
        return BTreeMap::new();
    };
    object
        .iter()
        .filter_map(|(name, value)| {
            if value.is_object() {
                Some((name.clone(), value.clone()))
            } else {
                diagnostics.push(warning(
                    "manifest_extension_ignored",
                    format!("plugin.json/extensions/{name}"),
                    "non-object extension namespace ignored",
                ));
                None
            }
        })
        .collect()
}

fn load_skills(
    root: &Path,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Vec<LoadedAgentPluginSkill> {
    let skills_path = root.join("skills");
    if !skills_path.exists() {
        return Vec::new();
    }
    let skills_root = match fs::canonicalize(&skills_path) {
        Ok(path) if path.starts_with(root) && path.is_dir() => path,
        Ok(_) => {
            diagnostics.push(error(
                "skills_location_invalid",
                "skills",
                "skills location is not a contained directory",
            ));
            return Vec::new();
        }
        Err(source) => {
            diagnostics.push(error(
                "skills_location_invalid",
                "skills",
                source.to_string(),
            ));
            return Vec::new();
        }
    };
    let entries = match fs::read_dir(&skills_root) {
        Ok(entries) => entries,
        Err(source) => {
            diagnostics.push(error("skills_read_failed", "skills", source.to_string()));
            return Vec::new();
        }
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let skill_root = match fs::canonicalize(entry.path()) {
            Ok(path) if path.starts_with(root) && path.is_dir() => path,
            _ => {
                diagnostics.push(warning(
                    "skill_directory_invalid",
                    format!("skills/{dir_name}"),
                    "skill directory escapes plugin root or is not a directory",
                ));
                continue;
            }
        };
        let skill_path = skill_root.join("SKILL.md");
        let skill_path = match canonical_file(root, &skill_path) {
            Ok(path) => path,
            Err(_) => continue,
        };
        match parse_skill(&dir_name, &skill_root, &skill_path) {
            Ok(skill) => skills.push(skill),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn parse_skill(
    dir_name: &str,
    skill_root: &Path,
    skill_path: &Path,
) -> Result<LoadedAgentPluginSkill, AgentPluginDiagnostic> {
    let text = fs::read_to_string(skill_path).map_err(|source| {
        warning(
            "skill_read_failed",
            format!("skills/{dir_name}/SKILL.md"),
            source.to_string(),
        )
    })?;
    let (frontmatter, body) = split_skill_frontmatter(&text).ok_or_else(|| {
        warning(
            "skill_frontmatter_missing",
            format!("skills/{dir_name}/SKILL.md"),
            "skill must start with YAML frontmatter",
        )
    })?;
    let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(frontmatter).map_err(|source| {
        warning(
            "skill_frontmatter_invalid",
            format!("skills/{dir_name}/SKILL.md"),
            source.to_string(),
        )
    })?;
    let serde_yaml_ng::Value::Mapping(map) = value else {
        return Err(warning(
            "skill_frontmatter_invalid",
            format!("skills/{dir_name}/SKILL.md"),
            "frontmatter must be a mapping",
        ));
    };
    let mut fields = BTreeMap::new();
    for (key, value) in map {
        let serde_yaml_ng::Value::String(key) = key else {
            return Err(warning(
                "skill_frontmatter_invalid",
                format!("skills/{dir_name}/SKILL.md"),
                "frontmatter keys must be strings",
            ));
        };
        fields.insert(key, value);
    }
    for key in fields.keys() {
        if !matches!(
            key.as_str(),
            "name" | "description" | "license" | "compatibility" | "metadata" | "allowed-tools"
        ) {
            return Err(warning(
                "skill_frontmatter_unknown_field",
                format!("skills/{dir_name}/SKILL.md/{key}"),
                "unknown Agent Skills frontmatter field",
            ));
        }
    }
    let name = yaml_required_string(&fields, "name", dir_name)?;
    if name != dir_name || !is_skill_name(&name) {
        return Err(warning(
            "skill_name_invalid",
            format!("skills/{dir_name}/SKILL.md/name"),
            "skill name must match its directory and Agent Skills name rules",
        ));
    }
    let description = yaml_required_string(&fields, "description", dir_name)?;
    let len = description.chars().count();
    if description.is_empty() || len > 1024 {
        return Err(warning(
            "skill_description_invalid",
            format!("skills/{dir_name}/SKILL.md/description"),
            format!("description length {len} is outside 1-1024 characters"),
        ));
    }
    let license = yaml_optional_string(&fields, "license", dir_name)?;
    let compatibility = yaml_optional_string(&fields, "compatibility", dir_name)?;
    if compatibility
        .as_ref()
        .is_some_and(|value| value.chars().count() > 500)
    {
        return Err(warning(
            "skill_compatibility_invalid",
            format!("skills/{dir_name}/SKILL.md/compatibility"),
            "compatibility must be at most 500 characters",
        ));
    }
    let allowed_tools = yaml_optional_string(&fields, "allowed-tools", dir_name)?;
    let metadata = yaml_metadata(&fields, dir_name)?;
    Ok(LoadedAgentPluginSkill {
        name,
        description,
        license,
        compatibility,
        allowed_tools,
        metadata,
        body: body.to_string(),
        path: skill_path.to_path_buf(),
        root: skill_root.to_path_buf(),
    })
}

fn load_mcp_servers(
    root: &Path,
    options: &AgentPluginLoadOptions,
    plugin_schema: &str,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> BTreeMap<String, AgentPluginMcpServer> {
    let mcp_path = root.join("mcp.json");
    if !mcp_path.exists() {
        return BTreeMap::new();
    }
    let mcp_path = match canonical_file(root, &mcp_path) {
        Ok(path) => path,
        Err(message) => {
            diagnostics.push(error("mcp_location_invalid", "mcp.json", message));
            return BTreeMap::new();
        }
    };
    let text = match fs::read_to_string(&mcp_path) {
        Ok(text) => text,
        Err(source) => {
            diagnostics.push(error("mcp_read_failed", "mcp.json", source.to_string()));
            return BTreeMap::new();
        }
    };
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) => value,
        Err(source) => {
            diagnostics.push(error("mcp_json_invalid", "mcp.json", source.to_string()));
            return BTreeMap::new();
        }
    };
    let Some(object) = value.as_object() else {
        diagnostics.push(error(
            "mcp_not_object",
            "mcp.json",
            "mcp.json must be an object",
        ));
        return BTreeMap::new();
    };
    if object.len() != 2 || !object.contains_key("$schema") || !object.contains_key("mcpServers") {
        diagnostics.push(error(
            "mcp_top_level_invalid",
            "mcp.json",
            "mcp.json must contain exactly $schema and mcpServers",
        ));
        return BTreeMap::new();
    }
    let schema = object.get("$schema").and_then(Value::as_str);
    if schema != Some(AGENT_PLUGIN_MCP_SCHEMA) || plugin_schema != AGENT_PLUGIN_SCHEMA {
        diagnostics.push(error(
            "mcp_schema_unsupported",
            "mcp.json/$schema",
            "unsupported or mismatched MCP schema",
        ));
        return BTreeMap::new();
    }
    let Some(servers) = object.get("mcpServers").and_then(Value::as_object) else {
        diagnostics.push(error(
            "mcp_servers_invalid",
            "mcp.json/mcpServers",
            "mcpServers must be an object",
        ));
        return BTreeMap::new();
    };
    let mut result = BTreeMap::new();
    for (name, server) in servers {
        match parse_mcp_server(name, server, root, options) {
            Ok((transport, server)) if options.supported_mcp_transports.contains(&transport) => {
                result.insert(name.clone(), server);
            }
            Ok((transport, _)) => diagnostics.push(warning(
                "mcp_transport_unsupported",
                format!("mcp.json/mcpServers/{name}/type"),
                format!("transport is not supported: {transport:?}"),
            )),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    result
}

fn parse_mcp_server(
    name: &str,
    value: &Value,
    root: &Path,
    options: &AgentPluginLoadOptions,
) -> Result<(AgentPluginMcpTransport, AgentPluginMcpServer), AgentPluginDiagnostic> {
    let Some(object) = value.as_object() else {
        return Err(warning(
            "mcp_server_invalid",
            format!("mcp.json/mcpServers/{name}"),
            "server entry must be an object",
        ));
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(warning(
            "mcp_server_type_invalid",
            format!("mcp.json/mcpServers/{name}/type"),
            "server type is required",
        ));
    };
    match kind {
        "stdio" => parse_stdio_server(name, object, root, options).map(|server| {
            (
                AgentPluginMcpTransport::Stdio,
                AgentPluginMcpServer::Stdio(server),
            )
        }),
        "streamable-http" => parse_http_server(name, object).map(|server| {
            (
                AgentPluginMcpTransport::StreamableHttp,
                AgentPluginMcpServer::StreamableHttp(server),
            )
        }),
        "sse" => parse_http_server(name, object).map(|server| {
            (
                AgentPluginMcpTransport::Sse,
                AgentPluginMcpServer::Sse(server),
            )
        }),
        _ => Err(warning(
            "mcp_server_type_invalid",
            format!("mcp.json/mcpServers/{name}/type"),
            format!("unknown server type: {kind}"),
        )),
    }
}

fn parse_stdio_server(
    name: &str,
    object: &serde_json::Map<String, Value>,
    root: &Path,
    options: &AgentPluginLoadOptions,
) -> Result<AgentPluginStdioMcpServer, AgentPluginDiagnostic> {
    for key in object.keys() {
        if !matches!(key.as_str(), "type" | "command" | "args" | "env" | "cwd") {
            return Err(warning(
                "mcp_server_unknown_field",
                format!("mcp.json/mcpServers/{name}/{key}"),
                "unknown stdio server field",
            ));
        }
    }
    let command = json_required_string(object, "command", name)?;
    let resolved_command = validate_command(root, name, &command)?;
    let args = json_string_array(object.get("args"), name, "args")?;
    let env = json_string_map(object.get("env"), name, "env")?;
    for key in env.keys() {
        if key.eq_ignore_ascii_case("PLUGIN_ROOT") || key.eq_ignore_ascii_case("PLUGIN_DATA") {
            return Err(warning(
                "mcp_env_reserved",
                format!("mcp.json/mcpServers/{name}/env/{key}"),
                "PLUGIN_ROOT and PLUGIN_DATA are reserved",
            ));
        }
    }
    let cwd = match object.get("cwd") {
        Some(Value::String(cwd)) => Some(cwd.clone()),
        Some(_) => {
            return Err(warning(
                "mcp_cwd_invalid",
                format!("mcp.json/mcpServers/{name}/cwd"),
                "cwd must be a string",
            ));
        }
        None => None,
    };
    let data_root = normalize_absolute(&options.plugin_data_root);
    let resolved_cwd = match &cwd {
        Some(cwd) => resolve_cwd(root, &data_root, name, cwd)?,
        None => root.to_path_buf(),
    };
    Ok(AgentPluginStdioMcpServer {
        command,
        resolved_command,
        resolved_args: args
            .iter()
            .map(|arg| expand_plugin_vars(arg, root, &data_root))
            .collect(),
        resolved_env: env
            .iter()
            .map(|(key, value)| (key.clone(), expand_plugin_vars(value, root, &data_root)))
            .collect(),
        args,
        env,
        cwd,
        resolved_cwd,
    })
}

fn parse_http_server(
    name: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<AgentPluginHttpMcpServer, AgentPluginDiagnostic> {
    for key in object.keys() {
        if !matches!(key.as_str(), "type" | "url" | "headers") {
            return Err(warning(
                "mcp_server_unknown_field",
                format!("mcp.json/mcpServers/{name}/{key}"),
                "unknown HTTP server field",
            ));
        }
    }
    let url = json_required_string(object, "url", name)?;
    validate_mcp_url(name, &url)?;
    let headers = json_string_map(object.get("headers"), name, "headers")?;
    let mut seen = BTreeSet::new();
    for (key, value) in &headers {
        if !seen.insert(key.to_ascii_lowercase()) {
            return Err(warning(
                "mcp_header_duplicate",
                format!("mcp.json/mcpServers/{name}/headers/{key}"),
                "duplicate header name with different casing",
            ));
        }
        http::HeaderName::from_bytes(key.as_bytes()).map_err(|_| {
            warning(
                "mcp_header_invalid",
                format!("mcp.json/mcpServers/{name}/headers/{key}"),
                "invalid HTTP header name",
            )
        })?;
        http::HeaderValue::from_str(value).map_err(|_| {
            warning(
                "mcp_header_invalid",
                format!("mcp.json/mcpServers/{name}/headers/{key}"),
                "invalid HTTP header value",
            )
        })?;
    }
    Ok(AgentPluginHttpMcpServer { url, headers })
}

fn validate_command(
    root: &Path,
    server: &str,
    command: &str,
) -> Result<Option<PathBuf>, AgentPluginDiagnostic> {
    if command.is_empty() || command.chars().any(char::is_whitespace) {
        return Err(warning(
            "mcp_command_invalid",
            format!("mcp.json/mcpServers/{server}/command"),
            "command must be one executable token",
        ));
    }
    if let Some(rest) = command.strip_prefix("./") {
        if rest.is_empty() {
            return Err(warning(
                "mcp_command_invalid",
                format!("mcp.json/mcpServers/{server}/command"),
                "plugin-relative command must name a file",
            ));
        }
        let path = root.join(rest);
        return canonical_file(root, &path).map(Some).map_err(|message| {
            warning(
                "mcp_command_invalid",
                format!("mcp.json/mcpServers/{server}/command"),
                message,
            )
        });
    }
    if command.contains('/') || command.contains('\\') || command == "." || command == ".." {
        return Err(warning(
            "mcp_command_invalid",
            format!("mcp.json/mcpServers/{server}/command"),
            "bare command must not contain path separators",
        ));
    }
    Ok(None)
}

fn resolve_cwd(
    root: &Path,
    data_root: &Path,
    server: &str,
    cwd: &str,
) -> Result<PathBuf, AgentPluginDiagnostic> {
    if let Some(rest) = cwd.strip_prefix("./") {
        let path = fs::canonicalize(root.join(rest)).map_err(|source| {
            warning(
                "mcp_cwd_invalid",
                format!("mcp.json/mcpServers/{server}/cwd"),
                source.to_string(),
            )
        })?;
        if path.starts_with(root) && path.is_dir() {
            return Ok(path);
        }
    } else if cwd == "${PLUGIN_ROOT}" || cwd.starts_with("${PLUGIN_ROOT}/") {
        let path = fs::canonicalize(
            root.join(
                cwd.trim_start_matches("${PLUGIN_ROOT}")
                    .trim_start_matches('/'),
            ),
        )
        .map_err(|source| {
            warning(
                "mcp_cwd_invalid",
                format!("mcp.json/mcpServers/{server}/cwd"),
                source.to_string(),
            )
        })?;
        if path.starts_with(root) && path.is_dir() {
            return Ok(path);
        }
    } else if cwd == "${PLUGIN_DATA}" || cwd.starts_with("${PLUGIN_DATA}/") {
        let path = normalize_absolute(
            &data_root.join(
                cwd.trim_start_matches("${PLUGIN_DATA}")
                    .trim_start_matches('/'),
            ),
        );
        if path.starts_with(data_root) {
            return Ok(path);
        }
    }
    Err(warning(
        "mcp_cwd_invalid",
        format!("mcp.json/mcpServers/{server}/cwd"),
        "cwd must be contained in PLUGIN_ROOT or PLUGIN_DATA",
    ))
}

fn validate_mcp_url(server: &str, raw: &str) -> Result<(), AgentPluginDiagnostic> {
    let url = url::Url::parse(raw).map_err(|source| {
        warning(
            "mcp_url_invalid",
            format!("mcp.json/mcpServers/{server}/url"),
            source.to_string(),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(warning(
            "mcp_url_invalid",
            format!("mcp.json/mcpServers/{server}/url"),
            "url must be absolute HTTP(S), without userinfo or fragment",
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(warning(
            "mcp_url_insecure",
            format!("mcp.json/mcpServers/{server}/url"),
            "non-loopback MCP endpoints must use HTTPS",
        ));
    }
    Ok(())
}

fn is_loopback_url(url: &url::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn canonical_file(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path).map_err(|source| source.to_string())?;
    if path.starts_with(root) && path.is_file() {
        Ok(path)
    } else {
        Err("path is not a contained regular file".to_string())
    }
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Option<String> {
    match object.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) | None => {
            diagnostics.push(error(
                "manifest_required_field_invalid",
                path,
                format!("{key} must be a non-empty string"),
            ));
            None
        }
    }
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Option<Option<String>> {
    match object.get(key) {
        None => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => {
            diagnostics.push(error(
                "manifest_field_invalid",
                path,
                format!("{key} must be a string"),
            ));
            None
        }
    }
}

fn json_required_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    server: &str,
) -> Result<String, AgentPluginDiagnostic> {
    match object.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(warning(
            "mcp_string_invalid",
            format!("mcp.json/mcpServers/{server}/{key}"),
            format!("{key} must be a non-empty string"),
        )),
    }
}

fn json_string_array(
    value: Option<&Value>,
    server: &str,
    key: &str,
) -> Result<Vec<String>, AgentPluginDiagnostic> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                value.as_str().map(ToString::to_string).ok_or_else(|| {
                    warning(
                        "mcp_array_invalid",
                        format!("mcp.json/mcpServers/{server}/{key}/{index}"),
                        "array item must be a string",
                    )
                })
            })
            .collect(),
        Some(_) => Err(warning(
            "mcp_array_invalid",
            format!("mcp.json/mcpServers/{server}/{key}"),
            "field must be an array of strings",
        )),
    }
}

fn json_string_map(
    value: Option<&Value>,
    server: &str,
    key: &str,
) -> Result<BTreeMap<String, String>, AgentPluginDiagnostic> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_string()))
                    .ok_or_else(|| {
                        warning(
                            "mcp_map_invalid",
                            format!("mcp.json/mcpServers/{server}/{key}/{name}"),
                            "map value must be a string",
                        )
                    })
            })
            .collect(),
        Some(_) => Err(warning(
            "mcp_map_invalid",
            format!("mcp.json/mcpServers/{server}/{key}"),
            "field must be an object of strings",
        )),
    }
}

fn yaml_required_string(
    fields: &BTreeMap<String, serde_yaml_ng::Value>,
    key: &str,
    dir_name: &str,
) -> Result<String, AgentPluginDiagnostic> {
    match fields.get(key) {
        Some(serde_yaml_ng::Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(warning(
            "skill_field_invalid",
            format!("skills/{dir_name}/SKILL.md/{key}"),
            format!("{key} must be a non-empty string"),
        )),
    }
}

fn yaml_optional_string(
    fields: &BTreeMap<String, serde_yaml_ng::Value>,
    key: &str,
    dir_name: &str,
) -> Result<Option<String>, AgentPluginDiagnostic> {
    match fields.get(key) {
        None => Ok(None),
        Some(serde_yaml_ng::Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(warning(
            "skill_field_invalid",
            format!("skills/{dir_name}/SKILL.md/{key}"),
            format!("{key} must be a string"),
        )),
    }
}

fn yaml_metadata(
    fields: &BTreeMap<String, serde_yaml_ng::Value>,
    dir_name: &str,
) -> Result<BTreeMap<String, String>, AgentPluginDiagnostic> {
    let Some(value) = fields.get("metadata") else {
        return Ok(BTreeMap::new());
    };
    let serde_yaml_ng::Value::Mapping(map) = value else {
        return Err(warning(
            "skill_metadata_invalid",
            format!("skills/{dir_name}/SKILL.md/metadata"),
            "metadata must be a string map",
        ));
    };
    let mut result = BTreeMap::new();
    for (key, value) in map {
        let serde_yaml_ng::Value::String(key) = key else {
            return Err(warning(
                "skill_metadata_invalid",
                format!("skills/{dir_name}/SKILL.md/metadata"),
                "metadata keys must be strings",
            ));
        };
        let serde_yaml_ng::Value::String(value) = value else {
            return Err(warning(
                "skill_metadata_invalid",
                format!("skills/{dir_name}/SKILL.md/metadata/{key}"),
                "metadata values must be strings",
            ));
        };
        result.insert(key.clone(), value.clone());
    }
    Ok(result)
}

fn split_skill_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let body = rest[end + "\n---".len()..].trim_start_matches('\n');
    Some((frontmatter, body))
}

fn expand_plugin_vars(value: &str, root: &Path, data_root: &Path) -> String {
    value
        .replace("${PLUGIN_ROOT}", &root.to_string_lossy())
        .replace("${PLUGIN_DATA}", &data_root.to_string_lossy())
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn is_plugin_name(name: &str) -> bool {
    name_len(name) <= 64
        && !name.is_empty()
        && !name.contains("--")
        && !name.contains("..")
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

fn is_skill_name(name: &str) -> bool {
    name_len(name) <= 64
        && !name.is_empty()
        && !name.contains("--")
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn name_len(name: &str) -> usize {
    name.chars().count()
}

fn warning(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> AgentPluginDiagnostic {
    AgentPluginDiagnostic {
        severity: AgentPluginDiagnosticSeverity::Warning,
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn error(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> AgentPluginDiagnostic {
    AgentPluginDiagnostic {
        severity: AgentPluginDiagnosticSeverity::Error,
        code,
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "incurs-agent-plugin-loader-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn manifest(root: &Path) {
        write(
            root.join("plugin.json").as_path(),
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo.plugin","extensions":{"com.example.client":{"enabled":true}}}"#,
        );
    }

    #[test]
    fn load_discovers_manifest_extensions_skill_and_all_mcp_transports() {
        let root = root("happy");
        manifest(&root);
        write(
            root.join("skills/deploy/SKILL.md").as_path(),
            "---\nname: deploy\ndescription: Deploy the project.\nmetadata:\n  incurs.command: demo deploy\n---\n\n# Deploy\n",
        );
        write(root.join("bin/server").as_path(), "");
        let mcp = r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "local":{"type":"stdio","command":"./bin/server","args":["--root","${PLUGIN_ROOT}"],"env":{"CACHE":"${PLUGIN_DATA}/cache"},"cwd":"${PLUGIN_ROOT}"},
            "remote":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"X-Tenant":"public"}},
            "legacy":{"type":"sse","url":"http://localhost:3000/sse"}
          }
        }"#;
        write(root.join("mcp.json").as_path(), mcp);

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(plugin.manifest.name, "demo.plugin");
        assert_eq!(
            plugin.extensions.keys().cloned().collect::<Vec<_>>(),
            vec!["com.example.client"]
        );
        assert_eq!(plugin.skills[0].name, "deploy");
        assert_eq!(plugin.mcp_servers.len(), 3);
        assert!(matches!(
            plugin.mcp_servers["local"],
            AgentPluginMcpServer::Stdio(_)
        ));
        assert!(matches!(
            plugin.mcp_servers["remote"],
            AgentPluginMcpServer::StreamableHttp(_)
        ));
        assert!(matches!(
            plugin.mcp_servers["legacy"],
            AgentPluginMcpServer::Sse(_)
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_unknown_fields_and_non_object_extensions_are_nonfatal() {
        let root = root("manifest-warnings");
        write(
            root.join("plugin.json").as_path(),
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo","unknown":true,"extensions":false}"#,
        );

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());

        assert!(report.plugin.is_some());
        assert_eq!(
            report
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            vec!["manifest_unknown_field", "manifest_extensions_ignored"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fatal_manifest_error_rejects_all_components() {
        let root = root("fatal-manifest");
        write(
            root.join("plugin.json").as_path(),
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"Bad"}"#,
        );
        write(
            root.join("skills/deploy/SKILL.md").as_path(),
            "---\nname: deploy\ndescription: Deploy.\n---\n",
        );

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());

        assert!(report.plugin.is_none());
        assert_eq!(report.diagnostics[0].code, "manifest_name_invalid");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_skill_and_invalid_mcp_server_are_skipped_independently() {
        let root = root("component-boundaries");
        manifest(&root);
        write(
            root.join("skills/good/SKILL.md").as_path(),
            "---\nname: good\ndescription: Good skill.\n---\n",
        );
        write(
            root.join("skills/bad/SKILL.md").as_path(),
            "---\nname: other\ndescription: Bad skill.\n---\n",
        );
        let mcp = r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "bad":{"type":"stdio","command":"./missing"},
            "good":{"type":"streamable-http","url":"https://example.com/mcp"}
          }
        }"#;
        write(root.join("mcp.json").as_path(), mcp);

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert_eq!(
            plugin
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["good"]
        );
        assert_eq!(
            plugin.mcp_servers.keys().cloned().collect::<Vec<_>>(),
            vec!["good"]
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "skill_name_invalid")
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "mcp_command_invalid")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_top_level_error_disables_only_mcp() {
        let root = root("mcp-top");
        manifest(&root);
        write(
            root.join("skills/good/SKILL.md").as_path(),
            "---\nname: good\ndescription: Good skill.\n---\n",
        );
        write(
            root.join("mcp.json").as_path(),
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":{},"extra":true}"#,
        );

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert_eq!(plugin.skills.len(), 1);
        assert!(plugin.mcp_servers.is_empty());
        assert_eq!(report.diagnostics[0].code, "mcp_top_level_invalid");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_insecure_non_loopback_http_url_and_duplicate_headers() {
        let root = root("http-invalid");
        manifest(&root);
        write(
            root.join("mcp.json").as_path(),
            r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "insecure":{"type":"streamable-http","url":"http://example.com/mcp"},
            "headers":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"X-Test":"a","x-test":"b"}}
          }
        }"#,
        );

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert!(plugin.mcp_servers.is_empty());
        assert_eq!(
            report
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            vec!["mcp_url_insecure", "mcp_header_duplicate"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stdio_rejects_reserved_env_and_contained_data_cwd_escape() {
        let root = root("stdio-invalid");
        manifest(&root);
        write(
            root.join("mcp.json").as_path(),
            r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "env":{"type":"stdio","command":"demo","env":{"PLUGIN_ROOT":"bad"}},
            "cwd":{"type":"stdio","command":"demo","cwd":"${PLUGIN_DATA}/../escape"}
          }
        }"#,
        );

        let report = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert!(plugin.mcp_servers.is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "mcp_env_reserved")
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "mcp_cwd_invalid")
        );

        let _ = fs::remove_dir_all(root);
    }
}
