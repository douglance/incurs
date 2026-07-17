//! OpenAPI spec to command generation.
//!
//! Ported from `src/Openapi.ts`. Parses an OpenAPI 3.x specification and
//! generates command definitions that can be registered with the incurs CLI
//! framework. Gated behind the `openapi` feature flag.
//!
//! Uses `serde_json::Value` to walk the spec directly rather than depending
//! on external OpenAPI parsing crates.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::schema::{FieldMeta, FieldType};

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

/// Options for OpenAPI command generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GenerateMode {
    /// Use each operationId as the generated command name.
    #[default]
    Operation,
    /// Derive command names from path namespaces and HTTP methods.
    Namespace,
}

/// Options for OpenAPI command generation.
#[derive(Debug, Clone)]
pub struct GenerateOptions {
    /// Base path prefix prepended to all operation paths.
    pub base_path: Option<String>,
    /// Strip schema details that do not affect command input parsing.
    pub compact: bool,
    /// Header names copied from inbound HTTP requests to upstream requests.
    pub forward_headers: Vec<String>,
    /// Command naming strategy.
    pub mode: GenerateMode,
    /// Whether credential header options are generated from security schemes.
    pub security: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            base_path: None,
            compact: false,
            forward_headers: Vec::new(),
            mode: GenerateMode::Operation,
            security: true,
        }
    }
}

/// Options for generating an OpenAPI document from a CLI.
#[derive(Debug, Clone, Default)]
pub struct DocumentOptions {
    /// API description override.
    pub description: Option<String>,
    /// API version. Defaults to `0.0.0`.
    pub version: Option<String>,
}

/// OpenAPI document source accepted by hosted command generation.
#[derive(Debug, Clone)]
pub enum OpenApiSource {
    /// An already parsed OpenAPI document.
    Document(Value),
    /// An HTTP(S) URL or local JSON/YAML document string.
    Text(String),
}

/// Loads an OpenAPI document from a parsed value, URL, JSON, or YAML string.
#[cfg(feature = "openapi")]
pub async fn load_source(source: OpenApiSource) -> Result<Value, Box<dyn std::error::Error>> {
    match source {
        OpenApiSource::Document(document) => Ok(document),
        OpenApiSource::Text(source)
            if source.starts_with("http://") || source.starts_with("https://") =>
        {
            let text = reqwest::get(source)
                .await?
                .error_for_status()?
                .text()
                .await?;
            serde_json::from_str(&text)
                .or_else(|_| serde_yaml_ng::from_str(&text))
                .map_err(Into::into)
        }
        OpenApiSource::Text(source) => serde_json::from_str(&source)
            .or_else(|_| serde_yaml_ng::from_str(&source))
            .map_err(Into::into),
    }
}

/// Generates an OpenAPI 3.2 document from a CLI command tree.
pub fn from_cli(cli: &crate::cli::Cli, options: &DocumentOptions) -> Value {
    fn collect(
        commands: &BTreeMap<String, crate::cli::CommandEntry>,
        prefix: &[String],
        paths: &mut serde_json::Map<String, Value>,
    ) {
        for (name, entry) in commands {
            let mut command_path = prefix.to_vec();
            command_path.push(name.clone());
            match entry {
                crate::cli::CommandEntry::Leaf(command) => {
                    let args_path = command
                        .args_fields
                        .iter()
                        .map(|field| format!("/{{{}}}", field.name))
                        .collect::<String>();
                    let path = format!("/{}{}", command_path.join("/"), args_path);
                    let method = infer_document_method(name, command);
                    let parameters = command
                        .args_fields
                        .iter()
                        .map(|field| {
                            serde_json::json!({
                                "name": field.name,
                                "in": "path",
                                "required": true,
                                "description": field.description,
                                "schema": field_schema(field),
                            })
                        })
                        .chain(
                            (method == "get")
                                .then_some(command.options_fields.iter())
                                .into_iter()
                                .flatten()
                                .map(|field| {
                                    serde_json::json!({
                                        "name": field.name,
                                        "in": "query",
                                        "required": field.required,
                                        "description": field.description,
                                        "schema": field_schema(field),
                                    })
                                }),
                        )
                        .collect::<Vec<_>>();
                    let mut operation = serde_json::Map::from_iter([
                        (
                            "operationId".to_string(),
                            Value::String(operation_id(&method, &path)),
                        ),
                        ("parameters".to_string(), Value::Array(parameters)),
                        (
                            "responses".to_string(),
                            serde_json::json!({ "200": { "description": "Successful response" } }),
                        ),
                    ]);
                    if let Some(description) = &command.description {
                        operation.insert("summary".to_string(), Value::String(description.clone()));
                    }
                    if method != "get" && !command.options_fields.is_empty() {
                        operation.insert(
                            "requestBody".to_string(),
                            serde_json::json!({
                                "required": command.options_fields.iter().any(|field| field.required),
                                "content": {
                                    "application/json": {
                                        "schema": crate::schema::to_json_schema(&command.options_fields),
                                    }
                                }
                            }),
                        );
                    }
                    if let Some(output) = &command.output_schema {
                        operation["responses"]["200"]["content"] = serde_json::json!({
                            "application/json": { "schema": output }
                        });
                    }
                    paths.insert(
                        path,
                        serde_json::json!({ method: Value::Object(operation) }),
                    );
                }
                crate::cli::CommandEntry::Group { commands, .. } => {
                    collect(commands, &command_path, paths)
                }
                crate::cli::CommandEntry::FetchGateway { .. } => {}
            }
        }
    }

    let mut paths = serde_json::Map::new();
    collect(&cli.commands, &[], &mut paths);
    serde_json::json!({
        "openapi": "3.2.0",
        "info": {
            "title": cli.name,
            "version": options.version.as_deref().unwrap_or("0.0.0"),
            "description": options.description.as_ref().or(cli.description.as_ref()),
        },
        "paths": paths,
    })
}

