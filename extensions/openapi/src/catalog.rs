//! Compile resolved OpenAPI contracts into incurs commands and tool discovery.
//! Hosts supply outbound HTTP and declared-server selection; registration performs no I/O.
use crate::{
    OpenApiError, OpenApiResult, Operation, ResolvedOpenApi,
    adapters::IncursHttpTransport,
    runtime::{HttpBinding, HttpBindingError, OpenApiTransport},
    servers::{ServerSelection, select_server_url},
};
use incurs::{
    cli::Cli,
    command::{CommandContext, CommandDef, CommandHandler},
    output::CommandResult,
    schema::{FieldMeta, FieldType},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

/// Return the deterministic leaf command name for an operation.
/// An unmounted compiled CLI exposes this as its ToolCatalog name.
///
/// The op_ prefix avoids built-in command names. Non-alphanumeric UTF-8 bytes
/// are hex escaped, and long names use their SHA-256 digest. Compilation checks
/// the complete set for collisions before returning a CLI.
pub fn tool_name(operation: &Operation) -> String {
    let mut name = String::from("op_");
    for byte in operation.name.bytes() {
        if byte.is_ascii_alphanumeric() {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("_{byte:02x}"));
        }
    }
    if name.len() > 128 {
        format!("op_{}", hex_digest(operation.name.as_bytes()))
    } else {
        name
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Compile every resolved operation into a new, mountable incurs CLI.
///
/// ToolCatalog discovery receives nested JSON Schemas and stable operation IDs.
/// Location objects can be supplied as JSON objects through tools or JSON text
/// through CLI flags. The body field is never implicitly decoded; body_json is
/// the explicit encoded-JSON alternative. Responses contain status, repeated
/// header pairs, and raw body bytes, including non-success HTTP responses.
///
/// The host supplies authentication and HTTP policy through its client. This
/// compiler validates normalized logical arguments against the selected request
/// schema before transport. The media codec handles raw bytes and serialization.
pub fn compile_cli(
    contract: &ResolvedOpenApi,
    transport: IncursHttpTransport,
    selection: &ServerSelection,
) -> OpenApiResult<Cli> {
    let binding = Arc::new(HttpBinding::new(contract)?);
    let mut names = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut cli = Cli::create(&contract.namespace);
    for operation in &contract.operations {
        let name = tool_name(operation);
        if !names.insert(name.clone()) || !identities.insert(operation.id.clone()) {
            return Err(OpenApiError(format!(
                "duplicate operation identity or tool name: {}",
                operation.id
            )));
        }
        let endpoint = select_server_url(operation, selection)?;
        let input = binding.input_schema(&operation.id).unwrap().clone();
        let mut command = CommandDef::build(
            &name,
            HttpOperation {
                operation_id: operation.id.clone(),
                binding: binding.clone(),
                endpoint,
                transport: transport.clone(),
            },
        )
        .mcp_input_schema(input.clone())
        .done();
        command.description = operation
            .description
            .clone()
            .or_else(|| Some(format!("{} {}", operation.method, operation.path)));
        command.options_fields = [
            ("path", "Path parameters as a JSON object."),
            ("query", "Query parameters as a JSON object."),
            ("querystring", "Whole-query parameters as a JSON object."),
            ("header", "Header parameters as a JSON object."),
            ("cookie", "Cookie parameters as a JSON object."),
            ("body", "Request body; CLI strings remain strings."),
            (
                "body_json",
                "Request body encoded as JSON text; mutually exclusive with body.",
            ),
            (
                "body_base64",
                "Raw request bytes encoded as base64; exclusive with body and body_json.",
            ),
            ("media_type", "Request media type declared by the API."),
        ]
        .into_iter()
        .filter(|(field, _)| input["properties"].get(*field).is_some())
        .map(|(field, description)| FieldMeta {
            name: field,
            cli_name: field.replace('_', "-"),
            description: Some(description),
            field_type: if matches!(field, "body_json" | "body_base64" | "media_type") {
                FieldType::String
            } else {
                FieldType::Value
            },
            required: input["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|v| v == field)),
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        })
        .collect();
        command.output_schema = Some(json!({
            "type":"object","additionalProperties":false,"required":["status","headers","body"],
            "properties":{
                "status":{"type":"integer","minimum":0,"maximum":65535},
                "headers":{"type":"array","items":{"type":"array","items":{"type":"string"},"minItems":2,"maxItems":2}},
                "body":{"type":"array","items":{"type":"integer","minimum":0,"maximum":255}}
            }
        }));
        cli = cli.command(name, command);
    }
    Ok(cli)
}

struct HttpOperation {
    operation_id: String,
    binding: Arc<HttpBinding>,
    endpoint: String,
    transport: IncursHttpTransport,
}

#[async_trait::async_trait]
impl CommandHandler for HttpOperation {
    async fn run(&self, context: CommandContext) -> CommandResult {
        let arguments = match normalize_arguments(context.options) {
            Ok(arguments) => arguments,
            Err(error) => return command_error("OPENAPI_ARGUMENTS", error),
        };
        let request =
            match self
                .binding
                .build_request(&self.operation_id, &self.endpoint, &arguments)
            {
                Ok(request) => request,
                Err(HttpBindingError::Binding(error)) => {
                    return command_error("OPENAPI_BINDING", error);
                }
                Err(HttpBindingError::Validation(error)) => {
                    return command_error("OPENAPI_VALIDATION", error);
                }
                Err(HttpBindingError::Transport(error)) => {
                    return command_error("OPENAPI_HTTP", error);
                }
            };
        match self.transport.exchange(request).await {
            Ok(response) => CommandResult::Ok {
                data: json!({"status":response.status,"headers":response.headers,"body":response.body}),
                cta: None,
                exit_code: None,
            },
            Err(error) => command_error("OPENAPI_HTTP", error),
        }
    }
}

fn command_error(code: &str, error: OpenApiError) -> CommandResult {
    CommandResult::Error {
        code: code.into(),
        message: error.to_string(),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    }
}

fn normalize_arguments(mut value: Value) -> OpenApiResult<Value> {
    let arguments = value
        .as_object_mut()
        .ok_or_else(|| OpenApiError("operation arguments must be an object".into()))?;
    for location in ["path", "query", "querystring", "header", "cookie"] {
        if let Some(Value::String(text)) = arguments.get(location) {
            let parsed = serde_json::from_str(text).map_err(|error| {
                OpenApiError(format!("{location} must be a JSON object: {error}"))
            })?;
            arguments.insert(location.into(), parsed);
        }
    }
    if let Some(encoded) = arguments.remove("body_json") {
        if arguments.contains_key("body") || arguments.contains_key("body_base64") {
            return Err(OpenApiError(
                "body, body_json, and body_base64 are mutually exclusive".into(),
            ));
        }
        let text = encoded
            .as_str()
            .ok_or_else(|| OpenApiError("body_json must be JSON text".into()))?;
        let body = serde_json::from_str(text)
            .map_err(|error| OpenApiError(format!("invalid body_json: {error}")))?;
        arguments.insert("body".into(), body);
    }
    Ok(value)
}
