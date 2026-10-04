//! A real generated-client consumer against an isolated loopback HTTP fixture.
use incurs_forge::runtime::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport};
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
impl ForgeTransport for Exchange {
    fn exchange(
        &self,
        request: ForgeHttpRequest,
    ) -> impl Future<Output = incurs_forge::ForgeResult<ForgeHttpResponse>> {
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
            Ok(ForgeHttpResponse {
                status,
                headers,
                body: bytes[split + 4..].to_vec(),
            })
        }
    }
}

struct Bridge {
    binding: incurs_forge::runtime::HttpBinding,
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

const USER_JSON: &str = r#"{"success":true,"errors":[],"messages":[],"result":{"id":"6d7f2f5f5b1d4a0e9081fdc98d432fd1","email":"alice@example.com","first_name":"Alice"}}"#;
const ERROR_JSON: &str = r#"{"success":false,"errors":[{"code":10000,"message":"slow down"}],"messages":[],"result":null}"#;

fn main() {
    for status in 0..=u16::MAX {
        let raw = OperationResponse {
            status,
            headers: vec![
                ("set-cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
            ],
            body: Field::Value(JsonValue::Bytes(vec![0, 255, 1])),
        };
        let classified = UserUserDetailsResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = match classified {
            UserUserDetailsResponse::Status200(value) => ("exact", value),
            UserUserDetailsResponse::Status4XX(value) => ("family", value),
            UserUserDetailsResponse::Other(value) => ("other", value),
        };
        let expected = if status == 200 {
            "exact"
        } else if status / 100 == 4 {
            "family"
        } else {
            "other"
        };
        assert_eq!(label, expected, "Cloudflare user status {status}");
        assert_eq!(retained, raw);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let cases: Vec<(&str, Option<&str>, &[u8], u16)> = vec![
            ("GET /user HTTP/1.1\r\n", None, b"", 200),
            ("GET /user HTTP/1.1\r\n", None, b"", 429),
            (
                "POST /accounts/account/dlp/datasets/7cf13eac-d73b-4f47-a839-6176403cddcc/versions/7/entries/0a79c2d8-cab8-4eb8-a2b6-9d50aef598d8 HTTP/1.1\r\n",
                Some("application/octet-stream"),
                &[0, 255, 128, 13, 10],
                200,
            ),
            (
                "POST /accounts/account/pay-per-use/usage-reports HTTP/1.1\r\n",
                Some("application/jsonl"),
                b"{\"id\":\"usage-1\",\"source\":\"https://example.test/usage\",\"used_at\":\"2026-10-01T12:00:00Z\"}\n",
                200,
            ),
            (
                "POST /accounts/account/vectorize/v2/indexes/index/insert HTTP/1.1\r\n",
                Some("application/x-ndjson"),
                b"{\"id\":\"vector-1\",\"values\":[1,2,3]}\n{\"id\":\"vector-2\",\"values\":[4,5,6]}\n",
                200,
            ),
        ];
        for (request_line, content_type, expected, status) in cases {
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
            assert_eq!(line, request_line);
            let mut length = None;
            let mut received_type = None;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
                let (name, value) = line.trim_end().split_once(':').unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
                if name.eq_ignore_ascii_case("content-type") {
                    received_type = Some(value.trim().to_owned());
                }
            }
            assert_eq!(received_type.as_deref(), content_type);
            assert_eq!(length, Some(expected.len()));
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(body, expected);
            let reply = if request_line == "GET /user HTTP/1.1\r\n" {
                if status == 200 { USER_JSON } else { ERROR_JSON }
            } else {
                "ok"
            };
            write!(stream,"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nX-Forge-Proof: full-cloudflare\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!(env!("FORGE_CORPUS_CONTRACT"))).unwrap();
    let client = Client::new(Bridge {
        binding: incurs_forge::runtime::HttpBinding::new(&contract).unwrap(),
        exchange: Exchange { address },
    });
    let UserUserDetailsTypedResponse::Status200(decoded) =
        ready(client.user_user_details_typed(UserUserDetailsArgs {})).unwrap()
    else {
        panic!("Cloudflare HTTP 200 did not decode its declared response");
    };
    assert_eq!(decoded.raw.status, 200);
    assert_eq!(
        decoded.raw.body,
        Field::Value(JsonValue::Bytes(USER_JSON.as_bytes().to_vec()))
    );
    assert_eq!(decoded.media_type.as_deref(), Some("application/json"));
    assert_eq!(
        decoded
            .raw
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>(),
        ["a=1", "b=2"]
    );
    let Field::Value(envelope) = decoded.body else {
        panic!("Cloudflare user envelope missing");
    };
    assert!(envelope.success);
    assert_eq!(envelope.errors.as_json(), &JsonValue::Array(vec![]));
    let Field::Value(user) = envelope.result else {
        panic!("Cloudflare user result missing");
    };
    assert_eq!(user.id, "6d7f2f5f5b1d4a0e9081fdc98d432fd1");
    assert_eq!(user.email, "alice@example.com");
    assert_eq!(user.first_name, Field::Value("Alice".into()));
    assert!(matches!(user.suspended, Field::Missing));

    let UserUserDetailsTypedResponse::Status4XX(decoded) =
        ready(client.user_user_details_typed(UserUserDetailsArgs {})).unwrap()
    else {
        panic!("Cloudflare HTTP 429 did not decode its declared 4XX response");
    };
    assert_eq!(decoded.raw.status, 429);
    assert_eq!(
        decoded.raw.body,
        Field::Value(JsonValue::Bytes(ERROR_JSON.as_bytes().to_vec()))
    );
    assert!(
        decoded
            .raw
            .headers
            .contains(&("X-Forge-Proof".into(), "full-cloudflare".into()))
    );
    let Field::Value(problem) = decoded.body else {
        panic!("Cloudflare error envelope missing");
    };
    assert!(!problem.success);
    assert!(matches!(problem.result, Field::Null));
    assert_eq!(
        problem.errors.as_json().to_json_string().unwrap(),
        r#"[{"code":10000,"message":"slow down"}]"#
    );
    assert_eq!(
        ready(
            client.dlp_datasets_upload_dataset_column(DlpDatasetsUploadDatasetColumnArgs {
                account_id: "account".into(),
                dataset_id: "7cf13eac-d73b-4f47-a839-6176403cddcc".into(),
                entry_id: "0a79c2d8-cab8-4eb8-a2b6-9d50aef598d8".into(),
                version: 7,
                body: DlpDatasetsUploadDatasetColumnBody::ApplicationOctetStream(vec![
                    0, 255, 128, 13, 10
                ]),
            })
        )
        .unwrap()
        .status(),
        200
    );
    assert_eq!(
        ready(client.pay_per_use_submit_usage_report(PayPerUseSubmitUsageReportArgs {
            account_id: "account".into(),
            body: "{\"id\":\"usage-1\",\"source\":\"https://example.test/usage\",\"used_at\":\"2026-10-01T12:00:00Z\"}\n".into(),
        }))
        .unwrap()
        .status(),
        200
    );
    assert_eq!(
        ready(client.vectorize_insert_vector(VectorizeInsertVectorArgs {
            account_id: "account".into(),
            index_name: "index".into(),
            unparsable_behavior: Field::Missing,
            body: VectorizeInsertVectorBody::ApplicationXNdjson(
                b"{\"id\":\"vector-1\",\"values\":[1,2,3]}\n{\"id\":\"vector-2\",\"values\":[4,5,6]}\n".to_vec(),
            ),
        }))
        .unwrap()
        .status(),
        200
    );
    server.join().unwrap();
    println!(
        "FULL_CLOUDFLARE_PACKAGED_SDK_HTTP_PASSED: 65536 user-response classifications; GET typed user 200 and 429 envelopes, binary upload, JSONL, and NDJSON through validated loopback binding"
    );
}