fn field_schema(field: &FieldMeta) -> Value {
    crate::schema::to_json_schema(std::slice::from_ref(field))["properties"][&field.cli_name]
        .clone()
}

fn infer_document_method(name: &str, command: &crate::command::CommandDef) -> String {
    let destructive = command
        .handler
        .mcp_options()
        .and_then(|options| options.annotations.as_ref())
        .and_then(|annotations| annotations.destructive_hint)
        == Some(true);
    match name.to_lowercase().as_str() {
        name if name.contains("delete") || name.contains("remove") => "delete".to_string(),
        name if name.contains("update") || name.contains("patch") => "patch".to_string(),
        name if name.contains("create") || name.contains("add") => "post".to_string(),
        _ if destructive => "post".to_string(),
        _ => "get".to_string(),
    }
}

fn operation_id(method: &str, path: &str) -> String {
    let suffix = path
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
                .unwrap_or_default()
        })
        .collect::<String>();
    format!("{method}{suffix}")
}

// ---------------------------------------------------------------------------
// Public API (feature-gated)
// ---------------------------------------------------------------------------

/// The fetch function signature for OpenAPI-generated command handlers.
///
/// Parameters: (url, method, headers as key-value pairs, optional body)
/// Returns: a future resolving to a JSON value.
#[cfg(feature = "openapi")]
pub type FetchFn = std::sync::Arc<
    dyn Fn(
            String,
            String,
            Vec<(String, String)>,
            Option<String>,
        ) -> futures::future::BoxFuture<'static, Value>
        + Send
        + Sync,
>;

