//! Agent configuration and skill installation for AI coding agents.
//!
//! Ported from `src/internal/agents.ts`. Defines 21 agent configurations and
//! provides install/remove/detect operations that manage skill files across
//! the canonical `.agents/skills/` directory and agent-specific locations.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration for a single AI coding agent.
#[derive(Debug, Clone)]
pub struct Agent {
    /// Display name (e.g. "Claude Code").
    pub name: &'static str,
    /// Absolute path to the global skills directory.
    pub global_skills_dir: PathBuf,
    /// Project-relative skills directory path (e.g. ".claude/skills").
    pub project_skills_dir: &'static str,
    /// Whether this agent uses the canonical `.agents/skills` path.
    pub universal: bool,
    /// Detection function: returns true if the agent is installed.
    detect_fn: fn() -> bool,
}

impl Agent {
    /// Returns true if the agent appears to be installed on this system.
    pub fn detect(&self) -> bool {
        (self.detect_fn)()
    }
}

/// Options for [`install`].
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Override detected agents (defaults to auto-detection).
    pub agents: Option<Vec<Agent>>,
    /// Working directory for project-local installs. Defaults to current dir.
    pub cwd: Option<String>,
    /// Install globally (`true`) or project-local (`false`). Defaults to `true`.
    pub global: Option<bool>,
}

/// Details about a single agent's install for a skill.
#[derive(Debug, Clone)]
pub struct AgentInstall {
    /// Agent display name.
    pub agent: String,
    /// Installed path.
    pub path: PathBuf,
    /// Whether it was symlinked or copied.
    pub mode: InstallMode,
}

/// How a skill was installed for a non-universal agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Symlink,
    Copy,
}

/// Result of an [`install`] operation.
#[derive(Debug, Clone)]
pub struct InstallResult {
    /// Canonical install paths.
    pub paths: Vec<PathBuf>,
    /// Per-agent install details (non-universal agents only).
    pub agents: Vec<AgentInstall>,
}

/// Options for [`remove`].
#[derive(Debug, Clone, Default)]
pub struct RemoveOptions {
    /// Remove globally. Defaults to `true`.
    pub global: Option<bool>,
    /// Working directory for project-local removes.
    pub cwd: Option<String>,
}

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
}

fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

fn claude_home() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
}

fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

// ---------------------------------------------------------------------------
// Agent definitions
// ---------------------------------------------------------------------------

