//! Error types for MCP App transports and helpers.

use thiserror::Error;

/// Result type returned by MCP App operations.
pub type AppResult<T> = Result<T, AppError>;

/// Errors surfaced by an MCP App transport or helper.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AppError {
    /// The app is not embedded in an MCP App host.
    #[error("open this app in an MCP host to use {method}")]
    UnavailableHost {
        /// Method that required a host.
        method: String,
    },
    /// The transport has been disposed and no longer accepts work.
    #[error("app transport disposed")]
    Disposed,
    /// The host did not answer before the caller's timeout expired.
    #[error("{method} timed out")]
    Timeout {
        /// Method that timed out.
        method: String,
    },
    /// The caller cancelled the request before the host answered.
    #[error("{method} cancelled")]
    Cancelled {
        /// Method that was cancelled.
        method: String,
    },
    /// The host returned a JSON-RPC error response.
    #[error("JSON-RPC error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// JSON-RPC error message.
        message: String,
    },
    /// The app sent data that failed local validation.
    #[error("invalid {field}: {message}")]
    Validation {
        /// Field or conceptual input that failed validation.
        field: &'static str,
        /// Human-readable validation message.
        message: String,
    },
    /// The host response could not be decoded into the expected result type.
    #[error("failed to decode {method} response: {message}")]
    Decode {
        /// Method whose result failed to decode.
        method: String,
        /// Decoder diagnostic.
        message: String,
    },
    /// A transport-specific failure that is not otherwise classified.
    #[error("transport error: {0}")]
    Transport(String),
}

impl AppError {
    /// Creates a validation error for one field.
    pub fn validation(field: &'static str, message: impl Into<String>) -> Self {
        Self::Validation {
            field,
            message: message.into(),
        }
    }
}
