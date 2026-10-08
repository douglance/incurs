use sdk::{Field, IntoJson, JsonValue};
use worker::{Context, Env, Request, Response, Result, event};
fn main() {}
fn run_validation() -> std::result::Result<serde_json::Value, String> {
    let body = sdk::Bounded::try_new(JsonValue::Integer(4))
        .map_err(|e| e.to_string())?
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    let recursive = sdk::RecursiveAll {
        value: 1,
        next: Field::Value(Box::new(sdk::RecursiveAll {
            value: 2,
            next: Field::Missing,
        })),
    }
    .into_json()
    .to_json_string()
    .map_err(|e| e.to_string())?;
    let overlap = sdk::AnyNumeric::Integer(3)
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    let exact = sdk::ExactUnsigned::try_new(JsonValue::Unsigned(u64::MAX))
        .map_err(|e| e.to_string())?
        .into_json()
        .to_json_string()
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "body":body,"recursive":recursive,"overlap":overlap,"exact":exact,
        "invalid_low":sdk::Bounded::try_new(JsonValue::Integer(0)).is_err(),
        "invalid_pattern":sdk::PatternChoice::String("ab".into()).into_json().to_json_string().is_err(),
        "invalid_multiple":sdk::PatternChoice::Integer(7).into_json().to_json_string().is_err(),
        "impossible":sdk::Impossible::try_new(JsonValue::Null).is_err(),
        "decimal":sdk::Decimal::try_new(JsonValue::Number(0.3)).is_ok(),
        "invalid_decimal":sdk::Decimal::try_new(JsonValue::Number(0.31)).is_err()
    }))
}

fn run_media() -> std::result::Result<serde_json::Value, String> {
    MEDIA_BINDING.with(|binding| {
        let binding = binding.as_ref().map_err(Clone::clone)?;
    use media_sdk::*;
    let inputs = [
        UploadBinaryArgs {
            body: UploadBinaryBody::Missing,
        }
        .into_request(),
        UploadBinaryArgs {
            body: UploadBinaryBody::ApplicationOctetStream(Vec::new()),
        }
        .into_request(),
        UploadBinaryArgs {
            body: UploadBinaryBody::ApplicationOctetStream(vec![0, 255, 128, 13, 10]),
        }
        .into_request(),
        UploadBinaryArgs {
            body: UploadBinaryBody::ApplicationOctetStream((0_u8..=255).collect()),
        }
        .into_request(),
        UploadBinaryArgs {
            body: UploadBinaryBody::ApplicationOctetStream((0_u8..255).collect()),
        }
        .into_request(),
        UploadBinaryArgs {
            body: UploadBinaryBody::ApplicationOctetStream((0_u8..254).collect()),
        }
        .into_request(),
        SendMarkdownArgs {
            body: SendMarkdownBody::TextPlain(Field::Value("snow 雪\n".into())),
        }
        .into_request(),
        SendMarkdownArgs {
            body: SendMarkdownBody::TextXMarkdown(Field::Value("snow 雪\n".into())),
        }
        .into_request(),
        UploadRecordsArgs {
            body: "{\"id\":1}\nnull".into(),
        }
        .into_request(),
        UploadNdjsonArgs {
            body: UploadNdjsonBody::ApplicationXNdjson(b"{\"id\":1}\n".to_vec()),
        }
        .into_request(),
        SendMixedArgs {
            body: SendMixedBody::ApplicationJson(Field::Null),
        }
        .into_request(),
        SendMixedArgs {
            body: SendMixedBody::ApplicationJson(Field::Default(JsonValue::Null)),
        }
        .into_request(),
    ];
    let mut media = Vec::new();
    for request in inputs {
        let arguments: serde_json::Value =
            serde_json::from_str(&request.arguments_json().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let bound = binding.build_request(request.operation_id, "https://example.test", &arguments)
            .map_err(|e| e.to_string())?;
        let content_type = bound
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        media.push(serde_json::json!({"url":bound.url,"method":bound.method,"content_type":content_type,"body":bound.body,"arguments":arguments}));
    }
    let false_body = SendMixedArgs {
        body: SendMixedBody::ApplicationXForbiddenJson(Field::Null),
    }
    .into_request()
    .arguments_json()
    .is_err();
    let false_default = SendMixedArgs {
        body: SendMixedBody::ApplicationXForbiddenJson(Field::Default(JsonValue::Null)),
    }
    .into_request()
    .arguments_json()
    .is_err();
    let mut conflict = UploadBinaryArgs {
        body: UploadBinaryBody::ApplicationOctetStream(vec![1]),
    }
    .into_request();
    conflict.body = Field::Value(JsonValue::String("conflict".into()));
    Ok(
        serde_json::json!({"requests":media,"false_body":false_body,"false_default":false_default,"conflicting_slots":conflict.arguments_json().is_err()}),
    )

    })
}

fn run_numeric() -> std::result::Result<serde_json::Value, String> {
    NUMERIC_BINDING.with(|binding| {
        let binding = binding.as_ref().map_err(Clone::clone)?;
    use numeric_sdk::*;
    let request = SendNumbersArgs {
        high: SendNumbersArgs::high_default(),
        low: SendNumbersArgs::low_default(),
        body: NumericDefaults {
            huge: NumericDefaults::huge_default(),
            nested: NumericDefaults::nested_default(),
            precise: NumericDefaults::precise_default(),
            tiny: NumericDefaults::tiny_default(),
        },
    }
    .into_request();
    let arguments = request.arguments_json().map_err(|e| e.to_string())?;
    let parsed = serde_json::from_str(&arguments).map_err(|e| e.to_string())?;
    let bound = binding.build_request(request.operation_id, "https://example.test", &parsed)
        .map_err(|e| e.to_string())?;
    let body =
        String::from_utf8(bound.body.ok_or("numeric body missing")?).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "arguments": arguments, "url": bound.url, "body": body,
        "valid_bound": ExactBound::try_new(JsonValue::ExactNumber("18446744073709551617".into())).is_ok(),
        "invalid_bound": ExactBound::try_new(JsonValue::ExactNumber("18446744073709551618".into())).is_err(),
        "malformed_rejected": JsonValue::ExactNumber("1,\"injected\":true".into()).to_json_string().is_err()
    }))

    })
}