/// Returns all known agent definitions.
pub fn all_agents() -> Vec<Agent> {
    let home = home_dir();
    let config = config_home();
    let claude = claude_home();
    let codex = codex_home();

    vec![
        // ---- Universal agents (project_skills_dir = ".agents/skills") ----
        Agent {
            name: "Amp",
            global_skills_dir: config.join("agents").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || config_home().join("amp").exists(),
        },
        Agent {
            name: "Cline",
            global_skills_dir: home.join(".agents").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || home_dir().join(".cline").exists(),
        },
        Agent {
            name: "Codex",
            global_skills_dir: codex.join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || codex_home().exists(),
        },
        Agent {
            name: "Cursor",
            global_skills_dir: home.join(".cursor").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || home_dir().join(".cursor").exists(),
        },
        Agent {
            name: "Gemini CLI",
            global_skills_dir: home.join(".gemini").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || home_dir().join(".gemini").exists(),
        },
        Agent {
            name: "GitHub Copilot",
            global_skills_dir: home.join(".copilot").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || home_dir().join(".copilot").exists(),
        },
        Agent {
            name: "Kimi CLI",
            global_skills_dir: config.join("agents").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || home_dir().join(".kimi").exists(),
        },
        Agent {
            name: "OpenCode",
            global_skills_dir: config.join("opencode").join("skills"),
            project_skills_dir: ".agents/skills",
            universal: true,
            detect_fn: || config_home().join("opencode").exists(),
        },
        // ---- Non-universal agents ----
        Agent {
            name: "Claude Code",
            global_skills_dir: claude.join("skills"),
            project_skills_dir: ".claude/skills",
            universal: false,
            detect_fn: || claude_home().exists(),
        },
        Agent {
            name: "Windsurf",
            global_skills_dir: home.join(".codeium").join("windsurf").join("skills"),
            project_skills_dir: ".windsurf/skills",
            universal: false,
            detect_fn: || home_dir().join(".codeium").join("windsurf").exists(),
        },
        Agent {
            name: "Continue",
            global_skills_dir: home.join(".continue").join("skills"),
            project_skills_dir: ".continue/skills",
            universal: false,
            detect_fn: || home_dir().join(".continue").exists(),
        },
        Agent {
            name: "Roo",
            global_skills_dir: home.join(".roo").join("skills"),
            project_skills_dir: ".roo/skills",
            universal: false,
            detect_fn: || home_dir().join(".roo").exists(),
        },
        Agent {
            name: "Kilo",
            global_skills_dir: home.join(".kilocode").join("skills"),
            project_skills_dir: ".kilocode/skills",
            universal: false,
            detect_fn: || home_dir().join(".kilocode").exists(),
        },
        Agent {
            name: "Goose",
            global_skills_dir: config.join("goose").join("skills"),
            project_skills_dir: ".goose/skills",
            universal: false,
            detect_fn: || config_home().join("goose").exists(),
        },
        Agent {
            name: "Augment",
            global_skills_dir: home.join(".augment").join("skills"),
            project_skills_dir: ".augment/skills",
            universal: false,
            detect_fn: || home_dir().join(".augment").exists(),
        },
        Agent {
            name: "Trae",
            global_skills_dir: home.join(".trae").join("skills"),
            project_skills_dir: ".trae/skills",
            universal: false,
            detect_fn: || home_dir().join(".trae").exists(),
        },
        Agent {
            name: "Junie",
            global_skills_dir: home.join(".junie").join("skills"),
            project_skills_dir: ".junie/skills",
            universal: false,
            detect_fn: || home_dir().join(".junie").exists(),
        },
        Agent {
            name: "Crush",
            global_skills_dir: config.join("crush").join("skills"),
            project_skills_dir: ".crush/skills",
            universal: false,
            detect_fn: || config_home().join("crush").exists(),
        },
        Agent {
            name: "Kiro CLI",
            global_skills_dir: home.join(".kiro").join("skills"),
            project_skills_dir: ".kiro/skills",
            universal: false,
            detect_fn: || home_dir().join(".kiro").exists(),
        },
        Agent {
            name: "Qwen Code",
            global_skills_dir: home.join(".qwen").join("skills"),
            project_skills_dir: ".qwen/skills",
            universal: false,
            detect_fn: || home_dir().join(".qwen").exists(),
        },
        Agent {
            name: "OpenHands",
            global_skills_dir: home.join(".openhands").join("skills"),
            project_skills_dir: ".openhands/skills",
            universal: false,
            detect_fn: || home_dir().join(".openhands").exists(),
        },
    ]
}

