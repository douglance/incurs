//! A real generated-client consumer against an isolated loopback HTTP fixture.
use incurs_openapi::runtime::{OpenApiHttpRequest, OpenApiHttpResponse, OpenApiTransport};
use sdk::*;
use std::{
    future::Future,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    task::{Context, Poll, Waker},
    time::Duration,
};

struct Exchange {
    address: std::net::SocketAddr,
}
impl OpenApiTransport for Exchange {
    fn exchange(
        &self,
        request: OpenApiHttpRequest,
    ) -> impl Future<Output = incurs_openapi::OpenApiResult<OpenApiHttpResponse>> {
        async move {
            let mut stream = TcpStream::connect(self.address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let base = format!("http://{}", self.address);
            let target = request.url.strip_prefix(&base).unwrap();
            write!(
                stream,
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
                request.method, target, self.address
            )
            .unwrap();
            for (name, value) in request.headers {
                write!(stream, "{name}: {value}\r\n").unwrap();
            }
            let body = request.body.unwrap_or_default();
            write!(stream, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let split = bytes
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .unwrap();
            let text = std::str::from_utf8(&bytes[..split]).unwrap();
            let mut lines = text.lines();
            let status = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            let headers = lines
                .map(|line| {
                    let (name, value) = line.split_once(':').unwrap();
                    (name.to_owned(), value.trim().to_owned())
                })
                .collect();
            Ok(OpenApiHttpResponse {
                status,
                headers,
                body: bytes[split + 4..].to_vec(),
            })
        }
    }
}

struct Bridge {
    binding: incurs_openapi::runtime::HttpBinding,
    exchange: Exchange,
}
impl Transport for Bridge {
    type Error = String;
    fn call(
        &self,
        request: OperationRequest,
    ) -> impl Future<Output = Result<OperationResponse, Self::Error>> {
        async move {
            let arguments =
                serde_json::from_str(&request.arguments_json().map_err(|e| format!("{e:?}"))?)
                    .map_err(|e| format!("{e}"))?;
            let response = self
                .binding
                .invoke(
                    request.operation_id,
                    &format!("http://{}", self.exchange.address),
                    &arguments,
                    &self.exchange,
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(OperationResponse {
                status: response.status,
                headers: response.headers,
                body: Field::Value(JsonValue::Bytes(response.body)),
            })
        }
    }
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("blocking fixture yielded");
    };
    result
}

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
    let RepresentTypedResponse::Other(retained) = RepresentResponse::from_response(source.clone())
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
        let BoundedBinaryTypedResponse::Status200(decoded) = BoundedBinaryResponse::from_response(
            raw(200, Some("application/octet-stream"), &bytes),
        )
        .decode()
        .unwrap() else {
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
    let JsonBinaryTypedResponse::Status200(decoded) =
        JsonBinaryResponse::from_response(raw(200, Some("application/json"), br#""json string""#))
            .decode()
            .unwrap()
    else {
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
    let NullableRefsTypedResponse::Status200(decoded) = NullableRefsResponse::from_response(raw(
        200,
        Some("application/json"),
        br#"{"name":null,"items":[null,"hi"]}"#,
    ))
    .decode()
    .unwrap() else {
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
            BinaryAllOfResponse::from_response(raw(200, Some("application/octet-stream"), body))
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
    let NullableObjectTypedResponse::Status200(decoded) = NullableObjectResponse::from_response(
        raw(200, Some("application/json"), br#"{"flag":true}"#),
    )
    .decode()
    .unwrap() else {
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

fn main() {
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

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let cases: Vec<(&str, &str, Vec<u8>)> = vec![
            ("/record", "application/json", VALID.as_bytes().to_vec()),
            (
                "/represent",
                "text/plain; charset=utf-8",
                "hello 雪\n".as_bytes().to_vec(),
            ),
            (
                "/represent",
                "application/octet-stream",
                (0..=255).collect(),
            ),
            ("/record", "application/json", b"{broken".to_vec()),
        ];
        for (path, media, body) in cases {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
            }
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        binding: incurs_openapi::runtime::HttpBinding::new(&contract).unwrap(),
        exchange: Exchange { address },
    });
    let FetchRecordTypedResponse::Status200(decoded) =
        ready(client.fetch_record_typed(FetchRecordArgs {})).unwrap()
    else {
        panic!("HTTP typed record missing")
    };
    assert_record(decoded);
    let RepresentTypedResponse::Status200(decoded) =
        ready(client.represent_typed(RepresentArgs {})).unwrap()
    else {
        panic!("HTTP text missing")
    };
    assert_eq!(
        decoded.body,
        Field::Value(RepresentStatus200Body::TextPlain("hello 雪\n".into()))
    );
    assert_eq!(
        decoded.raw.body,
        Field::Value(JsonValue::Bytes("hello 雪\n".as_bytes().to_vec()))
    );
    let RepresentTypedResponse::Status200(decoded) =
        ready(client.represent_typed(RepresentArgs {})).unwrap()
    else {
        panic!("HTTP binary missing")
    };
    assert_eq!(
        decoded.body,
        Field::Value(RepresentStatus200Body::ApplicationOctetStream(
            (0..=255).collect()
        ))
    );
    assert_eq!(
        decoded.raw.body,
        Field::Value(JsonValue::Bytes((0..=255).collect()))
    );
    let ClientError::Decode(error) =
        ready(client.fetch_record_typed(FetchRecordArgs {})).unwrap_err()
    else {
        panic!("HTTP decode error changed kind")
    };
    assert_eq!(error.kind, ResponseDecodeErrorKind::Json);
    assert_eq!(error.raw.status, 200);
    assert_eq!(
        error.raw.body,
        Field::Value(JsonValue::Bytes(b"{broken".to_vec()))
    );
    assert_eq!(
        error
            .raw
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .count(),
        2
    );
    server.join().unwrap();
    struct Unavailable;
    impl Transport for Unavailable {
        type Error = &'static str;
        async fn call(&self, _: OperationRequest) -> Result<OperationResponse, Self::Error> {
            Err("offline")
        }
    }
    assert!(matches!(
        ready(Client::new(Unavailable).fetch_record_typed(FetchRecordArgs {})),
        Err(ClientError::Transport("offline"))
    ));
    println!("TYPED_RESPONSE_ARCHIVE_HTTP_PASSED exchanges=4");
}