fn source_binding() -> std::result::Result<incurs_openapi::runtime::HttpBinding, String> {
    let document = serde_json::from_str(include_str!(env!("OPENAPI_REQUEST_VALIDATION_DOCUMENT")))
        .map_err(|e| e.to_string())?;
    let contract = incurs_openapi::resolve_document(
        &document,
        incurs_openapi::ResolveOptions::new("validation-proof"),
    )
    .map_err(|e| e.to_string())?;
    incurs_openapi::runtime::HttpBinding::new(&contract).map_err(|e| e.to_string())
}

fn compiled_binding(
    text: &str,
) -> std::result::Result<incurs_openapi::runtime::HttpBinding, String> {
    let contract = serde_json::from_str(text).map_err(|e| e.to_string())?;
    incurs_openapi::runtime::HttpBinding::new(&contract).map_err(|e| e.to_string())
}

std::thread_local! {
    static REQUEST_BINDING: std::result::Result<incurs_openapi::runtime::HttpBinding, String> = source_binding();
    static MEDIA_BINDING: std::result::Result<incurs_openapi::runtime::HttpBinding, String> =
        compiled_binding(include_str!(env!("OPENAPI_MEDIA_CONTRACT")));
    static NUMERIC_BINDING: std::result::Result<incurs_openapi::runtime::HttpBinding, String> =
        compiled_binding(include_str!(env!("OPENAPI_NUMERIC_CONTRACT")));
}

