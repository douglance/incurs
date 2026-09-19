//! Assigning each server a JavaScript namespace that stays put.

use std::collections::{BTreeMap, BTreeSet};

use incurs_codemode::sanitize_namespace;
use incurs_mcp_discovery::ServerId;
use serde::{Deserialize, Serialize};

/// Names the generated Code Mode program already binds, or that JavaScript
/// refuses as a binding.
///
/// A connector taking one of these is rejected by `build_program_source`, or
/// produces a syntax error inside the sandbox, and either way the failure takes
/// down the whole program rather than that one connector. `fetch` is the case
/// that matters in practice: it is both a reserved harness name and one of the
/// most commonly installed MCP servers.
const UNUSABLE: &[&str] = &[
    "__incursDispatch",
    "__dispatch",
    "__encode",
    "__decode",
    "__unwrap",
    "__seq",
    "__logs",
    "__program",
    "__result",
    "__base64Encode",
    "__base64Decode",
    "__callBuiltin",
    "codemode",
    "console",
    "Promise",
    "setTimeout",
    "Error",
    "fetch",
    "JSON",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "BigInt",
    "Uint8Array",
    "Math",
    "Symbol",
    "globalThis",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// How long a released namespace stays reserved, in milliseconds.
///
/// Reissuing a name is worse than leaking one. If `linear` is removed and a
/// different server later takes that name, every saved snippet calling
/// `linear.*` silently binds to the wrong server and keeps running.
pub const RETENTION_MS: u64 = 90 * 24 * 60 * 60 * 1000;

/// A namespace previously assigned and not yet free to reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetiredNamespace {
    /// The namespace held back.
    pub namespace: String,
    /// The server that held it.
    pub server_id: String,
    /// When it was released, in Unix milliseconds.
    pub retired_at_ms: u64,
}

/// The durable record of which server owns which namespace.
///
/// Namespaces are **assigned once and then frozen**, not derived on each run. A
/// derived name cannot be stable, because the display name depends on which
/// hosts happen to be installed: renaming a server in one agent's config, or
/// uninstalling that agent, would silently rename the namespace and break every
/// snippet that used it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceLedger {
    /// Server identity to assigned namespace.
    #[serde(default)]
    pub assigned: BTreeMap<String, String>,
    /// Namespaces held back from reuse.
    #[serde(default)]
    pub retired: Vec<RetiredNamespace>,
}

/// Makes a candidate name usable as a JavaScript binding.
#[must_use]
pub fn guard_reserved(candidate: &str) -> String {
    let sanitized = sanitize_namespace(candidate);
    if UNUSABLE.contains(&sanitized.as_str()) {
        format!("mcp_{sanitized}")
    } else {
        sanitized
    }
}

/// One server awaiting a namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRequest {
    /// The server's identity.
    pub id: ServerId,
    /// The name to derive a namespace from on first assignment.
    pub display_name: String,
}

impl NamespaceLedger {
    /// Assigns a namespace to every request, preserving existing assignments.
    ///
    /// Iterating in server-identity order rather than input order is what makes
    /// the result independent of how discovery happened to enumerate hosts, and
    /// resolving collisions with an identity-derived suffix rather than a
    /// counter is what stops an unrelated addition from renaming an incumbent.
    pub fn assign(
        &mut self,
        requests: &[NamespaceRequest],
        now_ms: u64,
    ) -> BTreeMap<String, String> {
        let mut ordered: Vec<&NamespaceRequest> = requests.iter().collect();
        ordered.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));

        let mut claimed: BTreeSet<String> = BTreeSet::new();
        let mut result: BTreeMap<String, String> = BTreeMap::new();

        // Every incumbent reclaims its namespace before any new server derives
        // one, so a newcomer can never take a name that is already in use.
        for request in &ordered {
            if let Some(existing) = self.assigned.get(request.id.as_str())
                && claimed.insert(existing.clone())
            {
                result.insert(request.id.as_str().to_string(), existing.clone());
            }
        }

        let retained: BTreeSet<&str> = self
            .retired
            .iter()
            .filter(|entry| now_ms.saturating_sub(entry.retired_at_ms) < RETENTION_MS)
            .map(|entry| entry.namespace.as_str())
            .collect();

        for request in &ordered {
            if result.contains_key(request.id.as_str()) {
                continue;
            }
            let base = guard_reserved(&request.display_name);
            let namespace = if !claimed.contains(&base) && !retained.contains(base.as_str()) {
                base
            } else {
                let mut chosen = None;
                for width in [6_usize, 12, 24, 64] {
                    let candidate = format!("{base}_{}", &request.id.as_str()[..width]);
                    if !claimed.contains(&candidate) {
                        chosen = Some(candidate);
                        break;
                    }
                }
                chosen.unwrap_or_else(|| format!("{base}_{}", request.id.as_str()))
            };
            claimed.insert(namespace.clone());
            result.insert(request.id.as_str().to_string(), namespace);
        }

        let present: BTreeSet<&str> = requests.iter().map(|request| request.id.as_str()).collect();
        let released: Vec<(String, String)> = self
            .assigned
            .iter()
            .filter(|(id, _)| !present.contains(id.as_str()))
            .map(|(id, namespace)| (id.clone(), namespace.clone()))
            .collect();
        for (id, namespace) in released {
            self.assigned.remove(&id);
            self.retired.push(RetiredNamespace {
                namespace,
                server_id: id,
                retired_at_ms: now_ms,
            });
        }
        self.retired
            .retain(|entry| now_ms.saturating_sub(entry.retired_at_ms) < RETENTION_MS);
        for (id, namespace) in &result {
            self.assigned.insert(id.clone(), namespace.clone());
        }
        result
    }
}
