//! Wire expectations are literal and independent of the binding implementation.
use incurs_forge::runtime::{
    ForgeHttpRequest, ForgeHttpResponse, ForgeTransport, build_http_request, invoke_http,
};
use incurs_forge::{ApiResponse, Operation, Parameter, RequestBody};
use serde_json::json;
use std::{
    collections::BTreeMap,
    future::Future,
    task::{Context, Poll, Waker},
};

fn parameter(name: &str, location: &str, schema: serde_json::Value) -> Parameter {
    Parameter {
        content: None,
        name: name.into(),
        location: location.into(),
        required: location == "path",
        schema,
        style: if ["path", "header"].contains(&location) {
            "simple"
        } else {
            "form"
        }
        .into(),
        explode: ["query", "cookie"].contains(&location),
    }
}

fn operation() -> Operation {
    Operation {
        id: "proof/update".into(),
        name: "update".into(),
        description: None,
        method: "PATCH".into(),
        path: "/things/{id}".into(),
        servers: Vec::new(),
        parameters: vec![
            parameter("id", "path", json!({"type":"string"})),
            parameter(
                "tag",
                "query",
                json!({"type":"array","items":{"type":"string"}}),
            ),
            parameter("limit", "query", json!({"type":"integer","default":25})),
            parameter("X-Trace", "header", json!({"type":"string"})),
            parameter("session", "cookie", json!({"type":"string"})),
        ],
        request_body: Some(RequestBody {
            encoding: BTreeMap::new(),
            required: false,
            content: BTreeMap::from([(
                "application/json".into(),
                json!({"type":["object","null"]}),
            )]),
        }),
        responses: BTreeMap::from([(
            "204".into(),
            ApiResponse {
                description: "No content".into(),
                content: BTreeMap::new(),
                headers: BTreeMap::new(),
            },
        )]),
        security: json!([]),
    }
}

#[test]
fn preserves_wire_locations_explicit_defaults_and_null() {
    let request = build_http_request(
        &operation(),
        &Default::default(),
        "https://example.test/v1/",
        &json!({
            "path":{"id":"a/b?"}, "query":{"tag":["a&b","c d"],"limit":25},
            "header":{"X-Trace":"trace-7"}, "cookie":{"session":"one;two"}, "body":null
        }),
    )
    .unwrap();
    assert_eq!(request.method, "PATCH");
    assert_eq!(
        request.url,
        "https://example.test/v1/things/a%2Fb%3F?tag=a%26b&tag=c%20d&limit=25"
    );
    assert_eq!(
        request.headers,
        vec![
            ("X-Trace".into(), "trace-7".into()),
            ("Cookie".into(), "session=one%3Btwo".into()),
            ("Content-Type".into(), "application/json".into()),
        ]
    );
    assert_eq!(request.body, Some(b"null".to_vec()));
}

#[test]
fn omission_is_not_null_and_required_body_rejects_omission() {
    let mut op = operation();
    let args = json!({"path":{"id":"7"}});
    let request =
        build_http_request(&op, &Default::default(), "https://example.test", &args).unwrap();
    assert_eq!(request.body, None);
    assert!(request.headers.is_empty());
    op.request_body.as_mut().unwrap().required = true;
    assert!(
        build_http_request(&op, &Default::default(), "https://example.test", &args)
            .unwrap_err()
            .0
            .contains("required request body")
    );
}

#[test]
fn rejects_unknown_arguments_missing_path_and_header_injection() {
    for args in [
        json!({}),
        json!({"path":{"id":"7","unknown":"x"}}),
        json!({"path":{"id":"7"},"mystery":true}),
        json!({"path":{"id":"7"},"query":[]}),
        json!({"path":{"id":"7"},"header":{"X-Trace":"valid\r\nInjected: yes"}}),
    ] {
        assert!(
            build_http_request(
                &operation(),
                &Default::default(),
                "https://example.test",
                &args
            )
            .is_err(),
            "{args}"
        );
    }
}