fn run_request_validation() -> std::result::Result<serde_json::Value, String> {
    REQUEST_BINDING.with(|binding| {
        let binding = binding.as_ref().map_err(Clone::clone)?;
        let valid = serde_json::json!({
            "query":{"limit":3}, "media_type":"application/json",
            "body":{"profile":{"age":21,"state":"active","name":"Alice","roles":["admin"],
                "id":9007199254740993_u64,"mode":5,"child":{"age":22,"state":"active"}}}
        });
        let mut cases = Vec::new();
        for (pointer, value) in [
            ("/body/profile/age", serde_json::json!(17)),
            ("/body/profile/age", serde_json::json!(21.5)),
            ("/body/profile/state", serde_json::json!("deleted")),
            ("/body/profile", serde_json::json!({})),
            ("/body/profile", serde_json::json!("wrong")),
            ("/body/profile", serde_json::json!(null)),
            ("/body/profile/name", serde_json::json!("a")),
            ("/body/profile/roles", serde_json::json!([])),
            ("/body/profile/roles", serde_json::json!(["admin","admin"])),
            ("/body/profile/roles", serde_json::json!(["unknown"])),
            ("/body/profile/id", serde_json::json!(9007199254740992_u64)),
            ("/body/profile/mode", serde_json::json!(10)),
            ("/body/profile/child/age", serde_json::json!(17)),
            ("/query/limit", serde_json::json!(0)),
            ("/query/limit", serde_json::json!("3")),
            ("/media_type", serde_json::json!("application/vnd.other+json")),
        ] {
            let mut arguments = valid.clone();
            *arguments.pointer_mut(pointer).ok_or("missing control pointer")? = value;
            cases.push(arguments);
        }
        for pointer in ["/body", "/body/profile"] {
            let mut arguments = valid.clone();
            arguments.pointer_mut(pointer).and_then(serde_json::Value::as_object_mut)
                .ok_or("missing control object")?.insert("extra".into(), serde_json::json!(true));
            cases.push(arguments);
        }
        let mut rejected = 0;
        for arguments in cases {
            match binding.build_request("validation-proof/submit", "https://example.test", &arguments) {
                Err(incurs_openapi::runtime::HttpBindingError::Validation(_)) => rejected += 1,
                other => return Err(format!("invalid request control failed: {other:?}")),
            }
        }
        let mut requests = Vec::new();
        for arguments in [valid, serde_json::json!({"media_type":"application/vnd.other+json","body":{"other":true}})] {
            let request = binding.build_request("validation-proof/submit", "https://example.test", &arguments)
                .map_err(|e| e.to_string())?;
            requests.push(serde_json::json!({"url":request.url,"body":String::from_utf8(request.body.ok_or("missing body")?).map_err(|e| e.to_string())?}));
        }
        Ok(serde_json::json!({"schema_rejections":rejected,"requests":requests}))
    })
}

