//! The host configuration formats Composite reads, as data.
//!
//! Every host stores a map of name to server entry. They differ only in where
//! the file lives, what the map is called, and how the document is encoded, so
//! adding support for another agent is a table entry rather than a parser.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::DiscoveryError;

/// A host configuration format that can declare MCP servers.
///
/// The declaration order is also the precedence order used when one server is
/// declared by several hosts: the earliest variant supplies the display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpDiscoverySource {
    /// A portable `<project>/.mcp.json`.
    PortableProject,
    /// Claude Code's per-project map in `~/.claude.json`.
    ClaudeCodeProject,
    /// A workspace `<project>/.vscode/mcp.json`.
    VsCodeProject,
    /// A workspace `<project>/.cursor/mcp.json`.
    CursorProject,
    /// Claude Code's user-scope map in `~/.claude.json`.
    ClaudeCodeUser,
    /// Codex `~/.codex/config.toml`.
    Codex,
    /// Cursor `~/.cursor/mcp.json`.
    CursorUser,
    /// VS Code's user `mcp.json`.
    VsCodeUser,
    /// GitHub Copilot CLI `~/.copilot/mcp-config.json`.
    CopilotCli,
    /// Amp `~/.config/amp/settings.json`.
    Amp,
    /// Gemini CLI `~/.gemini/settings.json`.
    GeminiCli,
    /// Windsurf `~/.codeium/windsurf/mcp_config.json`.
    Windsurf,
    /// OpenCode `~/.config/opencode/opencode.json`.
    OpenCode,
}

impl McpDiscoverySource {
    /// Returns a stable lowercase token used in namespaces and diagnostics.
    pub fn slug(self) -> &'static str {
        match self {
            Self::PortableProject => "project",
            Self::ClaudeCodeProject => "claude_project",
            Self::VsCodeProject => "vscode_project",
            Self::CursorProject => "cursor_project",
            Self::ClaudeCodeUser => "claude",
            Self::Codex => "codex",
            Self::CursorUser => "cursor",
            Self::VsCodeUser => "vscode",
            Self::CopilotCli => "copilot",
            Self::Amp => "amp",
            Self::GeminiCli => "gemini",
            Self::Windsurf => "windsurf",
            Self::OpenCode => "opencode",
        }
    }
}

/// Whether a configuration file applies to a whole user or to one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ConfigScope {
    /// Applies to every project for this user.
    User,
    /// Applies only inside one project root.
    Project {
        /// Absolute project root.
        root: PathBuf,
    },
}

/// Provenance for exactly one parsed server entry.
///
/// Every field is safe to persist and to show a model: it names files and
/// document locations, never configured values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigSource {
    /// Which host format declared the entry.
    pub discovery: McpDiscoverySource,
    /// Whether the declaring file is user-global or project-scoped.
    pub scope: ConfigScope,
    /// Absolute path of the declaring file.
    pub path: PathBuf,
    /// JSON Pointer, or dotted TOML path, to the entry within that file.
    pub pointer: String,
    /// Directory that relative paths in this entry resolve against.
    pub base_dir: PathBuf,
}

/// How a configuration document is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigEncoding {
    /// Strict JSON.
    Json,
    /// JSON permitting comments and trailing commas.
    ///
    /// VS Code documents its configuration as JSONC, and real files use it.
    Jsonc,
    /// TOML.
    Toml,
}

/// Every host directory Composite reads, resolved once.
///
/// [`HostPaths::from_env`] is the only place in this crate that reads the
/// environment. Every parser takes a `&HostPaths` instead, which is what makes
/// the crate testable against a fixture tree: `std::env::set_var` is `unsafe` in
/// edition 2024 and races across the test harness's threads.
#[derive(Debug, Clone)]
pub struct HostPaths {
    /// The user's home directory.
    pub home: PathBuf,
    /// Base for XDG-style configuration.
    pub config_home: PathBuf,
    /// Codex's home, honouring `CODEX_HOME`.
    pub codex_home: PathBuf,
    /// Directory holding VS Code user configuration.
    pub vscode_user: PathBuf,
    /// Project root used for project-scoped files.
    pub project: Option<PathBuf>,
    /// Environment used to expand `${env:NAME}` placeholders.
    pub env: BTreeMap<String, String>,
}

