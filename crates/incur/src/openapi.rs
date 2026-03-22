//! OpenAPI spec to command generation.
//!
//! Ported from `src/Openapi.ts`. Parses an OpenAPI 3.x specification and
//! generates command definitions that can be registered with the incur CLI
//! framework. Gated behind the `openapi` feature flag.

use std::collections::BTreeMap;

use crate::schema::FieldMeta;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A generated command definition from an OpenAPI operation.
#[derive(Debug, Clone)]
pub struct GeneratedCommand {
    /// Human-readable description (from `summary` or `description`).
    pub description: Option<String>,
    /// Positional argument fields (from path parameters).
    pub args_fields: Vec<FieldMeta>,
    /// Option fields (from query parameters and request body properties).
    pub options_fields: Vec<FieldMeta>,
    /// The HTTP method (e.g. "GET", "POST").
    pub http_method: String,
    /// The URL path template (e.g. "/users/{id}").
    pub path_template: String,
}

// ---------------------------------------------------------------------------
// Public API (behind feature flag)
// ---------------------------------------------------------------------------

/// Generates incur command definitions from an OpenAPI spec.
///
/// Resolves all `$ref` pointers, iterates over paths and methods, and produces
/// a command for each operation. Path parameters become positional args, query
/// parameters become options, and request body properties are merged into options.
///
/// # Feature gate
///
/// This function requires the `openapi` feature to be enabled (`oas3` and
/// `openapiv3` crates).
#[cfg(feature = "openapi")]
pub async fn generate_commands(
    spec: &serde_json::Value,
) -> Result<BTreeMap<String, GeneratedCommand>, crate::errors::Error> {
    // TODO: Full implementation using oas3/openapiv3 crates.
    //
    // The implementation should:
    // 1. Parse and dereference the spec using oas3.
    // 2. Iterate over all paths and methods.
    // 3. For each operation:
    //    a. Extract path parameters as args_fields.
    //    b. Extract query parameters as options_fields.
    //    c. Extract request body properties as additional options_fields.
    //    d. Derive a command name from operationId or method+path.
    // 4. Return the map of command names to GeneratedCommand.
    let _ = spec;
    Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "OpenAPI command generation not yet implemented for Rust",
    ))))
}

/// Generates command definitions from an OpenAPI spec (non-async version for
/// use in contexts where the spec is already fully resolved).
///
/// This is a stub that returns an error. The full implementation requires the
/// `openapi` feature flag and the async variant.
#[cfg(not(feature = "openapi"))]
pub fn generate_commands(
    _spec: &serde_json::Value,
) -> Result<BTreeMap<String, GeneratedCommand>, crate::errors::Error> {
    Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "OpenAPI support requires the 'openapi' feature flag",
    ))))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generated_command_struct() {
        let cmd = GeneratedCommand {
            description: Some("List users".to_string()),
            args_fields: vec![],
            options_fields: vec![],
            http_method: "GET".to_string(),
            path_template: "/users".to_string(),
        };
        assert_eq!(cmd.http_method, "GET");
        assert_eq!(cmd.path_template, "/users");
        assert_eq!(cmd.description.as_deref(), Some("List users"));
    }

    #[cfg(not(feature = "openapi"))]
    #[test]
    fn test_generate_commands_without_feature() {
        let spec = serde_json::json!({});
        let result = generate_commands(&spec);
        assert!(result.is_err());
    }
}