/// Returns only agents that are detected as installed on this system.
pub fn detect() -> Vec<Agent> {
    all_agents().into_iter().filter(|a| a.detect()).collect()
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// Installs skill directories to the canonical location and creates symlinks
/// for detected non-universal agents.
///
/// Copies each discovered skill (directory containing `SKILL.md`) into
/// `<base>/.agents/skills/<name>/`, then symlinks from each non-universal
/// agent's skill directory. Falls back to copy if symlink creation fails.
pub fn install(source_dir: &Path, options: &InstallOptions) -> InstallResult {
    let is_global = options.global.unwrap_or(true);
    let cwd = options
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let base = if is_global { home_dir() } else { cwd.clone() };
    let canonical_base = base.join(".agents").join("skills");
    let detected = options.agents.clone().unwrap_or_else(detect);

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut agents: Vec<AgentInstall> = Vec::new();

    for skill in discover_skills(source_dir) {
        let canonical_dir = canonical_base.join(&skill.name);

        // Copy to canonical location
        rm_force(&canonical_dir);
        let _ = fs::create_dir_all(&canonical_dir);

        if skill.root {
            // Single SKILL.md at root — just copy the file
            let _ = fs::copy(
                Path::new(&skill.dir).join("SKILL.md"),
                canonical_dir.join("SKILL.md"),
            );
        } else {
            // Copy entire directory
            copy_dir_recursive(&skill.dir, &canonical_dir);
        }
        paths.push(canonical_dir.clone());

        // Create symlinks for non-universal agents
        for agent in &detected {
            if agent.universal {
                continue;
            }
            let agent_skills_dir = if is_global {
                agent.global_skills_dir.clone()
            } else {
                cwd.join(agent.project_skills_dir)
            };
            let agent_dir = agent_skills_dir.join(&skill.name);

            // Skip if agent dir resolves to canonical (no symlink needed)
            if agent_dir == canonical_dir {
                continue;
            }

            // Try symlink first
            let symlink_result = (|| -> std::io::Result<()> {
                rm_force(&agent_dir);
                if let Some(parent) = agent_dir.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Resolve through existing symlinks in parent directories
                let real_link_dir = resolve_parent(agent_dir.parent().unwrap_or(Path::new(".")));
                let real_target = resolve_parent(&canonical_dir);
                let rel = pathdiff::diff_paths(&real_target, &real_link_dir)
                    .unwrap_or_else(|| real_target.clone());
                #[cfg(unix)]
                std::os::unix::fs::symlink(&rel, &agent_dir)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir(&rel, &agent_dir)?;
                Ok(())
            })();

            match symlink_result {
                Ok(()) => {
                    agents.push(AgentInstall {
                        agent: agent.name.to_string(),
                        path: agent_dir,
                        mode: InstallMode::Symlink,
                    });
                }
                Err(_) => {
                    // Fallback to copy
                    if copy_dir_recursive_result(&canonical_dir, &agent_dir).is_ok() {
                        agents.push(AgentInstall {
                            agent: agent.name.to_string(),
                            path: agent_dir,
                            mode: InstallMode::Copy,
                        });
                    }
                }
            }
        }
    }

    InstallResult { paths, agents }
}

// ---------------------------------------------------------------------------
// Remove
// ---------------------------------------------------------------------------

/// Removes a skill by name from the canonical location and all detected
/// agent directories.
pub fn remove(skill_name: &str, options: &RemoveOptions) {
    let is_global = options.global.unwrap_or(true);
    let cwd = options
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let base = if is_global { home_dir() } else { cwd.clone() };
    let canonical_dir = base.join(".agents").join("skills").join(skill_name);
    rm_force(&canonical_dir);

    for agent in detect() {
        if agent.universal {
            continue;
        }
        let agent_skills_dir = if is_global {
            agent.global_skills_dir.clone()
        } else {
            cwd.join(agent.project_skills_dir)
        };
        let agent_dir = agent_skills_dir.join(skill_name);
        rm_force(&agent_dir);
    }
}

// ---------------------------------------------------------------------------
// Skill discovery
// ---------------------------------------------------------------------------

/// A discovered skill directory.
struct DiscoveredSkill {
    /// Sanitized skill name.
    name: String,
    /// Absolute path to the skill directory.
    dir: PathBuf,
    /// Whether this is a root-level SKILL.md (not in a subdirectory).
    root: bool,
}

/// Recursively discovers skill directories (those containing a `SKILL.md`).
fn discover_skills(root_dir: &Path) -> Vec<DiscoveredSkill> {
    let mut results = Vec::new();
    visit_skills(root_dir, &mut results);

    // Root-level SKILL.md
    let root_skill = root_dir.join("SKILL.md");
    if root_skill.exists() {
        if let Ok(content) = fs::read_to_string(&root_skill) {
            let name = extract_skill_name(&content).unwrap_or_else(|| "skill".to_string());
            let name = sanitize_name(&name);
            if !results.iter().any(|r| r.name == name) {
                results.push(DiscoveredSkill {
                    name,
                    dir: root_dir.to_path_buf(),
                    root: true,
                });
            }
        }
    }

    results
}

fn visit_skills(dir: &Path, results: &mut Vec<DiscoveredSkill>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_path = path.join("SKILL.md");
        if skill_path.exists() {
            if let Ok(content) = fs::read_to_string(&skill_path) {
                let entry_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("skill")
                    .to_string();
                let name = extract_skill_name(&content).unwrap_or(entry_name);
                let name = sanitize_name(&name);
                results.push(DiscoveredSkill {
                    name,
                    dir: path.clone(),
                    root: false,
                });
            }
        }
        visit_skills(&path, results);
    }
}

