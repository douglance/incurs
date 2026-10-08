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
        for expected in [
            "GET /accounts/account/datasets/value%20dataset HTTP/1.1\r\n",
            "GET /accounts/account/datasets/default%20dataset HTTP/1.1\r\n",
            "GET /datasets/first/zones/zone HTTP/1.1\r\n",
            "GET /content/null HTTP/1.1\r\n",
            "GET /content/%22json%20dataset%22 HTTP/1.1\r\n",
            "GET /content/null HTTP/1.1\r\n",
        ] {
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
            assert_eq!(line, expected);
            let mut length = None;
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
            }
            assert_eq!(length, Some(0));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    for dataset_id in [Field::Missing] {
        assert!(
            NullablePathArgs {
                account_id: "account".into(),
                dataset_id
            }
            .into_request()
            .arguments_json()
            .is_err(),
            "required path cannot be omitted"
        );
    }
    for dataset_id in [Field::Null, Field::Default(JsonValue::Null)] {
        let error = ready(client.nullable_path(NullablePathArgs {
            account_id: "account".into(),
            dataset_id,
        }))
        .unwrap_err();
        assert!(error.contains("null parameters are unsupported"), "{error}");
    }
    for dataset_id in [
        Field::Value("value dataset".into()),
        NullablePathArgs::dataset_id_default(),
    ] {
        assert_eq!(
            ready(client.nullable_path(NullablePathArgs {
                account_id: "account".into(),
                dataset_id
            }))
            .unwrap()
            .status(),
            200
        );
    }
    assert_eq!(
        ready(client.nullable_first(NullableFirstArgs {
            dataset_id: Field::Value("first".into()),
            zone_id: "zone".into()
        }))
        .unwrap()
        .status(),
        200
    );
    for dataset_id in [
        Field::Null,
        Field::Value("json dataset".into()),
        Field::Default(JsonValue::Null),
    ] {
        assert_eq!(
            ready(client.nullable_content(NullableContentArgs { dataset_id }))
                .unwrap()
                .status(),
            200
        );
    }
    assert!(
        NullableContentArgs {
            dataset_id: Field::Missing
        }
        .into_request()
        .arguments_json()
        .is_err()
    );
    server.join().unwrap();
    println!(
        "NULLABLE_PATH_PACKAGED_HTTP_PASSED: six literal requests; missing rejected; null preserved for JSON content and rejected for ordinary path serialization"
    );
}
