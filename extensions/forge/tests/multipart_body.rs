//! Multipart wire checks use literal headers and independently split MIME frames.
use incurs_forge::{
    ResolveOptions, resolve_document,
    runtime::{ForgeHttpRequest, build_http_request},
};
use serde_json::{Value, json};

fn document() -> Value {
    serde_json::from_str(include_str!("fixtures/multipart_openapi.json")).unwrap()
}
fn bind(doc: &Value, args: Value) -> Result<ForgeHttpRequest, incurs_forge::ForgeError> {
    let contract = resolve_document(doc, ResolveOptions::new("multipart"))?;
    build_http_request(
        &contract.operations[0],
        &contract.schemas,
        "https://example.test",
        &args,
    )
}
fn frames(request: &ForgeHttpRequest) -> Vec<String> {
    let content_type = &request
        .headers
        .iter()
        .find(|(name, _)| name == "Content-Type")
        .unwrap()
        .1;
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    assert!(boundary.len() <= 70);
    let body = String::from_utf8(request.body.clone().unwrap()).unwrap();
    let closing = format!("--{boundary}--\r\n");
    assert!(body.ends_with(&closing));
    let prefix = format!("--{boundary}\r\n");
    body[..body.len() - closing.len()]
        .split(&prefix)
        .skip(1)
        .map(|part| part.strip_suffix("\r\n").unwrap().to_string())
        .collect()
}

#[test]
fn multipart_preserves_content_headers_arrays_utf8_and_encoded_data() {
    let request=bind(&document(),json!({"body":{"text":"hello\r\n雪 +&","count":9007199254740993_i64,
        "tags":["red blue","+&"],"address":{"city":"New York"},"encoded":"AP8B","packed":["a","b"]}})).unwrap();
    assert_eq!(
        frames(&request),
        vec![
            "Content-Disposition: form-data; name=\"address\"\r\nContent-Type: application/json\r\nX-Part: proof\r\n\r\n{\"city\":\"New York\"}",
            "Content-Disposition: form-data; name=\"count\"\r\nContent-Type: text/plain\r\n\r\n9007199254740993",
            "Content-Disposition: form-data; name=\"encoded\"\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8B",
            "Content-Disposition: form-data; name=\"packed\"\r\nContent-Type: text/plain\r\n\r\na,b",
            "Content-Disposition: form-data; name=\"tags\"\r\nContent-Type: text/plain\r\n\r\nred blue",
            "Content-Disposition: form-data; name=\"tags\"\r\nContent-Type: text/plain\r\n\r\n+&",
            "Content-Disposition: form-data; name=\"text\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n雪 +&",
        ]
    );
}

#[test]
fn multipart_boundary_is_selected_from_actual_payload_and_presence_is_distinct() {
    let request = bind(
        &document(),
        json!({"body":{"text":"--incurs-forge-0\r\n--incurs-forge-1"}}),
    )
    .unwrap();
    assert_eq!(
        request.headers[0].1,
        "multipart/form-data; boundary=incurs-forge-2"
    );
    assert!(frames(&request)[0].ends_with("--incurs-forge-0\r\n--incurs-forge-1"));
    let absent = bind(&document(), json!({})).unwrap();
    assert_eq!(absent.body, None);
    assert!(absent.headers.is_empty());
    assert!(frames(&bind(&document(), json!({"body":{}})).unwrap()).is_empty());
    let mut doc = document();
    doc["paths"]["/multipart"]["post"]["requestBody"]["required"] = json!(true);
    assert!(
        bind(&doc, json!({}))
            .unwrap_err()
            .to_string()
            .contains("missing required")
    );
}

#[test]
fn multipart_encoding_style_does_not_percent_encode_payloads() {
    let mut doc = document();
    let media =
        &mut doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"];
    media["encoding"]["tags"] = json!({"style":"form","explode":true,"allowReserved":false});
    media["encoding"]["address"] = json!({"style":"deepObject","explode":true});
    let request = bind(
        &doc,
        json!({"body":{"tags":["%20 +&"],"address":{"city":"雪 +&"}}}),
    )
    .unwrap();
    assert_eq!(
        frames(&request),
        vec![
            "Content-Disposition: form-data; name=\"address[city]\"\r\nContent-Type: text/plain\r\n\r\n雪 +&",
            "Content-Disposition: form-data; name=\"tags\"\r\nContent-Type: text/plain\r\n\r\n%20 +&",
        ]
    );
}

