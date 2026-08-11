//! Publisher for portable Agent Plugin directories.

#[cfg(feature = "agent-plugins")]
pub mod loader;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::skill::{self, CommandInfo, SkillFile};

/// Options that identify a portable Agent Plugin package.
#[derive(Debug, Clone)]
pub struct AgentPluginPublisherOptions {
    /// Standard root plugin manifest metadata.
    pub manifest: AgentPluginManifestMetadata,
    /// Prompt artifact publication options.
    pub skills: AgentPluginSkillOptions,
    /// Optional tool bindings published with the plugin.
    pub tools: AgentPluginToolBindingOptions,
    /// Optional executable Tool Runtime bundled under `bin/`.
    pub tool_runtime: Option<AgentPluginToolRuntimeOptions>,
    /// Filesystem publication policy.
    pub policy: AgentPluginPublicationPolicy,
}

/// Standard root plugin manifest metadata.
#[derive(Debug, Clone)]
pub struct AgentPluginManifestMetadata {
    /// Stable Agent Plugin identifier.
    pub name: String,
    /// Optional semantic plugin version.
    pub version: Option<String>,
    /// Optional human-readable plugin description.
    pub description: Option<String>,
    /// Optional author metadata.
    pub author: Option<AgentPluginAuthor>,
    /// Optional plugin homepage URL.
    pub homepage: Option<String>,
    /// Optional source repository URL.
    pub repository: Option<String>,
    /// Optional license identifier or license reference.
    pub license: Option<String>,
    /// Optional search and discovery keywords.
    pub keywords: Vec<String>,
    /// Optional client extension metadata keyed by reverse-domain namespace.
    pub extensions: BTreeMap<String, Value>,
}

/// Standard Agent Plugin author metadata.
#[derive(Debug, Clone, Default)]
pub struct AgentPluginAuthor {
    /// Optional author or organization name.
    pub name: Option<String>,
    /// Optional author email address.
    pub email: Option<String>,
    /// Optional author URL.
    pub url: Option<String>,
}

/// Prompt artifact publication options.
#[derive(Debug, Clone)]
pub struct AgentPluginSkillOptions {
    /// CLI name shown in generated skill prompt artifacts.
    pub cli_name: String,
    /// Depth used when splitting commands into skill folders.
    pub depth: usize,
    /// Already-authored Agent Skill directories to copy into `skills/`.
    pub additional_skills: Vec<AgentPluginSkillDirectory>,
}

/// One already-authored Agent Skill directory copied into the plugin.
#[derive(Debug, Clone)]
pub struct AgentPluginSkillDirectory {
    /// Source directory containing a `SKILL.md` file and optional resources.
    pub source: PathBuf,
}

/// Optional tool bindings published with the plugin.
#[derive(Debug, Clone)]
pub struct AgentPluginToolBindingOptions {
    /// Optional legacy single bundled MCP stdio server binding.
    pub mcp_server: Option<AgentPluginMcpServer>,
    /// Named MCP server bindings written under `mcpServers`.
    pub mcp_servers: BTreeMap<String, AgentPluginMcpServerConfig>,
}

/// One executable Tool Runtime bundled with an Agent Plugin.
#[derive(Debug, Clone)]
pub struct AgentPluginToolRuntimeOptions {
    /// Source executable copied into the plugin package.
    pub source: PathBuf,
    /// Command name published for interactive shell use.
    pub shell_command: String,
    /// Target operating system for the executable.
    pub target_os: String,
    /// Target architecture for the executable.
    pub target_arch: String,
}

/// Filesystem publication policy.
#[derive(Debug, Clone)]
pub struct AgentPluginPublicationPolicy {
    /// Whether to replace existing owned plugin artifacts.
    pub overwrite: bool,
}

/// Bundled MCP stdio server binding for an Agent Plugin.
#[derive(Debug, Clone)]
pub struct AgentPluginMcpServer {
    /// Executable command used for the bundled MCP stdio server.
    pub command: String,
    /// Arguments passed to the MCP stdio command.
    pub args: Vec<String>,
}

/// Closed Agent Plugins MCP server transport variants.
#[derive(Debug, Clone)]
pub enum AgentPluginMcpServerConfig {
    /// MCP server launched with the stdio transport.
    Stdio(AgentPluginStdioMcpServer),
    /// MCP server reached through the Streamable HTTP transport.
    StreamableHttp(AgentPluginRemoteMcpServer),
    /// MCP server reached through the legacy HTTP+SSE transport.
    Sse(AgentPluginRemoteMcpServer),
}

/// Stdio MCP server configuration.
#[derive(Debug, Clone)]
pub struct AgentPluginStdioMcpServer {
    /// Executable command token.
    pub command: String,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Environment variables overlaid before `PLUGIN_ROOT` and `PLUGIN_DATA`.
    pub env: BTreeMap<String, String>,
    /// Optional subprocess working directory.
    pub cwd: Option<String>,
}

/// Remote MCP server configuration.
#[derive(Debug, Clone)]
pub struct AgentPluginRemoteMcpServer {
    /// Absolute HTTP or HTTPS MCP endpoint URL.
    pub url: String,
    /// Fixed non-secret HTTP headers sent to the configured origin.
    pub headers: BTreeMap<String, String>,
}

/// Files written by an Agent Plugin publish operation.
#[derive(Debug, Clone)]
pub struct PublishedAgentPlugin {
    /// Plugin root directory.
    pub root: PathBuf,
    /// Files written relative to the plugin root.
    pub files: Vec<PathBuf>,
}