/// Generates incur `CommandDef`s from an OpenAPI 3.x spec.
///
/// Walks the `paths` object, extracting each method/operation and creating a
/// command for it. Path parameters become positional args, query parameters
/// and request body properties become options.
///
/// Each generated command's handler constructs an HTTP request and calls the
/// provided `fetch_fn` to execute it.
#[cfg(feature = "openapi")]
pub async fn generate_commands(
    spec: &Value,
    fetch_fn: FetchFn,
    options: &GenerateOptions,
) -> Result<BTreeMap<String, crate::command::CommandDef>, Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use crate::command::CommandDef;

    let resolved = resolve_refs(spec, spec);
    let paths = match resolved.get("paths").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(BTreeMap::new()),
    };

    let mut commands = BTreeMap::new();
    let namespace_paths = paths
        .keys()
        .map(|path| namespace_segments(path))
        .collect::<Vec<_>>();
    let http_methods = [
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ];

    for (path, methods_val) in paths {
        let methods = match methods_val.as_object() {
            Some(m) => m,
            None => continue,
        };

        for (method, operation_val) in methods {
            if method.starts_with("x-") {
                continue;
            }
            if !http_methods.contains(&method.as_str()) {
                continue;
            }

            let op = match operation_val.as_object() {
                Some(o) => o,
                None => continue,
            };

            let operation_id = op.get("operationId").and_then(|v| v.as_str());
            let name = match options.mode {
                GenerateMode::Operation => operation_id
                    .map(ToString::to_string)
                    .unwrap_or_else(|| generate_operation_name(method, path)),
                GenerateMode::Namespace => {
                    let segments = namespace_segments(path);
                    let operation_count = methods
                        .keys()
                        .filter(|candidate| http_methods.contains(&candidate.as_str()))
                        .count();
                    let parent = namespace_paths.iter().any(|candidate| {
                        candidate.len() > segments.len() && candidate.starts_with(&segments)
                    });
                    generate_namespace_name(method, &segments, operation_count > 1 || parent)
                }
            };

            let http_method = method.to_uppercase();
            let summary = op.get("summary").and_then(|v| v.as_str());
            let operation_description = op.get("description").and_then(|v| v.as_str());
            let description = summary.or(operation_description).map(ToString::to_string);
            let mcp_description = match (summary, operation_description) {
                (Some(summary), Some(description)) if summary != description => {
                    Some(format!("{summary}\n\n{description}"))
                }
                _ => description.clone(),
            };

            let parameters = op
                .get("parameters")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let path_params: Vec<&Value> = parameters
                .iter()
                .filter(|p| p.get("in").and_then(|v| v.as_str()) == Some("path"))
                .collect();

            let query_params: Vec<&Value> = parameters
                .iter()
                .filter(|p| p.get("in").and_then(|v| v.as_str()) == Some("query"))
                .collect();
            let mut header_params = parameters
                .iter()
                .filter(|p| p.get("in").and_then(|v| v.as_str()) == Some("header"))
                .cloned()
                .collect::<Vec<_>>();
            if options.security {
                header_params.extend(security_header_params(&resolved, op));
            }

            let (body_props, body_required_set) = extract_body_schema(op);
            let mut mcp_input_schema = operation_input_schema(
                &path_params,
                &query_params,
                &header_params,
                &body_props,
                &body_required_set,
            );
            if options.compact {
                mcp_input_schema = compact_schema(&mcp_input_schema);
            }
            let output_schema = extract_output_schema(op);

            let args_fields: Vec<FieldMeta> = path_params
                .iter()
                .map(|p| param_to_field_meta(p, true))
                .collect();

            let mut options_fields: Vec<FieldMeta> = Vec::new();

            for p in &query_params {
                let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                options_fields.push(param_to_field_meta(p, required));
            }

            for p in &header_params {
                let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                let mut field = param_to_field_meta(p, required);
                let normalized = p
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("header")
                    .to_lowercase();
                field.name = Box::leak(normalized.clone().into_boxed_str());
                field.cli_name = normalized;
                options_fields.push(field);
            }

            for (key, schema) in &body_props {
                let required = body_required_set.contains(key.as_str());
                options_fields.push(body_prop_to_field_meta(key, schema, required));
            }

            let handler_path = path.clone();
            let handler_method = http_method.clone();
            let handler_base_path = options.base_path.clone();
            let handler_fetch = Arc::clone(&fetch_fn);
            let handler_path_param_names: Vec<String> = path_params
                .iter()
                .filter_map(|p| {
                    p.get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            let handler_query_param_names: Vec<String> = query_params
                .iter()
                .filter_map(|p| {
                    p.get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            let handler_body_prop_names: Vec<String> =
                body_props.iter().map(|(k, _)| k.clone()).collect();
            let handler_header_params = header_params
                .iter()
                .filter_map(|parameter| {
                    let name = parameter.get("name")?.as_str()?.to_string();
                    Some((name.to_lowercase(), name))
                })
                .collect();

            let handler = OpenApiHandler {
                fetch_fn: handler_fetch,
                http_method: handler_method,
                path_template: handler_path,
                base_path: handler_base_path,
                path_param_names: handler_path_param_names,
                query_param_names: handler_query_param_names,
                body_prop_names: handler_body_prop_names,
                header_params: handler_header_params,
                forward_headers: options.forward_headers.clone(),
                mcp: crate::command::McpCommandOptions {
                    description: mcp_description,
                    annotations: Some(crate::command::McpAnnotations {
                        read_only_hint: Some(matches!(
                            http_method.as_str(),
                            "GET" | "HEAD" | "OPTIONS"
                        )),
                        destructive_hint: Some(!matches!(
                            http_method.as_str(),
                            "GET" | "HEAD" | "OPTIONS"
                        )),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                mcp_input_schema,
            };

            let cmd_def = CommandDef {
                name: name.clone(),
                description: description.clone(),
                args_fields: args_fields.clone(),
                options_fields: options_fields.clone(),
                env_fields: Vec::new(),
                aliases: std::collections::HashMap::new(),
                command_aliases: Vec::new(),
                examples: Vec::new(),
                hint: None,
                format: None,
                output_policy: None,
                handler: Box::new(handler),
                middleware: Vec::new(),
                output_schema,
            };

            commands.insert(name, cmd_def);
        }
    }

    Ok(commands)
}

/// Stub for when the `openapi` feature is disabled. Always returns an error.
#[cfg(not(feature = "openapi"))]
pub fn generate_commands(
    _spec: &Value,
) -> Result<BTreeMap<String, GeneratedCommand>, crate::errors::Error> {
    Err(crate::errors::Error::Other(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "OpenAPI support requires the 'openapi' feature flag",
    ))))
}

// ---------------------------------------------------------------------------
// Handler (behind feature flag)
// ---------------------------------------------------------------------------

#[cfg(feature = "openapi")]
struct OpenApiHandler {
    fetch_fn: FetchFn,
    http_method: String,
    path_template: String,
    base_path: Option<String>,
    path_param_names: Vec<String>,
    query_param_names: Vec<String>,
    body_prop_names: Vec<String>,
    header_params: Vec<(String, String)>,
    forward_headers: Vec<String>,
    mcp: crate::command::McpCommandOptions,
    mcp_input_schema: Value,
}

#[cfg(feature = "openapi")]
impl std::fmt::Debug for OpenApiHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenApiHandler")
            .field("http_method", &self.http_method)
            .field("path_template", &self.path_template)
            .finish()
    }
}

#[cfg(feature = "openapi")]
#[async_trait::async_trait]
impl crate::command::CommandHandler for OpenApiHandler {
    async fn run(&self, ctx: crate::command::CommandContext) -> crate::output::CommandResult {
        let args = ctx.args.as_object().cloned().unwrap_or_default();
        let options = ctx.options.as_object().cloned().unwrap_or_default();

        let mut url_path = format!(
            "{}{}",
            self.base_path.as_deref().unwrap_or(""),
            self.path_template
        );
        for param_name in &self.path_param_names {
            if let Some(value) = args.get(param_name) {
                let str_val = value_to_string(value);
                url_path = url_path.replace(&format!("{{{}}}", param_name), &str_val);
            }
        }

        let mut query_parts: Vec<String> = Vec::new();
        for param_name in &self.query_param_names {
            if let Some(value) = options.get(param_name) {
                if !value.is_null() {
                    let str_val = value_to_string(value);
                    query_parts.push(format!(
                        "{}={}",
                        urlencoding::encode(param_name),
                        urlencoding::encode(&str_val)
                    ));
                }
            }
        }

        let full_url = if query_parts.is_empty() {
            url_path
        } else {
            format!("{}?{}", url_path, query_parts.join("&"))
        };

        let mut headers: Vec<(String, String)> = self
            .header_params
            .iter()
            .filter_map(|(option, header)| {
                options
                    .get(option)
                    .filter(|value| !value.is_null())
                    .map(|value| (header.clone(), value_to_string(value)))
            })
            .collect();
        for name in &self.forward_headers {
            if headers
                .iter()
                .any(|(header, _)| header.eq_ignore_ascii_case(name))
            {
                continue;
            }
            if let Some(value) = ctx
                .request
                .as_ref()
                .and_then(|request| request.headers.get(&name.to_lowercase()))
            {
                headers.push((name.clone(), value.clone()));
            }
        }
        let body = if !self.body_prop_names.is_empty() {
            let mut body_obj = serde_json::Map::new();
            for key in &self.body_prop_names {
                if let Some(value) = options.get(key) {
                    if !value.is_null() {
                        body_obj.insert(key.clone(), value.clone());
                    }
                }
            }
            if body_obj.is_empty() {
                None
            } else {
                headers.push(("content-type".to_string(), "application/json".to_string()));
                Some(serde_json::to_string(&body_obj).unwrap_or_default())
            }
        } else {
            None
        };

        let result = (self.fetch_fn)(full_url, self.http_method.clone(), headers, body).await;

        if let Some(obj) = result.as_object() {
            if obj.get("ok") == Some(&Value::Bool(false)) {
                let message = obj
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| obj.get("error").and_then(|v| v.as_str()))
                    .unwrap_or("Request failed")
                    .to_string();
                let code = obj
                    .get("status")
                    .and_then(|v| v.as_u64())
                    .map(|s| format!("HTTP_{}", s))
                    .unwrap_or_else(|| "HTTP_ERROR".to_string());
                return crate::output::CommandResult::Error {
                    code,
                    message,
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        }

        crate::output::CommandResult::Ok {
            data: result,
            cta: None,
        }
    }

    fn mcp_options(&self) -> Option<&crate::command::McpCommandOptions> {
        Some(&self.mcp)
    }

    fn mcp_input_schema(&self) -> Option<&Value> {
        Some(&self.mcp_input_schema)
    }
}

// ---------------------------------------------------------------------------
// URL encoding (simple inline implementation)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
mod urlencoding {
    pub fn encode(input: &str) -> String {
        let mut encoded = String::with_capacity(input.len());
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => {
                    encoded.push('%');
                    encoded.push(HEX_UPPER[(byte >> 4) as usize] as char);
                    encoded.push(HEX_UPPER[(byte & 0x0f) as usize] as char);
                }
            }
        }
        encoded
    }

    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[allow(dead_code)]
fn generate_operation_name(method: &str, path: &str) -> String {
    let sanitized: String = path
        .chars()
        .map(|c| match c {
            '/' | '{' | '}' => '_',
            _ => c,
        })
        .collect();
    format!("{}_{}", method, sanitized)
}

#[cfg(feature = "openapi")]
fn namespace_segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment
                .trim_start_matches('{')
                .trim_end_matches('}')
                .to_string()
        })
        .collect()
}