/// Extracts the skill name from SKILL.md frontmatter (`name: ...`).
fn extract_skill_name(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("name:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Sanitizes a skill name for use as a directory name.
fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .trim()
        .replace(['/', '\\'], "-")
        .replace("..", "");
    if sanitized.len() > 255 {
        sanitized[..255].to_string()
    } else {
        sanitized
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Removes a file, directory, or symlink (including broken symlinks).
fn rm_force(target: &Path) {
    // Check symlink first (lstat equivalent)
    if let Ok(meta) = fs::symlink_metadata(target) {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(target);
        } else {
            let _ = fs::remove_dir_all(target);
        }
    }
}

/// Resolves parent directories through symlinks.
fn resolve_parent(dir: &Path) -> PathBuf {
    match fs::canonicalize(dir) {
        Ok(resolved) => resolved,
        Err(_) => {
            if let Some(parent) = dir.parent() {
                if parent == dir {
                    return dir.to_path_buf();
                }
                match fs::canonicalize(parent) {
                    Ok(real_parent) => {
                        if let Some(basename) = dir.file_name() {
                            real_parent.join(basename)
                        } else {
                            dir.to_path_buf()
                        }
                    }
                    Err(_) => dir.to_path_buf(),
                }
            } else {
                dir.to_path_buf()
            }
        }
    }
}

/// Recursively copies a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) {
    let _ = copy_dir_recursive_result(src, dst);
}

fn copy_dir_recursive_result(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive_result(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// We use a simple relative-path calculation instead of the `pathdiff` crate
// to avoid adding an external dependency.
mod pathdiff {
    use std::path::{Component, Path, PathBuf};

    /// Computes a relative path from `base` to `target`.
    pub fn diff_paths(target: &Path, base: &Path) -> Option<PathBuf> {
        let target_components: Vec<Component<'_>> = target.components().collect();
        let base_components: Vec<Component<'_>> = base.components().collect();

        // Find common prefix length.
        let common = target_components
            .iter()
            .zip(base_components.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let mut result = PathBuf::new();
        // Go up from base to the common ancestor.
        for _ in common..base_components.len() {
            result.push("..");
        }
        // Then descend into target.
        for component in &target_components[common..] {
            result.push(component);
        }
        Some(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_agents_count() {
        let agents = all_agents();
        assert_eq!(agents.len(), 21, "Expected 21 agent definitions");
    }

    #[test]
    fn test_universal_agents() {
        let agents = all_agents();
        let universal: Vec<&str> = agents.iter().filter(|a| a.universal).map(|a| a.name).collect();
        assert_eq!(
            universal,
            vec!["Amp", "Cline", "Codex", "Cursor", "Gemini CLI", "GitHub Copilot", "Kimi CLI", "OpenCode"]
        );
    }

    #[test]
    fn test_non_universal_project_dirs() {
        let agents = all_agents();
        for agent in &agents {
            if !agent.universal {
                assert_ne!(
                    agent.project_skills_dir, ".agents/skills",
                    "Non-universal agent {} should have a unique project skills dir",
                    agent.name
                );
            }
        }
    }

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("my/skill"), "my-skill");
        assert_eq!(sanitize_name("my\\skill"), "my-skill");
        assert_eq!(sanitize_name("my..skill"), "myskill");
        assert_eq!(sanitize_name("  trimmed  "), "trimmed");
    }

    #[test]
    fn test_extract_skill_name() {
        let content = "---\nname: my-skill\ndescription: A skill\n---\n";
        assert_eq!(extract_skill_name(content), Some("my-skill".to_string()));
    }

    #[test]
    fn test_extract_skill_name_missing() {
        let content = "---\ndescription: No name here\n---\n";
        assert_eq!(extract_skill_name(content), None);
    }

    #[test]
    fn test_discover_skills_empty_dir() {
        let dir = std::env::temp_dir().join("incurs-test-discover-empty");
        let _ = fs::create_dir_all(&dir);
        let skills = discover_skills(&dir);
        assert!(skills.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pathdiff() {
        let target = Path::new("/a/b/c/d");
        let base = Path::new("/a/b/x/y");
        let rel = pathdiff::diff_paths(target, base).unwrap();
        assert_eq!(rel, PathBuf::from("../../c/d"));
    }
}
