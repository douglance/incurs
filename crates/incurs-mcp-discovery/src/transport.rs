//! How a discovered MCP server is reached, normalized for stable identity.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::secret::EnvValue;

/// A lexically normalized program token.
///
/// Discovery never performs a `PATH` lookup, so a bare name stays a bare name.
/// Resolving would make identity depend on machine state at scan time: `which
/// npx` differs between an nvm-activated shell and a login shell, and changes
/// after a runtime upgrade. An identity that flaps would churn namespaces and
/// discard the tool cache for no semantic reason.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ProgramToken {
    /// A bare executable name, resolved from `PATH` at launch time.
    Name {
        /// The token exactly as written.
        value: String,
    },
    /// A filesystem path, `~`-expanded and lexically normalized.
    ///
    /// Not canonicalized: symlink resolution touches the filesystem and would
    /// make identity depend on what happens to be installed.
    Path {
        /// The normalized path.
        value: PathBuf,
    },
    /// A package launched through a known runner such as `npx` or `uvx`.
    Package {
        /// Ecosystem the runner belongs to.
        ecosystem: String,
        /// Package name with any version specifier removed.
        name: String,
    },
}

/// A local subprocess MCP binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioTransport {
    /// Program token exactly as written in the host configuration.
    pub command: String,
    /// Program token after launcher stripping and lexical normalization.
    pub program: ProgramToken,
    /// Arguments exactly as written.
    pub args: Vec<String>,
    /// Arguments after launcher stripping: the server's own argv.
    pub server_args: Vec<String>,
    /// Environment overlay, each value classified and credentials held opaque.
    pub env: BTreeMap<String, EnvValue>,
    /// Working directory as written, if any.
    pub cwd: Option<String>,
    /// Working directory resolved against the declaring file's directory.
    pub resolved_cwd: Option<PathBuf>,
}

/// A remote HTTP MCP binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransport {
    /// Endpoint exactly as written.
    pub url: String,
    /// Endpoint with userinfo, fragment, and secret query values removed.
    pub canonical_url: String,
    /// Header overlay, each value classified and credentials held opaque.
    pub headers: BTreeMap<String, EnvValue>,
    /// Authentication identity, derived from names and schemes only.
    pub auth_class: AuthClass,
}

/// How a remote server is authenticated, derived without reading a credential.
///
/// Two hosts configuring the same server with two different tokens produce the
/// same `AuthClass` and therefore collapse to one server. The same URL with and
/// without authentication does not collapse, because an unauthenticated
/// connection lists a different tool set and would poison the schema cache.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthClass(pub String);

impl AuthClass {
    /// Derives the authentication class from header names and scheme tokens.
    pub fn derive(headers: &BTreeMap<String, EnvValue>, url: &str) -> Self {
        let mut parts: Vec<String> = Vec::new();
        for (name, value) in headers {
            let lower = name.to_ascii_lowercase();
            if lower == "authorization" {
                parts.push(format!(
                    "header:authorization:{}",
                    scheme_of(value.expose())
                ));
            } else {
                parts.push(format!("header:{lower}"));
            }
        }
        if has_userinfo(url) {
            parts.push("url-userinfo".to_string());
        }
        for key in secret_query_keys(url) {
            parts.push(format!("query:{key}"));
        }
        if parts.is_empty() {
            return Self("none".to_string());
        }
        parts.sort();
        parts.dedup();
        Self(parts.join("+"))
    }
}

/// Authentication schemes recognised by name.
const KNOWN_SCHEMES: &[&str] = &["bearer", "basic", "token", "dpop"];

/// Returns the lowercased scheme token of an `Authorization` value.
///
/// Only the scheme is read. The credential after it is never inspected, hashed,
/// or recorded.
fn scheme_of(value: &str) -> String {
    let token = value.split_whitespace().next().unwrap_or_default();
    let lower = token.to_ascii_lowercase();
    let well_formed = !lower.is_empty()
        && lower.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && lower
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'));
    if well_formed && KNOWN_SCHEMES.contains(&lower.as_str()) {
        lower
    } else {
        "other".to_string()
    }
}

/// Returns whether a URL carries `user:password@` before its host.
fn has_userinfo(url: &str) -> bool {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.contains('@')
}

/// Query parameter names that name a credential.
fn secret_query_keys(url: &str) -> Vec<String> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = query.split('#').next().unwrap_or_default();
    query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .filter(|key| !key.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|key| crate::secret::classify_key(key) == crate::secret::EnvClass::Secret)
        .collect()
}

/// How a discovered MCP server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// A local subprocess speaking MCP over stdio.
    Stdio(StdioTransport),
    /// A Streamable HTTP endpoint, MCP 2025-03-26 and later.
    StreamableHttp(HttpTransport),
    /// A legacy HTTP and SSE endpoint, MCP 2024-11-05.
    Sse(HttpTransport),
}