#[test]
fn rejects_undefined_style_combinations_and_nested_query_values() {
    let mut op = operation();
    op.parameters[1].style = "pipeDelimited".into();
    op.parameters[1].explode = true;
    assert!(
        build_http_request(
            &op,
            &Default::default(),
            "https://example.test",
            &json!({
                "path":{"id":"7"},"query":{"tag":["a","b"]}
            })
        )
        .unwrap_err()
        .0
        .contains("explode=true is undefined")
    );
    assert!(
        build_http_request(
            &operation(),
            &Default::default(),
            "https://example.test",
            &json!({
                "path":{"id":"7"},"query":{"tag":[{"nested":true}]}
            })
        )
        .is_err()
    );
}

#[test]
fn query_objects_and_simple_path_objects_have_explicit_serialization() {
    let mut op = operation();
    op.parameters = vec![
        parameter("id", "path", json!({})),
        parameter("filter", "query", json!({})),
    ];
    op.parameters[0].explode = true;
    op.parameters[1].style = "deepObject".into();
    let request = build_http_request(
        &op,
        &Default::default(),
        "https://example.test",
        &json!({
            "path":{"id":{"x":"a/b","y":2}}, "query":{"filter":{"label":"a b","on":true}}
        }),
    )
    .unwrap();
    assert_eq!(
        request.url,
        "https://example.test/things/x=a%2Fb,y=2?filter%5Blabel%5D=a%20b&filter%5Bon%5D=true"
    );
}

#[test]
fn media_selection_is_explicit_and_undeclared_bodies_fail() {
    let mut op = operation();
    let body = op.request_body.as_mut().unwrap();
    body.content
        .insert("application/merge-patch+json".into(), json!({}));
    body.content
        .insert("text/plain".into(), json!({"type":"string"}));
    assert!(
        build_http_request(
            &op,
            &Default::default(),
            "https://example.test",
            &json!({"path":{"id":"7"},"body":null})
        )
        .is_err()
    );
    for media in ["application/unknown", "application/xml"] {
        assert!(
            build_http_request(
                &op,
                &Default::default(),
                "https://example.test",
                &json!({
                    "path":{"id":"7"},"body":"hello","media_type":media
                })
            )
            .is_err()
        );
    }
    let request = build_http_request(
        &op,
        &Default::default(),
        "https://example.test",
        &json!({
            "path":{"id":"7"},"body":{"name":null},"media_type":"application/merge-patch+json"
        }),
    )
    .unwrap();
    assert_eq!(request.body, Some(br#"{"name":null}"#.to_vec()));
}

struct RecordedExchange;
impl ForgeTransport for RecordedExchange {
    fn exchange(
        &self,
        request: ForgeHttpRequest,
    ) -> impl Future<Output = incurs_forge::ForgeResult<ForgeHttpResponse>> {
        assert_eq!(request.url, "https://example.test/things/7");
        std::future::ready(Ok(ForgeHttpResponse {
            status: 429,
            headers: vec![
                ("Retry-After".into(), "12".into()),
                ("Set-Cookie".into(), "a=1".into()),
                ("Set-Cookie".into(), "b=2".into()),
            ],
            body: vec![0, 255, 1],
        }))
    }
}

#[test]
fn exchange_retains_non_success_status_duplicate_headers_and_binary_body() {
    let op = operation();
    let args = json!({"path":{"id":"7"}});
    let schemas = Default::default();
    let future = invoke_http(
        &op,
        &schemas,
        "https://example.test",
        &args,
        &RecordedExchange,
    );
    let mut future = std::pin::pin!(future);
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("immediate exchange unexpectedly pending");
    };
    let response = result.unwrap();
    assert_eq!(response.status, 429);
    assert_eq!(response.headers.len(), 3);
    assert_eq!(response.headers[0], ("Retry-After".into(), "12".into()));
    assert_eq!(response.body, vec![0, 255, 1]);
}

#[test]
fn rejects_values_that_would_disappear_or_change_the_route() {
    for args in [
        json!({"path":{"id":".."}}),
        json!({"path":{"id":"."}}),
        json!({"path":{"id":"7"},"query":{"tag":[]}}),
    ] {
        assert!(
            build_http_request(
                &operation(),
                &Default::default(),
                "https://example.test",
                &args
            )
            .is_err(),
            "{args}"
        );
    }
}

#[test]
fn absent_defaulted_parameter_stays_absent() {
    let request = build_http_request(
        &operation(),
        &Default::default(),
        "https://example.test",
        &json!({
            "path": {"id": "7"}
        }),
    )
    .unwrap();
    assert_eq!(request.url, "https://example.test/things/7");
}
