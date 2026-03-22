//! Error types for the incur framework.
//!
//! Ported from `src/Errors.ts`.

use std::fmt;

/// A field-level validation error detail.
#[derive(Debug, Clone)]
pub struct FieldError {
    /// The field path that failed validation.
    pub path: String,
    /// The expected value or type.
    pub expected: String,
    /// The value that was received.
    pub received: String,
    /// Human-readable validation message.
    pub message: String,
}

/// Base error with short message, details from cause chain, and walk().
#[derive(Debug)]
pub struct BaseError {
    /// The short, human-readable error message (without details).
    pub short_message: String,
    /// Details extracted from the cause's message, if any.
    pub details: Option<String>,
    /// The underlying cause.
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for BaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(details) = &self.details {
            write!(f, "{}\n\nDetails: {}", self.short_message, details)
        } else {
            write!(f, "{}", self.short_message)
        }
    }
}

impl std::error::Error for BaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// CLI error with code, hint, and retryable flag.
#[derive(Debug)]
pub struct IncurError {
    /// The short, human-readable error message.
    pub message: String,
    /// Machine-readable error code (e.g. `"NOT_AUTHENTICATED"`).
    pub code: String,
    /// Actionable hint for the user.
    pub hint: Option<String>,
    /// Whether the operation can be retried.
    pub retryable: bool,
    /// Process exit code. When set, `serve()` uses this instead of `1`.
    pub exit_code: Option<i32>,
    /// The underlying cause.
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for IncurError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IncurError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Validation error with per-field error details.
#[derive(Debug)]
pub struct ValidationError {
    /// Human-readable error message.
    pub message: String,
    /// Per-field validation errors.
    pub field_errors: Vec<FieldError>,
    /// The underlying cause.
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Error thrown when argument parsing fails (unknown flags, missing values).
#[derive(Debug)]
pub struct ParseError {
    /// Human-readable error message.
    pub message: String,
    /// The underlying cause.
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Unified error type for the incur framework.
#[derive(Debug)]
pub enum Error {
    Incur(IncurError),
    Validation(ValidationError),
    Parse(ParseError),
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Incur(e) => write!(f, "{}", e),
            Error::Validation(e) => write!(f, "{}", e),
            Error::Parse(e) => write!(f, "{}", e),
            Error::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Incur(e) => e.source(),
            Error::Validation(e) => e.source(),
            Error::Parse(e) => e.source(),
            Error::Other(e) => e.source(),
        }
    }
}

impl From<IncurError> for Error {
    fn from(e: IncurError) -> Self {
        Error::Incur(e)
    }
}

impl From<ValidationError> for Error {
    fn from(e: ValidationError) -> Self {
        Error::Validation(e)
    }
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}

/// Result type alias using the incur Error.
pub type Result<T> = std::result::Result<T, Error>;