/// Error returned while publishing an Agent Plugin directory.
#[derive(Debug, thiserror::Error)]
pub enum AgentPluginPublisherError {
    /// The plugin name does not match Agent Plugins 1.0 rules.
    #[error("invalid plugin name: {name}")]
    InvalidPluginName {
        /// Rejected plugin name.
        name: String,
    },
    /// A skill directory and frontmatter name do not match Agent Skills rules.
    #[error("invalid skill name: {name}")]
    InvalidSkillName {
        /// Rejected skill name.
        name: String,
    },
    /// The MCP stdio command is not a bare token or plugin-relative path.
    #[error("invalid MCP stdio command: {command}")]
    InvalidMcpCommand {
        /// Rejected stdio command.
        command: String,
    },
    /// A named MCP server binding violates Agent Plugins rules.
    #[error("invalid MCP server {name}: {reason}")]
    InvalidMcpServer {
        /// Server name.
        name: String,
        /// Validation failure.
        reason: String,
    },
    /// A manifest extension namespace is not reverse-domain shaped.
    #[error("invalid extension namespace: {namespace}")]
    InvalidExtensionNamespace {
        /// Rejected namespace.
        namespace: String,
    },
    /// A generated skill description is missing or too long.
    #[error("invalid skill description for {name}: length {len}")]
    InvalidSkillDescription {
        /// Skill name.
        name: String,
        /// Description length in characters.
        len: usize,
    },
    /// An authored skill directory cannot be copied safely.
    #[error("invalid skill directory {path}: {reason}")]
    InvalidSkillDirectory {
        /// Rejected source path.
        path: PathBuf,
        /// Validation failure.
        reason: String,
    },
    /// A bundled Tool Runtime cannot be published safely.
    #[error("invalid Tool Runtime {path}: {reason}")]
    InvalidToolRuntime {
        /// Rejected source path.
        path: PathBuf,
        /// Validation failure.
        reason: String,
    },
    /// The output directory already contains owned plugin artifacts.
    #[error("output path already exists: {path}")]
    OutputExists {
        /// Existing owned artifact path.
        path: PathBuf,
    },
    /// A file operation failed.
    #[error("cannot write {path}: {source}")]
    Io {
        /// Path being written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A JSON serialization operation failed.
    #[error("cannot serialize {artifact}: {source}")]
    Json {
        /// Artifact being serialized.
        artifact: &'static str,
        /// Underlying JSON error.
        source: serde_json::Error,
    },
}

/// Publishes a portable Agent Plugin directory.
pub fn publish(
    root: &Path,
    commands: &[CommandInfo],
    groups: &BTreeMap<String, String>,
    options: &AgentPluginPublisherOptions,
) -> Result<PublishedAgentPlugin, AgentPluginPublisherError> {
    validate_plugin_name(&options.manifest.name)?;
    validate_manifest_metadata(&options.manifest)?;
    let mcp_servers = configured_mcp_servers(options)?;
    let tool_runtime = options
        .tool_runtime
        .as_ref()
        .map(|runtime| validate_tool_runtime(runtime, &options.manifest))
        .transpose()?;

    let skills = skill::split(
        &options.skills.cli_name,
        commands,
        options.skills.depth,
        groups,
    );
    let skills = skills
        .into_iter()
        .map(|file| portable_skill_file(&options.skills.cli_name, &options.manifest.name, file))
        .collect::<Result<Vec<_>, _>>()?;
    let authored_skills = options
        .skills
        .additional_skills
        .iter()
        .map(collect_authored_skill)
        .collect::<Result<Vec<_>, _>>()?;
    validate_unique_skill_names(&skills, &authored_skills)?;

    prepare_output(root, options.policy.overwrite)?;

    let mut files = Vec::new();
    let plugin_json = plugin_manifest(options);
    write_json(&root.join("plugin.json"), &plugin_json, "plugin manifest")?;
    files.push(PathBuf::from("plugin.json"));

    for file in skills {
        let path = PathBuf::from("skills").join(&file.dir).join("SKILL.md");
        write_text(&root.join(&path), &(file.content + "\n"))?;
        files.push(path);
    }

    for skill in authored_skills {
        for file in skill.files {
            let path = PathBuf::from("skills")
                .join(&skill.name)
                .join(&file.relative);
            copy_file(&file.source, &root.join(&path))?;
            files.push(path);
        }
    }

    if let Some((runtime, path)) = tool_runtime {
        copy_file(&runtime.source, &root.join(&path))?;
        files.push(path);
    }

    if !mcp_servers.is_empty() {
        write_json(
            &root.join("mcp.json"),
            &mcp_json(&mcp_servers),
            "mcp server map",
        )?;
        files.push(PathBuf::from("mcp.json"));
    }

    Ok(PublishedAgentPlugin {
        root: root.to_path_buf(),
        files,
    })
}

fn plugin_manifest(options: &AgentPluginPublisherOptions) -> Value {
    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "$schema".to_string(),
        Value::String("https://agent-plugins.org/schemas/1.0.0/plugin.schema.json".to_string()),
    );
    manifest.insert(
        "name".to_string(),
        Value::String(options.manifest.name.clone()),
    );
    if let Some(version) = &options.manifest.version {
        manifest.insert("version".to_string(), Value::String(version.clone()));
    }
    if let Some(description) = &options.manifest.description {
        manifest.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    if let Some(author) = &options.manifest.author {
        let mut value = serde_json::Map::new();
        if let Some(name) = &author.name {
            value.insert("name".to_string(), Value::String(name.clone()));
        }
        if let Some(email) = &author.email {
            value.insert("email".to_string(), Value::String(email.clone()));
        }
        if let Some(url) = &author.url {
            value.insert("url".to_string(), Value::String(url.clone()));
        }
        manifest.insert("author".to_string(), Value::Object(value));
    }
    if let Some(homepage) = &options.manifest.homepage {
        manifest.insert("homepage".to_string(), Value::String(homepage.clone()));
    }
    if let Some(repository) = &options.manifest.repository {
        manifest.insert("repository".to_string(), Value::String(repository.clone()));
    }
    if let Some(license) = &options.manifest.license {
        manifest.insert("license".to_string(), Value::String(license.clone()));
    }
    if !options.manifest.keywords.is_empty() {
        manifest.insert(
            "keywords".to_string(),
            Value::Array(
                options
                    .manifest
                    .keywords
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    let mut extensions = options.manifest.extensions.clone();
    if let Some(runtime) = &options.tool_runtime {
        let executable = tool_runtime_file_name(runtime);
        extensions.insert(
            "io.github.douglance.incurs".to_string(),
            json!({
                "toolRuntime": {
                    "arch": runtime.target_arch,
                    "os": runtime.target_os,
                    "path": format!("./bin/{executable}"),
                    "shellCommand": runtime.shell_command,
                }
            }),
        );
    }
    if !extensions.is_empty() {
        manifest.insert(
            "extensions".to_string(),
            Value::Object(
                extensions
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }
    canonicalize(Value::Object(manifest))
}

fn mcp_json(servers: &BTreeMap<String, AgentPluginMcpServerConfig>) -> Value {
    let mcp_servers = servers
        .iter()
        .map(|(name, server)| (name.clone(), mcp_server_json(server)))
        .collect::<serde_json::Map<_, _>>();
    canonicalize(json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers": mcp_servers
    }))
}

fn mcp_server_json(server: &AgentPluginMcpServerConfig) -> Value {
    match server {
        AgentPluginMcpServerConfig::Stdio(server) => {
            let mut value = serde_json::Map::from_iter([
                ("type".to_string(), Value::String("stdio".to_string())),
                ("command".to_string(), Value::String(server.command.clone())),
            ]);
            if !server.args.is_empty() {
                value.insert(
                    "args".to_string(),
                    Value::Array(server.args.iter().cloned().map(Value::String).collect()),
                );
            }
            if !server.env.is_empty() {
                value.insert(
                    "env".to_string(),
                    Value::Object(
                        server
                            .env
                            .iter()
                            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                            .collect(),
                    ),
                );
            }
            if let Some(cwd) = &server.cwd {
                value.insert("cwd".to_string(), Value::String(cwd.clone()));
            }
            Value::Object(value)
        }
        AgentPluginMcpServerConfig::StreamableHttp(server) => {
            remote_mcp_json("streamable-http", server)
        }
        AgentPluginMcpServerConfig::Sse(server) => remote_mcp_json("sse", server),
    }
}

fn remote_mcp_json(kind: &str, server: &AgentPluginRemoteMcpServer) -> Value {
    let mut value = serde_json::Map::from_iter([
        ("type".to_string(), Value::String(kind.to_string())),
        ("url".to_string(), Value::String(server.url.clone())),
    ]);
    if !server.headers.is_empty() {
        value.insert(
            "headers".to_string(),
            Value::Object(
                server
                    .headers
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
    }
    Value::Object(value)
}

fn portable_skill_file(
    cli: &str,
    plugin_name: &str,
    file: SkillFile,
) -> Result<SkillFile, AgentPluginPublisherError> {
    let name = if file.dir.is_empty() {
        root_skill_name(plugin_name)
    } else {
        file.dir.clone()
    };
    validate_skill_name(&name)?;

    let (frontmatter, body) = split_frontmatter(&file.content);
    let description = frontmatter
        .iter()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or("Run this plugin workflow.");
    validate_skill_description(&name, description)?;
    let command = frontmatter
        .iter()
        .find_map(|line| line.strip_prefix("command: "))
        .unwrap_or(cli);
    let requires = frontmatter
        .iter()
        .find_map(|line| line.strip_prefix("requires_bin: "))
        .unwrap_or(cli);
    let content = [
        "---".to_string(),
        format!("name: {name}"),
        format!("description: {}", yaml_string(description)?),
        "metadata:".to_string(),
        format!("  incurs.command: {}", yaml_string(command)?),
        format!("  incurs.requires_bin: {}", yaml_string(requires)?),
        "---".to_string(),
        String::new(),
        body.to_string(),
    ]
    .join("\n");

    Ok(SkillFile { dir: name, content })
}

#[derive(Debug)]
struct AuthoredSkill {
    name: String,
    files: Vec<AuthoredSkillFile>,
}

#[derive(Debug)]
struct AuthoredSkillFile {
    source: PathBuf,
    relative: PathBuf,
}

fn validate_manifest_metadata(
    manifest: &AgentPluginManifestMetadata,
) -> Result<(), AgentPluginPublisherError> {
    for (namespace, value) in &manifest.extensions {
        if !is_extension_namespace(namespace) {
            return Err(AgentPluginPublisherError::InvalidExtensionNamespace {
                namespace: namespace.clone(),
            });
        }
        if !value.is_object() {
            return Err(AgentPluginPublisherError::InvalidExtensionNamespace {
                namespace: namespace.clone(),
            });
        }
    }
    Ok(())
}

fn configured_mcp_servers(
    options: &AgentPluginPublisherOptions,
) -> Result<BTreeMap<String, AgentPluginMcpServerConfig>, AgentPluginPublisherError> {
    let mut servers = options.tools.mcp_servers.clone();
    if let Some(server) = &options.tools.mcp_server {
        if servers.contains_key(&options.manifest.name) {
            return Err(AgentPluginPublisherError::InvalidMcpServer {
                name: options.manifest.name.clone(),
                reason: "legacy mcp_server duplicates mcp_servers entry".to_string(),
            });
        }
        validate_mcp_command(&server.command)?;
        servers.insert(
            options.manifest.name.clone(),
            AgentPluginMcpServerConfig::Stdio(AgentPluginStdioMcpServer {
                command: server.command.clone(),
                args: server.args.clone(),
                env: BTreeMap::new(),
                cwd: None,
            }),
        );
    }
    for (name, server) in &servers {
        validate_mcp_server_name(name)?;
        validate_mcp_server(name, server)?;
    }
    Ok(servers)
}

fn validate_mcp_server_name(name: &str) -> Result<(), AgentPluginPublisherError> {
    if !name.is_empty() && !name.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(AgentPluginPublisherError::InvalidMcpServer {
            name: name.to_string(),
            reason: "server name must be non-empty and contain no whitespace".to_string(),
        })
    }
}

fn validate_mcp_server(
    name: &str,
    server: &AgentPluginMcpServerConfig,
) -> Result<(), AgentPluginPublisherError> {
    match server {
        AgentPluginMcpServerConfig::Stdio(server) => validate_stdio_mcp_server(name, server),
        AgentPluginMcpServerConfig::StreamableHttp(server)
        | AgentPluginMcpServerConfig::Sse(server) => validate_remote_mcp_server(name, server),
    }
}

fn validate_stdio_mcp_server(
    name: &str,
    server: &AgentPluginStdioMcpServer,
) -> Result<(), AgentPluginPublisherError> {
    validate_mcp_command(&server.command)?;
    for key in server.env.keys() {
        if key.eq_ignore_ascii_case("PLUGIN_ROOT") || key.eq_ignore_ascii_case("PLUGIN_DATA") {
            return invalid_mcp_server(name, "env must not set PLUGIN_ROOT or PLUGIN_DATA");
        }
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return invalid_mcp_server(name, "env keys must be non-empty variable names");
        }
    }
    if let Some(cwd) = &server.cwd {
        validate_mcp_cwd(name, cwd)?;
    }
    Ok(())
}

fn validate_remote_mcp_server(
    name: &str,
    server: &AgentPluginRemoteMcpServer,
) -> Result<(), AgentPluginPublisherError> {
    validate_remote_mcp_url(name, &server.url)?;
    let mut names = BTreeSet::new();
    for (header, value) in &server.headers {
        if !is_header_name(header) {
            return invalid_mcp_server(name, "header names must be valid HTTP field names");
        }
        if !is_header_value(value) {
            return invalid_mcp_server(name, "header values must not contain control characters");
        }
        if !names.insert(header.to_ascii_lowercase()) {
            return invalid_mcp_server(name, "header names must be unique case-insensitively");
        }
    }
    Ok(())
}

fn validate_remote_mcp_url(name: &str, url: &str) -> Result<(), AgentPluginPublisherError> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return invalid_mcp_server(name, "url must be absolute HTTP or HTTPS");
    };
    if scheme != "http" && scheme != "https" {
        return invalid_mcp_server(name, "url must use HTTP or HTTPS");
    }
    if rest.contains('#') {
        return invalid_mcp_server(name, "url must not contain a fragment");
    }
    let authority = rest
        .split(['/', '?'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentPluginPublisherError::InvalidMcpServer {
            name: name.to_string(),
            reason: "url must include a host".to_string(),
        })?;
    if authority.contains('@') {
        return invalid_mcp_server(name, "url must not contain user information");
    }
    let host = url_host(authority).ok_or_else(|| AgentPluginPublisherError::InvalidMcpServer {
        name: name.to_string(),
        reason: "url must include a host".to_string(),
    })?;
    if scheme == "http" && !is_loopback_host(host) {
        return invalid_mcp_server(name, "non-loopback MCP URLs must use HTTPS");
    }
    Ok(())
}

fn validate_mcp_cwd(name: &str, cwd: &str) -> Result<(), AgentPluginPublisherError> {
    if let Some(suffix) = cwd.strip_prefix("./") {
        if is_safe_relative_path(suffix) {
            return Ok(());
        }
    } else if cwd == "${PLUGIN_ROOT}" || cwd == "${PLUGIN_DATA}" {
        return Ok(());
    } else if let Some(suffix) = cwd.strip_prefix("${PLUGIN_ROOT}/") {
        if is_safe_relative_path(suffix) {
            return Ok(());
        }
    } else if let Some(suffix) = cwd.strip_prefix("${PLUGIN_DATA}/")
        && is_safe_relative_path(suffix)
    {
        return Ok(());
    }
    invalid_mcp_server(name, "cwd must remain within PLUGIN_ROOT or PLUGIN_DATA")
}

fn invalid_mcp_server<T>(
    name: &str,
    reason: impl Into<String>,
) -> Result<T, AgentPluginPublisherError> {
    Err(AgentPluginPublisherError::InvalidMcpServer {
        name: name.to_string(),
        reason: reason.into(),
    })
}

fn collect_authored_skill(
    skill: &AgentPluginSkillDirectory,
) -> Result<AuthoredSkill, AgentPluginPublisherError> {
    let source = fs::canonicalize(&skill.source).map_err(|source_error| {
        AgentPluginPublisherError::InvalidSkillDirectory {
            path: skill.source.clone(),
            reason: source_error.to_string(),
        }
    })?;
    if !source.is_dir() {
        return Err(AgentPluginPublisherError::InvalidSkillDirectory {
            path: skill.source.clone(),
            reason: "source must be a directory".to_string(),
        });
    }
    let skill_md = source.join("SKILL.md");
    if !skill_md.is_file() {
        return Err(AgentPluginPublisherError::InvalidSkillDirectory {
            path: skill.source.clone(),
            reason: "source must contain SKILL.md".to_string(),
        });
    }
    let content =
        fs::read_to_string(&skill_md).map_err(|source_error| AgentPluginPublisherError::Io {
            path: skill_md.clone(),
            source: source_error,
        })?;
    let (frontmatter, _) = split_frontmatter(&content);
    let name = frontmatter
        .iter()
        .find_map(|line| line.strip_prefix("name: "))
        .map(parse_yaml_scalar)
        .ok_or_else(|| AgentPluginPublisherError::InvalidSkillDirectory {
            path: skill.source.clone(),
            reason: "SKILL.md must declare name".to_string(),
        })?;
    validate_skill_name(&name)?;
    if source.file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
        return Err(AgentPluginPublisherError::InvalidSkillDirectory {
            path: skill.source.clone(),
            reason: "source directory name must match SKILL.md name".to_string(),
        });
    }
    let mut files = Vec::new();
    collect_authored_skill_files(&source, &source, &mut files)?;
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(AuthoredSkill { name, files })
}

fn collect_authored_skill_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<AuthoredSkillFile>,
) -> Result<(), AgentPluginPublisherError> {
    let mut entries = fs::read_dir(dir)
        .map_err(|source| AgentPluginPublisherError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| AgentPluginPublisherError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .map_err(|source| AgentPluginPublisherError::Io {
                path: path.clone(),
                source,
            })?
            .file_type()
            .is_symlink()
        {
            return Err(AgentPluginPublisherError::InvalidSkillDirectory {
                path,
                reason: "symlinks are not copied into portable skill directories".to_string(),
            });
        }
        let resolved = fs::canonicalize(&path).map_err(|source| AgentPluginPublisherError::Io {
            path: path.clone(),
            source,
        })?;
        if !resolved.starts_with(root) {
            return Err(AgentPluginPublisherError::InvalidSkillDirectory {
                path,
                reason: "source path resolves outside the skill directory".to_string(),
            });
        }
        if resolved.is_dir() {
            collect_authored_skill_files(root, &resolved, files)?;
        } else if resolved.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            files.push(AuthoredSkillFile {
                source: resolved,
                relative,
            });
        }
    }
    Ok(())
}