#[cfg(feature = "openapi")]
fn generate_namespace_name(method: &str, segments: &[String], needs_method: bool) -> String {
    if segments.is_empty() {
        method.to_string()
    } else if needs_method {
        format!("{} {method}", segments.join(" "))
    } else {
        segments.join(" ")
    }
}

#[cfg(feature = "openapi")]
fn security_header_params(spec: &Value, operation: &serde_json::Map<String, Value>) -> Vec<Value> {
    let schemes = spec
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object);
    let requirements = operation
        .get("security")
        .or_else(|| spec.get("security"))
        .and_then(Value::as_array);
    let mut headers = Vec::new();
    for requirement in requirements.into_iter().flatten() {
        for name in requirement
            .as_object()
            .into_iter()
            .flat_map(|value| value.keys())
        {
            let Some(scheme) = schemes.and_then(|schemes| schemes.get(name)) else {
                continue;
            };
            let parameter = if scheme.get("type").and_then(Value::as_str) == Some("apiKey")
                && scheme.get("in").and_then(Value::as_str) == Some("header")
            {
                scheme.get("name").and_then(Value::as_str).map(|header| {
                    serde_json::json!({
                        "name": header,
                        "in": "header",
                        "required": false,
                        "description": scheme.get("description").and_then(Value::as_str).unwrap_or("Credential header"),
                        "schema": { "type": "string" },
                    })
                })
            } else if scheme.get("type").and_then(Value::as_str) == Some("http")
                && matches!(
                    scheme
                        .get("scheme")
                        .and_then(Value::as_str)
                        .map(str::to_lowercase)
                        .as_deref(),
                    Some("basic" | "bearer")
                )
            {
                Some(serde_json::json!({
                    "name": "authorization",
                    "in": "header",
                    "required": false,
                    "description": scheme.get("description").and_then(Value::as_str).unwrap_or("Authorization header"),
                    "schema": { "type": "string" },
                }))
            } else {
                None
            };
            if let Some(parameter) = parameter
                && !headers
                    .iter()
                    .any(|existing: &Value| existing["name"].as_str() == parameter["name"].as_str())
            {
                headers.push(parameter);
            }
        }
    }
    headers
}

