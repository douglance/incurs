//! Failures and non-fatal findings produced while reading host configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A failure that prevented one configuration file from being read at all.
///
/// A problem with a single server entry is never an error: it becomes a
/// [`DiscoveryDiagnostic`] so one malformed entry cannot hide the healthy
/// entries beside it in the same file.
///
/// No variant carries file contents. The `#[source]` chain carries parser
/// errors, which report position but not text, so a credential in a malformed
/// file cannot reach a log through an error message.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The configuration file could not be read.
    #[error("cannot read {path}")]
    Read {
        /// The file that failed.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// The file was not valid JSON, or valid JSONC where the format allows it.
    #[error("{path} is not valid JSON")]
    Json {
        /// The file that failed.
        path: PathBuf,
        /// The underlying parse failure.
        #[source]
        source: serde_json::Error,
    },
    /// The file was not valid TOML.
    #[error("{path} is not valid TOML")]
    Toml {
        /// The file that failed.
        path: PathBuf,
        /// The underlying parse failure.
        #[source]
        source: toml::de::Error,
    },
    /// The document parsed, but its root was not the expected shape.
    #[error("{path} does not contain a \"{key}\" object")]
    Shape {
        /// The file that failed.
        path: PathBuf,
        /// The key the format requires.
        key: String,
    },
    /// No home directory could be resolved, so no host path is known.
    #[error("cannot resolve the user home directory")]
    NoHome,
}

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// The entry was kept, possibly with a coerced or defaulted value.
    Warning,
    /// The entry was rejected.
    Error,
}

/// One non-fatal finding about a single configuration entry.
///
/// `message` is generated from a fixed template. It never embeds a configured
/// value, so a diagnostic is safe to persist and to show a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryDiagnostic {
    /// How much the finding matters.
    pub severity: Severity,
    /// Stable machine-readable reason, drawn from a closed vocabulary.
    pub code: String,
    /// File and document location the finding refers to.
    pub path: String,
    /// Human-readable explanation built from a fixed template.
    pub message: String,
}

impl DiscoveryDiagnostic {
    /// Records a kept-but-adjusted entry.
    pub fn warning(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.to_string(),
            path: path.into(),
            message: message.into(),
        }
    }

    /// Records a rejected entry.
    pub fn error(code: &str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_string(),
            path: path.into(),
            message: message.into(),
        }
    }
}
