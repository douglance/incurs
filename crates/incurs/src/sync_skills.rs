//! Skill file synchronization — generates and installs skill files from commands.
//!
//! Ported from `src/SyncSkills.ts`. Generates SKILL.md files from the command
//! tree, installs them to agent directories, and tracks a hash for staleness
//! detection so repeated syncs are no-ops when commands haven't changed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::agents::{self, AgentInstall, InstallOptions, RemoveOptions};
use crate::skill::{self, CommandInfo, SkillFile};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Options for [`sync`].
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Working directory for resolving include globs. Defaults to current dir.
    pub cwd: Option<String>,
    /// Grouping depth for skill files. Defaults to `1`.
    pub depth: Option<usize>,
    /// CLI description, used as the top-level group description.
    pub description: Option<String>,
    /// Install globally (`true`) or project-local (`false`). Defaults to `true`.
    pub global: bool,
    /// Glob patterns for directories containing additional SKILL.md files to include.
    pub include: Option<Vec<String>>,
}

/// A synced skill entry.
#[derive(Debug, Clone)]
pub struct SyncedSkill {
    /// Skill directory name.
    pub name: String,
    /// Description extracted from skill frontmatter.
    pub description: Option<String>,
    /// Whether this skill was included from a local file (not generated from commands).
    pub external: bool,
}

/// Result of a [`sync`] operation.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Synced skills with metadata.
    pub skills: Vec<SyncedSkill>,
    /// Canonical install paths.
    pub paths: Vec<PathBuf>,
    /// Per-agent install details (non-universal agents only).
    pub agents: Vec<AgentInstall>,
}

/// Stored metadata for staleness detection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Meta {
    hash: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    at: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generates skill files from commands and installs them to agent directories.
///
/// Creates a temporary directory, writes SKILL.md files, installs them via
/// [`agents::install`], cleans up stale skills from previous syncs, and
/// writes a hash file for future staleness detection.
pub async fn sync(
    name: &str,
    commands: &[CommandInfo],
    options: &SyncOptions,
) -> Result<SyncResult, crate::errors::Error> {
    let depth = options.depth.unwrap_or(1);
    let is_global = options.global;

    let cwd = options
        .cwd
        .clone()
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

    // Build groups from description
    let mut groups: BTreeMap<String, String> = BTreeMap::new();
    if let Some(desc) = &options.description {
        groups.insert(name.to_string(), desc.clone());
    }

    let files = skill::split(name, commands, depth, &groups);

    // Create temp directory
    let tmp_dir = std::env::temp_dir().join(format!("incurs-skills-{}-{}", name, std::process::id()));
    let _ = fs::create_dir_all(&tmp_dir);

    let result = sync_inner(name, commands, &files, &tmp_dir, &cwd, is_global, &options.include);

    // Cleanup temp directory
    let _ = fs::remove_dir_all(&tmp_dir);

    result
}

