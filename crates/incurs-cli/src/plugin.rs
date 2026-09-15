//! Agent Plugins 1.0 client commands.
//!
//! These load, connect to, and invoke a portable plugin directory. Loading and
//! connection are owned by `incurs::agent_plugin`; this module only shapes the
//! results into typed command output.

use std::collections::BTreeMap;
use std::path::PathBuf;

use incurs::agent_plugin::loader::{
    AgentPluginDiagnostic, AgentPluginDiagnosticSeverity, AgentPluginLoadOptions,
    AgentPluginLoadReport, AgentPluginMcpTransport, load_agent_plugin,
};
use incurs::agent_plugin_runtime::{
    AgentPluginMcpServerStatus, AgentPluginRuntimeOptions, connect_agent_plugin,
};
use incurs::command::{TypedContext, TypedResult};
use incurs::tool::{ToolCallOptions, ToolCallOutcome};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plugin_install;

/// The plugin directory to act on.
#[derive(Deserialize, incurs::Args)]
pub struct PluginArgs {
    /// Path to the Agent Plugin directory.
    pub path: String,
}

/// The plugin directory and the tool to invoke.
#[derive(Deserialize, incurs::Args)]
pub struct CallArgs {
    /// Path to the Agent Plugin directory.
    pub path: String,
    /// Namespaced tool name to invoke.
    pub tool: String,
}

/// Options shared by every command that loads a plugin.
#[derive(Deserialize, incurs::Options)]
pub struct LoadOptions {
    /// Dedicated persistent writable directory for this plugin instance.
    pub data_dir: String,
}

/// Options for invoking one tool.
#[derive(Deserialize, incurs::Options)]
pub struct CallOptions {
    /// Dedicated persistent writable directory for this plugin instance.
    pub data_dir: String,
    /// Flat JSON object passed to the selected tool.
    pub arguments: Option<String>,
}

/// One diagnostic emitted while loading a plugin.
#[derive(Debug, JsonSchema, Serialize)]
pub struct Diagnostic {
    /// Machine-readable diagnostic code.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Path the diagnostic applies to.
    pub path: String,
    /// One of `info`, `warning`, or `error`.
    pub severity: String,
}

/// What a valid plugin manifest declares.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    /// Resolved persistent data directory.
    pub data_dir: PathBuf,
    /// Declared extension names.
    pub extensions: Vec<String>,
    /// Declared MCP server names.
    pub mcp_servers: Vec<String>,
    /// Plugin name.
    pub name: String,
    /// Plugin root directory.
    pub root: PathBuf,
    /// Declared skill names.
    pub skills: Vec<String>,
    /// Plugin version.
    pub version: String,
}

/// The result of loading a plugin without connecting to it.
#[derive(Debug, JsonSchema, Serialize)]
pub struct ValidateOutput {
    /// Every diagnostic produced while loading.
    pub diagnostics: Vec<Diagnostic>,
    /// The loaded plugin, when the manifest is valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginSummary>,
    /// Whether the manifest loaded.
    pub valid: bool,
}

/// One connected MCP server.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSummary {
    /// Connection error, when the server did not connect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Declared server name.
    pub name: String,
    /// Number of tools the server exposed.
    pub tool_count: usize,
    /// Transport used to reach the server.
    pub transport: String,
}

/// The result of connecting every declared MCP server.
#[derive(Debug, JsonSchema, Serialize)]
pub struct ToolsOutput {
    /// Every diagnostic produced while loading.
    pub diagnostics: Vec<Diagnostic>,
    /// Plugin name.
    pub name: String,
    /// Connection result per declared server.
    pub servers: Vec<ServerSummary>,
    /// Namespaced tool definitions, in stable name order.
    pub tools: Value,
}

/// Loads a plugin and reports its diagnostics without connecting.
pub async fn validate(
    ctx: TypedContext<PluginArgs, LoadOptions, ()>,
) -> TypedResult<ValidateOutput> {
    let report = load(&ctx.args.path, &ctx.options.data_dir);
    let valid = report.plugin.is_some();
    let output = ValidateOutput {
        diagnostics: report.diagnostics.iter().map(diagnostic).collect(),
        plugin: report.plugin.as_ref().map(|plugin| PluginSummary {
            data_dir: plugin.data_root.clone(),
            extensions: plugin.extensions.keys().cloned().collect(),
            mcp_servers: plugin.mcp_servers.keys().cloned().collect(),
            name: plugin.manifest.name.clone(),
            root: plugin.root.clone(),
            skills: plugin
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect(),
            version: plugin.manifest.version.clone().unwrap_or_default(),
        }),
        valid,
    };

    if valid {
        TypedResult::ok(output)
    } else {
        TypedResult::error("PLUGIN_INVALID", "Agent Plugin manifest is invalid")
    }
}

/// Connects every declared MCP server and lists the tools they expose.
pub async fn tools(ctx: TypedContext<PluginArgs, LoadOptions, ()>) -> TypedResult<ToolsOutput> {
    let report = load(&ctx.args.path, &ctx.options.data_dir);
    let diagnostics = report.diagnostics.iter().map(diagnostic).collect();
    let Some(plugin) = report.plugin else {
        return TypedResult::error("PLUGIN_INVALID", describe(&report.diagnostics));
    };

    let connected = match connect_agent_plugin(&plugin, &AgentPluginRuntimeOptions::default()).await
    {
        Ok(connected) => connected,
        Err(error) => return TypedResult::error("PLUGIN_CONNECT_FAILED", error.to_string()),
    };

    TypedResult::ok(ToolsOutput {
        diagnostics,
        name: plugin.manifest.name.clone(),
        servers: connected.servers.iter().map(server).collect(),
        tools: serde_json::to_value(connected.catalog.definitions()).unwrap_or(Value::Null),
    })
}