fn validate_unique_skill_names(
    generated: &[SkillFile],
    authored: &[AuthoredSkill],
) -> Result<(), AgentPluginPublisherError> {
    let mut names = BTreeSet::new();
    for file in generated {
        if !names.insert(file.dir.clone()) {
            return Err(AgentPluginPublisherError::InvalidSkillName {
                name: file.dir.clone(),
            });
        }
    }
    for skill in authored {
        if !names.insert(skill.name.clone()) {
            return Err(AgentPluginPublisherError::InvalidSkillName {
                name: skill.name.clone(),
            });
        }
    }
    Ok(())
}

fn root_skill_name(plugin_name: &str) -> String {
    let mut name = String::with_capacity(plugin_name.len());
    let mut last_was_dash = false;
    for byte in plugin_name.bytes() {
        if byte == b'.' || byte == b'-' {
            if !last_was_dash && !name.is_empty() {
                name.push('-');
                last_was_dash = true;
            }
        } else {
            name.push(byte as char);
            last_was_dash = false;
        }
    }
    name.trim_matches('-').to_string()
}

fn split_frontmatter(content: &str) -> (Vec<&str>, &str) {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return (Vec::new(), content);
    }

    let mut frontmatter = Vec::new();
    let mut consumed = "---\n".len();
    for line in content["---\n".len()..].lines() {
        consumed += line.len() + 1;
        if line == "---" {
            let body = content
                .get(consumed..)
                .unwrap_or("")
                .trim_start_matches('\n');
            return (frontmatter, body);
        }
        frontmatter.push(line);
    }

    (frontmatter, "")
}

