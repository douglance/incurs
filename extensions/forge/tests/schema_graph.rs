//! Shared schema graph regression tests with independently specified references.
use incurs_forge::{ResolveOptions, resolve_document};
use serde_json::{Value, json};

fn document(schema: Value) -> Value {
    json!({
        "openapi":"3.1.0", "info":{"title":"Shared graph","version":"1"},
        "paths":{"/fan":{"post":{
            "operationId":"postFan",
            "requestBody":{"required":true,"content":{"application/json":{"schema":schema.clone()}}},
            "responses":{"200":{"description":"ok","content":{"application/json":{"schema":schema}}}}
        }}}
    })
}

fn shared_document() -> Value {
    let properties = (0..128)
        .map(|n| {
            (
                format!("field{n}"),
                json!({"$ref":"#/components/schemas/Leaf"}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let mut doc = document(json!({"$ref":"#/components/schemas/Fan"}));
    doc["components"] = json!({"schemas":{
        "Leaf":{"type":"string","description":"x".repeat(4096),"minLength":2},
        "Fan":{"type":"object","properties":properties}
    }});
    doc
}

#[test]
fn shared_schemas_are_stored_once_and_operations_keep_references() {
    let contract = resolve_document(&shared_document(), ResolveOptions::new("graph")).unwrap();
    assert_eq!(contract.schemas.len(), 2);
    let bytes = serde_json::to_vec(&contract).unwrap().len();
    assert!(
        bytes < 24_000,
        "128 edges to one 4 KiB leaf must not duplicate it: {bytes} bytes"
    );

    assert_eq!(
        contract.schemas["Fan"]["properties"]["field0"],
        json!({"$ref":"#/components/schemas/Leaf"})
    );
    let op = &contract.operations[0];
    assert_eq!(
        op.request_body.as_ref().unwrap().content["application/json"],
        json!({"$ref":"#/components/schemas/Fan"})
    );
    assert_eq!(
        op.responses["200"].content["application/json"],
        json!({"$ref":"#/components/schemas/Fan"})
    );
    assert_eq!(contract.schemas["Leaf"]["minLength"], 2);
}

#[test]
fn reference_siblings_and_instance_data_survive_without_expansion() {
    let mut document = shared_document();
    let expected = json!({
        "$ref":"#/components/schemas/Leaf","maxLength":5,
        "default":{"$ref":"this is data, not a schema reference"}
    });
    document["components"]["schemas"]["Fan"]["properties"]["field0"] = expected.clone();
    let contract = resolve_document(&document, ResolveOptions::new("graph")).unwrap();
    assert_eq!(contract.schemas["Fan"]["properties"]["field0"], expected);
}

#[test]
fn externally_referenced_schema_is_retained_once_with_its_document_context() {
    let document = document(json!({"$ref":"common.json#/Thing"}));
    let mut options = ResolveOptions::new("graph");
    options.document_uri = "apis/root.json".into();
    options.documents.insert(
        "apis/common.json".into(),
        json!({
            "Thing":{"type":"object","properties":{"value":{"$ref":"inner.json#/Value"}}}
        }),
    );
    options.documents.insert(
        "apis/inner.json".into(),
        json!({"Value":{"type":"integer","minimum":7}}),
    );
    let contract = resolve_document(&document, options).unwrap();
    assert_eq!(contract.schemas.len(), 2);
    let graph = json!({"components":{"schemas":contract.schemas}});
    let response = &contract.operations[0].responses["200"].content["application/json"];
    let reference = response["$ref"]
        .as_str()
        .expect("response retains a local reference");
    let outer = graph
        .pointer(reference.strip_prefix('#').unwrap())
        .expect("external target is in the graph");
    let nested = outer["properties"]["value"]["$ref"].as_str().unwrap();
    assert_eq!(
        graph.pointer(nested.strip_prefix('#').unwrap()).unwrap(),
        &json!({"type":"integer","minimum":7})
    );
}

#[test]
fn referenced_multipart_metadata_and_headers_reach_exact_wire_bytes() {
    let mut doc = document(json!({}));
    doc["components"] = json!({"schemas":{
        "Upload":{"type":"object","properties":{"file":{"$ref":"#/components/schemas/Encoded"}}},
        "Encoded":{"type":"string","contentEncoding":"base64"},
        "PartHeader":{"type":"string","const":"graph"}
    }});
    doc["paths"]["/fan"]["post"]["requestBody"]["content"] = json!({
        "multipart/form-data":{
            "schema":{"$ref":"#/components/schemas/Upload"},
            "encoding":{"file":{"headers":{"X-Proof":{"schema":{"$ref":"#/components/schemas/PartHeader"}}}}}
        }
    });
    let contract = resolve_document(&doc, ResolveOptions::new("graph")).unwrap();
    let request = incurs_forge::runtime::build_http_request(
        &contract.operations[0],
        &contract.schemas,
        "https://example.test",
        &json!({"body":{"file":"AP8B"}}),
    )
    .unwrap();
    assert_eq!(
        request.headers,
        vec![(
            "Content-Type".into(),
            "multipart/form-data; boundary=incurs-forge-0".into()
        )]
    );
    assert_eq!(request.body.unwrap(), b"--incurs-forge-0\r\nContent-Disposition: form-data; name=\"file\"\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\nX-Proof: graph\r\n\r\nAP8B\r\n--incurs-forge-0--\r\n");
}

#[test]
fn false_schema_alias_rejects_a_supplied_body_and_missing_definitions_fail() {
    let mut doc = document(json!({"$ref":"#/components/schemas/Blocked"}));
    doc["components"] = json!({"schemas":{"Blocked":false}});
    let mut contract = resolve_document(&doc, ResolveOptions::new("graph")).unwrap();
    let op = &contract.operations[0];
    let error = incurs_forge::runtime::build_http_request(
        op,
        &contract.schemas,
        "https://example.test",
        &json!({"body":null}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("false schema"));
    contract.schemas.clear();
    let error = incurs_forge::runtime::build_http_request(
        op,
        &contract.schemas,
        "https://example.test",
        &json!({"body":null}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("target not found"));
}

#[test]
fn external_response_object_keeps_nested_schema_document_context() {
    let mut doc = document(json!({}));
    doc["paths"]["/fan"]["post"]["responses"]["200"] = json!({"$ref":"responses.json#/Ok"});
    let mut options = ResolveOptions::new("graph");
    options.document_uri = "apis/root.json".into();
    options.documents.insert("apis/responses.json".into(), json!({
        "Ok":{"description":"ok","content":{"application/json":{"schema":{"$ref":"types.json#/Value"}}}}
    }));
    options.documents.insert(
        "apis/types.json".into(),
        json!({"Value":{"type":"integer","minimum":11}}),
    );
    let contract = resolve_document(&doc, options).unwrap();
    let graph = json!({"components":{"schemas":contract.schemas}});
    let reference = contract.operations[0].responses["200"].content["application/json"]["$ref"]
        .as_str()
        .unwrap();
    assert_eq!(
        graph.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
        &json!({"type":"integer","minimum":11})
    );
}
