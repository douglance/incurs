//! A real generated-client consumer against an isolated loopback HTTP fixture.
use incurs_openapi::{
    ResolvedOpenApi,
    runtime::{OpenApiHttpRequest, OpenApiHttpResponse, OpenApiTransport, invoke_http},
};
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
    contract: ResolvedOpenApi,
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
            let operation = self
                .contract
                .operations
                .iter()
                .find(|op| op.id == request.operation_id)
                .ok_or("unknown operation")?;
            let response = invoke_http(
                operation,
                &self.contract.schemas,
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

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let cases: Vec<(&str, Option<&str>, &[u8], u16)> = vec![
            ("GET /zen HTTP/1.1\r\n", None, b"", 200),
            (
                "POST /markdown/raw HTTP/1.1\r\n",
                Some("text/plain"),
                "hello\n雪".as_bytes(),
                200,
            ),
            (
                "POST /markdown/raw HTTP/1.1\r\n",
                Some("text/x-markdown"),
                "hello\n雪".as_bytes(),
                200,
            ),
            (
                "POST /repos/owner/repository/releases/7/assets?name=file%20name.bin HTTP/1.1\r\n",
                Some("application/octet-stream"),
                &[0, 255, 128, 13, 10],
                201,
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
            write!(stream,"HTTP/1.1 {status} OK\r\nContent-Length: 2\r\nX-OpenAPI-Proof: full-github\r\nConnection: close\r\n\r\nok").unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!(env!("OPENAPI_CORPUS_CONTRACT"))).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    assert_eq!(
        ready(client.meta_get_zen(MetaGetZenArgs {}))
            .unwrap()
            .status(),
        200
    );
    for body in [
        MarkdownRenderRawBody::TextPlain(Field::Value("hello\n雪".into())),
        MarkdownRenderRawBody::TextXMarkdown(Field::Value("hello\n雪".into())),
    ] {
        assert_eq!(
            ready(client.markdown_render_raw(MarkdownRenderRawArgs { body }))
                .unwrap()
                .status(),
            200
        );
    }
    let uploaded = ready(
        client.repos_upload_release_asset(ReposUploadReleaseAssetArgs {
            owner: "owner".into(),
            repo: "repository".into(),
            release_id: 7,
            name: "file name.bin".into(),
            label: Field::Missing,
            body: ReposUploadReleaseAssetBody::ApplicationOctetStream(vec![0, 255, 128, 13, 10]),
        }),
    )
    .unwrap();
    assert_eq!(uploaded.status(), 201);
    server.join().unwrap();
    println!(
        "FULL_GITHUB_PACKAGED_SDK_HTTP_PASSED: GET zen, both Markdown media, and literal binary upload through loopback"
    );
}
