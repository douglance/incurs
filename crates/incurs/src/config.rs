//! Config file loading for the incurs framework.
//!
//! Supports loading JSON config files that provide default option values
//! for commands. The config tree mirrors the command tree structure:
//!
//! ```json
//! {
//!   "commands": {
//!     "deploy": {
//!       "options": {
//!         "environment": "staging"
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! Ported from config loading logic in `src/Cli.ts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Loads config defaults from a JSON file.
///
/// Reads the file at `path`, parses it as JSON, and returns the top-level
/// object as a map. Returns an error if the file cannot be read, contains
/// invalid JSON, or the top-level value is not an object.
pub fn load_config(path: &str) -> Result<BTreeMap<String, Value>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config file '{path}': {e}"))?;

    let parsed: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Invalid JSON config file '{path}': {e}"))?;

    match parsed {
        Value::Object(map) => {
            let btree: BTreeMap<String, Value> = map.into_iter().collect();
            Ok(btree)
        }
        _ => Err(format!(
            "Invalid config file: expected a top-level object in '{path}'"
        )
        .into()),
    }
}

/// Resolves the config file path from an explicit flag value or default search locations.
///
/// If `flag_value` is `Some`, expands `~` to the home directory and resolves
/// relative to the current working directory.
///
/// If `flag_value` is `None`, searches `files` in order and returns the first
/// existing file path.
///
/// Returns `None` if no config file is found.
pub fn resolve_config_path(flag_value: Option<&str>, files: &[String]) -> Option<String> {
    if let Some(explicit) = flag_value {
        return Some(resolve_path(explicit));
    }

    // Search default file locations
    for file in files {
        let resolved = resolve_path(file);
        if Path::new(&resolved).exists() {
            return Some(resolved);
        }
    }

    None
}

/// Extracts command-specific option defaults from a parsed config tree.
///
/// Walks the nested config structure following the command path segments.
/// For a command path like `"users list"`, looks for:
///
/// ```json
/// { "commands": { "users": { "commands": { "list": { "options": { ... } } } } } }
/// ```
///
/// Returns `None` if the command section or its `options` key is not found.
pub fn extract_command_section(
    config: &BTreeMap<String, Value>,
    cli_name: &str,
    command_path: &str,
) -> Result<Option<BTreeMap<String, Value>>, Box<dyn std::error::Error>> {
    // If the command path is the CLI name itself (root command), look for
    // options at the top level.
    let segments: Vec<&str> = if command_path == cli_name {
        Vec::new()
    } else {
        command_path.split(' ').collect()
    };

    let mut node: &Value = &Value::Object(
        config
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );

    for seg in &segments {
        let obj = node
            .as_object()
            .ok_or_else(|| format!("Invalid config section for '{command_path}': expected an object"))?;

        let commands = match obj.get("commands") {
            Some(c) => c,
            None => return Ok(None),
        };

        let commands_obj = commands
            .as_object()
            .ok_or_else(|| format!("Invalid config 'commands' for '{command_path}': expected an object"))?;

        node = match commands_obj.get(*seg) {
            Some(n) => n,
            None => return Ok(None),
        };
    }

    let obj = node
        .as_object()
        .ok_or_else(|| format!("Invalid config section for '{command_path}': expected an object"))?;

    let options = match obj.get("options") {
        Some(o) => o,
        None => return Ok(None),
    };

    let options_obj = options
        .as_object()
        .ok_or_else(|| format!("Invalid config 'options' for '{command_path}': expected an object"))?;

    if options_obj.is_empty() {
        return Ok(None);
    }

    let btree: BTreeMap<String, Value> = options_obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok(Some(btree))
}

/// Resolves a file path, expanding `~` to the user's home directory.
fn resolve_path(file_path: &str) -> String {
    if (file_path.starts_with("~/") || file_path == "~") && let Some(home) = dirs::home_dir() {
        return home
            .join(&file_path[1..])
            .to_string_lossy()
            .into_owned();
    }

    // Resolve relative to current working directory
    let path = PathBuf::from(file_path);
    if path.is_absolute() {
        file_path.to_string()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(file_path).to_string_lossy().into_owned(),
            Err(_) => file_path.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        assert_eq!(resolve_path("/etc/config.json"), "/etc/config.json");
    }

    #[test]
    fn test_resolve_path_tilde() {
        let resolved = resolve_path("~/config.json");
        assert!(!resolved.starts_with("~"));
        assert!(resolved.ends_with("config.json"));
    }

    #[test]
    fn test_extract_command_section_root() {
        let config: BTreeMap<String, Value> = serde_json::from_str(
            r#"{ "options": { "verbose": true } }"#,
        )
        .unwrap();

        let result = extract_command_section(&config, "my-cli", "my-cli").unwrap();
        let opts = result.unwrap();
        assert_eq!(opts.get("verbose"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_extract_command_section_nested() {
        let config: BTreeMap<String, Value> = serde_json::from_str(
            r#"{
                "commands": {
                    "deploy": {
                        "options": {
                            "environment": "staging"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let result = extract_command_section(&config, "my-cli", "deploy").unwrap();
        let opts = result.unwrap();
        assert_eq!(
            opts.get("environment"),
            Some(&Value::String("staging".to_string()))
        );
    }

    #[test]
    fn test_extract_command_section_missing() {
        let config: BTreeMap<String, Value> =
            serde_json::from_str(r#"{ "commands": {} }"#).unwrap();

        let result = extract_command_section(&config, "my-cli", "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_command_section_empty_options() {
        let config: BTreeMap<String, Value> = serde_json::from_str(
            r#"{ "commands": { "test": { "options": {} } } }"#,
        )
        .unwrap();

        let result = extract_command_section(&config, "my-cli", "test").unwrap();
        assert!(result.is_none());
    }
}
