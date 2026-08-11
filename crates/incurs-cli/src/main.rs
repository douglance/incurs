use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Map, Value, json};

mod plugin_install;

#[derive(Debug)]
struct GenOptions {
    dir: PathBuf,
    entry: Option<String>,
    output: Option<PathBuf>,
    json_output: Option<PathBuf>,
    config_schema: bool,
    plugin_output: Option<PathBuf>,
    plugin_skill_depth: usize,
    plugin_mcp: bool,
    plugin_bundle_cli: bool,
    plugin_force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginAction {
    Validate,
    Tools,
    Call,
}

#[derive(Debug)]
struct PluginOptions {
    action: PluginAction,
    root: PathBuf,
    data_dir: PathBuf,
    tool: Option<String>,
    arguments: BTreeMap<String, Value>,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = match args.first().map(String::as_str) {
        Some("gen") => parse_gen(&args[1..]).and_then(generate),
        Some("plugin")
            if args
                .get(1)
                .is_none_or(|arg| matches!(arg.as_str(), "--help" | "-h")) =>
        {
            print_plugin_help();
            Ok(())
        }
        Some("plugin") if args.get(1).is_some_and(|arg| arg == "install") => {
            run_plugin_install_command(&args[2..])
        }
        Some("plugin") if args.get(1).is_some_and(|arg| arg == "uninstall") => {
            run_plugin_uninstall_command(&args[2..])
        }
        Some("plugin") => parse_plugin(&args[1..]).and_then(run_plugin),
        Some("--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}")),
    };

    if let Err(error) = result {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "incurs - CLI for incurs\n\nUsage: incurs <command>\n\nCommands:\n  gen     Generate Rust command types and JSON manifests\n  plugin  Validate or connect an Agent Plugins 1.0 directory\n"
    );
}

fn print_plugin_help() {
    println!(
        "Install, validate, connect, or call an Agent Plugins 1.0 directory.\n\nUsage:\n  incurs plugin install <path> [--bin-dir <path>] [--force]\n  incurs plugin uninstall <name> [--purge]\n  incurs plugin validate <path> --data-dir <path>\n  incurs plugin tools <path> --data-dir <path>\n  incurs plugin call <path> <tool> [--arguments <json>] --data-dir <path>\n\nCommands:\n  install    Install a portable plugin and its declared shell command\n  uninstall  Remove installer-owned plugin and command artifacts\n  validate   Load all fixed components and print diagnostics\n  tools      Connect every valid MCP server and print namespaced tools\n  call       Invoke one namespaced tool with flat JSON arguments\n\nOptions:\n  --arguments <json>  Flat JSON object passed to the selected tool\n  --bin-dir <path>    Managed user command directory\n  --data-dir <path>   Dedicated persistent writable directory for this plugin instance\n  --force             Replace artifacts owned by the same installed plugin\n  --purge             Remove persistent plugin data during uninstall\n"
    );
}

fn run_plugin_install_command(args: &[String]) -> Result<(), String> {
    let options = parse_plugin_install(
        args,
        plugin_install::default_data_home()?,
        plugin_install::default_bin_dir()?,
    )?;
    let result = plugin_install::install(options)?;
    if let Some(warning) = &result.warning {
        eprintln!("Warning: {warning}");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn run_plugin_uninstall_command(args: &[String]) -> Result<(), String> {
    let options = parse_plugin_uninstall(args, plugin_install::default_data_home()?)?;
    let result = plugin_install::uninstall(options)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn parse_plugin_install(
    args: &[String],
    data_home: PathBuf,
    default_bin_dir: PathBuf,
) -> Result<plugin_install::InstallOptions, String> {
    let source = args
        .first()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| "missing Agent Plugin path".to_string())?;
    let mut bin_dir = default_bin_dir;
    let mut force = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--bin-dir" => {
                index += 1;
                bin_dir = PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "missing value for --bin-dir".to_string())?,
                );
            }
            "--force" => force = true,
            flag => return Err(format!("unknown plugin install option: {flag}")),
        }
        index += 1;
    }
    Ok(plugin_install::InstallOptions {
        source,
        data_home,
        bin_dir,
        force,
    })
}

