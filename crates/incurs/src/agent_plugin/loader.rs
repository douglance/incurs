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

/// Plugin data root that [`AgentPluginLoadOptions::default`] uses on `wasm32` targets.
///
/// `wasm32` targets such as Cloudflare Workers have no temporary directory, so
/// the default data root there is this fixed absolute path. The loader never
/// creates or reads it; it only expands `${PLUGIN_DATA}` to it. A host that
/// persists plugin data maps this path to its own storage.
pub const AGENT_PLUGIN_VIRTUAL_DATA_ROOT: &str = "/incurs-agent-plugin-data";

/// Virtual root that [`AgentPluginFiles::new`] gives an in-memory plugin package.
pub const AGENT_PLUGIN_VIRTUAL_ROOT: &str = "/agent-plugin";

impl Default for AgentPluginLoadOptions {
    /// Returns options that accept every MCP transport and place plugin data in
    /// `incurs-agent-plugin-data` under the process temporary directory, or at
    /// [`AGENT_PLUGIN_VIRTUAL_DATA_ROOT`] on `wasm32` targets.
    fn default() -> Self {
        Self {
            plugin_data_root: default_plugin_data_root(),
            supported_mcp_transports: AgentPluginMcpTransport::all().into_iter().collect(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn default_plugin_data_root() -> PathBuf {
    std::env::temp_dir().join("incurs-agent-plugin-data")
}

#[cfg(target_arch = "wasm32")]
fn default_plugin_data_root() -> PathBuf {
    PathBuf::from(AGENT_PLUGIN_VIRTUAL_DATA_ROOT)
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
    /// Resolved plugin root that every other resolved path sits under.
    ///
    /// For [`load_agent_plugin`] this is the canonical directory on disk. For
    /// [`load_agent_plugin_from_files`] it is the package's virtual root
    /// ([`AgentPluginFiles::virtual_root`]), which names no real directory.
    /// `${PLUGIN_ROOT}` expands to this path in both cases.
    pub root: PathBuf,
    /// Lexically normalized client-managed writable data root from
    /// [`AgentPluginLoadOptions::plugin_data_root`].
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
    /// Resolved `SKILL.md` path under [`LoadedAgentPlugin::root`]; virtual for
    /// an in-memory package.
    pub path: PathBuf,
    /// Resolved skill root directory under [`LoadedAgentPlugin::root`]; virtual
    /// for an in-memory package.
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
    /// Resolved command path under [`LoadedAgentPlugin::root`] when `command` is
    /// plugin-relative.
    ///
    /// For an in-memory package this is a virtual path naming a package file. It
    /// is reported so the binding is not lost, but it cannot be executed until a
    /// host writes the package to a filesystem at that root.
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
    /// Working directory after placeholder expansion, under
    /// [`LoadedAgentPlugin::root`] or [`LoadedAgentPlugin::data_root`]; virtual
    /// for a plugin-root directory of an in-memory package.
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

/// Portable Agent Plugin package whose files are held in memory.
///
/// Keys are plugin-relative paths with `/` separators, such as `plugin.json`
/// or `skills/deploy/SKILL.md`; empty and `.` segments are ignored.
/// Directories are implicit: a directory exists when a file lies beneath it,
/// so an empty directory cannot be represented.
///
/// A path that is absolute, contains a `..` segment, or contains a backslash
/// never resolves to contents. When the loader reaches such a path, through a
/// reference in `mcp.json` or an entry under `skills/`, it reports the same
/// diagnostic that [`load_agent_plugin`] reports for a path escaping the root.
///
/// Loaded paths sit under a virtual root, [`AGENT_PLUGIN_VIRTUAL_ROOT`] unless
/// [`AgentPluginFiles::with_virtual_root`] names another. The virtual root is
/// never touched on disk.
#[derive(Debug, Clone)]
pub struct AgentPluginFiles {
    root: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    unresolvable: BTreeSet<String>,
}

impl AgentPluginFiles {
    /// Creates an empty package rooted at [`AGENT_PLUGIN_VIRTUAL_ROOT`].
    pub fn new() -> Self {
        Self::with_virtual_root(AGENT_PLUGIN_VIRTUAL_ROOT)
    }

    /// Creates an empty package whose loaded paths sit under `root`.
    ///
    /// `root` is normalized lexically and never read. It should be absolute,
    /// because `${PLUGIN_ROOT}` expands to it in stdio server arguments and
    /// environment values.
    pub fn with_virtual_root(root: impl AsRef<Path>) -> Self {
        Self {
            root: normalize_absolute(root.as_ref()),
            files: BTreeMap::new(),
            unresolvable: BTreeSet::new(),
        }
    }

    /// Returns the virtual root that loaded paths are resolved under.
    pub fn virtual_root(&self) -> &Path {
        &self.root
    }

    /// Adds or replaces the file at plugin-relative `path`, returning the
    /// previous contents of that file.
    ///
    /// A path that escapes the package, or that names the root itself, is
    /// recorded without contents and returns `None`.
    pub fn insert(
        &mut self,
        path: impl AsRef<str>,
        contents: impl Into<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        let path = path.as_ref();
        match normalize_package_path(path) {
            Some(key) if !key.is_empty() => self.files.insert(key, contents.into()),
            _ => {
                self.unresolvable.insert(path.to_string());
                None
            }
        }
    }

    fn key_for<'a>(&self, resolved: &'a Path) -> Result<&'a str, String> {
        resolved
            .strip_prefix(&self.root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| {
                format!(
                    "path is outside the in-memory plugin package: {}",
                    resolved.display()
                )
            })
    }

    fn is_dir_key(&self, key: &str) -> bool {
        if key.is_empty() {
            return true;
        }
        let prefix = format!("{key}/");
        self.files
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(path, _)| path.starts_with(&prefix))
    }
}

impl Default for AgentPluginFiles {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: AsRef<str>, V: Into<Vec<u8>>> FromIterator<(K, V)> for AgentPluginFiles {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut files = Self::new();
        for (path, contents) in iter {
            files.insert(path, contents);
        }
        files
    }
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
    load_from_source(&DiskSource { root }, options)
}

/// Loads a portable Agent Plugins 1.0 package held in memory, without touching
/// the filesystem or fetching schemas.
///
/// This works on targets without a filesystem, such as
/// `wasm32-unknown-unknown`. Diagnostics use the same codes as
/// [`load_agent_plugin`], and escaping paths are rejected as described on
/// [`AgentPluginFiles`]. Resolved paths sit under
/// [`AgentPluginFiles::virtual_root`]; stdio MCP servers are loaded and
/// reported exactly as on disk even though their virtual command paths cannot
/// be launched in place.
pub fn load_agent_plugin_from_files(
    files: &AgentPluginFiles,
    options: &AgentPluginLoadOptions,
) -> AgentPluginLoadReport {
    load_from_source(files, options)
}

fn load_from_source(
    source: &impl PluginSource,
    options: &AgentPluginLoadOptions,
) -> AgentPluginLoadReport {
    let mut diagnostics = Vec::new();
    let plugin_json = match contained_file(source, Path::new("plugin.json")) {
        Ok(path) => path,
        Err(message) => {
            diagnostics.push(error("manifest_path_invalid", "plugin.json", message));
            return AgentPluginLoadReport {
                plugin: None,
                diagnostics,
            };
        }
    };
    let manifest_text = match source.read_to_string(&plugin_json) {
        Ok(text) => text,
        Err(message) => {
            diagnostics.push(error("manifest_read_failed", "plugin.json", message));
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
    let skills = load_skills(source, &mut diagnostics);
    let data_root = normalize_absolute(&options.plugin_data_root);
    let mcp_servers = load_mcp_servers(source, options, &manifest.schema, &mut diagnostics);
    let plugin = LoadedAgentPlugin {
        root: source.root().to_path_buf(),
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
    source: &impl PluginSource,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> Vec<LoadedAgentPluginSkill> {
    let skills_path = Path::new("skills");
    if !source.exists(skills_path) {
        return Vec::new();
    }
    let skills_root = match source.resolve(skills_path) {
        Ok(Resolved::Contained {
            path,
            kind: EntryKind::Dir,
        }) => path,
        Ok(_) => {
            diagnostics.push(error(
                "skills_location_invalid",
                "skills",
                "skills location is not a contained directory",
            ));
            return Vec::new();
        }
        Err(message) => {
            diagnostics.push(error("skills_location_invalid", "skills", message));
            return Vec::new();
        }
    };
    let entries = match source.children(&skills_root) {
        Ok(entries) => entries,
        Err(message) => {
            diagnostics.push(error("skills_read_failed", "skills", message));
            return Vec::new();
        }
    };
    let mut skills = Vec::new();
    for file_name in entries {
        let dir_name = file_name.to_string_lossy().into_owned();
        let relative = skills_path.join(&file_name);
        let skill_root = match source.resolve(&relative) {
            Ok(Resolved::Contained {
                path,
                kind: EntryKind::Dir,
            }) => path,
            _ => {
                diagnostics.push(warning(
                    "skill_directory_invalid",
                    format!("skills/{dir_name}"),
                    "skill directory escapes plugin root or is not a directory",
                ));
                continue;
            }
        };
        let skill_path = match contained_file(source, &relative.join("SKILL.md")) {
            Ok(path) => path,
            Err(_) => continue,
        };
        match parse_skill(source, &dir_name, &skill_root, &skill_path) {
            Ok(skill) => skills.push(skill),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn parse_skill(
    source: &impl PluginSource,
    dir_name: &str,
    skill_root: &Path,
    skill_path: &Path,
) -> Result<LoadedAgentPluginSkill, AgentPluginDiagnostic> {
    let text = source.read_to_string(skill_path).map_err(|message| {
        warning(
            "skill_read_failed",
            format!("skills/{dir_name}/SKILL.md"),
            message,
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
    source: &impl PluginSource,
    options: &AgentPluginLoadOptions,
    plugin_schema: &str,
    diagnostics: &mut Vec<AgentPluginDiagnostic>,
) -> BTreeMap<String, AgentPluginMcpServer> {
    let mcp_path = Path::new("mcp.json");
    if !source.exists(mcp_path) {
        return BTreeMap::new();
    }
    let mcp_path = match contained_file(source, mcp_path) {
        Ok(path) => path,
        Err(message) => {
            diagnostics.push(error("mcp_location_invalid", "mcp.json", message));
            return BTreeMap::new();
        }
    };
    let text = match source.read_to_string(&mcp_path) {
        Ok(text) => text,
        Err(message) => {
            diagnostics.push(error("mcp_read_failed", "mcp.json", message));
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
        match parse_mcp_server(name, server, source, options) {
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
    source: &impl PluginSource,
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
        "stdio" => parse_stdio_server(name, object, source, options).map(|server| {
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
    source: &impl PluginSource,
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
    let resolved_command = validate_command(source, name, &command)?;
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
    let root = source.root();
    let resolved_cwd = match &cwd {
        Some(cwd) => resolve_cwd(source, &data_root, name, cwd)?,
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
    source: &impl PluginSource,
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
        return contained_file(source, Path::new(rest))
            .map(Some)
            .map_err(|message| {
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
    source: &impl PluginSource,
    data_root: &Path,
    server: &str,
    cwd: &str,
) -> Result<PathBuf, AgentPluginDiagnostic> {
    let plugin_relative = if let Some(rest) = cwd.strip_prefix("./") {
        Some(rest)
    } else if cwd == "${PLUGIN_ROOT}" || cwd.starts_with("${PLUGIN_ROOT}/") {
        Some(
            cwd.trim_start_matches("${PLUGIN_ROOT}")
                .trim_start_matches('/'),
        )
    } else {
        None
    };
    if let Some(relative) = plugin_relative {
        let resolved = source.resolve(Path::new(relative)).map_err(|message| {
            warning(
                "mcp_cwd_invalid",
                format!("mcp.json/mcpServers/{server}/cwd"),
                message,
            )
        })?;
        if let Resolved::Contained {
            path,
            kind: EntryKind::Dir,
        } = resolved
        {
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

/// Kind of a contained entry that a [`PluginSource`] resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
    Other,
}

/// Outcome of resolving a plugin-relative path that exists.
enum Resolved {
    /// The path stays inside the plugin root.
    Contained { path: PathBuf, kind: EntryKind },
    /// The path leaves the plugin root.
    Escaped,
}

/// File source the loader reads a plugin package through.
///
/// Relative paths are plugin-relative. Resolved paths are the ones reported in
/// loaded results and are the only paths passed back to `children` and
/// `read_to_string`.
trait PluginSource {
    /// Resolved plugin root.
    fn root(&self) -> &Path;
    /// Whether anything exists at `relative`, even if it escapes the root.
    fn exists(&self, relative: &Path) -> bool;
    /// Resolves `relative`, or returns why nothing exists there.
    fn resolve(&self, relative: &Path) -> Result<Resolved, String>;
    /// Names of the directory-like children of a resolved directory.
    fn children(&self, dir: &Path) -> Result<Vec<PathBuf>, String>;
    /// Reads a resolved file as UTF-8 text.
    fn read_to_string(&self, file: &Path) -> Result<String, String>;
}

/// Plugin directory on disk, with a canonical root.
struct DiskSource {
    root: PathBuf,
}

impl PluginSource for DiskSource {
    fn root(&self) -> &Path {
        &self.root
    }

    fn exists(&self, relative: &Path) -> bool {
        self.root.join(relative).exists()
    }

    fn resolve(&self, relative: &Path) -> Result<Resolved, String> {
        let path =
            fs::canonicalize(self.root.join(relative)).map_err(|source| source.to_string())?;
        if !path.starts_with(&self.root) {
            return Ok(Resolved::Escaped);
        }
        let kind = if path.is_file() {
            EntryKind::File
        } else if path.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::Other
        };
        Ok(Resolved::Contained { path, kind })
    }

    fn children(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let entries = fs::read_dir(dir).map_err(|source| source.to_string())?;
        Ok(entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|file_type| file_type.is_dir() || file_type.is_symlink())
            })
            .map(|entry| PathBuf::from(entry.file_name()))
            .collect())
    }

    fn read_to_string(&self, file: &Path) -> Result<String, String> {
        fs::read_to_string(file).map_err(|source| source.to_string())
    }
}

impl PluginSource for AgentPluginFiles {
    fn root(&self) -> &Path {
        &self.root
    }

    fn exists(&self, relative: &Path) -> bool {
        self.resolve(relative).is_ok()
    }

    fn resolve(&self, relative: &Path) -> Result<Resolved, String> {
        let Some(relative) = relative.to_str() else {
            return Err("path is not valid UTF-8".to_string());
        };
        let Some(key) = normalize_package_path(relative) else {
            return Ok(Resolved::Escaped);
        };
        let kind = if self.files.contains_key(&key) {
            EntryKind::File
        } else if self.is_dir_key(&key) {
            EntryKind::Dir
        } else {
            return Err(format!(
                "no such file or directory in the in-memory plugin package: {relative}"
            ));
        };
        let path = if key.is_empty() {
            self.root.clone()
        } else {
            self.root.join(&key)
        };
        Ok(Resolved::Contained { path, kind })
    }

    fn children(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let key = self.key_for(dir)?;
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        let mut children = BTreeSet::new();
        for path in self.files.keys().chain(&self.unresolvable) {
            let Some((child, _)) = path
                .strip_prefix(prefix.as_str())
                .and_then(|rest| rest.split_once('/'))
            else {
                continue;
            };
            if !child.is_empty() && child != "." {
                children.insert(child.to_string());
            }
        }
        Ok(children.into_iter().map(PathBuf::from).collect())
    }

    fn read_to_string(&self, file: &Path) -> Result<String, String> {
        let key = self.key_for(file)?;
        let bytes = self
            .files
            .get(key)
            .ok_or_else(|| format!("no such file in the in-memory plugin package: {key}"))?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|source| source.to_string())
    }
}

/// Returns `true` when an in-memory package path leaves the package root.
fn escapes_package(path: &str) -> bool {
    path.starts_with('/') || path.contains('\\') || path.split('/').any(|segment| segment == "..")
}

/// Normalizes an in-memory package path by dropping empty and `.` segments, or
/// returns `None` when it escapes the package root.
fn normalize_package_path(path: &str) -> Option<String> {
    if escapes_package(path) {
        return None;
    }
    Some(
        path.split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".")
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn contained_file(source: &impl PluginSource, relative: &Path) -> Result<PathBuf, String> {
    match source.resolve(relative)? {
        Resolved::Contained {
            path,
            kind: EntryKind::File,
        } => Ok(path),
        _ => Err("path is not a contained regular file".to_string()),
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

    /// Reads every file under `root` into an in-memory package keyed by its
    /// forward-slash plugin-relative path.
    fn files_under(root: &Path) -> AgentPluginFiles {
        fn walk(root: &Path, dir: &Path, files: &mut AgentPluginFiles) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, files);
                } else {
                    let key = path
                        .strip_prefix(root)
                        .unwrap()
                        .components()
                        .map(|component| component.as_os_str().to_str().unwrap())
                        .collect::<Vec<_>>()
                        .join("/");
                    files.insert(key, fs::read(&path).unwrap());
                }
            }
        }
        let mut files = AgentPluginFiles::new();
        walk(root, root, &mut files);
        files
    }

    fn diagnostic_keys(
        diagnostics: &[AgentPluginDiagnostic],
    ) -> Vec<(String, &'static str, String)> {
        let mut keys = diagnostics
            .iter()
            .map(|d| (format!("{:?}", d.severity), d.code, d.path.clone()))
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    fn codes(diagnostics: &[AgentPluginDiagnostic]) -> Vec<&'static str> {
        let mut codes = diagnostics.iter().map(|d| d.code).collect::<Vec<_>>();
        codes.sort_unstable();
        codes
    }

    #[test]
    fn in_memory_package_loads_the_same_plugin_as_its_disk_directory() {
        let root = root("equivalence");
        write(
            root.join("plugin.json").as_path(),
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo.plugin","version":"1.2.3","description":"Demo.","author":{"name":"Ada"},"keywords":["demo"],"unknown":1,"extensions":{"com.example.client":{"enabled":true}}}"#,
        );
        write(
            root.join("skills/deploy/SKILL.md").as_path(),
            "---\nname: deploy\ndescription: Deploy the project.\nmetadata:\n  incurs.command: demo deploy\n---\n\n# Deploy\n",
        );
        write(
            root.join("skills/review/SKILL.md").as_path(),
            "---\nname: review\ndescription: Review changes.\nlicense: MIT\ncompatibility: Any client.\nallowed-tools: Read\n---\nReview body.\n",
        );
        write(
            root.join("skills/bad/SKILL.md").as_path(),
            "---\nname: other\ndescription: Mismatched name.\n---\n",
        );
        write(root.join("skills/notes/README.md").as_path(), "not a skill");
        write(root.join("bin/server").as_path(), "#!/bin/sh\n");
        write(
            root.join("mcp.json").as_path(),
            r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "local":{"type":"stdio","command":"./bin/server","args":["--root","${PLUGIN_ROOT}"],"env":{"CACHE":"${PLUGIN_DATA}/cache"},"cwd":"./bin"},
            "rooted":{"type":"stdio","command":"node","cwd":"${PLUGIN_ROOT}"},
            "bare":{"type":"stdio","command":"node"},
            "remote":{"type":"streamable-http","url":"https://example.com/mcp","headers":{"X-Tenant":"public"}},
            "legacy":{"type":"sse","url":"http://localhost:3000/sse"},
            "missing":{"type":"stdio","command":"./missing"},
            "escape":{"type":"stdio","command":"node","cwd":"./../"},
            "insecure":{"type":"streamable-http","url":"http://example.com/mcp"}
          }
        }"#,
        );
        let options = AgentPluginLoadOptions::default();

        let disk = load_agent_plugin(&root, &options);
        let memory = load_agent_plugin_from_files(&files_under(&root), &options);
        let disk_plugin = disk.plugin.as_ref().unwrap();
        let memory_plugin = memory.plugin.as_ref().unwrap();

        // Roots are replaced only where a debug string starts with them, so
        // text that merely contains the root name stays compared verbatim.
        let disk_root = format!("\"{}", disk_plugin.root.to_string_lossy());
        let memory_root = format!("\"{AGENT_PLUGIN_VIRTUAL_ROOT}");
        assert_eq!(
            format!("{disk_plugin:#?}").replace(&disk_root, "\"<ROOT>"),
            format!("{memory_plugin:#?}").replace(&memory_root, "\"<ROOT>"),
        );
        assert_eq!(
            diagnostic_keys(&disk.diagnostics),
            diagnostic_keys(&memory.diagnostics)
        );

        assert_eq!(
            codes(&memory.diagnostics),
            vec![
                "manifest_unknown_field",
                "mcp_command_invalid",
                "mcp_cwd_invalid",
                "mcp_url_insecure",
                "skill_name_invalid",
            ]
        );
        assert_eq!(memory_plugin.root, Path::new("/agent-plugin"));
        assert_eq!(memory_plugin.manifest.name, "demo.plugin");
        assert_eq!(memory_plugin.manifest.keywords, vec!["demo"]);
        let skills = memory_plugin
            .skills
            .iter()
            .map(|skill| {
                (
                    skill.name.as_str(),
                    skill.description.as_str(),
                    skill.body.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            skills,
            vec![
                ("deploy", "Deploy the project.", "# Deploy\n"),
                ("review", "Review changes.", "Review body.\n"),
            ]
        );
        assert_eq!(
            memory_plugin.skills[0].path,
            Path::new("/agent-plugin/skills/deploy/SKILL.md")
        );
        assert_eq!(
            memory_plugin
                .mcp_servers
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["bare", "legacy", "local", "remote", "rooted"]
        );
        let AgentPluginMcpServer::Stdio(local) = &memory_plugin.mcp_servers["local"] else {
            panic!("local server must stay a stdio binding");
        };
        assert_eq!(
            local.resolved_command.as_deref(),
            Some(Path::new("/agent-plugin/bin/server"))
        );
        assert_eq!(local.resolved_cwd, Path::new("/agent-plugin/bin"));
        assert_eq!(local.resolved_args, vec!["--root", "/agent-plugin"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn in_memory_package_rejects_escaping_paths_with_disk_escape_codes() {
        // Every escaping target is present in the package, so only the escape
        // rule stands between each reference and a successful load.
        let files = AgentPluginFiles::from_iter([
            (
                "plugin.json",
                r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo"}"#,
            ),
            ("../x", ""),
            ("/etc/passwd", ""),
            ("a\\b", ""),
            ("../dir/file", ""),
            (
                "skills/../evil/SKILL.md",
                "---\nname: evil\ndescription: Escapes.\n---\n",
            ),
            (
                "skills/a\\b/SKILL.md",
                "---\nname: ab\ndescription: Escapes.\n---\n",
            ),
            (
                "skills/good/SKILL.md",
                "---\nname: good\ndescription: Good skill.\n---\n",
            ),
            (
                "mcp.json",
                r#"{
          "$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers":{
            "parent":{"type":"stdio","command":"./../x"},
            "absolute":{"type":"stdio","command":".//etc/passwd"},
            "backslash":{"type":"stdio","command":"./a\\b"},
            "parent_cwd":{"type":"stdio","command":"node","cwd":"./../dir"},
            "root_cwd":{"type":"stdio","command":"node","cwd":"${PLUGIN_ROOT}/../dir"}
          }
        }"#,
            ),
        ]);

        let report = load_agent_plugin_from_files(&files, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert_eq!(
            plugin
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["good"]
        );
        assert!(plugin.mcp_servers.is_empty(), "{:?}", plugin.mcp_servers);
        let mut found = report
            .diagnostics
            .iter()
            .map(|d| (d.code, d.path.as_str()))
            .collect::<Vec<_>>();
        found.sort_unstable();
        assert_eq!(
            found,
            vec![
                (
                    "mcp_command_invalid",
                    "mcp.json/mcpServers/absolute/command"
                ),
                (
                    "mcp_command_invalid",
                    "mcp.json/mcpServers/backslash/command"
                ),
                ("mcp_command_invalid", "mcp.json/mcpServers/parent/command"),
                ("mcp_cwd_invalid", "mcp.json/mcpServers/parent_cwd/cwd"),
                ("mcp_cwd_invalid", "mcp.json/mcpServers/root_cwd/cwd"),
                ("skill_directory_invalid", "skills/.."),
                ("skill_directory_invalid", "skills/a\\b"),
            ]
        );
    }

    #[test]
    fn missing_manifest_is_the_same_fatal_diagnostic_in_memory_and_on_disk() {
        let root = root("missing-manifest");
        write(
            root.join("skills/good/SKILL.md").as_path(),
            "---\nname: good\ndescription: Good skill.\n---\n",
        );

        let disk = load_agent_plugin(&root, &AgentPluginLoadOptions::default());
        let memory =
            load_agent_plugin_from_files(&files_under(&root), &AgentPluginLoadOptions::default());

        for report in [&disk, &memory] {
            assert!(report.plugin.is_none());
            assert_eq!(codes(&report.diagnostics), vec!["manifest_path_invalid"]);
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn in_memory_paths_ignore_empty_and_current_segments() {
        let mut files = AgentPluginFiles::with_virtual_root("/pkg/./demo");
        files.insert(
            "./plugin.json",
            r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo"}"#,
        );
        files.insert(
            "skills//good/./SKILL.md",
            "---\nname: good\ndescription: Good skill.\n---\n",
        );

        let report = load_agent_plugin_from_files(&files, &AgentPluginLoadOptions::default());
        let plugin = report.plugin.unwrap();

        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(plugin.root, Path::new("/pkg/demo"));
        assert_eq!(plugin.skills[0].root, Path::new("/pkg/demo/skills/good"));
    }
}