#[allow(dead_code)]
fn schema_type_to_field_type(schema: Option<&Value>) -> FieldType {
    let schema = match schema {
        Some(s) => s,
        None => return FieldType::String,
    };

    match schema.get("type").and_then(|t| t.as_str()) {
        Some("integer") | Some("number") => FieldType::Number,
        Some("boolean") => FieldType::Boolean,
        Some("array") => {
            let items_type = schema
                .get("items")
                .and_then(|i| i.get("type"))
                .and_then(|t| t.as_str());
            let inner = match items_type {
                Some("integer") | Some("number") => FieldType::Number,
                Some("boolean") => FieldType::Boolean,
                _ => FieldType::String,
            };
            FieldType::Array(Box::new(inner))
        }
        _ => FieldType::String,
    }
}

#[allow(dead_code)]
fn param_to_field_meta(param: &Value, required: bool) -> FieldMeta {
    let name = param
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let description = param.get("description").and_then(|v| v.as_str());
    let schema = param.get("schema");
    let field_type = schema_type_to_field_type(schema);

    let name_static: &'static str = Box::leak(name.to_string().into_boxed_str());
    let desc_static: Option<&'static str> =
        description.map(|d| &*Box::leak(d.to_string().into_boxed_str()));

    FieldMeta {
        name: name_static,
        cli_name: crate::schema::to_kebab(name),
        description: desc_static,
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

#[allow(dead_code)]
fn body_prop_to_field_meta(key: &str, schema: &Value, required: bool) -> FieldMeta {
    let description = schema.get("description").and_then(|v| v.as_str());
    let field_type = schema_type_to_field_type(Some(schema));

    let name_static: &'static str = Box::leak(key.to_string().into_boxed_str());
    let desc_static: Option<&'static str> =
        description.map(|d| &*Box::leak(d.to_string().into_boxed_str()));

    FieldMeta {
        name: name_static,
        cli_name: crate::schema::to_kebab(key),
        description: desc_static,
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

#[allow(dead_code)]
fn extract_body_schema(
    operation: &serde_json::Map<String, Value>,
) -> (Vec<(String, Value)>, std::collections::HashSet<String>) {
    let body = match operation.get("requestBody").and_then(|v| v.as_object()) {
        Some(b) => b,
        None => return (Vec::new(), std::collections::HashSet::new()),
    };

    let content = match body.get("content").and_then(|v| v.as_object()) {
        Some(c) => c,
        None => return (Vec::new(), std::collections::HashSet::new()),
    };

    let json_content = match content.get("application/json").and_then(|v| v.as_object()) {
        Some(j) => j,
        None => return (Vec::new(), std::collections::HashSet::new()),
    };

    let schema = match json_content.get("schema").and_then(|v| v.as_object()) {
        Some(s) => s,
        None => return (Vec::new(), std::collections::HashSet::new()),
    };

    let properties = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return (Vec::new(), std::collections::HashSet::new()),
    };

    let required_set: std::collections::HashSet<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let props: Vec<(String, Value)> = properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    (props, required_set)
}

#[cfg(feature = "openapi")]
fn operation_input_schema(
    path: &[&Value],
    query: &[&Value],
    headers: &[Value],
    body: &[(String, Value)],
    body_required: &std::collections::HashSet<String>,
) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in path
        .iter()
        .copied()
        .chain(query.iter().copied())
        .chain(headers.iter())
    {
        let Some(raw_name) = parameter.get("name").and_then(Value::as_str) else {
            continue;
        };
        let name = if parameter.get("in").and_then(Value::as_str) == Some("header") {
            raw_name.to_lowercase()
        } else {
            raw_name.to_string()
        };
        let mut schema = parameter
            .get("schema")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "type": "string" }));
        if let Some(description) = parameter.get("description") {
            schema["description"] = description.clone();
        }
        properties.insert(name.clone(), schema);
        if parameter.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(Value::String(name));
        }
    }
    for (name, schema) in body {
        properties.insert(name.clone(), schema.clone());
        if body_required.contains(name) {
            required.push(Value::String(name.clone()));
        }
    }
    let mut result = serde_json::Map::from_iter([
        ("type".to_string(), Value::String("object".to_string())),
        ("properties".to_string(), Value::Object(properties)),
    ]);
    if !required.is_empty() {
        result.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(result)
}

