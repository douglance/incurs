//! JSONL helper oracle for Python OpenAI MCP form-protocol parity.

use std::io::{self, BufRead};

use incurs_openai_mcp_protocol::{
    JsonObject, OpenAIForm, OpenAIFormField, ValidationError, complete_field_submission,
    is_valid_value_with_options, prepare_field_submission, validate_file_selections,
    validate_form_selections,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    field: Option<Value>,
    #[serde(default)]
    form: Option<Value>,
    #[serde(default)]
    value: Value,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    content: Option<JsonObject>,
    #[serde(default)]
    pending_uploads: Option<usize>,
    #[serde(default)]
    uploaded_uris: Option<Vec<String>>,
}

fn main() {
    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            Ok(_) => continue,
            Err(error) => {
                println!(
                    "{}",
                    json!({"ok": false, "error": format!("read_error: {error}")})
                );
                continue;
            }
        };
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => run_request(request),
            Err(error) => json!({"ok": false, "error": format!("invalid_request: {error}")}),
        };
        println!("{response}");
    }
}

fn run_request(request: Request) -> Value {
    let result = match request.operation.as_deref().or(request.op.as_deref()) {
        Some("is_valid_value") => is_valid_value_operation(&request),
        Some("prepare_field_submission") => prepare_field_submission_operation(&request),
        Some("complete_field_submission") => complete_field_submission_operation(&request),
        Some("validate_file_selections") => validate_file_selections_operation(&request),
        Some("validate_form_selections") => validate_form_selections_operation(&request),
        Some(operation) => Err(ValidationError::new(
            "OpenAIFormHelperOperation",
            "operation",
            format!("unsupported operation: {operation}"),
        )),
        None => Err(ValidationError::new(
            "OpenAIFormHelperOperation",
            "operation",
            "missing operation",
        )),
    };
    match result {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(_error) => json!({"ok": false, "error": "validation_error"}),
    }
}

fn is_valid_value_operation(request: &Request) -> Result<Value, ValidationError> {
    let field = parse_field(request)?;
    let uploaded_uris = request.uploaded_uris.as_deref().unwrap_or(&[]);
    let valid = is_valid_value_with_options(
        &field,
        &request.value,
        request.pending_uploads.unwrap_or_default(),
        uploaded_uris,
    )?;
    Ok(Value::Bool(valid))
}

fn prepare_field_submission_operation(request: &Request) -> Result<Value, ValidationError> {
    let field = parse_field(request)?;
    let name = request.name.as_deref().ok_or_else(|| missing("name"))?;
    let content = request.content.as_ref().ok_or_else(|| missing("content"))?;
    let accept = prepare_field_submission(
        &field,
        name,
        content,
        request.pending_uploads.unwrap_or_default(),
    )?;
    serde_json::to_value(accept)
        .map_err(|error| ValidationError::new("OpenAIFormHelperOperation", "", error.to_string()))
}

fn complete_field_submission_operation(request: &Request) -> Result<Value, ValidationError> {
    let field = parse_field(request)?;
    let name = request.name.as_deref().ok_or_else(|| missing("name"))?;
    let content = request.content.as_ref().ok_or_else(|| missing("content"))?;
    let uploaded_uris = request
        .uploaded_uris
        .clone()
        .ok_or_else(|| missing("uploaded_uris"))?;
    complete_field_submission(&field, name, content, uploaded_uris)
}

fn validate_file_selections_operation(request: &Request) -> Result<Value, ValidationError> {
    let form = parse_form(request)?;
    let content = request.content.as_ref().ok_or_else(|| missing("content"))?;
    validate_file_selections(&form, content)?;
    Ok(Value::Null)
}

fn validate_form_selections_operation(request: &Request) -> Result<Value, ValidationError> {
    let form = parse_form(request)?;
    let content = request.content.as_ref().ok_or_else(|| missing("content"))?;
    validate_form_selections(&form, content)?;
    Ok(Value::Null)
}

fn parse_field(request: &Request) -> Result<OpenAIFormField, ValidationError> {
    let field = request.field.as_ref().ok_or_else(|| missing("field"))?;
    serde_json::from_value(field.clone()).map_err(|error| {
        ValidationError::new("OpenAIFormHelperOperation", "field", error.to_string())
    })
}

fn parse_form(request: &Request) -> Result<OpenAIForm, ValidationError> {
    let form = request.form.as_ref().ok_or_else(|| missing("form"))?;
    serde_json::from_value(form.clone()).map_err(|error| {
        ValidationError::new("OpenAIFormHelperOperation", "form", error.to_string())
    })
}

fn missing(path: &'static str) -> ValidationError {
    ValidationError::new("OpenAIFormHelperOperation", path, "missing field")
}
