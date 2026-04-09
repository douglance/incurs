//! MCP server registration with AI coding agents.
//!
//! Ported from `src/SyncMcp.ts`. Registers the CLI binary as an MCP (Model
//! Context Protocol) server by writing agent-specific configuration files.
//! For Rust binaries, uses `std::env::current_exe()` instead of npx.

use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Options for [`register`].
#[derive(Debug, Clone, Default)]
pub struct RegisterOptions {
    /// Target specific agents (e.g. "claude-code", "cursor").
    /// Empty means register with all detected agents.
    pub agents: Option<Vec<String>>,
    /// Override the command agents will run.
    /// Defaults to `<exe_path> --mcp`.
    pub command: Option<String>,
    /// Install globally. Defaults to `true`.
    pub global: bool,
}

/// Result of a [`register`] operation.
#[derive(Debug, Clone)]
pub struct RegisterResult {
    /// Agents the server was registered with.
    pub agents: Vec<String>,
    /// The command that was registered.
    pub command: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Registers the CLI as an MCP server with detected coding agents.
///
/// For Rust binaries, the command defaults to the current executable path
/// with `--mcp` appended. Currently supports direct registration with Amp
/// (via its `settings.json`). Other agents may require `add-mcp` or manual
/// configuration.
pub async fn register(
    name: &str,
    options: &RegisterOptions,
) -> Result<RegisterResult, crate::errors::Error> {
    let command = options.command.clone().unwrap_or_else(|| {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| name.to_string());
        format!("{} --mcp", exe)
    });

    let target_agents = options.agents.clone().unwrap_or_default();
    let mut registered_agents: Vec<String> = Vec::new();

    // Register with Amp directly (writes to ~/.config/amp/settings.json)
    if target_agents.is_empty() || target_agents.iter().any(|a| a == "amp") {
        if register_amp(name, &command) {
            registered_agents.push("Amp".to_string());
        }
    }

    // Register with Claude Code (writes to ~/.claude.json or project .claude.json)
    if target_agents.is_empty() || target_agents.iter().any(|a| a == "claude-code" || a == "claude") {
        if register_claude_code(name, &command, options.global) {
            registered_agents.push("Claude Code".to_string());
        }
    }

    Ok(RegisterResult {
        command,
        agents: registered_agents,
    })
}

// ---------------------------------------------------------------------------
// Agent-specific registration
// ---------------------------------------------------------------------------

/// Registers an MCP server in Amp's `settings.json`.
fn register_amp(name: &str, command: &str) -> bool {
    let config_path = amp_config_path();

    let mut config: serde_json::Map<String, serde_json::Value> = if config_path.exists() {
        match fs::read_to_string(&config_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(c) => c,
                Err(_) => return false,
            },
            Err(_) => return false,
        }
    } else {
        serde_json::Map::new()
    };

    let parts: Vec<&str> = command.splitn(2, ' ').collect();
    let cmd = match parts.first() {
        Some(c) => *c,
        None => return false,
    };
    let args: Vec<&str> = if parts.len() > 1 {
        parts[1].split_whitespace().collect()
    } else {
        vec![]
    };

    let mut servers = config
        .remove("amp.mcpServers")
        .and_then(|v| match v {
            serde_json::Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();

    let mut entry = serde_json::Map::new();
    entry.insert(
        "command".to_string(),
        serde_json::Value::String(cmd.to_string()),
    );
    entry.insert(
        "args".to_string(),
        serde_json::Value::Array(
            args.iter()
                .map(|a| serde_json::Value::String(a.to_string()))
                .collect(),
        ),
    );
    servers.insert(name.to_string(), serde_json::Value::Object(entry));
    config.insert(
        "amp.mcpServers".to_string(),
        serde_json::Value::Object(servers),
    );

    if let Some(dir) = config_path.parent() {
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
    }

    match serde_json::to_string_pretty(&serde_json::Value::Object(config)) {
        Ok(json) => fs::write(&config_path, format!("{}\n", json)).is_ok(),
        Err(_) => false,
    }
}

/// Registers an MCP server with Claude Code's configuration.
fn register_claude_code(name: &str, command: &str, global: bool) -> bool {
    let config_path = if global {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".claude.json")
    } else {
        PathBuf::from(".claude.json")
    };

    let mut config: serde_json::Map<String, serde_json::Value> = if config_path.exists() {
        match fs::read_to_string(&config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => serde_json::Map::new(),
        }
    } else {
        serde_json::Map::new()
    };

    let parts: Vec<&str> = command.splitn(2, ' ').collect();
    let cmd = match parts.first() {
        Some(c) => *c,
        None => return false,
    };
    let args: Vec<&str> = if parts.len() > 1 {
        parts[1].split_whitespace().collect()
    } else {
        vec![]
    };

    let mut servers = config
        .remove("mcpServers")
        .and_then(|v| match v {
            serde_json::Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();

    let mut entry = serde_json::Map::new();
    entry.insert(
        "command".to_string(),
        serde_json::Value::String(cmd.to_string()),
    );
    entry.insert(
        "args".to_string(),
        serde_json::Value::Array(
            args.iter()
                .map(|a| serde_json::Value::String(a.to_string()))
                .collect(),
        ),
    );
    servers.insert(name.to_string(), serde_json::Value::Object(entry));
    config.insert(
        "mcpServers".to_string(),
        serde_json::Value::Object(servers),
    );

    if let Some(dir) = config_path.parent() {
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
    }

    match serde_json::to_string_pretty(&serde_json::Value::Object(config)) {
        Ok(json) => fs::write(&config_path, format!("{}\n", json)).is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Returns the path to Amp's settings.json.
fn amp_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config").join("amp").join("settings.json")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amp_config_path() {
        let path = amp_config_path();
        assert!(path.to_string_lossy().contains("amp"));
        assert!(path.to_string_lossy().ends_with("settings.json"));
    }

    #[test]
    fn test_register_options_default() {
        let opts = RegisterOptions::default();
        assert!(opts.agents.is_none());
        assert!(opts.command.is_none());
        assert!(!opts.global);
    }
}
