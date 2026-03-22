//! Curl-style argv parsing and HTTP request/response handling.
//!
//! Ported from `src/Fetch.ts`. Parses curl-style command-line arguments into
//! structured fetch input, and provides utilities for detecting streaming
//! responses.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Structured input parsed from curl-style argv.
#[derive(Debug, Clone)]
pub struct FetchInput {
    /// The request path (e.g. "/users/123").
    pub path: String,
    /// HTTP method (e.g. "GET", "POST").
    pub method: String,
    /// Request headers.
    pub headers: Vec<(String, String)>,
    /// Request body (for POST/PUT/PATCH).
    pub body: Option<String>,
    /// Query parameters.
    pub query: Vec<(String, String)>,
}

/// Structured output from a fetch response.
#[derive(Debug, Clone)]
pub struct FetchOutput {
    /// Whether the response status is in the 2xx range.
    pub ok: bool,
    /// HTTP status code.
    pub status: u16,
    /// Parsed response body (JSON parsed if possible, otherwise string).
    pub data: Value,
    /// Response headers.
    pub headers: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Reserved flags
// ---------------------------------------------------------------------------

/// Reserved flags consumed by the fetch gateway (not forwarded as query params).
fn is_reserved_flag(key: &str) -> bool {
    matches!(key, "method" | "body" | "data" | "header")
}

/// Maps short flags to their long-form reserved names.
fn reserved_short(ch: char) -> Option<&'static str> {
    match ch {
        'X' => Some("method"),
        'd' => Some("data"),
        'H' => Some("header"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parses curl-style argv into a structured fetch input.
///
/// Supports:
/// - Positional segments joined into a path (e.g. `users 123` -> `/users/123`)
/// - `-X METHOD` or `--method METHOD` to set the HTTP method
/// - `-d BODY` or `--data BODY` or `--body BODY` to set the request body
/// - `-H "Name: Value"` or `--header "Name: Value"` to set headers
/// - Unknown `--key value` pairs become query parameters
pub fn parse_argv(argv: &[String]) -> FetchInput {
    let mut segments: Vec<String> = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut query: Vec<(String, String)> = Vec::new();
    let mut method: Option<String> = None;
    let mut body: Option<String> = None;

    let mut handle_reserved = |key: &str, value: &str| {
        match key {
            "method" => method = Some(value.to_uppercase()),
            "body" | "data" => body = Some(value.to_string()),
            "header" => {
                if let Some(colon_idx) = value.find(':') {
                    let name = value[..colon_idx].trim().to_string();
                    let val = value[colon_idx + 1..].trim().to_string();
                    headers.push((name, val));
                }
            }
            _ => {}
        }
    };

    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];

        if token.starts_with("--") {
            if let Some(eq_idx) = token.find('=') {
                // --key=value
                let key = &token[2..eq_idx];
                let value = &token[eq_idx + 1..];
                if is_reserved_flag(key) {
                    handle_reserved(key, value);
                } else {
                    query.push((key.to_string(), value.to_string()));
                }
                i += 1;
            } else {
                let key = &token[2..];
                let value = argv.get(i + 1).map(|s| s.as_str()).unwrap_or("");
                if is_reserved_flag(key) {
                    handle_reserved(key, value);
                    i += 2;
                } else {
                    query.push((key.to_string(), value.to_string()));
                    i += 2;
                }
            }
        } else if token.starts_with('-') && token.len() == 2 {
            let short = token.chars().nth(1).unwrap_or('?');
            let value = argv.get(i + 1).map(|s| s.as_str()).unwrap_or("");
            if let Some(mapped) = reserved_short(short) {
                handle_reserved(mapped, value);
                i += 2;
            } else {
                // Unknown short flag — skip
                i += 2;
            }
        } else {
            segments.push(token.clone());
            i += 1;
        }
    }

    let path = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };

    let resolved_method = method.unwrap_or_else(|| {
        if body.is_some() {
            "POST".to_string()
        } else {
            "GET".to_string()
        }
    });

    FetchInput {
        path,
        method: resolved_method,
        headers,
        body,
        query,
    }
}

/// Returns true if the content-type indicates a streaming NDJSON response.
pub fn is_streaming_response(content_type: Option<&str>) -> bool {
    content_type == Some("application/x-ndjson")
}

// ---------------------------------------------------------------------------
// FetchHandler trait
// ---------------------------------------------------------------------------

