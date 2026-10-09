//! JSONL schema oracle for the OpenAI MCP protocol registry.

use std::io::{self, BufRead};

use incurs_openai_mcp_protocol::{ValidationError, validate_form_content, validate_schema};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Request {
    schema: String,
    value: Value,
    #[serde(default)]
    form: Option<Value>,
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
            Ok(request) => match run_request(request) {
                Ok(value) => json!({"ok": true, "value": value}),
                Err(error) => {
                    json!({"ok": false, "error": "validation_error", "message": error.message, "path": error.path})
                }
            },
            Err(error) => json!({"ok": false, "error": format!("invalid_request: {error}")}),
        };
        println!("{response}");
    }
}

fn run_request(request: Request) -> Result<Value, ValidationError> {
    if request.schema == "OpenAIFormContentSchema" {
        let form = request.form.as_ref().ok_or_else(|| {
            ValidationError::new("OpenAIFormContentSchema", "form", "missing form")
        })?;
        validate_form_content(form, &request.value)
    } else {
        validate_schema(&request.schema, &request.value)
    }
}