impl McpTransport {
    /// Returns the discriminator used in the identity document.
    ///
    /// `Sse` and `StreamableHttp` share one discriminator on purpose. When a
    /// host migrates a server from one to the other at the same URL, the two
    /// entries describe one server and must collapse; when they are genuinely
    /// different servers the URLs differ and already separate them.
    pub fn identity_kind(&self) -> &'static str {
        match self {
            Self::Stdio(_) => "stdio",
            Self::StreamableHttp(_) | Self::Sse(_) => "http",
        }
    }
}

/// Package runners whose own flags carry no server identity.
const RUNNERS: &[(&str, &str)] = &[
    ("npx", "npm"),
    ("npx.cmd", "npm"),
    ("bunx", "npm"),
    ("uvx", "pypi"),
    ("dlx", "npm"),
];

/// Runner flags that select behavior rather than a package.
const RUNNER_FLAGS: &[&str] = &["-y", "--yes", "-q", "--quiet", "--silent"];

/// Runner flags that take a value, which is also dropped.
const RUNNER_FLAGS_WITH_VALUE: &[&str] = &["--loglevel", "--package", "-p"];

/// Splits a package specifier into its name and optional version.
///
/// The split takes the last `@` that is not at index 0, so a scoped package such
/// as `@scope/name@1.2.3` yields `@scope/name`.
fn split_package(spec: &str) -> (String, Option<String>) {
    match spec.rfind('@') {
        Some(at) if at > 0 => (spec[..at].to_string(), Some(spec[at + 1..].to_string())),
        _ => (spec.to_string(), None),
    }
}

/// Expands a leading `~/` and lexically normalizes `.` and `..`.
pub fn normalize_path(value: &str, home: &Path) -> PathBuf {
    let expanded = if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(value)
    };
    let mut out = PathBuf::new();
    for part in expanded.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Classifies a command token, stripping a package runner when present.
///
/// Returns the program token and the server's own argv. `npx -y foo@latest bar`
/// and `npx foo bar` both yield the package `foo` with argv `["bar"]`, so a
/// version bump or a `-y` in one host's config does not split identity.
pub fn resolve_program(command: &str, args: &[String], home: &Path) -> (ProgramToken, Vec<String>) {
    let base = Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| command.to_string());

    if let Some((_, ecosystem)) = RUNNERS.iter().find(|(runner, _)| *runner == base) {
        let mut rest = args.iter();
        let mut spec: Option<String> = None;
        let mut server_args: Vec<String> = Vec::new();
        while let Some(arg) = rest.next() {
            if spec.is_none() {
                if RUNNER_FLAGS.contains(&arg.as_str()) {
                    continue;
                }
                if RUNNER_FLAGS_WITH_VALUE.contains(&arg.as_str()) {
                    let _ = rest.next();
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
                spec = Some(arg.clone());
                continue;
            }
            server_args.push(arg.clone());
        }
        if let Some(spec) = spec {
            let (name, _version) = split_package(&spec);
            return (
                ProgramToken::Package {
                    ecosystem: (*ecosystem).to_string(),
                    name,
                },
                server_args,
            );
        }
    }

    let token = if command.contains('/') || command.contains('\\') || command.starts_with('~') {
        ProgramToken::Path {
            value: normalize_path(command, home),
        }
    } else {
        ProgramToken::Name {
            value: command.to_string(),
        }
    };
    (token, args.to_vec())
}

/// Canonicalizes an endpoint for identity comparison.
///
/// Lowercases the scheme and host, drops a default port, strips userinfo and the
/// fragment, sorts query parameters, and replaces the value of any
/// credential-named parameter with a fixed marker so a token in a query string
/// never reaches the identity document or the cache.
pub fn canonical_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let scheme = scheme.to_ascii_lowercase();
    let rest = rest.split('#').next().unwrap_or_default();
    let (authority, remainder) = match rest.find(['/', '?']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => (host, Some(port)),
        _ => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    let port = match (scheme.as_str(), port) {
        ("https", Some("443")) | ("http", Some("80")) => None,
        (_, port) => port,
    };
    let (path, query) = match remainder.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (remainder, None),
    };
    let path = if path == "/" { "" } else { path };

    let mut out = String::new();
    if !scheme.is_empty() {
        out.push_str(&scheme);
        out.push_str("://");
    }
    out.push_str(&host);
    if let Some(port) = port {
        out.push(':');
        out.push_str(port);
    }
    out.push_str(path);
    if let Some(query) = query {
        let mut pairs: Vec<String> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                if crate::secret::classify_key(key) == crate::secret::EnvClass::Secret {
                    format!("{key}=<redacted>")
                } else {
                    format!("{key}={value}")
                }
            })
            .collect();
        pairs.sort();
        if !pairs.is_empty() {
            out.push('?');
            out.push_str(&pairs.join("&"));
        }
    }
    out
}