/// Invokes one namespaced tool with flat JSON arguments.
pub async fn call(ctx: TypedContext<CallArgs, CallOptions, ()>) -> TypedResult<Value> {
    let arguments: BTreeMap<String, Value> = match ctx.options.arguments.as_deref() {
        Some(raw) => match serde_json::from_str(raw) {
            Ok(arguments) => arguments,
            Err(error) => {
                return TypedResult::error(
                    "INVALID_ARGUMENTS",
                    format!("invalid --arguments JSON object: {error}"),
                );
            }
        },
        None => BTreeMap::new(),
    };

    let report = load(&ctx.args.path, &ctx.options.data_dir);
    let Some(plugin) = report.plugin else {
        return TypedResult::error("PLUGIN_INVALID", describe(&report.diagnostics));
    };

    let connected = match connect_agent_plugin(&plugin, &AgentPluginRuntimeOptions::default()).await
    {
        Ok(connected) => connected,
        Err(error) => return TypedResult::error("PLUGIN_CONNECT_FAILED", error.to_string()),
    };
    if connected
        .servers
        .iter()
        .all(|server| server.error.is_some())
    {
        return TypedResult::error(
            "PLUGIN_CONNECT_FAILED",
            connected
                .servers
                .iter()
                .filter_map(|server| server.error.as_deref())
                .collect::<Vec<_>>()
                .join("; "),
        );
    }

    let outcome = connected
        .catalog
        .call(&ctx.args.tool, arguments, ToolCallOptions::isolated())
        .await;
    match outcome {
        ToolCallOutcome::Ok { data, .. } => TypedResult::ok(data),
        ToolCallOutcome::Error { code, message, .. } => TypedResult::error(code, message),
    }
}

/// Options for installing a plugin.
#[derive(Deserialize, incurs::Options)]
pub struct InstallOptions {
    /// Managed user command directory.
    pub bin_dir: Option<String>,
    /// Replace artifacts owned by the same installed plugin.
    pub force: bool,
}

/// The plugin source to install.
#[derive(Deserialize, incurs::Args)]
pub struct InstallArgs {
    /// Path to the Agent Plugin directory to install.
    pub path: String,
}

/// Installs a portable plugin and its declared shell command.
pub async fn install(
    ctx: TypedContext<InstallArgs, InstallOptions, ()>,
) -> TypedResult<plugin_install::InstallResult> {
    let data_home = match plugin_install::default_data_home() {
        Ok(path) => path,
        Err(error) => return TypedResult::error("NO_DATA_HOME", error),
    };
    let bin_dir = match ctx.options.bin_dir.as_deref() {
        Some(dir) => PathBuf::from(dir),
        None => match plugin_install::default_bin_dir() {
            Ok(path) => path,
            Err(error) => return TypedResult::error("NO_BIN_DIR", error),
        },
    };

    match plugin_install::install(plugin_install::InstallOptions {
        source: PathBuf::from(&ctx.args.path),
        data_home,
        bin_dir,
        force: ctx.options.force,
    }) {
        Ok(report) => TypedResult::ok(report),
        Err(error) => TypedResult::error("INSTALL_FAILED", error),
    }
}

/// Options for uninstalling a plugin.
#[derive(Deserialize, incurs::Options)]
pub struct UninstallOptions {
    /// Remove persistent plugin data during uninstall.
    pub purge: bool,
}

/// The installed plugin to remove.
#[derive(Deserialize, incurs::Args)]
pub struct UninstallArgs {
    /// Installed plugin name.
    pub name: String,
}

/// Removes installer-owned plugin and command artifacts.
pub async fn uninstall(
    ctx: TypedContext<UninstallArgs, UninstallOptions, ()>,
) -> TypedResult<plugin_install::UninstallResult> {
    let data_home = match plugin_install::default_data_home() {
        Ok(path) => path,
        Err(error) => return TypedResult::error("NO_DATA_HOME", error),
    };

    match plugin_install::uninstall(plugin_install::UninstallOptions {
        name: ctx.args.name.clone(),
        data_home,
        purge: ctx.options.purge,
    }) {
        Ok(report) => TypedResult::ok(report),
        Err(error) => TypedResult::error("UNINSTALL_FAILED", error),
    }
}

/// Loads one plugin directory with a dedicated data root.
fn load(path: &str, data_dir: &str) -> AgentPluginLoadReport {
    load_agent_plugin(
        std::path::Path::new(path),
        &AgentPluginLoadOptions {
            plugin_data_root: PathBuf::from(data_dir),
            ..Default::default()
        },
    )
}

/// Shapes one loader diagnostic for typed output.
fn diagnostic(diagnostic: &AgentPluginDiagnostic) -> Diagnostic {
    Diagnostic {
        code: diagnostic.code.to_string(),
        message: diagnostic.message.clone(),
        path: diagnostic.path.clone(),
        severity: match diagnostic.severity {
            AgentPluginDiagnosticSeverity::Info => "info",
            AgentPluginDiagnosticSeverity::Warning => "warning",
            AgentPluginDiagnosticSeverity::Error => "error",
        }
        .to_string(),
    }
}

/// Joins diagnostics into one message for a failure that has no plugin.
fn describe(diagnostics: &[AgentPluginDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Shapes one connected server for typed output.
fn server(server: &AgentPluginMcpServerStatus) -> ServerSummary {
    ServerSummary {
        error: server.error.clone(),
        name: server.name.clone(),
        tool_count: server.tool_count,
        transport: match server.transport {
            AgentPluginMcpTransport::Stdio => "stdio",
            AgentPluginMcpTransport::StreamableHttp => "streamable-http",
            AgentPluginMcpTransport::Sse => "sse",
        }
        .to_string(),
    }
}
