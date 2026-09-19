//! Stable server identity and configuration fingerprints.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::secret::{EnvClass, EnvValue};
use crate::transport::{AuthClass, McpTransport, ProgramToken};

/// Stable identity of one MCP server, independent of which host configured it.
///
/// Two hosts naming the same server differently produce the same `ServerId`, and
/// that is what lets Composite present one connector instead of three.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerId(String);

impl ServerId {
    /// Returns the full 64-character hexadecimal digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the first six characters, used to disambiguate namespaces.
    pub fn short(&self) -> &str {
        &self.0[..6]
    }
}

impl Display for ServerId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Digest of every launch input, used to invalidate a cached tool schema.
///
/// Strictly wider than [`ServerId`]: it also covers credential values, because a
/// rotated token can grant different scopes and therefore list different tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigFingerprint(String);

impl ConfigFingerprint {
    /// Returns the hexadecimal digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How one environment key takes part in identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvIdentity {
    /// The key's classification.
    class: EnvClass,
    /// The value, present only for an identity-bearing key.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

/// The canonical document hashed to produce a [`ServerId`].
///
/// This is a derived struct with [`BTreeMap`] fields rather than a
/// `serde_json::Value`, and that is load-bearing. This workspace resolves
/// `serde_json` with the `preserve_order` feature enabled, so `serde_json::Map`
/// preserves insertion order; a document assembled through `json!` would hash in
/// construction order, and the same configuration could then produce different
/// identities on two runs. `BTreeMap` serializes in key order regardless.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityDocument {
    /// Document version, so a future change to the scheme is explicit.
    v: u32,
    /// Transport discriminator: `stdio` or `http`.
    transport: &'static str,
    /// Program token, for a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    program: Option<ProgramToken>,
    /// The server's own argv, for a stdio server.
    argv: Vec<String>,
    /// Resolved working directory, for a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    /// Canonical endpoint, for an HTTP server.
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    /// Authentication class, for an HTTP server.
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_class: Option<AuthClass>,
    /// Identity-bearing environment, in key order.
    env: BTreeMap<String, EnvIdentity>,
}

/// Builds the identity view of a configured environment overlay.
fn identity_env(env: &BTreeMap<String, EnvValue>) -> BTreeMap<String, EnvIdentity> {
    env.iter()
        .filter(|(_, value)| value.class() != EnvClass::Ambient)
        .map(|(key, value)| {
            (
                key.clone(),
                EnvIdentity {
                    class: value.class(),
                    value: value.identity_value().map(str::to_string),
                },
            )
        })
        .collect()
}

/// Renders a SHA-256 digest as lowercase hexadecimal.
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Derives the stable identity of one transport.
pub fn server_id(transport: &McpTransport) -> ServerId {
    let document = match transport {
        McpTransport::Stdio(stdio) => IdentityDocument {
            v: 1,
            transport: "stdio",
            program: Some(stdio.program.clone()),
            argv: stdio.server_args.clone(),
            cwd: stdio.resolved_cwd.clone(),
            endpoint: None,
            auth_class: None,
            env: identity_env(&stdio.env),
        },
        McpTransport::StreamableHttp(http) | McpTransport::Sse(http) => IdentityDocument {
            v: 1,
            transport: "http",
            program: None,
            argv: Vec::new(),
            cwd: None,
            endpoint: Some(http.canonical_url.clone()),
            auth_class: Some(http.auth_class.clone()),
            env: identity_env(&http.headers),
        },
    };
    let bytes = serde_json::to_vec(&document).expect("identity document is always serializable");
    ServerId(digest(&bytes))
}

/// Derives the fingerprint that decides whether a cached tool schema is usable.
///
/// Credential values take part, because a rotated token can change the tool
/// list, but each is first salted with the server identity. A bare
/// `sha256(value)` would make the persisted fingerprint a brute-force target for
/// a low-entropy credential; the salt is per-server and high-entropy, so a
/// precomputed table does not transfer.
pub fn config_fingerprint(
    id: &ServerId,
    transport: &McpTransport,
    standards: &[String],
) -> ConfigFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(id.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(transport.identity_kind().as_bytes());

    let overlay = match transport {
        McpTransport::Stdio(stdio) => {
            hasher.update([0]);
            hasher.update(stdio.command.as_bytes());
            for arg in &stdio.args {
                hasher.update([0x1f]);
                hasher.update(arg.as_bytes());
            }
            &stdio.env
        }
        McpTransport::StreamableHttp(http) | McpTransport::Sse(http) => {
            hasher.update([0]);
            hasher.update(http.canonical_url.as_bytes());
            &http.headers
        }
    };

    for (key, value) in overlay {
        hasher.update([0]);
        hasher.update(key.as_bytes());
        match value.class() {
            // An ambient value is excluded from identity and from the
            // fingerprint: it changes between shells without changing the server.
            EnvClass::Ambient => {}
            EnvClass::Identity => {
                hasher.update([1]);
                hasher.update(value.expose().as_bytes());
            }
            EnvClass::Secret => {
                hasher.update([2]);
                hasher.update(salted_secret(id, value.expose()).as_bytes());
            }
        }
    }

    for standard in standards {
        hasher.update([0]);
        hasher.update(standard.as_bytes());
    }
    ConfigFingerprint(format!("{:x}", hasher.finalize()))
}

/// Hashes a credential under the server identity as salt.
fn salted_secret(id: &ServerId, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(id.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}