fn validate_plugin_name(name: &str) -> Result<(), AgentPluginPublisherError> {
    if is_plugin_name(name) {
        Ok(())
    } else {
        Err(AgentPluginPublisherError::InvalidPluginName {
            name: name.to_string(),
        })
    }
}

fn validate_skill_name(name: &str) -> Result<(), AgentPluginPublisherError> {
    if is_skill_name(name) {
        Ok(())
    } else {
        Err(AgentPluginPublisherError::InvalidSkillName {
            name: name.to_string(),
        })
    }
}

fn validate_mcp_command(command: &str) -> Result<(), AgentPluginPublisherError> {
    if is_mcp_command(command) {
        Ok(())
    } else {
        Err(AgentPluginPublisherError::InvalidMcpCommand {
            command: command.to_string(),
        })
    }
}

fn validate_skill_description(
    name: &str,
    description: &str,
) -> Result<(), AgentPluginPublisherError> {
    let len = description.chars().count();
    if !description.is_empty() && len <= 1024 {
        Ok(())
    } else {
        Err(AgentPluginPublisherError::InvalidSkillDescription {
            name: name.to_string(),
            len,
        })
    }
}

fn is_plugin_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if name.contains("..") || name.contains("--") {
        return false;
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return false;
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return false;
    }
    bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'.'
    })
}

