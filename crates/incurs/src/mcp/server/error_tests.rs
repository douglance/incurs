//! MCP failures keep their machine-readable details and error flag.

use super::tool_call_result;
use crate::{
    output::{CtaBlock, CtaEntry, FieldErrorOutput},
    tool::ToolCallOutcome,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn failure(cta: Option<CtaBlock>) -> ToolCallOutcome {
    ToolCallOutcome::Error {
        code: "INPUT_INVALID".into(),
        message: "repair this field".into(),
        retryable: Some(false),
        exit_code: Some(75),
        field_errors: Some(vec![FieldErrorOutput {
            path: "count".into(),
            expected: "number".into(),
            received: "string".into(),
            message: "count must be numeric".into(),
        }]),
        cta,
    }
}

fn expected() -> Value {
    json!({
        "code": "INPUT_INVALID", "message": "repair this field",
        "retryable": false, "exit_code": 75,
        "field_errors": [{
            "path": "count", "expected": "number", "received": "string",
            "message": "count must be numeric",
        }],
    })
}

fn text_body(wire: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let text = wire["content"][0]["text"].as_str().ok_or("missing text")?;
    Ok(serde_json::from_str(text)?)
}

#[test]
fn structured_failure_preserves_all_fields_and_error_flag() -> TestResult {
    let result = tool_call_result("fixture", failure(None), true, &[]);
    let wire = serde_json::to_value(result)?;
    assert_eq!(wire["isError"], true);
    assert_eq!(wire["structuredContent"], expected());
    assert_eq!(text_body(&wire)?, expected());
    Ok(())
}

#[test]
fn text_only_failure_is_still_machine_readable() -> TestResult {
    let result = tool_call_result("fixture", failure(None), false, &[]);
    let wire = serde_json::to_value(result)?;
    assert_eq!(wire["isError"], true);
    assert!(wire.get("structuredContent").is_none());
    assert_eq!(text_body(&wire)?, expected());
    Ok(())
}

#[test]
fn follow_up_guidance_does_not_corrupt_the_error_json() -> TestResult {
    let cta = CtaBlock {
        description: Some("Try again:".into()),
        commands: vec![CtaEntry::Simple("retry".into())],
    };
    let result = tool_call_result("fixture", failure(Some(cta)), true, &[]);
    let wire = serde_json::to_value(result)?;
    assert_eq!(wire["isError"], true);
    assert_eq!(text_body(&wire)?, expected());
    assert_eq!(wire["structuredContent"], expected());
    assert_eq!(wire["content"][1]["text"], "Try again:\n  fixture retry");
    assert_eq!(
        wire["_meta"]["cta"]["commands"][0]["command"],
        "fixture retry"
    );
    Ok(())
}

#[test]
fn absent_error_details_stay_absent() -> TestResult {
    let outcome = ToolCallOutcome::Error {
        code: "NOT_FOUND".into(),
        message: String::new(),
        retryable: None,
        field_errors: None,
        cta: None,
        exit_code: None,
    };
    let result = tool_call_result("fixture", outcome, true, &[]);
    let wire = serde_json::to_value(result)?;
    let expected = json!({"code": "NOT_FOUND", "message": "Command failed"});
    assert_eq!(wire["isError"], true);
    assert_eq!(wire["structuredContent"], expected);
    assert_eq!(text_body(&wire)?, expected);
    Ok(())
}

#[test]
fn successful_output_keeps_its_original_shape() -> TestResult {
    let outcome = ToolCallOutcome::Ok {
        data: json!({"answer": 42}),
        cta: None,
    };
    let result = tool_call_result("fixture", outcome, true, &[]);
    let wire = serde_json::to_value(result)?;
    assert_ne!(wire["isError"], true);
    assert_eq!(wire["structuredContent"], json!({"answer": 42}));
    assert_eq!(text_body(&wire)?, json!({"answer": 42}));
    Ok(())
}
