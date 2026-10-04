//! Plain-text request bytes are checked independently of JSON serialization.
use incurs_forge::{ResolveOptions, resolve_document, runtime::build_http_request};
use serde_json::json;

#[test]
fn plain_text_preserves_utf8_empty_and_omitted_bodies() {
    let doc = serde_json::from_str(include_str!("fixtures/text_openapi.json")).unwrap();
    let contract = resolve_document(&doc, ResolveOptions::new("text")).unwrap();
    let operation = &contract.operations[0];
    for value in ["hello\n\"雪\"\0", ""] {
        let request = build_http_request(
            operation,
            &contract.schemas,
            "https://example.test",
            &json!({"body":value}),
        )
        .unwrap();
        assert_eq!(request.body.as_deref(), Some(value.as_bytes()));
        assert_eq!(
            request.headers,
            vec![("Content-Type".into(), "text/plain".into())]
        );
    }
    let request = build_http_request(
        operation,
        &contract.schemas,
        "https://example.test",
        &json!({}),
    )
    .unwrap();
    assert_eq!(request.body, None);
    assert!(request.headers.is_empty());
    for value in [json!(null), json!(1), json!({}), json!([])] {
        assert!(
            build_http_request(
                operation,
                &contract.schemas,
                "https://example.test",
                &json!({"body":value})
            )
            .is_err()
        );
    }
}