#[test]
fn multipart_rejects_unsafe_headers_and_unavailable_part_header_values() {
    for encoding in [
        json!({"contentType":"text/plain\r\nInjected: yes"}),
        json!({"contentType":"image/png, image/jpeg"}),
        json!({"headers":{"X-Test":{"schema":{"default":"value\r\nInjected: yes"}}}}),
        json!({"headers":{"Content-Disposition":{"schema":{"default":"form-data; name=wrong"}}}}),
        json!({"headers":{"X-Test":{"required":true,"schema":{"type":"string"}}}}),
        json!({"style":"deepObject"}),
    ] {
        let mut doc = document();
        doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"]["encoding"]
            ["text"] = encoding;
        assert!(bind(&doc, json!({"body":{"text":"value"}})).is_err());
    }
    assert!(bind(&document(), json!({"body":null})).is_err());
}

#[test]
fn multipart_names_cannot_inject_headers_and_literal_percent_names_stay_distinct() {
    let request = bind(
        &document(),
        json!({"body":{"a\"\r\nX-Test: yes":"first","a%22":"second"}}),
    )
    .unwrap();
    let parts = frames(&request);
    assert!(
        parts[0].starts_with("Content-Disposition: form-data; name=\"a%22%0D%0AX-Test: yes\"\r\n")
    );
    assert!(parts[1].starts_with("Content-Disposition: form-data; name=\"a%2522\"\r\n"));
    assert!(!parts.iter().any(|p| p.contains("\r\nX-Test:")));
}

#[test]
fn multipart_content_encoding_preserves_encoded_text_even_for_json_media() {
    let mut doc = document();
    doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"]["encoding"]
        ["encoded"] = json!({"contentType":"application/json"});
    let request = bind(&doc, json!({"body":{"encoded":"eyJvayI6dHJ1ZX0="}})).unwrap();
    assert_eq!(
        frames(&request),
        vec![
            "Content-Disposition: form-data; name=\"encoded\"\r\nContent-Type: application/json\r\nContent-Transfer-Encoding: base64\r\n\r\neyJvayI6dHJ1ZX0="
        ]
    );
    assert!(bind(&doc, json!({"body":{"encoded":{"ok":true}}})).is_err());
}

#[test]
fn multipart_scalar_style_retains_transfer_encoding_and_rejects_encoded_collection_styles() {
    let mut doc = document();
    let media =
        &mut doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"];
    media["encoding"]["encoded"] = json!({"style":"form","contentType":"application/json"});
    let request = bind(&doc, json!({"body":{"encoded":"AP8B"}})).unwrap();
    assert_eq!(
        frames(&request),
        vec![
            "Content-Disposition: form-data; name=\"encoded\"\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8B"
        ]
    );
    doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"]["schema"]
        ["properties"]["packed"]["items"]["contentEncoding"] = json!("base64");
    assert!(bind(&doc, json!({"body":{"packed":["AA==","AQ=="]}})).is_err());
}

#[test]
fn multipart_transfer_header_and_schema_encoding_are_equivalent_and_conflicts_fail() {
    let mut doc = document();
    let media =
        &mut doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"];
    media["schema"]["properties"]["encoded"]
        .as_object_mut()
        .unwrap()
        .remove("contentEncoding");
    media["encoding"]["encoded"] = json!({"contentType":"application/json","headers":{
        "Content-Transfer-Encoding":{"schema":{"type":"string","const":"base64"}}
    }});
    let request = bind(&doc, json!({"body":{"encoded":"eyJvayI6dHJ1ZX0="}})).unwrap();
    assert_eq!(
        frames(&request),
        vec![
            "Content-Disposition: form-data; name=\"encoded\"\r\nContent-Type: application/json\r\nContent-Transfer-Encoding: base64\r\n\r\neyJvayI6dHJ1ZX0="
        ]
    );
    doc["paths"]["/multipart"]["post"]["requestBody"]["content"]["multipart/form-data"]["schema"]
        ["properties"]["encoded"]["contentEncoding"] = json!("quoted-printable");
    assert!(bind(&doc, json!({"body":{"encoded":"AP8B"}})).is_err());
}
