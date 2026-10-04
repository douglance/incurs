//! Source dialect, graph, and media controls for compiled HTTP bindings.
use incurs_forge::{
    ResolveOptions, ResolvedOpenApi, resolve_document,
    runtime::{HttpBinding, HttpBindingError},
};
use serde_json::{Value, json};

fn resolve(version: &str, schema: Value) -> ResolvedOpenApi {
    resolve_document(
        &json!({
            "openapi":version,"info":{"title":"Request schema context","version":"1"},
            "components":{"schemas":{"Body":schema,"Name":{"type":"string"}}},
            "paths":{"/body":{"post":{"operationId":"send","requestBody":{"required":true,
                "content":{"application/json":{"schema":{"$ref":"#/components/schemas/Body"}}}},
                "responses":{"200":{"description":"OK"}}}}}
        }),
        ResolveOptions::new("context"),
    )
    .unwrap()
}

fn accepts(binding: &HttpBinding, value: Value) -> bool {
    binding
        .build_request(
            "context/send",
            "https://example.test",
            &json!({"body":value}),
        )
        .is_ok()
}

#[test]
fn legacy_nullable_and_readonly_are_request_context_sensitive() {
    let body = json!({"type":"object","required":["id","note","password"],"additionalProperties":false,
        "properties":{"id":{"type":"integer","readOnly":true},
        "note":{"type":"string","nullable":true},"password":{"type":"string","writeOnly":true}}});
    let legacy = HttpBinding::new(&resolve("3.0.4", body.clone())).unwrap();
    assert!(accepts(&legacy, json!({"note":null,"password":"secret"})));
    assert!(!accepts(&legacy, json!({"note":null})));
    assert!(!accepts(
        &legacy,
        json!({"note":null,"password":"secret","id":"wrong"})
    ));
    let modern = HttpBinding::new(&resolve("3.1.1", body)).unwrap();
    assert!(!accepts(&modern, json!({"note":null,"password":"secret"})));
    assert!(!accepts(
        &modern,
        json!({"note":null,"password":"secret","id":1})
    ));
    assert!(accepts(
        &modern,
        json!({"note":"present","password":"secret","id":1})
    ));
    assert_eq!(
        legacy.input_schema("context/send").unwrap()["$defs"]["Body"]["required"],
        json!(["note", "password"])
    );
}

#[test]
fn reference_siblings_follow_the_source_version() {
    for (version, allows_short) in [("3.0.4", true), ("3.1.1", false)] {
        let mut contract = resolve(
            version,
            json!({"type":"object","required":["name"],"properties":{
            "name":{"$ref":"#/components/schemas/Name","minLength":4}}}),
        );
        contract
            .schemas
            .insert("Name".into(), json!({"type":"string"}));
        let binding = HttpBinding::new(&contract).unwrap();
        assert_eq!(accepts(&binding, json!({"name":"AB"})), allows_short);
        assert!(accepts(&binding, json!({"name":"ABCD"})));
        assert!(!accepts(&binding, json!({"name":2})));
    }
}

#[test]
fn unknown_operations_and_unsupported_schema_scopes_fail_closed() {
    let valid = resolve("3.1.1", json!({"type":"object"}));
    let binding = HttpBinding::new(&valid).unwrap();
    assert!(
        binding
            .build_request("missing", "https://example.test", &json!({"body":{}}))
            .is_err()
    );
    for key in ["$id", "$anchor", "$dynamicAnchor", "$dynamicRef"] {
        let mut contract = valid.clone();
        contract
            .schemas
            .get_mut("Body")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!("unsupported"));
        assert!(HttpBinding::new(&contract).is_err(), "{key}");
    }
    let mut contract = valid.clone();
    contract.json_schema_dialect = Some("https://example.test/unknown-dialect".into());
    assert!(HttpBinding::new(&contract).is_err());
    let mut contract = valid;
    contract.schemas.insert(
        "Body".into(),
        json!({"$ref":"#/components/schemas/Missing"}),
    );
    assert!(HttpBinding::new(&contract).is_err());
}

