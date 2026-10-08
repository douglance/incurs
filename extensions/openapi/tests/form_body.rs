//! Form request wire expectations are independent of the binder.
use incurs_openapi::{ResolveOptions, resolve_document, runtime::build_http_request};
use serde_json::{Value, json};

fn document() -> Value {
    serde_json::from_str(include_str!("fixtures/form_openapi.json")).unwrap()
}

fn wire(document: &Value, body: Value) -> Result<Vec<u8>, incurs_openapi::OpenApiError> {
    let contract = resolve_document(document, ResolveOptions::new("form"))?;
    Ok(build_http_request(
        &contract.operations[0],
        &contract.schemas,
        "https://example.test",
        &json!({"body":body}),
    )?
    .body
    .unwrap())
}

#[test]
fn resolver_retains_encoding_presence_and_contract_round_trip() {
    let contract = resolve_document(&document(), ResolveOptions::new("form")).unwrap();
    let encoded = serde_json::to_value(&contract).unwrap();
    assert_eq!(
        encoded["operations"][0]["request_body"]["encoding"]["application/x-www-form-urlencoded"],
        json!({"tags":{"style":"form","explode":true},"quoted":{"contentType":"application/json"},"packed":{"style":"form","explode":false}})
    );
    let decoded: incurs_openapi::ResolvedOpenApi = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, contract);
}

#[test]
fn form_content_and_explicit_styles_preserve_wire_values() {
    let body = json!({"text":"a+b &雪","count":9007199254740993_i64,
        "tags":["red blue","+&"],"address":{"city":"New York"},
        "quoted":"123","packed":["a,b","c d"]});
    assert_eq!(wire(&document(),body).unwrap(),
        b"address=%7B%22city%22%3A%22New+York%22%7D&count=9007199254740993&packed=a%2Cb,c%20d&quoted=%22123%22&tags=red%20blue&tags=%2B%26&text=a%2Bb+%26%E9%9B%AA");
}

#[test]
fn form_absent_empty_and_required_body_remain_distinct() {
    let mut doc = document();
    let contract = resolve_document(&doc, ResolveOptions::new("form")).unwrap();
    let request = build_http_request(
        &contract.operations[0],
        &contract.schemas,
        "https://example.test",
        &json!({}),
    )
    .unwrap();
    assert_eq!(request.body, None);
    assert!(request.headers.is_empty());
    let empty = build_http_request(
        &contract.operations[0],
        &contract.schemas,
        "https://example.test",
        &json!({"body":{}}),
    )
    .unwrap();
    assert_eq!(empty.body, Some(vec![]));
    assert_eq!(
        empty.headers,
        vec![(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into()
        )]
    );
    doc["paths"]["/form"]["post"]["requestBody"]["required"] = json!(true);
    let contract = resolve_document(&doc, ResolveOptions::new("form")).unwrap();
    assert!(
        build_http_request(
            &contract.operations[0],
            &contract.schemas,
            "https://example.test",
            &json!({})
        )
        .unwrap_err()
        .to_string()
        .contains("missing required")
    );
    for body in [json!(null), json!("raw"), json!([]), json!(1)] {
        assert!(wire(&document(), body).is_err());
    }
}

#[test]
fn explicit_query_styles_and_reserved_expansion_are_applied() {
    let mut doc = document();
    let media = &mut doc["paths"]["/form"]["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"];
    media["schema"] = json!({"type":"object","properties":{
        "filter":{"type":"object"},"pipe":{"type":"array","items":{"type":"string"}},
        "spaces":{"type":"array","items":{"type":"string"}},"reserved":{"type":"string"},
        "object":{"type":"object"},"jsonArray":{"type":"array","items":{"type":"integer"}}
    }});
    media["encoding"] = json!({
        "filter":{"style":"deepObject","explode":true},
        "pipe":{"style":"pipeDelimited"},"spaces":{"style":"spaceDelimited"},
        "reserved":{"allowReserved":true},"object":{"explode":false},
        "jsonArray":{"contentType":"application/json"}
    });
    assert_eq!(wire(&doc,json!({"filter":{"id":7},"pipe":["a","b"],
        "spaces":["c","d"],"reserved":"/?:@!$'()*+,;=[]#&%2F%oops",
        "object":{"a":"b,c"},"jsonArray":[1,2]})).unwrap(),
        b"filter%5Bid%5D=7&jsonArray=%5B1%2C2%5D&object=a,b%2Cc&pipe=a%7Cb&reserved=/?:@!$'()*%2B,;%3D%5B%5D%23%26%2F%25oops&spaces=c%20d");
}

#[test]
fn unsupported_or_undefined_encodings_fail_instead_of_sending_wrong_bytes() {
    for encoding in [
        json!({"style":"matrix"}),
        json!({"style":"deepObject"}),
        json!({"style":"pipeDelimited","explode":true}),
        json!({"contentType":"application/xml"}),
    ] {
        let mut doc = document();
        doc["paths"]["/form"]["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"]
            ["encoding"]["text"] = encoding;
        assert!(wire(&doc, json!({"text":"value"})).is_err());
    }
    assert!(wire(&document(), json!({"text":null})).is_err());
    assert!(wire(&document(), json!({"tags":[{"nested":"not scalar"}]})).is_err());
    let mut doc = document();
    doc["paths"]["/form"]["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"]
        ["encoding"]["tags"]["explode"] = json!("yes");
    assert!(resolve_document(&doc, ResolveOptions::new("form")).is_err());
}