fn parse_plugin_uninstall(
    args: &[String],
    data_home: PathBuf,
) -> Result<plugin_install::UninstallOptions, String> {
    let name = args
        .first()
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| "missing installed plugin name".to_string())?;
    let mut purge = false;
    for flag in &args[1..] {
        match flag.as_str() {
            "--purge" => purge = true,
            flag => return Err(format!("unknown plugin uninstall option: {flag}")),
        }
    }
    Ok(plugin_install::UninstallOptions {
        name,
        data_home,
        purge,
    })
}

fn parse_plugin(args: &[String]) -> Result<PluginOptions, String> {
    let action = match args.first().map(String::as_str) {
        Some("validate") => PluginAction::Validate,
        Some("tools") => PluginAction::Tools,
        Some("call") => PluginAction::Call,
        Some(action) => return Err(format!("unknown plugin action: {action}")),
        None => return Err("missing plugin action: validate or tools".to_string()),
    };
    let root = args
        .get(1)
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| "missing Agent Plugin path".to_string())?;
    let tool = if action == PluginAction::Call {
        Some(
            args.get(2)
                .filter(|value| !value.starts_with('-'))
                .cloned()
                .ok_or_else(|| "missing namespaced tool name".to_string())?,
        )
    } else {
        None
    };
    let mut data_dir = None;
    let mut arguments = BTreeMap::new();
    let mut index = if tool.is_some() { 3 } else { 2 };
    while index < args.len() {
        match args[index].as_str() {
            "--data-dir" => {
                index += 1;
                data_dir = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "missing value for --data-dir".to_string())?,
                ));
            }
            "--arguments" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "missing value for --arguments".to_string())?;
                arguments = serde_json::from_str(value)
                    .map_err(|error| format!("invalid --arguments JSON object: {error}"))?;
            }
            flag => return Err(format!("unknown plugin option: {flag}")),
        }
        index += 1;
    }
    Ok(PluginOptions {
        action,
        root,
        data_dir: data_dir.ok_or_else(|| "missing required --data-dir <path>".to_string())?,
        tool,
        arguments,
    })
}

fn run_plugin(options: PluginOptions) -> Result<(), String> {
    let load_options = incurs::agent_plugin::loader::AgentPluginLoadOptions {
        plugin_data_root: options.data_dir,
        ..Default::default()
    };
    let report = incurs::agent_plugin::loader::load_agent_plugin(&options.root, &load_options);
    match options.action {
        PluginAction::Validate => print_plugin_validation(report),
        PluginAction::Tools => print_plugin_tools(report),
        PluginAction::Call => print_plugin_call(
            report,
            options.tool.expect("call parser requires a tool"),
            options.arguments,
        ),
    }
}

fn print_plugin_validation(
    report: incurs::agent_plugin::loader::AgentPluginLoadReport,
) -> Result<(), String> {
    let diagnostics = report
        .diagnostics
        .iter()
        .map(plugin_diagnostic_json)
        .collect::<Vec<_>>();
    let plugin = report.plugin.as_ref().map(|plugin| {
        json!({
            "dataDir": plugin.data_root,
            "extensions": plugin.extensions.keys().collect::<Vec<_>>(),
            "mcpServers": plugin.mcp_servers.keys().collect::<Vec<_>>(),
            "name": plugin.manifest.name,
            "root": plugin.root,
            "skills": plugin.skills.iter().map(|skill| &skill.name).collect::<Vec<_>>(),
            "version": plugin.manifest.version,
        })
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "diagnostics": diagnostics,
            "plugin": plugin,
            "valid": report.plugin.is_some(),
        }))
        .map_err(|error| error.to_string())?
    );
    if report.plugin.is_none() {
        return Err("Agent Plugin manifest is invalid".to_string());
    }
    Ok(())
}

