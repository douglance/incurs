//! A false body schema must reject every supplied JSON value before transport.
use incurs_openapi::{ResolveOptions, resolve_document, runtime::build_http_request};
use serde_json::json;

#[test]
fn false_body_schema_rejects_every_json_kind() {
    let document = serde_json::from_str(include_str!("fixtures/schema_openapi.json")).unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("schema")).unwrap();
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == "sendImpossible")
        .unwrap();
    for value in [
        json!(null),
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({}),
    ] {
        let error = build_http_request(
            operation,
            &contract.schemas,
            "https://example.test",
            &json!({"body":value}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("false schema"), "{error}");
    }
}