/// Trait for fetch gateway handlers.
///
/// Implementations receive a parsed [`FetchInput`] and return a [`FetchOutput`].
/// This allows CLIs to proxy HTTP-style requests through a command gateway.
#[async_trait::async_trait]
pub trait FetchHandler: Send + Sync {
    /// Handle a fetch request and return a response.
    async fn handle(&self, request: FetchInput) -> FetchOutput;
}

/// Options for configuring a fetch gateway command.
pub struct FetchGatewayOptions {
    /// A short description of the gateway.
    pub description: Option<String>,
    /// Base path prefix for request URLs.
    pub base_path: Option<String>,
    /// Output policy for the gateway.
    pub output_policy: Option<crate::output::OutputPolicy>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_basic_path() {
        let input = parse_argv(&argv(&["users", "123"]));
        assert_eq!(input.path, "/users/123");
        assert_eq!(input.method, "GET");
        assert!(input.body.is_none());
    }

    #[test]
    fn test_empty_path() {
        let input = parse_argv(&argv(&[]));
        assert_eq!(input.path, "/");
    }

    #[test]
    fn test_method_long() {
        let input = parse_argv(&argv(&["--method", "PUT", "users", "123"]));
        assert_eq!(input.method, "PUT");
        assert_eq!(input.path, "/users/123");
    }

    #[test]
    fn test_method_short() {
        let input = parse_argv(&argv(&["-X", "DELETE", "users", "123"]));
        assert_eq!(input.method, "DELETE");
    }

    #[test]
    fn test_body_sets_post() {
        let input = parse_argv(&argv(&["-d", r#"{"name":"test"}"#, "users"]));
        assert_eq!(input.method, "POST");
        assert_eq!(input.body.as_deref(), Some(r#"{"name":"test"}"#));
    }

    #[test]
    fn test_explicit_method_overrides_body() {
        let input = parse_argv(&argv(&["-X", "PUT", "-d", r#"{"name":"test"}"#, "users"]));
        assert_eq!(input.method, "PUT");
        assert!(input.body.is_some());
    }

    #[test]
    fn test_headers() {
        let input = parse_argv(&argv(&["-H", "Authorization: Bearer token123", "users"]));
        assert_eq!(input.headers.len(), 1);
        assert_eq!(input.headers[0].0, "Authorization");
        assert_eq!(input.headers[0].1, "Bearer token123");
    }

    #[test]
    fn test_query_params() {
        let input = parse_argv(&argv(&["users", "--limit", "10", "--offset", "20"]));
        assert_eq!(input.path, "/users");
        assert_eq!(input.query.len(), 2);
        assert!(input.query.iter().any(|(k, v)| k == "limit" && v == "10"));
        assert!(input.query.iter().any(|(k, v)| k == "offset" && v == "20"));
    }

    #[test]
    fn test_query_with_equals() {
        let input = parse_argv(&argv(&["users", "--limit=10"]));
        assert_eq!(input.query.len(), 1);
        assert_eq!(input.query[0].0, "limit");
        assert_eq!(input.query[0].1, "10");
    }

    #[test]
    fn test_data_long() {
        let input = parse_argv(&argv(&["--data", r#"{"x":1}"#, "api"]));
        assert_eq!(input.body.as_deref(), Some(r#"{"x":1}"#));
    }

    #[test]
    fn test_body_long() {
        let input = parse_argv(&argv(&["--body", r#"{"x":1}"#, "api"]));
        assert_eq!(input.body.as_deref(), Some(r#"{"x":1}"#));
    }

    #[test]
    fn test_is_streaming_response() {
        assert!(is_streaming_response(Some("application/x-ndjson")));
        assert!(!is_streaming_response(Some("application/json")));
        assert!(!is_streaming_response(None));
    }

    #[test]
    fn test_header_equals_syntax() {
        let input = parse_argv(&argv(&["--header=Content-Type: application/json", "api"]));
        assert_eq!(input.headers.len(), 1);
        assert_eq!(input.headers[0].0, "Content-Type");
        assert_eq!(input.headers[0].1, "application/json");
    }

    #[test]
    fn test_mixed_everything() {
        let input = parse_argv(&argv(&[
            "-X", "POST",
            "-H", "Authorization: Bearer tok",
            "-d", r#"{"a":1}"#,
            "--limit", "5",
            "api", "v1", "data",
        ]));
        assert_eq!(input.method, "POST");
        assert_eq!(input.path, "/api/v1/data");
        assert_eq!(input.body.as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(input.headers.len(), 1);
        assert_eq!(input.query.len(), 1);
        assert_eq!(input.query[0], ("limit".to_string(), "5".to_string()));
    }
}