#[cfg(feature = "openapi")]
fn extract_output_schema(operation: &serde_json::Map<String, Value>) -> Option<Value> {
    operation
        .get("responses")?
        .as_object()?
        .iter()
        .find(|(status, _)| status.starts_with('2'))?
        .1
        .pointer("/content/application~1json/schema")
        .cloned()
}

#[cfg(feature = "openapi")]
fn compact_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(schema) => {
            let mut result = serde_json::Map::new();
            for (key, value) in schema {
                if matches!(key.as_str(), "example" | "examples") {
                    continue;
                }
                if key == "pattern" && value.as_str().is_some_and(|pattern| pattern.len() > 100) {
                    continue;
                }
                if key == "format"
                    && matches!(
                        value.as_str(),
                        Some("date" | "date-time" | "duration" | "time")
                    )
                {
                    continue;
                }
                result.insert(key.clone(), compact_schema(value));
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(values.iter().map(compact_schema).collect()),
        value => value.clone(),
    }
}

#[allow(dead_code)]
fn resolve_refs(value: &Value, root: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(ref_str) = map.get("$ref").and_then(|v| v.as_str())
                && let Some(resolved) = resolve_json_pointer(root, ref_str)
            {
                return resolve_refs(resolved, root);
            }
            let new_map: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), resolve_refs(v, root)))
                .collect();
            Value::Object(new_map)
        }
        Value::Array(arr) => {
            let new_arr: Vec<Value> = arr.iter().map(|v| resolve_refs(v, root)).collect();
            Value::Array(new_arr)
        }
        other => other.clone(),
    }
}

