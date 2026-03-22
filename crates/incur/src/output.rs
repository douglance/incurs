//! Output envelope types for the incur framework.
//!
//! Ported from the output types in `src/Cli.ts` and `src/internal/command.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;

use crate::errors::FieldError;

/// A CTA (call-to-action) block for command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtaBlock {
    /// Commands to suggest.
    pub commands: Vec<CtaEntry>,
    /// Human-readable label. Defaults to "Suggested commands:".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A single CTA entry — either a string command or a structured command with description.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CtaEntry {
    Simple(String),
    Detailed {
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

/// Result of executing a command.
pub enum CommandResult {
    /// Successful execution with data.
    Ok {
        data: Value,
        cta: Option<CtaBlock>,
    },
    /// Failed execution with error details.
    Error {
        code: String,
        message: String,
        retryable: bool,
        exit_code: Option<i32>,
        cta: Option<CtaBlock>,
    },
    /// Streaming output.
    Stream(Pin<Box<dyn futures::Stream<Item = Value> + Send>>),
}

/// Execution result returned from `command::execute()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecuteResult {
    Ok {
        ok: bool,
        data: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cta: Option<CtaBlock>,
    },
    Error {
        ok: bool,
        error: ExecuteError,
        #[serde(skip_serializing_if = "Option::is_none")]
        cta: Option<CtaBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
}

/// Error details within an execute result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_errors: Option<Vec<FieldErrorOutput>>,
}

/// Serializable field error for output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldErrorOutput {
    pub path: String,
    pub expected: String,
    pub received: String,
    pub message: String,
}

impl From<&FieldError> for FieldErrorOutput {
    fn from(e: &FieldError) -> Self {
        FieldErrorOutput {
            path: e.path.clone(),
            expected: e.expected.clone(),
            received: e.received.clone(),
            message: e.message.clone(),
        }
    }
}

/// Output envelope wrapping command results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputEnvelope {
    #[serde(flatten)]
    pub result: ExecuteResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<OutputMeta>,
}

/// Metadata attached to the output envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputMeta {
    pub command: String,
    pub duration: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cta: Option<CtaBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

/// Supported output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Toon,
    Json,
    Yaml,
    Markdown,
    Jsonl,
    Table,
    Csv,
}

impl Format {
    /// Parse a format string.
    pub fn from_str_opt(s: &str) -> Option<Format> {
        match s {
            "toon" => Some(Format::Toon),
            "json" => Some(Format::Json),
            "yaml" => Some(Format::Yaml),
            "md" | "markdown" => Some(Format::Markdown),
            "jsonl" => Some(Format::Jsonl),
            "table" => Some(Format::Table),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Format::Toon => write!(f, "toon"),
            Format::Json => write!(f, "json"),
            Format::Yaml => write!(f, "yaml"),
            Format::Markdown => write!(f, "md"),
            Format::Jsonl => write!(f, "jsonl"),
            Format::Table => write!(f, "table"),
            Format::Csv => write!(f, "csv"),
        }
    }
}

/// Output policy controlling who sees output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputPolicy {
    #[default]
    All,
    AgentOnly,
}