fn is_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.contains("--") {
        return false;
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return false;
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn is_extension_namespace(namespace: &str) -> bool {
    let parts = namespace.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && part
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn is_mcp_command(command: &str) -> bool {
    if command.is_empty() || command.chars().any(char::is_whitespace) {
        return false;
    }
    if command.starts_with("./") {
        return is_plugin_relative_path(command);
    }
    !command.contains('/') && !command.contains('\\') && command != "." && command != ".."
}

fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\0'))
}

fn is_plugin_relative_path(command: &str) -> bool {
    if command.contains('\\') || command.contains("..") || command.ends_with('/') {
        return false;
    }
    command[2..].split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    })
}

fn url_host(authority: &str) -> Option<&str> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, _) = rest.split_once(']')?;
        return (!host.is_empty()).then_some(host);
    }
    let host = authority.split(':').next().unwrap_or(authority);
    (!host.is_empty()).then_some(host)
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        return true;
    }
    let parts = host.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "127"
        && parts
            .iter()
            .all(|part| part.parse::<u8>().is_ok_and(|_| true))
}

fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn is_header_value(value: &str) -> bool {
    !value
        .bytes()
        .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
}

fn parse_yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str(value).unwrap_or_else(|_| value.trim_matches('"').to_string())
    } else {
        value.to_string()
    }
}

fn yaml_string(value: &str) -> Result<String, AgentPluginPublisherError> {
    serde_json::to_string(value).map_err(|source| AgentPluginPublisherError::Json {
        artifact: "skill frontmatter",
        source,
    })
}

fn prepare_output(root: &Path, overwrite: bool) -> Result<(), AgentPluginPublisherError> {
    let owned = [
        root.join("plugin.json"),
        root.join("mcp.json"),
        root.join("skills"),
        root.join("bin"),
    ];
    if !overwrite {
        if let Some(path) = owned.iter().find(|path| path.exists()) {
            return Err(AgentPluginPublisherError::OutputExists {
                path: path.to_path_buf(),
            });
        }
        return Ok(());
    }
    for path in [root.join("skills"), root.join("bin")] {
        if path.exists() {
            fs::remove_dir_all(&path)
                .map_err(|source| AgentPluginPublisherError::Io { path, source })?;
        }
    }
    for path in [root.join("plugin.json"), root.join("mcp.json")] {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|source| AgentPluginPublisherError::Io { path, source })?;
        }
    }
    Ok(())
}

