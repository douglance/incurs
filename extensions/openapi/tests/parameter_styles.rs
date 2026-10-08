//! OpenAPI parameter style wire oracles for HTTP binding.

use incurs_openapi::runtime::build_http_request;
use incurs_openapi::{ApiResponse, Operation, Parameter};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn parameter(style: &str, explode: bool) -> Parameter {
    Parameter {
        content: None,
        name: "color".into(),
        location: "path".into(),
        required: true,
        schema: json!({}),
        style: style.into(),
        explode,
    }
}

fn operation(style: &str, explode: bool, location: &str) -> Operation {
    Operation {
        id: format!("styles/{style}/{explode}"),
        name: "style".into(),
        description: None,
        method: "GET".into(),
        path: if location == "path" {
            "/styles/{color}"
        } else {
            "/styles/color"
        }
        .into(),
        servers: Vec::new(),
        parameters: vec![Parameter {
            location: location.into(),
            required: location == "path",
            ..parameter(style, explode)
        }],
        request_body: None,
        responses: BTreeMap::from([(
            "200".into(),
            ApiResponse {
                description: "OK".into(),
                content: BTreeMap::new(),
                headers: BTreeMap::new(),
            },
        )]),
        security: json!([]),
    }
}

fn bound_url(style: &str, explode: bool, location: &str, value: Value) -> String {
    build_http_request(
        &operation(style, explode, location),
        &Default::default(),
        "https://example.test",
        &json!({ location: { "color": value } }),
    )
    .unwrap()
    .url
}

#[test]
fn path_matrix_and_label_styles_match_openapi_wire_forms() {
    let primitive = json!("blue/ä;=.");
    let array = json!(["blue", "black|white", "br own", "blå"]);
    let object = json!({"R":100,"G":"two words","B":"pipe|semi;equal=dot."});

    for (style, explode, value, expected_path) in [
        (
            "matrix",
            false,
            primitive.clone(),
            "/styles/;color=blue%2F%C3%A4%3B%3D.",
        ),
        (
            "matrix",
            true,
            primitive,
            "/styles/;color=blue%2F%C3%A4%3B%3D.",
        ),
        (
            "matrix",
            false,
            array.clone(),
            "/styles/;color=blue,black%7Cwhite,br%20own,bl%C3%A5",
        ),
        (
            "matrix",
            true,
            array,
            "/styles/;color=blue;color=black%7Cwhite;color=br%20own;color=bl%C3%A5",
        ),
        (
            "matrix",
            false,
            object.clone(),
            "/styles/;color=B,pipe%7Csemi%3Bequal%3Ddot.,G,two%20words,R,100",
        ),
        (
            "matrix",
            true,
            object.clone(),
            "/styles/;B=pipe%7Csemi%3Bequal%3Ddot.;G=two%20words;R=100",
        ),
        (
            "label",
            false,
            json!("blue/ä;=."),
            "/styles/.blue%2F%C3%A4%3B%3D%2E",
        ),
        (
            "label",
            true,
            json!("blue/ä;=."),
            "/styles/.blue%2F%C3%A4%3B%3D%2E",
        ),
        (
            "label",
            false,
            json!(["blue", "black|white", "br own", "blå"]),
            "/styles/.blue,black%7Cwhite,br%20own,bl%C3%A5",
        ),
        (
            "label",
            true,
            json!(["blue", "black|white", "br own", "blå"]),
            "/styles/.blue.black%7Cwhite.br%20own.bl%C3%A5",
        ),
        (
            "label",
            false,
            object.clone(),
            "/styles/.B,pipe%7Csemi%3Bequal%3Ddot%2E,G,two%20words,R,100",
        ),
        (
            "label",
            true,
            object,
            "/styles/.B=pipe%7Csemi%3Bequal%3Ddot%2E.G=two%20words.R=100",
        ),
    ] {
        assert_eq!(
            bound_url(style, explode, "path", value),
            format!("https://example.test{expected_path}"),
            "{style} explode={explode}"
        );
    }
}

#[test]
fn query_space_and_pipe_delimited_styles_match_openapi_wire_forms() {
    let array = json!(["blue", "black|white", "br own", "blå"]);
    let object = json!({"R":100,"G":"two words","B":"pipe|semi;equal=dot."});

    for (style, value, expected_query) in [
        (
            "spaceDelimited",
            array.clone(),
            "color=blue%20black%7Cwhite%20br%20own%20bl%C3%A5",
        ),
        (
            "spaceDelimited",
            object.clone(),
            "color=B%20pipe%7Csemi%3Bequal%3Ddot.%20G%20two%20words%20R%20100",
        ),
        (
            "pipeDelimited",
            array,
            "color=blue%7Cblack%7Cwhite%7Cbr%20own%7Cbl%C3%A5",
        ),
        (
            "pipeDelimited",
            object,
            "color=B%7Cpipe%7Csemi%3Bequal%3Ddot.%7CG%7Ctwo%20words%7CR%7C100",
        ),
    ] {
        assert_eq!(
            bound_url(style, false, "query", value),
            format!("https://example.test/styles/color?{expected_query}"),
            "{style}"
        );
    }
}

#[test]
fn undefined_delimited_query_style_combinations_fail_clearly() {
    for (style, explode, value, expected_error) in [
        (
            "spaceDelimited",
            true,
            json!(["blue", "black"]),
            "explode=true is undefined",
        ),
        (
            "pipeDelimited",
            true,
            json!({"R":100}),
            "explode=true is undefined",
        ),
        (
            "spaceDelimited",
            false,
            json!("blue"),
            "requires an array or object",
        ),
        (
            "pipeDelimited",
            false,
            json!("blue"),
            "requires an array or object",
        ),
    ] {
        let error = build_http_request(
            &operation(style, explode, "query"),
            &Default::default(),
            "https://example.test",
            &json!({ "query": { "color": value } }),
        )
        .unwrap_err()
        .0;
        assert!(error.contains(expected_error), "{style}: {error}");
    }
}
