//! A real generated-client consumer against an isolated loopback HTTP fixture.
use incurs_openapi::{
    ResolvedOpenApi,
    runtime::{OpenApiHttpRequest, OpenApiHttpResponse, OpenApiTransport, invoke_http},
};
use sdk::{
    Client, Field, JsonValue, OperationRequest, OperationResponse, SendMultipartArgs, Transport,
};
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

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = concat!(
        "--incurs-openapi-1\r\nContent-Disposition: form-data; name=\"encoded\"\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8B\r\n",
        "--incurs-openapi-1\r\nContent-Disposition: form-data; name=\"tags\"\r\nContent-Type: text/plain\r\n\r\nred blue\r\n",
        "--incurs-openapi-1\r\nContent-Disposition: form-data; name=\"tags\"\r\nContent-Type: text/plain\r\n\r\n+&\r\n",
        "--incurs-openapi-1\r\nContent-Disposition: form-data; name=\"text\"\r\nContent-Type: text/plain\r\n\r\n雪 --incurs-openapi-0\r\n",
        "--incurs-openapi-1--\r\n"
    );
    let server = std::thread::spawn(move || {
        for (expected, content_type) in [
            (
                expected,
                Some("multipart/form-data; boundary=incurs-openapi-1"),
            ),
            (
                "--incurs-openapi-0--\r\n",
                Some("multipart/form-data; boundary=incurs-openapi-0"),
            ),
            ("", None),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, "POST /multipart HTTP/1.1\r\n");
            let mut length = None;
            let mut actual_type = None;
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
                    actual_type = Some(value.trim().to_string());
                }
            }
            assert_eq!(actual_type.as_deref(), content_type);
            assert_eq!(length, Some(expected.len()));
            let mut received = vec![0; length.unwrap()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, expected.as_bytes());
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    for body in [
        Field::Value(JsonValue::Object(vec![
            (
                "text".into(),
                JsonValue::String("雪 --incurs-openapi-0".into()),
            ),
            (
                "tags".into(),
                JsonValue::Array(vec![
                    JsonValue::String("red blue".into()),
                    JsonValue::String("+&".into()),
                ]),
            ),
            ("encoded".into(), JsonValue::String("AP8B".into())),
        ])),
        Field::Value(JsonValue::Object(Vec::new())),
        Field::Missing,
    ] {
        let mut future = std::pin::pin!(client.send_multipart(SendMultipartArgs { body }));
        let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("blocking fixture yielded");
        };
        assert_eq!(result.unwrap().status(), 204);
    }
    server.join().unwrap();
    println!(
        "generated multipart SDK -> HTTP: repeated fields, encoded bytes, boundary collision, empty and omitted bodies passed"
    );
}
