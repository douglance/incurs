//! Raw and framed body representations are checked at the public HTTP boundary.
use incurs_openapi::{
    ResolveOptions, ResolvedOpenApi, resolve_document,
    runtime::{OpenApiHttpRequest, build_http_request},
};
use serde_json::{Value, json};

fn contract() -> ResolvedOpenApi {
    resolve_document(
        &serde_json::from_str(include_str!("fixtures/media_openapi.json")).unwrap(),
        ResolveOptions::new("media-proof"),
    )
    .unwrap()
}
fn request(name: &str, arguments: Value) -> incurs_openapi::OpenApiResult<OpenApiHttpRequest> {
    let contract = contract();
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == name)
        .unwrap();
    build_http_request(
        operation,
        &contract.schemas,
        "https://example.test",
        &arguments,
    )
}
fn assert_wire(name: &str, arguments: Value, content_type: &str, expected: &[u8]) {
    let actual = request(name, arguments).unwrap();
    assert_eq!(actual.method, "POST");
    assert_eq!(actual.body.as_deref(), Some(expected));
    assert_eq!(
        actual.headers,
        vec![("Content-Type".into(), content_type.into())]
    );
}

#[test]
fn raw_binary_has_an_explicit_lossless_adapter_slot() {
    assert_wire(
        "uploadBinary",
        json!({"body_base64":"AP+ADQo="}),
        "application/octet-stream",
        &[0, 255, 128, 13, 10],
    );
    assert_wire(
        "uploadBinary",
        json!({"body_base64":""}),
        "application/octet-stream",
        b"",
    );
    let missing = request("uploadBinary", json!({})).unwrap();
    assert!(missing.body.is_none() && missing.headers.is_empty());
    for bad in [
        json!({"body_base64":null}),
        json!({"body_base64":7}),
        json!({"body_base64":"!"}),
        json!({"body":"raw"}),
        json!({"body":null}),
        json!({"body":"raw","body_base64":"cmF3"}),
    ] {
        assert!(request("uploadBinary", bad).is_err());
    }
}

#[test]
fn text_alternatives_preserve_bytes_and_declared_content_types() {
    for media in ["text/plain", "text/x-markdown"] {
        assert_wire(
            "sendMarkdown",
            json!({"body":"snow 雪\n","media_type":media}),
            media,
            "snow 雪\n".as_bytes(),
        );
    }
    for media in ["application/javascript", "text/javascript"] {
        assert_wire(
            "sendScript",
            json!({"body":"export default 1;","media_type":media}),
            media,
            b"export default 1;",
        );
    }
    assert_wire(
        "sendCharset",
        json!({"body":"雪"}),
        "text/plain; charset=UTF-8",
        "雪".as_bytes(),
    );
    assert_wire(
        "sendJsonBinary",
        json!({"body":"raw"}),
        "application/json",
        br#""raw""#,
    );
    assert!(request("sendMarkdown", json!({"body":"x"})).is_err());
    assert!(
        request(
            "sendMarkdown",
            json!({"body_base64":"eA==","media_type":"text/plain"})
        )
        .is_err()
    );
    assert!(
        request(
            "sendConfig",
            json!({"body":{"name":"x"},"media_type":"text/plain;charset=UTF-8"})
        )
        .is_err()
    );
    assert_wire(
        "sendConfig",
        json!({"body":{"name":"x"},"media_type":"application/json"}),
        "application/json",
        br#"{"name":"x"}"#,
    );
}

#[test]
fn framed_json_documents_preserve_records_without_array_inference() {
    assert_wire(
        "uploadRecords",
        json!({"body":"{\"id\":1}\nnull"}),
        "application/jsonl",
        b"{\"id\":1}\nnull",
    );
    assert_wire(
        "uploadRecords",
        json!({"body":"1\r\n2\r\n"}),
        "application/jsonl",
        b"1\r\n2\r\n",
    );
    assert_wire(
        "uploadNdjson",
        json!({"body_base64":"eyJpZCI6MX0K"}),
        "application/x-ndjson",
        b"{\"id\":1}\n",
    );
    assert_wire(
        "uploadNdjson",
        json!({"body_base64":""}),
        "application/x-ndjson",
        b"",
    );
    for bad in [
        json!({"body":[{"id":1}]}),
        json!({"body":"1\n\n"}),
        json!({"body":"bad\n"}),
    ] {
        assert!(request("uploadRecords", bad).is_err());
    }
    for bad in [
        json!({"body":"1\n"}),
        json!({"body_base64":"MQ=="}),
        json!({"body_base64":"/w=="}),
        json!({"body_base64":"77u/MQo="}),
    ] {
        assert!(request("uploadNdjson", bad).is_err());
    }
}

#[test]
fn mixed_media_body_presence_and_slots_are_explicit() {
    assert_wire(
        "sendMixed",
        json!({"body":null,"media_type":"application/json"}),
        "application/json",
        b"null",
    );
    assert_wire(
        "sendMixed",
        json!({"body_base64":"AP+ADQo=","media_type":"application/octet-stream"}),
        "application/octet-stream",
        &[0, 255, 128, 13, 10],
    );
    assert_wire(
        "sendPatch",
        json!({"body":null,"media_type":"application/merge-patch+json"}),
        "application/merge-patch+json",
        b"null",
    );
    for bad in [
        json!({}),
        json!({"body_base64":""}),
        json!({"body":{},"body_base64":"","media_type":"application/json"}),
        json!({"body_base64":"","media_type":"application/json"}),
    ] {
        assert!(request("sendMixed", bad).is_err());
    }
}