fn validate_tool_runtime<'a>(
    runtime: &'a AgentPluginToolRuntimeOptions,
    manifest: &AgentPluginManifestMetadata,
) -> Result<(&'a AgentPluginToolRuntimeOptions, PathBuf), AgentPluginPublisherError> {
    let invalid = |reason: &str| AgentPluginPublisherError::InvalidToolRuntime {
        path: runtime.source.clone(),
        reason: reason.to_string(),
    };
    if manifest
        .extensions
        .contains_key("io.github.douglance.incurs")
    {
        return Err(invalid(
            "io.github.douglance.incurs is reserved for generated installer metadata",
        ));
    }
    if !is_mcp_command(&runtime.shell_command)
        || runtime.shell_command.starts_with("./")
        || runtime.shell_command.ends_with(".exe")
    {
        return Err(invalid(
            "shell command must be one bare executable token without an .exe suffix",
        ));
    }
    if !matches!(runtime.target_os.as_str(), "linux" | "macos" | "windows") {
        return Err(invalid("target OS must be linux, macos, or windows"));
    }
    if !matches!(runtime.target_arch.as_str(), "x86_64" | "aarch64") {
        return Err(invalid("target architecture must be x86_64 or aarch64"));
    }
    let metadata =
        fs::symlink_metadata(&runtime.source).map_err(|source| AgentPluginPublisherError::Io {
            path: runtime.source.clone(),
            source,
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("source must be a regular file and not a symlink"));
    }
    #[cfg(unix)]
    if runtime.target_os != "windows" {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid("source is not executable"));
        }
    }
    Ok((
        runtime,
        PathBuf::from("bin").join(tool_runtime_file_name(runtime)),
    ))
}

fn tool_runtime_file_name(runtime: &AgentPluginToolRuntimeOptions) -> String {
    if runtime.target_os == "windows" {
        format!("{}.exe", runtime.shell_command)
    } else {
        runtime.shell_command.clone()
    }
}

fn write_json(
    path: &Path,
    value: &Value,
    artifact: &'static str,
) -> Result<(), AgentPluginPublisherError> {
    let contents = serde_json::to_string_pretty(value)
        .map_err(|source| AgentPluginPublisherError::Json { artifact, source })?
        + "\n";
    write_text(path, &contents)
}