#[test]
fn selected_binary_and_json_media_preserve_distinct_logical_values() {
    let mut contract = resolve(
        "3.1.1",
        json!({"type":"object","required":["ok"],"properties":{"ok":{"const":true}}}),
    );
    let body = contract.operations[0].request_body.as_mut().unwrap();
    body.content.insert(
        "application/octet-stream".into(),
        json!({"type":"string","format":"binary"}),
    );
    let binding = HttpBinding::new(&contract).unwrap();
    let request = binding
        .build_request(
            "context/send",
            "https://example.test",
            &json!({"media_type":"application/octet-stream","body_base64":"AP+A"}),
        )
        .unwrap();
    assert_eq!(request.body, Some(vec![0, 255, 128]));
    assert!(
        binding
            .build_request(
                "context/send",
                "https://example.test",
                &json!({"media_type":"application/json","body":{"ok":true}})
            )
            .is_ok()
    );
    assert!(matches!(
        binding.build_request(
            "context/send",
            "https://example.test",
            &json!({"media_type":"application/json","body":{"ok":false}})
        ),
        Err(HttpBindingError::Validation(_))
    ));
    for arguments in [
        json!({"media_type":"application/json","body_base64":"AP+A"}),
        json!({"media_type":"application/octet-stream","body":{"ok":true}}),
        json!({"body":{"ok":true}}),
    ] {
        assert!(
            binding
                .build_request("context/send", "https://example.test", &arguments)
                .is_err()
        );
    }
}

#[test]
fn shared_references_and_recursive_values_validate_in_each_operation() {
    let mut contract = resolve(
        "3.1.1",
        json!({"type":"object","required":["count"],"additionalProperties":false,
        "properties":{"count":{"type":"integer","minimum":1},"next":{"$ref":"#/components/schemas/Body"}}}),
    );
    for index in 0..20 {
        let mut operation = contract.operations[0].clone();
        operation.id = format!("context/send{index}");
        operation.name = format!("send{index}");
        contract.operations.push(operation);
    }
    let binding = HttpBinding::new(&contract).unwrap();
    for operation in &contract.operations {
        assert!(
            binding
                .build_request(
                    &operation.id,
                    "https://example.test",
                    &json!({"body":{"count":1,"next":{"count":2}}})
                )
                .is_ok()
        );
        assert!(matches!(
            binding.build_request(
                &operation.id,
                "https://example.test",
                &json!({"body":{"count":1,"next":{"count":0}}})
            ),
            Err(HttpBindingError::Validation(_))
        ));
    }
}

#[test]
fn response_only_definitions_do_not_enter_request_validation() {
    let mut contract = resolve("3.1.1", json!({"type":"object"}));
    contract.schemas.insert(
        "ResponseOnly".into(),
        json!({"$id":"https://example.test/response","type":"object"}),
    );
    let binding = HttpBinding::new(&contract).unwrap();
    assert!(accepts(&binding, json!({})));
    assert!(
        binding.input_schema("context/send").unwrap()["$defs"]
            .get("ResponseOnly")
            .is_none()
    );
}

#[test]
fn generated_envelopes_accept_only_empty_unused_parameter_groups() {
    let binding = HttpBinding::new(&resolve(
        "3.1.1",
        json!({
            "type":"object","required":["ok"],"properties":{"ok":{"const":true}}
        }),
    ))
    .unwrap();
    let arguments =
        json!({"path":{},"query":{},"querystring":{},"header":{},"cookie":{},"body":{"ok":true}});
    assert!(
        binding
            .build_request("context/send", "https://example.test", &arguments)
            .is_ok()
    );
    for (key, value) in [
        ("header", json!({"undeclared":"value"})),
        ("extra", json!({})),
        ("query", json!([])),
        ("body", json!({})),
    ] {
        let mut invalid = arguments.clone();
        invalid[key] = value;
        assert!(
            binding
                .build_request("context/send", "https://example.test", &invalid)
                .is_err(),
            "{invalid}"
        );
    }
}
