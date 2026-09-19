//! Turning host configuration documents into normalized server entries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{DiscoveryDiagnostic, DiscoveryError};
use crate::hosts::{ConfigEncoding, ConfigLocation, ConfigScope, McpConfigSource};
use crate::secret::EnvValue;
use crate::transport::{
    AuthClass, HttpTransport, McpTransport, StdioTransport, canonical_url, normalize_path,
    resolve_program,
};
use crate::{DiscoveredMcpServer, identity};

/// Strips comments and trailing commas from a JSONC document.
///
/// VS Code documents its configuration as JSONC and real files use both, so a
/// strict parser rejects otherwise valid configuration. String literals and
/// their escapes are respected, so a `//` inside a value survives.
pub fn strip_jsonc(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                let _ = chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(ch),
        }
    }
    strip_trailing_commas(&out)
}

/// Removes a comma that immediately precedes a closing brace or bracket.
fn strip_trailing_commas(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escaped = false;
    let bytes: Vec<char> = source.chars().collect();
    for (index, &ch) in bytes.iter().enumerate() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let next = bytes[index + 1..]
                .iter()
                .find(|candidate| !candidate.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Reads one configuration file into a JSON document.
fn read_document(location: &ConfigLocation) -> Result<Value, DiscoveryError> {
    let text = std::fs::read_to_string(&location.path).map_err(|source| DiscoveryError::Read {
        path: location.path.clone(),
        source,
    })?;
    match location.encoding {
        ConfigEncoding::Json => {
            serde_json::from_str(&text).map_err(|source| DiscoveryError::Json {
                path: location.path.clone(),
                source,
            })
        }
        ConfigEncoding::Jsonc => {
            serde_json::from_str(&strip_jsonc(&text)).map_err(|source| DiscoveryError::Json {
                path: location.path.clone(),
                source,
            })
        }
        ConfigEncoding::Toml => {
            let parsed: toml::Value =
                toml::from_str(&text).map_err(|source| DiscoveryError::Toml {
                    path: location.path.clone(),
                    source,
                })?;
            serde_json::to_value(parsed).map_err(|source| DiscoveryError::Json {
                path: location.path.clone(),
                source,
            })
        }
    }
}

/// Escapes one JSON Pointer path segment.
fn escape_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Coerces a configured value into the string a subprocess environment needs.
///
/// Real configuration stores numbers and booleans here. Declaring the map as
/// `BTreeMap<String, String>` would fail the whole document and silently cost a
/// developer every server in it, so scalars are coerced and reported. Objects
/// and arrays have no textual meaning and are rejected per entry.
fn coerce_env_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Parses an `env` or `headers` overlay, classifying each value.
#[allow(clippy::too_many_arguments)]
fn parse_overlay(
    raw: Option<&Value>,
    pointer: &str,
    field: &str,
    source: &McpConfigSource,
    paths: &crate::HostPaths,
    unresolved: &mut Vec<String>,
    diagnostics: &mut Vec<DiscoveryDiagnostic>,
) -> BTreeMap<String, EnvValue> {
    let mut out = BTreeMap::new();
    let Some(Value::Object(map)) = raw else {
        return out;
    };
    for (key, value) in map {
        match coerce_env_value(value) {
            Some(text) => {
                if !matches!(value, Value::String(_)) {
                    diagnostics.push(DiscoveryDiagnostic::warning(
                        "mcp_env_value_coerced",
                        format!("{pointer}/{field}/{}", escape_pointer(key)),
                        "value was not a string and was coerced to its textual form",
                    ));
                }
                let (expanded, missing) = expand(&text, source, paths);
                unresolved.extend(missing);
                out.insert(key.clone(), EnvValue::classify(key, expanded));
            }
            None => diagnostics.push(DiscoveryDiagnostic::warning(
                "mcp_env_value_invalid",
                format!("{pointer}/{field}/{}", escape_pointer(key)),
                "value was not a scalar and was ignored",
            )),
        }
    }
    out
}

/// Finds every `${...}` placeholder in a value.
fn placeholders(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        out.push(rest[start + 2..start + end].to_string());
        rest = &rest[start + end + 1..];
    }
    out
}

/// Expands the placeholders that can be resolved without a person present.
///
/// `${env:NAME}`, `${userHome}`, and `${workspaceFolder}` all have a referent
/// here. `${input:ID}` does not: it names a prompt, and substituting a partial
/// value would launch a server with the literal text as its credential. Those
/// are reported instead, so the entry appears in diagnostics rather than
/// vanishing.
fn expand(
    value: &str,
    source: &McpConfigSource,
    paths: &crate::HostPaths,
) -> (String, Vec<String>) {
    let mut unresolved = Vec::new();
    let mut out = value.to_string();
    for placeholder in placeholders(value) {
        let replacement = if let Some(name) = placeholder.strip_prefix("env:") {
            paths.env.get(name).cloned()
        } else if placeholder == "userHome" {
            Some(paths.home.to_string_lossy().to_string())
        } else if placeholder == "workspaceFolder" {
            match &source.scope {
                ConfigScope::Project { root } => Some(root.to_string_lossy().to_string()),
                ConfigScope::User => None,
            }
        } else if placeholder == "pathSeparator" {
            Some(std::path::MAIN_SEPARATOR.to_string())
        } else {
            None
        };
        match replacement {
            Some(text) => out = out.replace(&format!("${{{placeholder}}}"), &text),
            None => unresolved.push(placeholder),
        }
    }
    (out, unresolved)
}

/// Parses one server entry into a normalized transport.
///
/// Returns `Err` with a diagnostic when the entry alone is unusable, so the
/// caller can keep every other entry in the same file.
#[allow(clippy::too_many_lines)]
fn parse_entry(
    name: &str,
    entry: &Value,
    pointer: &str,
    source: &McpConfigSource,
    paths: &crate::HostPaths,
    diagnostics: &mut Vec<DiscoveryDiagnostic>,
) -> Result<(McpTransport, bool, Vec<String>), DiscoveryDiagnostic> {
    let Value::Object(entry) = entry else {
        return Err(DiscoveryDiagnostic::error(
            "mcp_server_not_object",
            pointer,
            "server entry was not an object",
        ));
    };

    // An entry is disabled when the host says so. Codex sets this in practice,
    // and honouring it is the difference between exposing a server a developer
    // turned off and respecting their decision.
    let enabled = entry
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let declared = entry.get("type").and_then(Value::as_str);
    let mut unresolved: Vec<String> = Vec::new();

    let resolve = |raw: &str, out: &mut Vec<String>| {
        let (text, missing) = expand(raw, source, paths);
        out.extend(missing);
        text
    };

    // A URL selects an HTTP transport even when `type` is absent, and hosts
    // write both "http" and "streamable-http" for the same thing.
    if let Some(url) = entry.get("url").and_then(Value::as_str) {
        let url = resolve(url, &mut unresolved);
        let headers = parse_overlay(
            entry.get("headers"),
            pointer,
            "headers",
            source,
            paths,
            &mut unresolved,
            diagnostics,
        );
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(DiscoveryDiagnostic::error(
                "mcp_bad_url",
                pointer,
                "url was not an http or https endpoint",
            ));
        }
        let http = HttpTransport {
            canonical_url: canonical_url(&url),
            auth_class: AuthClass::derive(&headers, &url),
            url,
            headers,
        };
        let transport = if declared == Some("sse") {
            McpTransport::Sse(http)
        } else {
            McpTransport::StreamableHttp(http)
        };
        return Ok((transport, enabled, unresolved));
    }

    // OpenCode writes argv as an array whose first element is the program.
    let (command, mut args) = match entry.get("command") {
        Some(Value::String(command)) => (command.clone(), Vec::new()),
        Some(Value::Array(argv)) => {
            let mut parts = argv.iter().filter_map(Value::as_str).map(str::to_string);
            match parts.next() {
                Some(command) => (command, parts.collect()),
                None => {
                    return Err(DiscoveryDiagnostic::error(
                        "mcp_missing_command",
                        pointer,
                        "command array was empty",
                    ));
                }
            }
        }
        _ => {
            return Err(DiscoveryDiagnostic::error(
                "mcp_missing_command",
                pointer,
                "entry declared neither a command nor a url",
            ));
        }
    };

    if let Some(Value::Array(declared_args)) = entry.get("args") {
        args.extend(
            declared_args
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    let command = resolve(&command, &mut unresolved);
    let args: Vec<String> = args
        .iter()
        .map(|arg| resolve(arg, &mut unresolved))
        .collect();

    // Some hosts name the overlay `environment` rather than `env`.
    let mut env_unresolved = Vec::new();
    let env = parse_overlay(
        entry.get("env").or_else(|| entry.get("environment")),
        pointer,
        "env",
        source,
        paths,
        &mut env_unresolved,
        diagnostics,
    );
    unresolved.extend(env_unresolved);

    let cwd = entry.get("cwd").and_then(Value::as_str).map(str::to_string);
    let resolved_cwd = cwd.as_ref().map(|value| {
        let expanded = resolve(value, &mut unresolved);
        let path = normalize_path(&expanded, &paths.home);
        if path.is_absolute() {
            path
        } else {
            // A relative cwd has no documented base; resolving it against the
            // declaring file's directory keeps it auditable.
            normalize_path(&source.base_dir.join(&path).to_string_lossy(), &paths.home)
        }
    });

    let (program, server_args) = resolve_program(&command, &args, &paths.home);
    let _ = name;
    Ok((
        McpTransport::Stdio(StdioTransport {
            command,
            program,
            args,
            server_args,
            env,
            cwd,
            resolved_cwd,
        }),
        enabled,
        unresolved,
    ))
}

/// Looks up the server map inside a parsed document.
fn server_map<'a>(document: &'a Value, keys: &[&str]) -> Option<(&'a Map<String, Value>, String)> {
    for key in keys {
        if let Some(Value::Object(map)) = document.get(*key) {
            return Some((map, (*key).to_string()));
        }
    }
    None
}

/// Reads one configuration file into normalized server entries.
///
/// A failure to read or parse the file is returned as an error. A failure that
/// concerns one entry is recorded in `diagnostics` and the other entries in the
/// same file are still returned.
pub fn parse_location(
    location: &ConfigLocation,
    paths: &crate::HostPaths,
    diagnostics: &mut Vec<DiscoveryDiagnostic>,
) -> Result<Vec<DiscoveredMcpServer>, DiscoveryError> {
    let document = read_document(location)?;
    let base_dir = location
        .path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

    let mut out = Vec::new();
    let mut groups: Vec<(&Map<String, Value>, String, ConfigScope)> = Vec::new();

    if let Some((map, key)) = server_map(&document, location.map_keys) {
        if key != location.map_keys[0] {
            diagnostics.push(DiscoveryDiagnostic::warning(
                "mcp_legacy_map_key",
                location.path.to_string_lossy(),
                "server map used a key this format does not document",
            ));
        }
        groups.push((
            map,
            format!("/{}", escape_pointer(&key)),
            location.scope.clone(),
        ));
    }

    // Claude Code nests a second map per project directory.
    if location.discovery == crate::hosts::McpDiscoverySource::ClaudeCodeUser
        && let Some(Value::Object(projects)) = document.get("projects")
    {
        for (root, project) in projects {
            if let Some(Value::Object(map)) = project.get("mcpServers") {
                groups.push((
                    map,
                    format!("/projects/{}/mcpServers", escape_pointer(root)),
                    ConfigScope::Project {
                        root: PathBuf::from(root),
                    },
                ));
            }
        }
    }

    for (map, map_pointer, scope) in groups {
        let discovery = if matches!(scope, ConfigScope::Project { .. })
            && location.discovery == crate::hosts::McpDiscoverySource::ClaudeCodeUser
        {
            crate::hosts::McpDiscoverySource::ClaudeCodeProject
        } else {
            location.discovery
        };
        for (name, entry) in map {
            let pointer = format!("{map_pointer}/{}", escape_pointer(name));
            let source = McpConfigSource {
                discovery,
                scope: scope.clone(),
                path: location.path.clone(),
                pointer: pointer.clone(),
                base_dir: base_dir.clone(),
            };
            match parse_entry(name, entry, &pointer, &source, paths, diagnostics) {
                Ok((transport, enabled, requires_input)) => {
                    let id = identity::server_id(&transport);
                    let fingerprint = identity::config_fingerprint(&id, &transport, &[]);
                    if !requires_input.is_empty() {
                        diagnostics.push(DiscoveryDiagnostic::error(
                            "mcp_unresolved_placeholder",
                            &pointer,
                            "entry needs a value that cannot be resolved without a person",
                        ));
                    }
                    out.push(DiscoveredMcpServer {
                        id,
                        fingerprint,
                        local_name: name.clone(),
                        source,
                        transport,
                        enabled,
                        requires_input,
                    });
                }
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
    }
    Ok(out)
}
