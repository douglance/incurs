//! OpenAPI 3.2 operation and parameter wire semantics.
use incurs_forge::{ResolveOptions, resolve_document, runtime::build_http_request};
use serde_json::{Value, json};

fn document() -> Value {
    serde_json::from_str(include_str!("fixtures/openapi32.json")).unwrap()
}
fn request(
    name: &str,
    args: Value,
) -> incurs_forge::ForgeResult<incurs_forge::runtime::ForgeHttpRequest> {
    let contract = resolve_document(&document(), ResolveOptions::new("v32"))?;
    let operation = contract
        .operations
        .iter()
        .find(|op| op.name == name)
        .unwrap();
    build_http_request(operation, &contract.schemas, "https://example.test", &args)
}
#[test]
fn query_and_additional_methods_preserve_wire_case() {
    let contract = resolve_document(&document(), ResolveOptions::new("v32")).unwrap();
    assert_eq!(
        contract
            .operations
            .iter()
            .map(|op| (op.name.as_str(), op.method.as_str()))
            .collect::<Vec<_>>(),
        [
            ("copyItem", "x-Copy"),
            ("findItems", "QUERY"),
            ("readContent", "GET"),
            ("readRaw", "GET")
        ]
    );
    let req = request(
        "copyItem",
        json!({"path":{"id":"a/b"},"header":{"X-Meta":{"id":9007199254740993_u64}}}),
    )
    .unwrap();
    assert_eq!(req.method, "x-Copy");
    assert_eq!(req.url, "https://example.test/copy/a%2Fb");
    assert_eq!(
        req.headers,
        vec![("X-Meta".into(), "{\"id\":9007199254740993}".into())]
    );
}
#[test]
fn querystring_uses_whole_form_content_and_cookie_style_has_no_uri_encoding() {
    let req = request(
        "findItems",
        json!({"querystring":{"filter":{"term":"a + b/é","labels":["red","blue"]}},
        "cookie":{"prefs":{"mode":"dark","token":"a%2Fb"}}}),
    )
    .unwrap();
    assert_eq!(req.method, "QUERY");
    assert_eq!(
        req.url,
        "https://example.test/find?labels=red&labels=blue&term=a+%2B+b%2F%C3%A9"
    );
    assert_eq!(
        req.headers,
        vec![("Cookie".into(), "mode=dark; token=a%2Fb".into())]
    );
    assert_eq!(req.body, None);
}
#[test]
fn raw_querystring_preserves_valid_uri_text_and_presence() {
    assert_eq!(
        request("readRaw", json!({})).unwrap().url,
        "https://example.test/raw"
    );
    assert_eq!(
        request("readRaw", json!({"querystring":{"raw":""}}))
            .unwrap()
            .url,
        "https://example.test/raw?"
    );
    assert_eq!(
        request("readRaw", json!({"querystring":{"raw":"x=a%2Fb&x=c+d"}}))
            .unwrap()
            .url,
        "https://example.test/raw?x=a%2Fb&x=c+d"
    );
    for raw in ["x=a#fragment", "x=a b", "x=%GG", "x=é", "x=a\nb"] {
        assert!(
            request("readRaw", json!({"querystring":{"raw":raw}})).is_err(),
            "{raw:?}"
        );
    }
}
#[test]
fn content_parameters_serialize_once_before_location_encoding() {
    let req = request(
        "readContent",
        json!({"query":{"q":{"id":9007199254740993_u64,"name":"a/b"}}}),
    )
    .unwrap();
    assert_eq!(
        req.url,
        "https://example.test/content?q=%7B%22id%22%3A9007199254740993%2C%22name%22%3A%22a%2Fb%22%7D"
    );
    assert!(req.headers.is_empty());
}
#[test]
fn cookie_style_preserves_flat_collections_and_rejects_injection() {
    let mut doc = document();
    doc["paths"]["/find"]["query"]["parameters"][1]["schema"] =
        json!({"type":"array","items":{"type":"string"}});
    for (explode, expected) in [(true, "prefs=red; prefs=blue"), (false, "prefs=red,blue")] {
        doc["paths"]["/find"]["query"]["parameters"][1]["explode"] = json!(explode);
        let contract = resolve_document(&doc, ResolveOptions::new("v32")).unwrap();
        let operation = contract
            .operations
            .iter()
            .find(|op| op.name == "findItems")
            .unwrap();
        let req = build_http_request(
            operation,
            &contract.schemas,
            "https://example.test",
            &json!({"querystring":{"filter":{}},"cookie":{"prefs":["red","blue"]}}),
        )
        .unwrap();
        assert_eq!(req.headers, vec![("Cookie".into(), expected.into())]);
    }
    for value in [
        json!({"a":"b; injected=yes"}),
        json!({"a":"white space"}),
        json!({"a":"\"quote"}),
        json!({"bad=name":"x"}),
        json!({"a":"é"}),
    ] {
        assert!(
            request(
                "findItems",
                json!({"querystring":{"filter":{}},"cookie":{"prefs":value}})
            )
            .is_err()
        );
    }
}
#[test]
fn invalid_operation_and_parameter_contracts_fail_during_resolution() {
    let base = document();
    let mut cases = Vec::new();
    let mut d = base.clone();
    d["paths"]["/copy/{id}"]["additionalOperations"]["POST"] =
        json!({"responses":{"204":{"description":"ok"}}});
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/copy/{id}"]["additionalOperations"]["bad\nmethod"] = json!({});
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/find"]["query"]["parameters"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"q","in":"query","schema":{"type":"string"}}));
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/find"]["query"]["parameters"].as_array_mut().unwrap().push(json!({"name":"other","in":"querystring","content":{"text/plain":{"schema":{"type":"string"}}}}));
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/find"]["query"]["parameters"][0]["schema"] = json!({});
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/find"]["query"]["parameters"][0]["content"]["text/plain"] =
        json!({"schema":{"type":"string"}});
    cases.push(d);
    let mut d = base.clone();
    d["paths"]["/find"]["query"]["parameters"][0]
        .as_object_mut()
        .unwrap()
        .remove("content");
    cases.push(d);
    let mut d = base;
    d["openapi"] = json!("3.1.1");
    cases.push(d);
    for (index, doc) in cases.iter().enumerate() {
        assert!(
            resolve_document(doc, ResolveOptions::new("bad")).is_err(),
            "case {index}"
        );
    }
}