impl HostPaths {
    /// Resolves every host directory from this process's environment.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::NoHome`] when no home directory can be found.
    pub fn from_env() -> Result<Self, DiscoveryError> {
        let home = dirs::home_dir().ok_or(DiscoveryError::NoHome)?;
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from);
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".codex"), PathBuf::from);
        Ok(Self {
            vscode_user: default_vscode_user(&home, &config_home),
            home,
            config_home,
            codex_home,
            project: std::env::current_dir().ok(),
            env: std::env::vars().collect(),
        })
    }

    /// Resolves every host directory, rooted at an application home when given.
    ///
    /// An application that isolates its own state — a test, a sandbox, a second
    /// instance — must not read the developer's real agent configuration. Without
    /// this, discovery resolved from `dirs::home_dir()` regardless, so a caller
    /// that had carefully moved its home still reached the real machine's servers.
    ///
    /// The rooted form still reads the process environment, so `${env:NAME}`
    /// placeholders expand the same way; only the directories move.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::NoHome`] when no root is given and no home
    /// directory can be found.
    pub fn from_env_rooted(root: Option<&Path>) -> Result<Self, DiscoveryError> {
        let Some(root) = root else {
            return Self::from_env();
        };
        let mut paths = Self::rooted(root);
        paths.project = std::env::current_dir().ok();
        paths.env = std::env::vars().collect();
        Ok(paths)
    }

    /// Builds a path set rooted at one directory, for tests and fixtures.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let home = root.as_ref().to_path_buf();
        let config_home = home.join(".config");
        Self {
            codex_home: home.join(".codex"),
            vscode_user: default_vscode_user(&home, &config_home),
            config_home,
            project: None,
            env: BTreeMap::new(),
            home,
        }
    }

    /// Sets the project root used for project-scoped configuration.
    #[must_use]
    pub fn with_project(mut self, project: impl AsRef<Path>) -> Self {
        self.project = Some(project.as_ref().to_path_buf());
        self
    }
}

/// Returns the platform's VS Code user configuration directory.
fn default_vscode_user(home: &Path, config_home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code/User")
    } else if cfg!(windows) {
        dirs::config_dir()
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Code/User")
    } else {
        config_home.join("Code/User")
    }
}

/// One file a host may declare MCP servers in.
#[derive(Debug, Clone)]
pub struct ConfigLocation {
    /// Which host format this file belongs to.
    pub discovery: McpDiscoverySource,
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Scope the file applies to.
    pub scope: ConfigScope,
    /// Keys to try, in order, when looking for the server map.
    ///
    /// More than one entry accommodates a format whose real files disagree with
    /// its documentation, such as a `.vscode/mcp.json` copied from Cursor.
    pub map_keys: &'static [&'static str],
    /// How the document is encoded.
    pub encoding: ConfigEncoding,
}

/// Returns every configuration file Composite will try to read.
///
/// Presence is not checked here; a missing file is simply skipped later. The
/// order is the precedence order in [`McpDiscoverySource`].
pub fn locations(paths: &HostPaths) -> Vec<ConfigLocation> {
    let mut out = Vec::new();

    if let Some(project) = &paths.project {
        let scope = ConfigScope::Project {
            root: project.clone(),
        };
        out.push(ConfigLocation {
            discovery: McpDiscoverySource::PortableProject,
            path: project.join(".mcp.json"),
            scope: scope.clone(),
            map_keys: &["mcpServers"],
            encoding: ConfigEncoding::Json,
        });
        out.push(ConfigLocation {
            discovery: McpDiscoverySource::VsCodeProject,
            path: project.join(".vscode/mcp.json"),
            scope: scope.clone(),
            map_keys: &["servers", "mcpServers"],
            encoding: ConfigEncoding::Jsonc,
        });
        out.push(ConfigLocation {
            discovery: McpDiscoverySource::CursorProject,
            path: project.join(".cursor/mcp.json"),
            scope,
            map_keys: &["mcpServers"],
            encoding: ConfigEncoding::Json,
        });
    }

    // `~/.claude.json` sits beside `~/.claude/`, not inside it, and is resolved
    // from the home directory with no override. `CLAUDE_CONFIG_DIR` governs the
    // directory, not this file.
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::ClaudeCodeUser,
        path: paths.home.join(".claude.json"),
        scope: ConfigScope::User,
        map_keys: &["mcpServers"],
        encoding: ConfigEncoding::Json,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::Codex,
        path: paths.codex_home.join("config.toml"),
        scope: ConfigScope::User,
        map_keys: &["mcp_servers"],
        encoding: ConfigEncoding::Toml,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::CursorUser,
        path: paths.home.join(".cursor/mcp.json"),
        scope: ConfigScope::User,
        map_keys: &["mcpServers"],
        encoding: ConfigEncoding::Json,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::VsCodeUser,
        path: paths.vscode_user.join("mcp.json"),
        scope: ConfigScope::User,
        map_keys: &["servers", "mcpServers"],
        encoding: ConfigEncoding::Jsonc,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::CopilotCli,
        path: paths.home.join(".copilot/mcp-config.json"),
        scope: ConfigScope::User,
        map_keys: &["mcpServers"],
        encoding: ConfigEncoding::Json,
    });
    // Amp keys its map with a literal dotted string in a flat object.
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::Amp,
        path: paths.config_home.join("amp/settings.json"),
        scope: ConfigScope::User,
        map_keys: &["amp.mcpServers"],
        encoding: ConfigEncoding::Jsonc,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::GeminiCli,
        path: paths.home.join(".gemini/settings.json"),
        scope: ConfigScope::User,
        map_keys: &["mcpServers"],
        encoding: ConfigEncoding::Jsonc,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::Windsurf,
        path: paths.home.join(".codeium/windsurf/mcp_config.json"),
        scope: ConfigScope::User,
        map_keys: &["mcpServers"],
        encoding: ConfigEncoding::Json,
    });
    out.push(ConfigLocation {
        discovery: McpDiscoverySource::OpenCode,
        path: paths.config_home.join("opencode/opencode.json"),
        scope: ConfigScope::User,
        map_keys: &["mcp"],
        encoding: ConfigEncoding::Jsonc,
    });
    out
}