#[allow(dead_code)]
fn resolve_json_pointer<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    let path = pointer.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let decoded = segment.replace("~1", "/").replace("~0", "~");
        current = current.get(&decoded)?;
    }
    Some(current)
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

    #[test]
    fn test_generate_operation_name() {
        assert_eq!(generate_operation_name("get", "/users"), "get__users");
        assert_eq!(
            generate_operation_name("post", "/users/{id}"),
            "post__users__id_"
        );
    }

    #[test]
    fn test_schema_type_to_field_type() {
        assert_eq!(
            schema_type_to_field_type(Some(&serde_json::json!({"type": "string"}))),
            FieldType::String
        );
        assert_eq!(
            schema_type_to_field_type(Some(&serde_json::json!({"type": "number"}))),
            FieldType::Number
        );
        assert_eq!(
            schema_type_to_field_type(Some(&serde_json::json!({"type": "integer"}))),
            FieldType::Number
        );
        assert_eq!(
            schema_type_to_field_type(Some(&serde_json::json!({"type": "boolean"}))),
            FieldType::Boolean
        );
        assert_eq!(schema_type_to_field_type(None), FieldType::String);
    }

    #[test]
    fn test_param_to_field_meta() {
        let param = serde_json::json!({
            "name": "userId",
            "in": "path",
            "required": true,
            "schema": {"type": "integer"},
            "description": "The user ID"
        });
        let field = param_to_field_meta(&param, true);
        assert_eq!(field.name, "userId");
        assert_eq!(field.field_type, FieldType::Number);
        assert!(field.required);
        assert_eq!(field.description, Some("The user ID"));
    }

    #[test]
    fn test_extract_body_schema() {
        let op = serde_json::json!({
            "requestBody": {
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "age": {"type": "integer"}
                            },
                            "required": ["name"]
                        }
                    }
                }
            }
        });
        let op_obj = op.as_object().unwrap();
        let (props, required) = extract_body_schema(op_obj);
        assert_eq!(props.len(), 2);
        assert!(required.contains("name"));
        assert!(!required.contains("age"));
    }

    #[test]
    fn test_resolve_refs() {
        let spec = serde_json::json!({
            "components": {
                "schemas": {
                    "User": {
                        "type": "object",
                        "properties": { "name": {"type": "string"} }
                    }
                }
            },
            "paths": {
                "/users": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/User"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let resolved = resolve_refs(&spec, &spec);
        let schema = resolved
            .pointer("/paths/~1users/get/responses/200/content/application~1json/schema")
            .unwrap();
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[test]
    fn test_value_to_string() {
        assert_eq!(value_to_string(&Value::String("hello".into())), "hello");
        assert_eq!(value_to_string(&Value::from(42)), "42");
        assert_eq!(value_to_string(&Value::Bool(true)), "true");
        assert_eq!(value_to_string(&Value::Null), "");
    }

    #[test]
    fn test_resolve_json_pointer() {
        let root = serde_json::json!({
            "components": {
                "schemas": {
                    "User": {"type": "object"}
                }
            }
        });
        let result = resolve_json_pointer(&root, "#/components/schemas/User");
        assert!(result.is_some());
        assert!(resolve_json_pointer(&root, "#/nonexistent/path").is_none());
    }

    #[cfg(not(feature = "openapi"))]
    #[test]
    fn test_generate_commands_without_feature() {
        let spec = serde_json::json!({});
        let result = generate_commands(&spec);
        assert!(result.is_err());
    }

    #[test]
    fn test_url_encoding() {
        assert_eq!(urlencoding::encode("hello world"), "hello%20world");
        assert_eq!(urlencoding::encode("a=b&c=d"), "a%3Db%26c%3Dd");
        assert_eq!(urlencoding::encode("simple"), "simple");
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_generate_commands_from_spec() {
        use std::sync::Arc;

        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/users": {
                    "get": {
                        "operationId": "listUsers",
                        "summary": "List users",
                        "parameters": [{
                            "name": "limit",
                            "in": "query",
                            "schema": {"type": "number"},
                            "description": "Max results"
                        }]
                    },
                    "post": {
                        "operationId": "createUser",
                        "summary": "Create a user",
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": { "name": {"type": "string"} },
                                        "required": ["name"]
                                    }
                                }
                            }
                        }
                    }
                },
                "/users/{id}": {
                    "get": {
                        "operationId": "getUser",
                        "summary": "Get a user by ID",
                        "parameters": [{
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "number"},
                            "description": "User ID"
                        }]
                    },
                    "delete": {
                        "operationId": "deleteUser",
                        "summary": "Delete a user",
                        "parameters": [{
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "number"}
                        }]
                    }
                },
                "/health": {
                    "get": {
                        "operationId": "healthCheck",
                        "summary": "Health check"
                    }
                }
            }
        });

        let fetch_fn: FetchFn = Arc::new(|_url, _method, _headers, _body| {
            Box::pin(async { serde_json::json!({"ok": true}) })
        });

        let commands = generate_commands(&spec, fetch_fn, &GenerateOptions::default())
            .await
            .unwrap();

        assert!(commands.contains_key("listUsers"));
        assert!(commands.contains_key("createUser"));
        assert!(commands.contains_key("getUser"));
        assert!(commands.contains_key("deleteUser"));
        assert!(commands.contains_key("healthCheck"));

        let list_users = &commands["listUsers"];
        assert_eq!(list_users.description.as_deref(), Some("List users"));
        assert!(list_users.args_fields.is_empty());
        assert_eq!(list_users.options_fields.len(), 1);
        assert_eq!(list_users.options_fields[0].name, "limit");

        let get_user = &commands["getUser"];
        assert_eq!(get_user.args_fields.len(), 1);
        assert_eq!(get_user.args_fields[0].name, "id");
        assert_eq!(get_user.args_fields[0].field_type, FieldType::Number);
        assert!(get_user.args_fields[0].required);

        let create_user = &commands["createUser"];
        assert!(create_user.args_fields.is_empty());
        assert_eq!(create_user.options_fields.len(), 1);
        assert_eq!(create_user.options_fields[0].name, "name");
        assert!(create_user.options_fields[0].required);

        let namespace = generate_commands(
            &spec,
            Arc::new(|_url, _method, _headers, _body| {
                Box::pin(async { serde_json::json!({"ok": true}) })
            }),
            &GenerateOptions {
                mode: GenerateMode::Namespace,
                ..GenerateOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            namespace.keys().cloned().collect::<Vec<_>>(),
            vec![
                "health".to_string(),
                "users get".to_string(),
                "users id delete".to_string(),
                "users id get".to_string(),
                "users post".to_string(),
            ]
        );
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_handler_path_param_interpolation() {
        use std::sync::Arc;

        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/users/{id}": {
                    "get": {
                        "operationId": "getUser",
                        "parameters": [{
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "number"}
                        }]
                    }
                }
            }
        });

        let captured_url = Arc::new(tokio::sync::Mutex::new(String::new()));
        let captured_url_clone = Arc::clone(&captured_url);

        let fetch_fn: FetchFn = Arc::new(move |url, _method, _headers, _body| {
            let captured = Arc::clone(&captured_url_clone);
            Box::pin(async move {
                *captured.lock().await = url;
                serde_json::json!({"id": 42, "name": "Alice"})
            })
        });

        let commands = generate_commands(&spec, fetch_fn, &GenerateOptions::default())
            .await
            .unwrap();

        let ctx = crate::command::CommandContext {
            agent: false,
            args: serde_json::json!({"id": 42}),
            display_name: "test".to_string(),
            env: Value::Null,
            globals: Value::Null,
            options: serde_json::json!({}),
            request: None,
            format: crate::output::Format::Json,
            format_explicit: false,
            name: "test".to_string(),
            vars: Value::Null,
            version: None,
        };

        let _ = commands["getUser"].handler.run(ctx).await;
        let url = captured_url.lock().await;
        assert_eq!(*url, "/users/42");
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_handler_query_params() {
        use std::sync::Arc;

        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/users": {
                    "get": {
                        "operationId": "listUsers",
                        "parameters": [{
                            "name": "limit",
                            "in": "query",
                            "schema": {"type": "number"}
                        }]
                    }
                }
            }
        });

        let captured_url = Arc::new(tokio::sync::Mutex::new(String::new()));
        let captured_url_clone = Arc::clone(&captured_url);

        let fetch_fn: FetchFn = Arc::new(move |url, _method, _headers, _body| {
            let captured = Arc::clone(&captured_url_clone);
            Box::pin(async move {
                *captured.lock().await = url;
                serde_json::json!({"ok": true})
            })
        });

        let commands = generate_commands(&spec, fetch_fn, &GenerateOptions::default())
            .await
            .unwrap();

        let ctx = crate::command::CommandContext {
            agent: false,
            args: serde_json::json!({}),
            display_name: "test".to_string(),
            env: Value::Null,
            globals: Value::Null,
            options: serde_json::json!({"limit": 5}),
            request: None,
            format: crate::output::Format::Json,
            format_explicit: false,
            name: "test".to_string(),
            vars: Value::Null,
            version: None,
        };

        let _ = commands["listUsers"].handler.run(ctx).await;
        let url = captured_url.lock().await;
        assert_eq!(*url, "/users?limit=5");
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_handler_body_params() {
        use std::sync::Arc;

        let spec = serde_json::json!({
            "openapi": "3.0.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/users": {
                    "post": {
                        "operationId": "createUser",
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": { "name": {"type": "string"} },
                                        "required": ["name"]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        let captured_body = Arc::new(tokio::sync::Mutex::new(Option::<String>::None));
        let captured_body_clone = Arc::clone(&captured_body);

        let fetch_fn: FetchFn = Arc::new(move |_url, _method, _headers, body| {
            let captured = Arc::clone(&captured_body_clone);
            Box::pin(async move {
                *captured.lock().await = body;
                serde_json::json!({"created": true, "name": "Bob"})
            })
        });

        let commands = generate_commands(&spec, fetch_fn, &GenerateOptions::default())
            .await
            .unwrap();

        let ctx = crate::command::CommandContext {
            agent: false,
            args: serde_json::json!({}),
            display_name: "test".to_string(),
            env: Value::Null,
            globals: Value::Null,
            options: serde_json::json!({"name": "Bob"}),
            request: None,
            format: crate::output::Format::Json,
            format_explicit: false,
            name: "test".to_string(),
            vars: Value::Null,
            version: None,
        };

        let _ = commands["createUser"].handler.run(ctx).await;
        let body = captured_body.lock().await;
        let body_val: Value = serde_json::from_str(body.as_deref().unwrap()).unwrap();
        assert_eq!(body_val["name"], "Bob");
    }
}