fn sync_inner(
    name: &str,
    commands: &[CommandInfo],
    files: &[SkillFile],
    tmp_dir: &Path,
    cwd: &str,
    is_global: bool,
    include: &Option<Vec<String>>,
) -> Result<SyncResult, crate::errors::Error> {
    let mut skills: Vec<SyncedSkill> = Vec::new();

    for file in files {
        let file_path = if file.dir.is_empty() {
            tmp_dir.join("SKILL.md")
        } else {
            tmp_dir.join(&file.dir).join("SKILL.md")
        };
        if let Some(parent) = file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = format!("{}\n", file.content);
        let _ = fs::write(&file_path, &content);

        let desc = extract_description(&content);
        let skill_name = if file.dir.is_empty() {
            name.to_string()
        } else {
            file.dir.clone()
        };
        skills.push(SyncedSkill {
            name: skill_name,
            description: desc,
            external: false,
        });
    }

    // Include additional SKILL.md files matched by patterns
    if let Some(patterns) = include {
        for pattern in patterns {
            let is_root = pattern == "_root";
            let search_path = if is_root {
                PathBuf::from(cwd).join("SKILL.md")
            } else {
                PathBuf::from(cwd).join(pattern).join("SKILL.md")
            };

            if search_path.exists() {
                if let Ok(content) = fs::read_to_string(&search_path) {
                    let skill_name = if is_root {
                        extract_skill_name(&content).unwrap_or_else(|| name.to_string())
                    } else {
                        search_path
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or(pattern)
                            .to_string()
                    };

                    let dest = tmp_dir.join(&skill_name).join("SKILL.md");
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&dest, &content);

                    if !skills.iter().any(|s| s.name == skill_name) {
                        let desc = extract_description(&content);
                        skills.push(SyncedSkill {
                            name: skill_name,
                            description: desc,
                            external: true,
                        });
                    }
                }
            }
        }
    }

    // Install via agents module
    let install_result = agents::install(
        tmp_dir,
        &InstallOptions {
            global: Some(is_global),
            cwd: Some(cwd.to_string()),
            ..Default::default()
        },
    );

    // Remove stale skills from previous installs
    let current_names: std::collections::HashSet<String> = install_result
        .paths
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
        .collect();

    let prev = read_meta(name);
    if let Some(prev_meta) = prev {
        for old in &prev_meta.skills {
            if !current_names.contains(old) {
                agents::remove(
                    old,
                    &RemoveOptions {
                        global: Some(is_global),
                        cwd: Some(cwd.to_string()),
                    },
                );
            }
        }
    }

    // Write hash for staleness detection
    let hash = skill::hash(commands);
    let skill_names: Vec<String> = current_names.into_iter().collect();
    write_meta(name, &hash, &skill_names);

    Ok(SyncResult {
        skills,
        paths: install_result.paths,
        agents: install_result.agents,
    })
}

/// Reads the stored skills hash for a CLI. Returns `None` if no hash exists.
pub fn read_hash(name: &str) -> Option<String> {
    read_meta(name).map(|m| m.hash)
}

// ---------------------------------------------------------------------------
// Metadata persistence
// ---------------------------------------------------------------------------

/// Returns the metadata file path for a CLI.
fn meta_path(name: &str) -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local").join("share"));
    data_home.join("incurs").join(format!("{}.json", name))
}

/// Writes the skills metadata for staleness detection and cleanup.
fn write_meta(name: &str, hash: &str, skills: &[String]) {
    let file = meta_path(name);
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let meta = Meta {
        hash: hash.to_string(),
        skills: skills.to_vec(),
        at: chrono_now(),
    };
    if let Ok(json) = serde_json::to_string(&meta) {
        let _ = fs::write(&file, format!("{}\n", json));
    }
}

/// Reads the stored metadata for a CLI.
fn read_meta(name: &str) -> Option<Meta> {
    let file = meta_path(name);
    let content = fs::read_to_string(&file).ok()?;
    serde_json::from_str(&content).ok()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extracts the `description:` frontmatter value from SKILL.md content.
fn extract_description(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("description:") {
            let desc = rest.trim();
            if !desc.is_empty() {
                return Some(desc.to_string());
            }
        }
    }
    None
}

/// Extracts the `name:` frontmatter value from SKILL.md content.
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

/// Returns a basic ISO 8601 timestamp without pulling in the chrono crate.
fn chrono_now() -> String {
    // Use std SystemTime for a basic timestamp
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s", d.as_secs()),
        Err(_) => "0s".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_description() {
        let content = "---\nname: test\ndescription: A test skill\n---\n";
        assert_eq!(
            extract_description(content),
            Some("A test skill".to_string())
        );
    }

    #[test]
    fn test_extract_description_missing() {
        let content = "---\nname: test\n---\n";
        assert_eq!(extract_description(content), None);
    }

    #[test]
    fn test_extract_skill_name() {
        let content = "---\nname: my-skill\n---\n";
        assert_eq!(
            extract_skill_name(content),
            Some("my-skill".to_string())
        );
    }

    #[test]
    fn test_meta_path() {
        let path = meta_path("mycli");
        assert!(path.to_string_lossy().contains("incurs"));
        assert!(path.to_string_lossy().ends_with("mycli.json"));
    }

    #[test]
    fn test_read_hash_nonexistent() {
        assert_eq!(read_hash("nonexistent-test-cli-12345"), None);
    }
}