fn print_plugin_tools(
    report: incurs::agent_plugin::loader::AgentPluginLoadReport,
) -> Result<(), String> {
    let plugin = report
        .plugin
        .ok_or_else(|| format_plugin_diagnostics(&report.diagnostics))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let connected = runtime
        .block_on(incurs::agent_plugin_runtime::connect_agent_plugin(
            &plugin,
            &incurs::agent_plugin_runtime::AgentPluginRuntimeOptions::default(),
        ))
        .map_err(|error| error.to_string())?;
    let servers = connected
        .servers
        .iter()
        .map(|server| {
            json!({
                "error": server.error,
                "name": server.name,
                "toolCount": server.tool_count,
                "transport": plugin_transport_name(server.transport),
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "diagnostics": report.diagnostics.iter().map(plugin_diagnostic_json).collect::<Vec<_>>(),
            "name": plugin.manifest.name,
            "servers": servers,
            "tools": connected.catalog.definitions(),
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn print_plugin_call(
    report: incurs::agent_plugin::loader::AgentPluginLoadReport,
    tool: String,
    arguments: BTreeMap<String, Value>,
) -> Result<(), String> {
    let plugin = report
        .plugin
        .ok_or_else(|| format_plugin_diagnostics(&report.diagnostics))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let outcome = runtime.block_on(async {
        let connected = incurs::agent_plugin_runtime::connect_agent_plugin(
            &plugin,
            &incurs::agent_plugin_runtime::AgentPluginRuntimeOptions::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
        if connected
            .servers
            .iter()
            .all(|server| server.error.is_some())
        {
            return Err(connected
                .servers
                .iter()
                .filter_map(|server| server.error.as_deref())
                .collect::<Vec<_>>()
                .join("; "));
        }
        Ok(connected
            .catalog
            .call(&tool, arguments, incurs::tool::ToolCallOptions::isolated())
            .await)
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&outcome).map_err(|error| error.to_string())?
    );
    match outcome {
        incurs::tool::ToolCallOutcome::Ok { .. } => Ok(()),
        incurs::tool::ToolCallOutcome::Error { message, .. } => Err(message),
    }
}

fn plugin_diagnostic_json(
    diagnostic: &incurs::agent_plugin::loader::AgentPluginDiagnostic,
) -> Value {
    json!({
        "code": diagnostic.code,
        "message": diagnostic.message,
        "path": diagnostic.path,
        "severity": match diagnostic.severity {
            incurs::agent_plugin::loader::AgentPluginDiagnosticSeverity::Info => "info",
            incurs::agent_plugin::loader::AgentPluginDiagnosticSeverity::Warning => "warning",
            incurs::agent_plugin::loader::AgentPluginDiagnosticSeverity::Error => "error",
        },
    })
}

fn format_plugin_diagnostics(
    diagnostics: &[incurs::agent_plugin::loader::AgentPluginDiagnostic],
) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn plugin_transport_name(
    transport: incurs::agent_plugin::loader::AgentPluginMcpTransport,
) -> &'static str {
    match transport {
        incurs::agent_plugin::loader::AgentPluginMcpTransport::Stdio => "stdio",
        incurs::agent_plugin::loader::AgentPluginMcpTransport::StreamableHttp => "streamable-http",
        incurs::agent_plugin::loader::AgentPluginMcpTransport::Sse => "sse",
    }
}

fn parse_gen(args: &[String]) -> Result<GenOptions, String> {
    let mut options = GenOptions {
        dir: PathBuf::from("."),
        entry: None,
        output: None,
        json_output: None,
        config_schema: false,
        plugin_output: None,
        plugin_skill_depth: 1,
        plugin_mcp: true,
        plugin_bundle_cli: false,
        plugin_force: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                println!(
                    "Generate Rust command types and JSON manifests.\n\nUsage: incurs gen [options]\n\nOptions:\n  --dir <path>                   Cargo project root (default: .)\n  --entry <name|path>            Cargo binary name or executable path\n  --output <path>                Rust output (default: src/incurs_generated.rs)\n  --json-output <path>           JSON output (default: incurs.manifest.json)\n  --config-schema                Also generate config.schema.json\n  --plugin-output <path>         Also build an Agent Plugins 1.0 directory through the target CLI\n  --plugin-skill-depth <n>       Skill grouping depth (default: 1)\n  --plugin-bundle-cli            Bundle the target CLI as the plugin Tool Runtime\n  --plugin-no-mcp                Skip mcp.json generation\n  --plugin-force                 Replace existing owned plugin artifacts\n"
                );
                return Ok(options);
            }
            "--config-schema" => options.config_schema = true,
            "--plugin-bundle-cli" => options.plugin_bundle_cli = true,
            "--plugin-no-mcp" => options.plugin_mcp = false,
            "--plugin-force" => options.plugin_force = true,
            "--dir"
            | "--entry"
            | "--output"
            | "--json-output"
            | "--plugin-output"
            | "--plugin-skill-depth" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--dir" => options.dir = PathBuf::from(value),
                    "--entry" => options.entry = Some(value.clone()),
                    "--output" => options.output = Some(PathBuf::from(value)),
                    "--json-output" => options.json_output = Some(PathBuf::from(value)),
                    "--plugin-output" => options.plugin_output = Some(PathBuf::from(value)),
                    "--plugin-skill-depth" => {
                        options.plugin_skill_depth = value
                            .parse::<usize>()
                            .map_err(|_| format!("invalid value for {flag}: {value}"))?
                    }
                    _ => unreachable!(),
                }
            }
            flag => return Err(format!("unknown option: {flag}")),
        }
        index += 1;
    }
    Ok(options)
}

fn generate(options: GenOptions) -> Result<(), String> {
    let dir = options
        .dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", options.dir.display()))?;
    let manifest = load_manifest(&dir, options.entry.as_deref())?;
    validate_manifest(&manifest)?;

    let rust_output = resolve_output(
        &dir,
        options.output,
        PathBuf::from("src/incurs_generated.rs"),
    );
    let json_output = resolve_output(
        &dir,
        options.json_output,
        PathBuf::from("incurs.manifest.json"),
    );
    write(&rust_output, &rust_source(&manifest)?)?;
    write(
        &json_output,
        &(serde_json::to_string_pretty(&canonicalize(manifest.clone()))
            .map_err(|error| error.to_string())?
            + "\n"),
    )?;

    let mut result = Map::from_iter([
        ("dir".to_string(), json!(dir)),
        ("output".to_string(), json!(rust_output)),
        ("manifest".to_string(), json!(json_output)),
    ]);
    if options.config_schema {
        let output = run_target(
            &dir,
            options.entry.as_deref(),
            &["--config-schema", "--format", "json"],
        )?;
        if !output.status.success() {
            return Err(format!(
                "target could not generate config schema: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let schema: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("target returned invalid config schema: {error}"))?;
        let schema_output = dir.join("config.schema.json");
        write(
            &schema_output,
            &(serde_json::to_string_pretty(&canonicalize(schema))
                .map_err(|error| error.to_string())?
                + "\n"),
        )?;
        result.insert("configSchema".to_string(), json!(schema_output));
    }

    if let Some(plugin_output) = options.plugin_output {
        let plugin_root = resolve_output(&dir, Some(plugin_output), PathBuf::from("agent-plugin"));
        let mut args = vec![
            "plugin".to_string(),
            "build".to_string(),
            "--output".to_string(),
            plugin_root.to_string_lossy().to_string(),
            "--depth".to_string(),
            options.plugin_skill_depth.to_string(),
        ];
        if !options.plugin_mcp {
            args.push("--no-mcp".to_string());
        }
        if options.plugin_bundle_cli {
            args.push("--bundle-cli".to_string());
        }
        if options.plugin_force {
            args.push("--force".to_string());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_target(&dir, options.entry.as_deref(), &arg_refs)?;
        if !output.status.success() {
            return Err(format!(
                "target could not build Agent Plugin: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        result.insert("plugin".to_string(), json!(plugin_root));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(result)).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn resolve_output(dir: &Path, value: Option<PathBuf>, default: PathBuf) -> PathBuf {
    let output = value.unwrap_or(default);
    if output.is_absolute() {
        output
    } else {
        dir.join(output)
    }
}

fn load_manifest(dir: &Path, entry: Option<&str>) -> Result<Value, String> {
    let output = run_target(dir, entry, &["--llms-full", "--format", "json"])?;
    if !output.status.success() {
        return Err(format!(
            "target could not export its command manifest: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("target returned an invalid command manifest: {error}"))
}

fn run_target(dir: &Path, entry: Option<&str>, args: &[&str]) -> Result<Output, String> {
    if let Some(entry) = entry {
        let path = Path::new(entry);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            dir.join(path)
        };
        if path.is_file() {
            return Command::new(path)
                .current_dir(dir)
                .args(args)
                .output()
                .map_err(|error| error.to_string());
        }
    }

    let manifest = dir.join("Cargo.toml");
    let bin = match entry {
        Some(entry) => entry.to_string(),
        None => discover_bin(&manifest)?,
    };
    Command::new("cargo")
        .current_dir(dir)
        .args([
            "run",
            "--quiet",
            "--manifest-path",
            manifest
                .to_str()
                .ok_or_else(|| "Cargo.toml path is not UTF-8".to_string())?,
            "--bin",
            &bin,
            "--",
        ])
        .args(args)
        .output()
        .map_err(|error| error.to_string())
}

fn discover_bin(manifest: &Path) -> Result<String, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            manifest
                .to_str()
                .ok_or_else(|| "Cargo.toml path is not UTF-8".to_string())?,
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let manifest = manifest
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
    let mut bins = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|path| Path::new(path).canonicalize().ok())
                .as_ref()
                == Some(&manifest)
        })
        .flat_map(|package| package["targets"].as_array().into_iter().flatten())
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| target["name"].as_str().map(ToString::to_string))
        .collect::<Vec<_>>();
    bins.sort();
    match bins.as_slice() {
        [bin] => Ok(bin.clone()),
        [] => Err("no binary target found; pass --entry <bin>".to_string()),
        _ => Err(format!(
            "multiple binary targets found ({}); pass --entry <bin>",
            bins.join(", ")
        )),
    }
}

fn validate_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["version"] != "incur.v1" {
        return Err("target manifest version must be incur.v1".to_string());
    }
    if !manifest["commands"].is_array() {
        return Err("target manifest must contain a commands array".to_string());
    }
    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
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

fn rust_source(manifest: &Value) -> Result<String, String> {
    let commands = manifest["commands"]
        .as_array()
        .ok_or_else(|| "manifest commands must be an array".to_string())?;
    let manifest_json = serde_json::to_string(&canonicalize(manifest.clone()))
        .map_err(|error| error.to_string())?;
    let mut source = String::from(
        "// @generated by `incurs gen`; do not edit.\n\n\
         /// Canonical command manifest used to generate this module.\n",
    );
    source.push_str(&format!(
        "pub const MANIFEST_JSON: &str = {:?};\n\n",
        manifest_json
    ));
    source.push_str(
        "fn quote(value: &str) -> String {\n    if value.is_empty() || value.chars().any(char::is_whitespace) {\n        format!(\"{:?}\", value)\n    } else {\n        value.to_string()\n    }\n}\n\n",
    );

    for command in commands {
        let name = command["name"]
            .as_str()
            .ok_or_else(|| "command name must be a string".to_string())?;
        let module = rust_ident(&name.replace(' ', "_"));
        source.push_str(&format!(
            "/// Typed CTA helpers for `{name}`.\npub mod {module} {{\n"
        ));
        source.push_str(&format!(
            "    /// Canonical command name.\n    pub const NAME: &str = {name:?};\n\n"
        ));
        source.push_str("    /// Positional arguments for this command.\n    #[derive(Clone, Debug, Default)]\n    pub struct Args {\n");
        append_fields(&mut source, command.pointer("/schema/args"), true);
        source.push_str("    }\n\n    /// Named options for this command.\n    #[derive(Clone, Debug, Default)]\n    pub struct Options {\n");
        append_fields(&mut source, command.pointer("/schema/options"), false);
        source.push_str("    }\n\n");
        source.push_str("    /// Renders this typed invocation as a CTA entry.\n    pub fn cta(args: &Args, options: &Options) -> incurs::output::CtaEntry {\n        let mut parts = vec![NAME.to_string()];\n");
        append_render(&mut source, command.pointer("/schema/args"), true);
        append_render(&mut source, command.pointer("/schema/options"), false);
        source
            .push_str("        incurs::output::CtaEntry::Simple(parts.join(\" \"))\n    }\n}\n\n");
    }
    Ok(source)
}

fn append_fields(source: &mut String, schema: Option<&Value>, _positional: bool) {
    let required = schema
        .and_then(|schema| schema["required"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if let Some(properties) = schema.and_then(|schema| schema["properties"].as_object()) {
        for (name, property) in properties {
            let ident = rust_field_ident(name);
            let ty = rust_type(property);
            let optional = !required.contains(&name.as_str());
            let ty = if optional {
                format!("Option<{ty}>")
            } else {
                ty
            };
            source.push_str(&format!(
                "        /// Value for `{name}`.\n        pub {ident}: {ty},\n"
            ));
        }
    }
}

fn append_render(source: &mut String, schema: Option<&Value>, positional: bool) {
    let required = schema
        .and_then(|schema| schema["required"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if let Some(properties) = schema.and_then(|schema| schema["properties"].as_object()) {
        for (name, property) in properties {
            let ident = rust_field_ident(name);
            let is_required = required.contains(&name.as_str()) && positional;
            let render = render_value("value", property);
            if positional {
                if property["type"] == "array" {
                    let values = if is_required {
                        format!("&args.{ident}")
                    } else {
                        format!("args.{ident}.as_deref().unwrap_or_default()")
                    };
                    source.push_str(&format!("        for value in {values} {{ parts.push(super::quote(&value.to_string())); }}\n"));
                } else if is_required {
                    let render = render_value(&format!("args.{ident}"), property);
                    source.push_str(&format!("        parts.push(super::quote(&{render}));\n"));
                } else {
                    source.push_str(&format!("        if let Some(value) = &args.{ident} {{ parts.push(super::quote(&{render})); }}\n"));
                }
            } else if property["type"] == "boolean" {
                let condition = if required.contains(&name.as_str()) {
                    format!("options.{ident}")
                } else {
                    format!("options.{ident} == Some(true)")
                };
                source.push_str(&format!(
                    "        if {condition} {{ parts.push(\"--{}\".to_string()); }}\n",
                    kebab(name)
                ));
            } else if property["type"] == "array" {
                let values = if required.contains(&name.as_str()) {
                    format!("&options.{ident}")
                } else {
                    format!("options.{ident}.as_deref().unwrap_or_default()")
                };
                source.push_str(&format!("        for value in {values} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&value.to_string())); }}\n", kebab(name)));
            } else if required.contains(&name.as_str()) {
                let render = render_value(&format!("options.{ident}"), property);
                source.push_str(&format!("        parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render}));\n", kebab(name)));
            } else {
                source.push_str(&format!("        if let Some(value) = &options.{ident} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render})); }}\n", kebab(name)));
            }
        }
    }
}

fn rust_type(schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("boolean") => "bool".to_string(),
        Some("integer") => "i64".to_string(),
        Some("number") => "f64".to_string(),
        Some("array") => format!("Vec<{}>", rust_type(&schema["items"])),
        _ => "String".to_string(),
    }
}

fn render_value(value: &str, schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("string") | None => format!("{value}.to_string()"),
        Some("array") => {
            format!("{value}.iter().map(ToString::to_string).collect::<Vec<_>>().join(\",\")")
        }
        _ => format!("{value}.to_string()"),
    }
}

fn rust_ident(value: &str) -> String {
    let mut result = value
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if character.is_ascii_alphanumeric() || character == '_' {
                if index == 0 && character.is_ascii_digit() {
                    format!("_{character}")
                } else {
                    character.to_string()
                }
            } else {
                "_".to_string()
            }
        })
        .collect::<String>();
    if [
        "type", "match", "mod", "self", "crate", "super", "use", "pub", "fn",
    ]
    .contains(&result.as_str())
    {
        result = format!("r#{result}");
    }
    result
}

fn rust_field_ident(value: &str) -> String {
    rust_ident(&kebab(value).replace('-', "_"))
}

fn kebab(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('-');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_contains_typed_command_modules() {
        let manifest = json!({
            "version": "incur.v1",
            "commands": [{
                "name": "user create",
                "schema": {
                    "args": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    },
                    "options": {
                        "type": "object",
                        "properties": { "dryRun": { "type": "boolean" } }
                    }
                }
            }]
        });
        let source = rust_source(&manifest).unwrap();
        assert!(source.contains("pub mod user_create"));
        assert!(source.contains("pub name: String"));
        assert!(source.contains("pub dry_run: Option<bool>"));
        assert!(source.contains("--dry-run"));
        syn::parse_file(&source).unwrap();
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let value = canonicalize(json!({ "z": 1, "a": { "z": 2, "a": 3 } }));
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"a":{"a":3,"z":2},"z":1}"#
        );
    }

    #[test]
    fn parse_gen_accepts_agent_plugin_options() {
        let args = vec![
            "--plugin-output".to_string(),
            "dist/plugin".to_string(),
            "--plugin-skill-depth".to_string(),
            "2".to_string(),
            "--plugin-no-mcp".to_string(),
            "--plugin-bundle-cli".to_string(),
            "--plugin-force".to_string(),
        ];

        let options = parse_gen(&args).unwrap();

        assert_eq!(options.plugin_output, Some(PathBuf::from("dist/plugin")));
        assert_eq!(options.plugin_skill_depth, 2);
        assert!(!options.plugin_mcp);
        assert!(options.plugin_bundle_cli);
        assert!(options.plugin_force);
    }

    #[test]
    fn parse_gen_rejects_removed_agent_plugin_metadata_options() {
        let error = parse_gen(&["--plugin-name".to_string(), "demo".to_string()]).unwrap_err();

        assert_eq!(error, "unknown option: --plugin-name");
    }

    #[test]
    fn parse_plugin_requires_a_dedicated_data_directory() {
        let error = parse_plugin(&["validate".to_string(), "./demo".to_string()]).unwrap_err();

        assert_eq!(error, "missing required --data-dir <path>");
    }

    #[test]
    fn parse_plugin_accepts_validate_and_tools_actions() {
        for (action, expected) in [
            ("validate", PluginAction::Validate),
            ("tools", PluginAction::Tools),
        ] {
            let options = parse_plugin(&[
                action.to_string(),
                "./demo".to_string(),
                "--data-dir".to_string(),
                "./data".to_string(),
            ])
            .unwrap();

            assert_eq!(options.root, PathBuf::from("./demo"));
            assert_eq!(options.data_dir, PathBuf::from("./data"));
            assert_eq!(options.action, expected);
            assert!(options.tool.is_none());
            assert!(options.arguments.is_empty());
        }
    }

    #[test]
    fn parse_plugin_accepts_call_arguments() {
        let options = parse_plugin(&[
            "call".to_string(),
            "./demo".to_string(),
            "server_ping".to_string(),
            "--arguments".to_string(),
            r#"{"message":"hello"}"#.to_string(),
            "--data-dir".to_string(),
            "./data".to_string(),
        ])
        .unwrap();

        assert_eq!(options.action, PluginAction::Call);
        assert_eq!(options.tool.as_deref(), Some("server_ping"));
        assert_eq!(options.arguments["message"], "hello");
    }

    #[test]
    fn parse_plugin_install_accepts_managed_locations_and_force() {
        let options = parse_plugin_install(
            &[
                "./demo".to_string(),
                "--bin-dir".to_string(),
                "./commands".to_string(),
                "--force".to_string(),
            ],
            PathBuf::from("./data-home"),
            PathBuf::from("./default-bin"),
        )
        .unwrap();

        assert_eq!(options.source, PathBuf::from("./demo"));
        assert_eq!(options.data_home, PathBuf::from("./data-home"));
        assert_eq!(options.bin_dir, PathBuf::from("./commands"));
        assert!(options.force);
    }

    #[test]
    fn parse_plugin_uninstall_accepts_purge() {
        let options = parse_plugin_uninstall(
            &["demo-tools".to_string(), "--purge".to_string()],
            PathBuf::from("./data-home"),
        )
        .unwrap();

        assert_eq!(options.name, "demo-tools");
        assert_eq!(options.data_home, PathBuf::from("./data-home"));
        assert!(options.purge);
    }
}