fn verify_response_statuses() -> std::result::Result<serde_json::Value, String> {
    use response_sdk::*;
    fn all_parts(response: AllStatusesResponse) -> (&'static str, OperationResponse) {
        match response {
            AllStatusesResponse::Status1XX(raw) => ("1XX", raw),
            AllStatusesResponse::Status2XX(raw) => ("2XX", raw),
            AllStatusesResponse::Status3XX(raw) => ("3XX", raw),
            AllStatusesResponse::Status4XX(raw) => ("4XX", raw),
            AllStatusesResponse::Status5XX(raw) => ("5XX", raw),
            AllStatusesResponse::Status201(raw) => ("201", raw),
            AllStatusesResponse::Status404(raw) => ("404", raw),
            AllStatusesResponse::Default(raw) => ("default", raw),
            AllStatusesResponse::Other(raw) => ("other", raw),
        }
    }

    fn sparse_parts(response: SparseStatusesResponse) -> (&'static str, OperationResponse) {
        match response {
            SparseStatusesResponse::Status2XX(raw) => ("2XX", raw),
            SparseStatusesResponse::Status4XX(raw) => ("4XX", raw),
            SparseStatusesResponse::Status418(raw) => ("418", raw),
            SparseStatusesResponse::Other(raw) => ("other", raw),
        }
    }

    fn fallback_parts(response: FallbackStatusesResponse) -> (&'static str, OperationResponse) {
        match response {
            FallbackStatusesResponse::Status2XX(raw) => ("2XX", raw),
            FallbackStatusesResponse::Default(raw) => ("default", raw),
            FallbackStatusesResponse::Other(raw) => ("other", raw),
        }
    }

    fn response(status: u16) -> OperationResponse {
        OperationResponse {
            status,
            headers: vec![
                ("set-cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
            ],
            body: Field::Value(JsonValue::Bytes(vec![0, 255, 1])),
        }
    }

    let mut classifications = 0_u32;
    for status in 0..=u16::MAX {
        let raw = response(status);
        let family = match status / 100 {
            1 => "1XX",
            2 => "2XX",
            3 => "3XX",
            4 => "4XX",
            5 => "5XX",
            _ => "default",
        };
        let expected = match status {
            201 => "201",
            404 => "404",
            _ => family,
        };
        let classified = AllStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = all_parts(classified);
        assert_eq!(label, expected, "all families: status {status}");
        assert_eq!(retained, raw);
        classifications += 1;

        let expected = if status == 418 {
            "418"
        } else {
            match status / 100 {
                2 => "2XX",
                4 => "4XX",
                _ => "other",
            }
        };
        let classified = SparseStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = sparse_parts(classified);
        assert_eq!(label, expected, "sparse families: status {status}");
        assert_eq!(retained, raw);
        classifications += 1;

        let expected = if status / 100 == 2 { "2XX" } else { "default" };
        let classified = FallbackStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = fallback_parts(classified);
        assert_eq!(label, expected, "family with default: status {status}");
        assert_eq!(retained, raw);
        classifications += 1;
    }

    Ok(serde_json::json!({"classifications": classifications,
        "layouts": 3, "raw_body_and_duplicate_headers": true}))
}

std::thread_local! {
    static RESPONSE_STATUS_RESULT: std::result::Result<serde_json::Value, String> = verify_response_statuses();
}

fn run_response_statuses() -> std::result::Result<serde_json::Value, String> {
    RESPONSE_STATUS_RESULT.with(Clone::clone)
}

async fn run_typed_responses() -> std::result::Result<serde_json::Value, String> {
    use typed_sdk::*;
    const VALID: &str = r#"{"id":18446744073709551617,"state":"active","stamp":"server","labels":[null,"hi"],"amount":0.12345678901234567890123456789,"choice":5,"joined":{"key":"k","count":2},"child":{"value":3,"next":{"value":4}},"extra":{"kept":true}}"#;

    fn raw(status: u16, content_type: Option<&str>, body: &[u8]) -> OperationResponse {
        let mut headers = vec![
            ("Set-Cookie".into(), "a=1".into()),
            ("Set-Cookie".into(), "b=2".into()),
        ];
        if let Some(content_type) = content_type {
            headers.push(("Content-Type".into(), content_type.into()));
        }
        OperationResponse {
            status,
            headers,
            body: Field::Value(JsonValue::Bytes(body.to_vec())),
        }
    }

    fn record(raw: OperationResponse) -> DecodedResponse<ReadRecord> {
        let FetchRecordTypedResponse::Status200(decoded) =
            FetchRecordResponse::from_response(raw).decode().unwrap()
        else {
            panic!("expected typed exact 200 response")
        };
        decoded
    }

    fn assert_record(decoded: DecodedResponse<ReadRecord>) {
        let Field::Value(value) = decoded.body else {
            panic!("record missing")
        };
        assert_eq!(value.id.as_str(), "18446744073709551617");
        assert_eq!(value.amount.as_str(), "0.12345678901234567890123456789");
        assert_eq!(value.state, ReadState::Active);
        assert_eq!(value.stamp, "server");
        assert!(matches!(value.secret, Field::Missing));
        assert!(matches!(value.note, Field::Missing));
        assert_eq!(value.labels, vec![None, Some("hi".to_string())]);
        let Field::Value(child) = value.child else {
            panic!("recursive child missing")
        };
        assert_eq!(child.value.as_str(), "3");
        let Field::Value(next) = child.next else {
            panic!("recursive next missing")
        };
        assert_eq!(next.value.as_str(), "4");
        assert_eq!(
            value.additional_properties,
            vec![(
                "extra".to_string(),
                JsonValue::Object(vec![("kept".to_string(), JsonValue::Bool(true))])
            )]
        );
        assert_eq!(
            decoded.raw.body,
            Field::Value(JsonValue::Bytes(VALID.as_bytes().to_vec()))
        );
        let cookies: Vec<_> = decoded
            .raw
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(cookies, ["a=1", "b=2"]);
    }

    fn extra_controls() {
        for (token, expected) in [
            ("9007199254740993", None),
            ("9007199254740992", Some(9007199254740992.0)),
            ("0.12345678901234567890123456789", None),
            ("1e-400", None),
            ("1e400", None),
            ("1.0", Some(1.0)),
            ("1e3", Some(1000.0)),
            ("-0", Some(-0.0)),
        ] {
            assert_eq!(
                ResponseNumber::new(token).unwrap().as_f64(),
                expected,
                "{token}"
            );
        }
        assert_eq!(
            ResponseInteger::new("18446744073709551617")
                .unwrap()
                .as_u64(),
            None
        );
        assert_eq!(ResponseInteger::new("1e3").unwrap().as_i64(), Some(1000));
        assert!(ResponseInteger::new("1e-400").is_err());

        for bytes in [Vec::new(), (0_u8..=255).collect::<Vec<_>>()] {
            let source = raw(200, Some("application/octet-stream"), &bytes);
            let RepresentTypedResponse::Status200(decoded) =
                RepresentResponse::from_response(source.clone())
                    .decode()
                    .unwrap()
            else {
                panic!("binary response status changed")
            };
            let Field::Value(RepresentStatus200Body::ApplicationOctetStream(actual)) = decoded.body
            else {
                panic!("binary response representation changed")
            };
            assert_eq!(actual, bytes);
            assert_eq!(decoded.raw, source);
        }
        let source = raw(
            200,
            Some("text/plain; charset=utf-8"),
            "hello 雪".as_bytes(),
        );
        let RepresentTypedResponse::Status200(decoded) =
            RepresentResponse::from_response(source.clone())
                .decode()
                .unwrap()
        else {
            panic!("text response status changed")
        };
        let Field::Value(RepresentStatus200Body::TextPlain(actual)) = decoded.body else {
            panic!("text response representation changed")
        };
        assert_eq!(actual, "hello 雪");
        assert_eq!(decoded.raw, source);
        for (content_type, bytes) in [
            (None, b"hello".as_slice()),
            (Some("text/plain"), b"wrong"),
            (Some("text/plain"), b""),
            (Some("text/plain; charset=us-ascii"), "hello 雪".as_bytes()),
            (Some("text/plain; charset=utf-8"), b"hello\xff"),
        ] {
            let source = raw(200, content_type, bytes);
            let error = RepresentResponse::from_response(source.clone())
                .decode()
                .unwrap_err();
            assert_eq!(error.raw, source);
        }
        let mut duplicate = raw(200, Some("application/json"), VALID.as_bytes());
        duplicate
            .headers
            .push(("content-type".into(), "text/plain".into()));
        let error = FetchRecordResponse::from_response(duplicate.clone())
            .decode()
            .unwrap_err();
        assert_eq!(error.kind, ResponseDecodeErrorKind::MediaType);
        assert_eq!(error.raw, duplicate);

        let source = raw(200, Some("image/png"), &[0, 255, 1]);
        let ImageTypedResponse::Status200(decoded) = ImageResponse::from_response(source.clone())
            .decode()
            .unwrap()
        else {
            panic!("image wildcard response status changed")
        };
        assert_eq!(decoded.body, Field::Value(vec![0, 255, 1]));
        assert_eq!(decoded.raw, source);
        assert!(
            ImageResponse::from_response(raw(200, None, b"image"))
                .decode()
                .is_err()
        );

        let source = OperationResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Field::Value(JsonValue::Object(vec![
                ("x".into(), JsonValue::Integer(1)),
                ("x".into(), JsonValue::Integer(2)),
            ])),
        };
        let error = AnyValueResponse::from_response(source.clone())
            .decode()
            .unwrap_err();
        assert_eq!(error.raw, source);
        let source = OperationResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Field::Value(JsonValue::ExactNumber("1,false".into())),
        };
        assert_eq!(
            AnyValueResponse::from_response(source.clone())
                .decode()
                .unwrap_err()
                .raw,
            source
        );

        let source = raw(777, Some("application/unknown"), b"untouched");
        let RepresentTypedResponse::Other(retained) =
            RepresentResponse::from_response(source.clone())
                .decode()
                .unwrap()
        else {
            panic!("unknown status changed")
        };
        assert_eq!(retained, source);
    }

    fn boundary_controls() {
        assert!(
            BoundedRefResponse::from_response(raw(200, Some("application/json"), br#""ok""#))
                .decode()
                .is_ok()
        );
        assert!(
            BoundedRefResponse::from_response(raw(200, Some("application/json"), br#""long""#))
                .decode()
                .is_err()
        );
        for bytes in [vec![0, 255], vec![0, 255, 1, 2]] {
            let BoundedBinaryTypedResponse::Status200(decoded) =
                BoundedBinaryResponse::from_response(raw(
                    200,
                    Some("application/octet-stream"),
                    &bytes,
                ))
                .decode()
                .unwrap()
            else {
                panic!("bounded binary status changed")
            };
            assert_eq!(decoded.body, Field::Value(bytes));
        }
        for bytes in [vec![], vec![0], vec![0, 1, 2, 3, 4]] {
            let source = raw(200, Some("application/octet-stream"), &bytes);
            let error = BoundedBinaryResponse::from_response(source.clone())
                .decode()
                .unwrap_err();
            assert_eq!(error.kind, ResponseDecodeErrorKind::Schema);
            assert_eq!(error.raw, source);
        }
        assert!(
            FalseBinaryResponse::from_response(raw(200, Some("application/octet-stream"), b""))
                .decode()
                .is_err()
        );
        let JsonBinaryTypedResponse::Status200(decoded) = JsonBinaryResponse::from_response(raw(
            200,
            Some("application/json"),
            br#""json string""#,
        ))
        .decode()
        .unwrap() else {
            panic!("JSON binary annotation changed status")
        };
        assert_eq!(decoded.body, Field::Value("json string".to_owned()));
        let NullableTypedResponse::Status200(decoded) =
            NullableResponse::from_response(raw(200, Some("application/json"), b"null"))
                .decode()
                .unwrap()
        else {
            panic!("nullable response status changed")
        };
        assert!(matches!(decoded.body, Field::Null));
        assert!(
            NullableResponse::from_response(raw(200, Some("application/json"), b""))
                .decode()
                .is_err()
        );
    }

    fn constraint_only_controls() {
        for body in [
            b"7".as_slice(),
            b"null",
            br#""text""#,
            b"[1]",
            br#"{"name":"ok"}"#,
        ] {
            assert!(
                LooseObjectResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_ok(),
                "constraint-only schema rejected {}",
                String::from_utf8_lossy(body)
            );
            assert!(
                LooseAllOfResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_ok(),
                "constraint-only allOf rejected {}",
                String::from_utf8_lossy(body)
            );
        }
        for body in [b"{}".as_slice(), br#"{"name":false}"#] {
            assert!(
                LooseObjectResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_err()
            );
            assert!(
                LooseAllOfResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_err()
            );
        }
    }

    fn keyword_controls() {
        let KeywordsTypedResponse::Status200(decoded) = KeywordsResponse::from_response(raw(200, Some("application/json"), br#"{"box":"b","object":"o","known":"k","value":"v","additional_properties":"named","empty":{},"line\nbreak\rcell\u0000":"split","extra":true}"#)).decode().unwrap() else {
        panic!("keyword response status changed")
    };
        let Field::Value(value) = decoded.body else {
            panic!("keyword object missing")
        };
        assert_eq!(value.box_, Field::Value("b".into()));
        assert_eq!(value.line_break_cell, Field::Value("split".into()));
        assert_eq!(value.object, Field::Value("o".into()));
        assert_eq!(value.known, Field::Value("k".into()));
        assert_eq!(value.value, Field::Value("v".into()));
        assert_eq!(value.additional_properties, Field::Value("named".into()));
        assert_eq!(
            value.additional_properties_2,
            vec![("extra".into(), JsonValue::Bool(true))]
        );
        let Field::Value(empty) = value.empty else {
            panic!("empty object missing")
        };
        assert!(empty.additional_properties.is_empty());
    }

    fn nullable_reference_controls() {
        let NullableRefsTypedResponse::Status200(decoded) =
            NullableRefsResponse::from_response(raw(
                200,
                Some("application/json"),
                br#"{"name":null,"items":[null,"hi"]}"#,
            ))
            .decode()
            .unwrap()
        else {
            panic!("nullable references changed status")
        };
        let Field::Value(value) = decoded.body else {
            panic!("nullable referenced response missing")
        };
        assert_eq!(value.name, Field::Null);
        assert_eq!(value.items, vec![None, Some("hi".to_string())]);
        for invalid in [
            br#"{"items":[]}"#.as_slice(),
            br#"{"name":"x","items":[]}"#,
            br#"{"name":null,"items":["x"]}"#,
        ] {
            assert!(
                NullableRefsResponse::from_response(raw(200, Some("application/json"), invalid))
                    .decode()
                    .is_err()
            );
        }
    }

    fn binary_reference_controls() {
        for body in [b"ab".as_slice(), b"abc"] {
            assert!(
                BinaryReferenceResponse::from_response(raw(
                    200,
                    Some("application/octet-stream"),
                    body
                ))
                .decode()
                .is_ok()
            );
        }
        for body in [b"a".as_slice(), b"abcd"] {
            assert!(
                BinaryReferenceResponse::from_response(raw(
                    200,
                    Some("application/octet-stream"),
                    body
                ))
                .decode()
                .is_err(),
                "referenced binary bound was overwritten"
            );
        }
        assert!(
            BinaryAllOfResponse::from_response(raw(200, Some("application/octet-stream"), b"ab"))
                .decode()
                .is_ok()
        );
        for body in [b"a".as_slice(), b"abc"] {
            assert!(
                BinaryAllOfResponse::from_response(raw(
                    200,
                    Some("application/octet-stream"),
                    body
                ))
                .decode()
                .is_err()
            );
        }
    }

    fn mixed_type_controls() {
        for body in [
            br#""hi""#.as_slice(),
            b"2",
            b"2.5",
            b"18446744073709551617",
            b"null",
        ] {
            assert!(
                MixedTypesResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_ok(),
                "mixed type response rejected {}",
                String::from_utf8_lossy(body)
            );
        }
        for body in [br#""x""#.as_slice(), b"1", b"false", b"[]", b"{}"] {
            assert!(
                MixedTypesResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_err(),
                "mixed type response admitted {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    fn nullable_object_controls() {
        let NullableObjectTypedResponse::Status200(decoded) =
            NullableObjectResponse::from_response(raw(200, Some("application/json"), b"null"))
                .decode()
                .unwrap()
        else {
            panic!("nullable object changed status")
        };
        assert!(matches!(decoded.body, Field::Null));
        let NullableObjectTypedResponse::Status200(decoded) =
            NullableObjectResponse::from_response(raw(
                200,
                Some("application/json"),
                br#"{"flag":true}"#,
            ))
            .decode()
            .unwrap()
        else {
            panic!("nullable object changed status")
        };
        let Field::Value(value) = decoded.body else {
            panic!("nullable object value missing")
        };
        assert!(value.flag);
        for body in [
            b"{}".as_slice(),
            br#"{"flag":null}"#,
            b"false",
            br#"{"flag":true,"extra":1}"#,
        ] {
            assert!(
                NullableObjectResponse::from_response(raw(200, Some("application/json"), body))
                    .decode()
                    .is_err()
            );
        }
    }

    nullable_object_controls();
    mixed_type_controls();
    binary_reference_controls();
    nullable_reference_controls();
    keyword_controls();
    constraint_only_controls();
    boundary_controls();
    extra_controls();
    assert_record(record(raw(
        200,
        Some("Application/JSON; charset=utf-8"),
        VALID.as_bytes(),
    )));
    assert_record(record(raw(200, None, VALID.as_bytes())));
    for bad in [
        VALID.replace("\"stamp\":\"server\",", ""),
        VALID.replace("\"active\"", "\"deleted\""),
        VALID.replace("18446744073709551617", "18446744073709551618"),
        VALID.replace("\"choice\":5", "\"choice\":11"),
        VALID.replace("\"count\":2", "\"count\":2,\"extra\":true"),
        VALID.replace("\"value\":4", "\"value\":\"bad\""),
        VALID.replace(
            "\"state\":\"active\"",
            "\"state\":\"active\",\"state\":\"paused\"",
        ),
        "null".into(),
        "".into(),
        "{} trailing".into(),
    ] {
        let source = raw(200, Some("application/json"), bad.as_bytes());
        let error = FetchRecordResponse::from_response(source.clone())
            .decode()
            .unwrap_err();
        assert_eq!(error.raw, source, "decode failure lost raw response");
    }
    let source = raw(200, Some("image/png"), VALID.as_bytes());
    let error = FetchRecordResponse::from_response(source.clone())
        .decode()
        .unwrap_err();
    assert_eq!(error.kind, ResponseDecodeErrorKind::MediaType);
    assert_eq!(error.raw, source);

    let source = raw(
        429,
        Some("application/problem+json"),
        br#"{"message":"slow down"}"#,
    );
    let FetchRecordTypedResponse::Status4XX(decoded) =
        FetchRecordResponse::from_response(source.clone())
            .decode()
            .unwrap()
    else {
        panic!("expected typed 4XX family")
    };
    let Field::Value(problem) = decoded.body else {
        panic!("problem missing")
    };
    assert_eq!(problem.message, "slow down");
    assert_eq!(decoded.raw, source);

    let source = raw(
        404,
        Some("application/problem+json"),
        br#"{"message":"missing"}"#,
    );
    assert!(matches!(
        FetchRecordResponse::from_response(source).decode().unwrap(),
        FetchRecordTypedResponse::Status404(_)
    ));

    let source = raw(503, Some("application/json"), br#"{"message":"fallback"}"#);
    assert!(matches!(
        FetchRecordResponse::from_response(source).decode().unwrap(),
        FetchRecordTypedResponse::Default(_)
    ));

    for status in [204, 304] {
        assert!(
            NoContentResponse::from_response(raw(status, None, b""))
                .decode()
                .is_ok()
        );
        assert!(
            NoContentResponse::from_response(raw(status, None, b"unexpected"))
                .decode()
                .is_err()
        );
    }
    assert!(
        HeadRecordResponse::from_response(raw(200, Some("application/json"), b""))
            .decode()
            .is_ok()
    );
    assert!(
        HeadRecordResponse::from_response(raw(200, Some("application/json"), b"unexpected"))
            .decode()
            .is_err()
    );

    let source = raw(
        200,
        Some("application/json"),
        br#"{"$serde_json::private::Number":"not-a-number","n":1e400}"#,
    );
    let AnyValueTypedResponse::Status200(decoded) =
        AnyValueResponse::from_response(source).decode().unwrap()
    else {
        panic!("expected arbitrary JSON response")
    };
    let Field::Value(JsonValue::Object(fields)) = decoded.body else {
        panic!("object changed")
    };
    assert_eq!(
        fields[0],
        (
            "$serde_json::private::Number".into(),
            JsonValue::String("not-a-number".into())
        )
    );
    assert_eq!(
        fields[1],
        ("n".into(), JsonValue::ExactNumber("1e400".into()))
    );

    let source = raw(
        200,
        Some("application/json"),
        b"[1.0,1e3,18446744073709551617]",
    );
    let NumbersTypedResponse::Status200(decoded) =
        NumbersResponse::from_response(source).decode().unwrap()
    else {
        panic!("expected integer sequence")
    };
    let Field::Value(values) = decoded.body else {
        panic!("integers missing")
    };
    assert_eq!(
        values
            .iter()
            .map(ResponseInteger::as_str)
            .collect::<Vec<_>>(),
        ["1.0", "1e3", "18446744073709551617"]
    );
    assert!(
        NumbersResponse::from_response(raw(200, Some("application/json"), b"[1.1]"))
            .decode()
            .is_err()
    );

    struct ProofTransport;
    impl Transport for ProofTransport {
        type Error = String;
        fn call(
            &self,
            request: OperationRequest,
        ) -> impl std::future::Future<Output = std::result::Result<OperationResponse, String>>
        {
            async move {
                if request.operation_id != "worker-proof/fetchRecord"
                    || request.method != "GET"
                    || request.path_template != "/record"
                {
                    return Err("typed client changed operation binding".into());
                }
                Ok(raw(200, Some("application/json"), VALID.as_bytes()))
            }
        }
    }
    let FetchRecordTypedResponse::Status200(decoded) = Client::new(ProofTransport)
        .fetch_record_typed(FetchRecordArgs {})
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("typed client changed response status".into());
    };
    let Field::Value(value) = decoded.body else {
        return Err("typed record absent".into());
    };
    let cookies = decoded
        .raw
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    let raw_body_retained =
        decoded.raw.body == Field::Value(JsonValue::Bytes(VALID.as_bytes().to_vec()));
    let mut errors = Vec::new();
    for source in [
        raw(200, Some("application/json"), b"{broken"),
        raw(200, Some("image/png"), VALID.as_bytes()),
        raw(
            200,
            Some("application/json"),
            VALID
                .replace("18446744073709551617", "18446744073709551618")
                .as_bytes(),
        ),
    ] {
        let error = FetchRecordResponse::from_response(source.clone())
            .decode()
            .unwrap_err();
        assert_eq!(error.raw, source);
        errors.push(format!("{:?}", error.kind));
    }
    let Field::Value(child) = value.child else {
        return Err("child absent".into());
    };
    let Field::Value(next) = child.next else {
        return Err("next child absent".into());
    };
    Ok(serde_json::json!({
        "id":value.id.as_str(),"amount":value.amount.as_str(),"stamp":value.stamp,
        "labels":value.labels,"nested_integer":next.value.as_str(),
        "write_only_omitted":matches!(value.secret,Field::Missing),
        "default_not_inserted":matches!(value.note,Field::Missing),
        "extras":JsonValue::Object(value.additional_properties).to_json_string().map_err(|error|error.to_string())?,
        "raw_body_retained":raw_body_retained,"cookies":cookies,
        "media_type":decoded.media_type,"error_kinds":errors
    }))
}

#[event(fetch)]
async fn fetch(_request: Request, _env: Env, _context: Context) -> Result<Response> {
    let result = run_validation().and_then(|validation| {
        run_media().and_then(|media| {
            run_numeric().and_then(|numeric| {
                run_request_validation().and_then(|requests| run_response_statuses().map(|responses| serde_json::json!({"validation":validation,"media":media,"numeric":numeric,"request_validation":requests,"response_statuses":responses})))
            })
        })
    });
    let result = match result {
        Ok(mut value) => match run_typed_responses().await {
            Ok(typed) => {
                value["typed_responses"] = typed;
                Ok(value)
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(value) => Response::from_json(&value),
        Err(error) => Response::error(error, 500),
    }
}