#[test]
fn exploded_cookie_objects_do_not_serialize_the_container_name() {
    let mut doc = document();
    doc["paths"]["/find"]["query"]["parameters"][1]["name"] = json!("unused container name");
    let contract = resolve_document(&doc, ResolveOptions::new("v32")).unwrap();
    let op = contract
        .operations
        .iter()
        .find(|op| op.name == "findItems")
        .unwrap();
    let req = build_http_request(
        op,
        &contract.schemas,
        "https://example.test",
        &json!({"querystring":{"filter":{}},"cookie":{"unused container name":{"token":"a%2Fb"}}}),
    )
    .unwrap();
    assert_eq!(req.headers, vec![("Cookie".into(), "token=a%2Fb".into())]);
}
#[test]
fn referenced_media_preserves_schema_and_encoding_and_content_works_on_31() {
    let mut doc = document();
    let media=doc["paths"]["/find"]["query"]["parameters"][0]["content"]["application/x-www-form-urlencoded"].clone();
    doc["components"] = json!({"mediaTypes":{"Form":media}});
    doc["paths"]["/find"]["query"]["parameters"][0]["content"]["application/x-www-form-urlencoded"] =
        json!({"$ref":"#/components/mediaTypes/Form"});
    let contract = resolve_document(&doc, ResolveOptions::new("v32")).unwrap();
    let op = contract
        .operations
        .iter()
        .find(|op| op.name == "findItems")
        .unwrap();
    let parameter = op.parameters.iter().find(|p| p.name == "filter").unwrap();
    assert_eq!(parameter.schema["properties"]["term"]["type"], "string");
    assert_eq!(
        parameter.content.as_ref().unwrap().encoding["labels"].explode,
        Some(true)
    );
    let req = build_http_request(
        op,
        &contract.schemas,
        "https://example.test",
        &json!({"querystring":{"filter":{"labels":["a b","c"]}}}),
    )
    .unwrap();
    assert_eq!(req.url, "https://example.test/find?labels=a%20b&labels=c");
    let content = doc["paths"]["/content"].clone();
    doc["paths"] = json!({"/content":content});
    doc["openapi"] = json!("3.1.1");
    doc.as_object_mut().unwrap().remove("components");
    assert_eq!(
        resolve_document(&doc, ResolveOptions::new("v31"))
            .unwrap()
            .operations
            .len(),
        1
    );
}
#[test]
fn unimplemented_sequential_media_is_rejected_without_discarding_contracts() {
    for field in ["itemSchema", "prefixEncoding", "itemEncoding"] {
        let mut doc = document();
        doc["paths"]["/content"]["get"]["responses"]["204"]["content"] =
            json!({"application/x-ndjson":{"schema":{"type":"array"} }});
        doc["paths"]["/content"]["get"]["responses"]["204"]["content"]["application/x-ndjson"]
            [field] = if field == "prefixEncoding" {
            json!([])
        } else {
            json!({})
        };
        let error = resolve_document(&doc, ResolveOptions::new("v32")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("sequential or positional media contract"),
            "{error}"
        );
    }
}