fn write_text(path: &Path, contents: &str) -> Result<(), AgentPluginPublisherError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AgentPluginPublisherError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, contents).map_err(|source| AgentPluginPublisherError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), AgentPluginPublisherError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source| AgentPluginPublisherError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|source| AgentPluginPublisherError::Io {
            path: destination.to_path_buf(),
            source,
        })
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{CommandInfo, Example};
    use std::fs;

    fn command(name: &str) -> CommandInfo {
        CommandInfo {
            name: name.to_string(),
            description: Some(format!("Run {name}")),
            args_fields: Vec::new(),
            options_fields: Vec::new(),
            env_fields: Vec::new(),
            hint: None,
            examples: Vec::new(),
            output_schema: None,
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "incurs-agent-plugin-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn options(
        name: &str,
        cli_name: &str,
        depth: usize,
        mcp_server: Option<AgentPluginMcpServer>,
        overwrite: bool,
    ) -> AgentPluginPublisherOptions {
        AgentPluginPublisherOptions {
            manifest: AgentPluginManifestMetadata {
                name: name.to_string(),
                version: Some("1.2.3".to_string()),
                description: Some("Demo tools for repeatable work.".to_string()),
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: Vec::new(),
                extensions: BTreeMap::new(),
            },
            skills: AgentPluginSkillOptions {
                cli_name: cli_name.to_string(),
                depth,
                additional_skills: Vec::new(),
            },
            tools: AgentPluginToolBindingOptions {
                mcp_server,
                mcp_servers: BTreeMap::new(),
            },
            tool_runtime: None,
            policy: AgentPluginPublicationPolicy { overwrite },
        }
    }

    #[test]
    fn publish_writes_deterministic_plugin_skills_and_mcp_files() {
        let root = temp_root("publish");
        let options = options(
            "demo-tools",
            "demo-tools",
            1,
            Some(AgentPluginMcpServer {
                command: "demo-tools".to_string(),
                args: vec!["--mcp".to_string()],
            }),
            false,
        );

        let result = publish(
            &root,
            &[command("project list"), command("project create")],
            &BTreeMap::new(),
            &options,
        )
        .unwrap();

        assert_eq!(
            result.files,
            vec![
                PathBuf::from("plugin.json"),
                PathBuf::from("skills/project/SKILL.md"),
                PathBuf::from("mcp.json"),
            ]
        );
        assert_eq!(
            fs::read_to_string(root.join("plugin.json")).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json\",\n  \"description\": \"Demo tools for repeatable work.\",\n  \"name\": \"demo-tools\",\n  \"version\": \"1.2.3\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("mcp.json")).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json\",\n  \"mcpServers\": {\n    \"demo-tools\": {\n      \"args\": [\n        \"--mcp\"\n      ],\n      \"command\": \"demo-tools\",\n      \"type\": \"stdio\"\n    }\n  }\n}\n"
        );

        let skill = fs::read_to_string(root.join("skills/project/SKILL.md")).unwrap();
        assert_eq!(
            skill,
            "---\nname: project\ndescription: \"Run project list, Run project create. Run `demo-tools project --help` for usage details.\"\nmetadata:\n  incurs.command: \"demo-tools project\"\n  incurs.requires_bin: \"demo-tools\"\n---\n\n# demo-tools project list\n\nRun project list\n\n---\n\n# demo-tools project create\n\nRun project create\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_bundles_tool_runtime_and_declares_installer_metadata() {
        let root = temp_root("runtime");
        let source = temp_root("runtime-source").join("demo-source");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "runtime").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut options = options(
            "demo-tools",
            "demo-tools",
            0,
            Some(AgentPluginMcpServer {
                command: "./bin/demo-tools".to_string(),
                args: vec!["--mcp".to_string()],
            }),
            false,
        );
        options.tool_runtime = Some(AgentPluginToolRuntimeOptions {
            source: source.clone(),
            shell_command: "demo-tools".to_string(),
            target_os: "linux".to_string(),
            target_arch: "x86_64".to_string(),
        });

        let result = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap();

        assert_eq!(fs::read(root.join("bin/demo-tools")).unwrap(), b"runtime");
        assert!(result.files.contains(&PathBuf::from("bin/demo-tools")));
        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(root.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["extensions"]["io.github.douglance.incurs"]["toolRuntime"],
            serde_json::json!({
                "arch": "x86_64",
                "os": "linux",
                "path": "./bin/demo-tools",
                "shellCommand": "demo-tools"
            })
        );

        let _ = fs::remove_dir_all(source.parent().unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn publish_rejects_a_non_executable_tool_runtime() {
        let root = temp_root("runtime-not-executable");
        let source = temp_root("runtime-not-executable-source").join("demo-source");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "runtime").unwrap();
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options.tool_runtime = Some(AgentPluginToolRuntimeOptions {
            source: source.clone(),
            shell_command: "demo-tools".to_string(),
            target_os: "linux".to_string(),
            target_arch: "x86_64".to_string(),
        });

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidToolRuntime { .. }
        ));
        assert!(!root.exists());

        let _ = fs::remove_dir_all(source.parent().unwrap());
    }

    #[test]
    fn publish_writes_full_manifest_metadata_and_multiple_mcp_transports() {
        let root = temp_root("full");
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options.manifest.author = Some(AgentPluginAuthor {
            name: Some("Demo Org".to_string()),
            email: Some("dev@example.com".to_string()),
            url: Some("https://example.com".to_string()),
        });
        options.manifest.homepage = Some("https://example.com/demo".to_string());
        options.manifest.repository = Some("https://github.com/example/demo".to_string());
        options.manifest.license = Some("MIT".to_string());
        options.manifest.keywords = vec!["demo".to_string(), "tools".to_string()];
        options.manifest.extensions.insert(
            "com.example.client".to_string(),
            serde_json::json!({ "enabled": true }),
        );
        options.tools.mcp_servers.insert(
            "local".to_string(),
            AgentPluginMcpServerConfig::Stdio(AgentPluginStdioMcpServer {
                command: "./bin/demo".to_string(),
                args: vec![
                    "--mcp".to_string(),
                    "${PLUGIN_ROOT}/config.json".to_string(),
                ],
                env: BTreeMap::from([("DATA_DIR".to_string(), "${PLUGIN_DATA}/demo".to_string())]),
                cwd: Some("${PLUGIN_ROOT}".to_string()),
            }),
        );
        options.tools.mcp_servers.insert(
            "remote".to_string(),
            AgentPluginMcpServerConfig::StreamableHttp(AgentPluginRemoteMcpServer {
                url: "https://api.example.com/mcp".to_string(),
                headers: BTreeMap::from([("X-Tenant".to_string(), "public".to_string())]),
            }),
        );
        options.tools.mcp_servers.insert(
            "legacy".to_string(),
            AgentPluginMcpServerConfig::Sse(AgentPluginRemoteMcpServer {
                url: "http://localhost:8080/sse".to_string(),
                headers: BTreeMap::new(),
            }),
        );

        publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("plugin.json")).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json\",\n  \"author\": {\n    \"email\": \"dev@example.com\",\n    \"name\": \"Demo Org\",\n    \"url\": \"https://example.com\"\n  },\n  \"description\": \"Demo tools for repeatable work.\",\n  \"extensions\": {\n    \"com.example.client\": {\n      \"enabled\": true\n    }\n  },\n  \"homepage\": \"https://example.com/demo\",\n  \"keywords\": [\n    \"demo\",\n    \"tools\"\n  ],\n  \"license\": \"MIT\",\n  \"name\": \"demo-tools\",\n  \"repository\": \"https://github.com/example/demo\",\n  \"version\": \"1.2.3\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("mcp.json")).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json\",\n  \"mcpServers\": {\n    \"legacy\": {\n      \"type\": \"sse\",\n      \"url\": \"http://localhost:8080/sse\"\n    },\n    \"local\": {\n      \"args\": [\n        \"--mcp\",\n        \"${PLUGIN_ROOT}/config.json\"\n      ],\n      \"command\": \"./bin/demo\",\n      \"cwd\": \"${PLUGIN_ROOT}\",\n      \"env\": {\n        \"DATA_DIR\": \"${PLUGIN_DATA}/demo\"\n      },\n      \"type\": \"stdio\"\n    },\n    \"remote\": {\n      \"headers\": {\n        \"X-Tenant\": \"public\"\n      },\n      \"type\": \"streamable-http\",\n      \"url\": \"https://api.example.com/mcp\"\n    }\n  }\n}\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_rejects_invalid_remote_mcp_security_boundary() {
        let root = temp_root("invalid-remote");
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options.tools.mcp_servers.insert(
            "remote".to_string(),
            AgentPluginMcpServerConfig::StreamableHttp(AgentPluginRemoteMcpServer {
                url: "http://api.example.com/mcp#frag".to_string(),
                headers: BTreeMap::from([
                    ("X-Tenant".to_string(), "public".to_string()),
                    ("x-tenant".to_string(), "other".to_string()),
                ]),
            }),
        );

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidMcpServer { .. }
        ));
        assert!(!root.exists());
    }

    #[test]
    fn publish_rejects_reserved_stdio_env_and_escaping_cwd() {
        let root = temp_root("invalid-stdio");
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options.tools.mcp_servers.insert(
            "local".to_string(),
            AgentPluginMcpServerConfig::Stdio(AgentPluginStdioMcpServer {
                command: "./bin/demo".to_string(),
                args: Vec::new(),
                env: BTreeMap::from([("PLUGIN_ROOT".to_string(), "/tmp".to_string())]),
                cwd: Some("${PLUGIN_DATA}/../escape".to_string()),
            }),
        );

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidMcpServer { .. }
        ));
        assert!(!root.exists());
    }

    #[test]
    fn publish_copies_authored_skill_directory_resources() {
        let root = temp_root("copy");
        let source = temp_root("source-skill").join("lint");
        fs::create_dir_all(source.join("references")).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: lint\ndescription: Run lint checks.\n---\n\nUse the bundled script.\n",
        )
        .unwrap();
        fs::write(source.join("references/checklist.md"), "checklist").unwrap();
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options
            .skills
            .additional_skills
            .push(AgentPluginSkillDirectory {
                source: source.clone(),
            });

        let result = publish(&root, &[], &BTreeMap::new(), &options).unwrap();

        assert!(
            result
                .files
                .contains(&PathBuf::from("skills/lint/references/checklist.md"))
        );
        assert_eq!(
            fs::read_to_string(root.join("skills/lint/references/checklist.md")).unwrap(),
            "checklist"
        );

        let _ = fs::remove_dir_all(source.parent().unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_rejects_invalid_plugin_name_before_writing() {
        let root = temp_root("invalid");
        let options = options("../bad", "bad", 0, None, false);

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidPluginName { .. }
        ));
        assert!(!root.exists());
    }

    #[test]
    fn publish_rejects_invalid_skill_name_before_writing() {
        let root = temp_root("invalid-skill");
        let options = options("demo.tools", "demo-tools", 1, None, false);

        let error = publish(
            &root,
            &[command("bad--group run")],
            &BTreeMap::new(),
            &options,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidSkillName { .. }
        ));
        assert!(!root.exists());
    }

    #[test]
    fn publish_rejects_invalid_mcp_command_before_writing() {
        let root = temp_root("invalid-mcp");
        let options = options(
            "demo-tools",
            "demo-tools",
            0,
            Some(AgentPluginMcpServer {
                command: "../demo".to_string(),
                args: vec!["--mcp".to_string()],
            }),
            false,
        );

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::InvalidMcpCommand { .. }
        ));
        assert!(!root.exists());
    }

    #[test]
    fn publish_depth_zero_maps_dotted_plugin_name_to_valid_skill_name() {
        let root = temp_root("dotted");
        let options = options("demo.tools", "demo-tools", 0, None, false);

        let result = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap();

        assert!(
            result
                .files
                .contains(&PathBuf::from("skills/demo-tools/SKILL.md"))
        );
        let skill = fs::read_to_string(root.join("skills/demo-tools/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: demo-tools\n"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_omits_unknown_optional_manifest_metadata() {
        let root = temp_root("optional-metadata");
        let mut options = options("demo-tools", "demo-tools", 0, None, false);
        options.manifest.version = None;
        options.manifest.description = None;

        publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("plugin.json")).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json\",\n  \"name\": \"demo-tools\"\n}\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_quotes_portable_skill_yaml_strings() {
        let root = temp_root("quote");
        let mut cmd = command("say");
        cmd.description = Some("Say \"hello\"".to_string());
        cmd.examples = vec![Example {
            command: "--message \"hello\"".to_string(),
            description: Some("Quoted example".to_string()),
        }];
        let options = options("demo-tools", "./bin/demo-tools", 0, None, false);

        publish(&root, &[cmd], &BTreeMap::new(), &options).unwrap();

        let skill = fs::read_to_string(root.join("skills/demo-tools/SKILL.md")).unwrap();
        assert!(skill.contains(
            "description: \"Say \\\"hello\\\". Run `./bin/demo-tools --help` for usage details.\""
        ));
        assert!(skill.contains("  incurs.command: \"./bin/demo-tools\""));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_validates_skill_description_length_by_character_count() {
        let root = temp_root("unicode-description");
        let mut cmd = command("say");
        cmd.description = Some("é".repeat(900));
        let options = options("demo-tools", "demo-tools", 0, None, false);

        publish(&root, &[cmd], &BTreeMap::new(), &options).unwrap();

        assert!(root.join("skills/demo-tools/SKILL.md").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_rejects_existing_owned_artifacts_without_overwrite() {
        let root = temp_root("exists");
        fs::create_dir_all(root.join("skills/stale")).unwrap();
        fs::write(root.join("plugin.json"), "stale").unwrap();
        fs::write(root.join("skills/stale/SKILL.md"), "stale").unwrap();
        let options = options("demo-tools", "demo-tools", 0, None, false);

        let error = publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap_err();

        assert!(matches!(
            error,
            AgentPluginPublisherError::OutputExists { .. }
        ));
        assert_eq!(
            fs::read_to_string(root.join("plugin.json")).unwrap(),
            "stale"
        );
        assert_eq!(
            fs::read_to_string(root.join("skills/stale/SKILL.md")).unwrap(),
            "stale"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_removes_stale_owned_artifacts_with_overwrite() {
        let root = temp_root("stale");
        fs::create_dir_all(root.join("skills/stale")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("skills/stale/SKILL.md"), "stale").unwrap();
        fs::write(root.join("bin/stale"), "stale").unwrap();
        fs::write(root.join("mcp.json"), "stale").unwrap();
        fs::write(root.join("keep.txt"), "keep").unwrap();
        let options = options("demo-tools", "demo-tools", 0, None, true);

        publish(&root, &[command("run")], &BTreeMap::new(), &options).unwrap();

        assert!(!root.join("mcp.json").exists());
        assert!(!root.join("bin").exists());
        assert!(!root.join("skills/stale/SKILL.md").exists());
        assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "keep");

        let _ = fs::remove_dir_all(&root);
    }
}
