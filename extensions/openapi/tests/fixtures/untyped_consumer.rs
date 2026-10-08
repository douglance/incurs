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

fn main() {
    assert!(Loose::try_new(JsonValue::String("scalar".into())).is_ok());
    assert!(Loose::try_new(JsonValue::Integer(0)).is_ok());
    assert!(Loose::try_new(JsonValue::Null).is_ok());
    assert!(Loose::try_new(JsonValue::Bool(false)).is_ok());
    assert!(Loose::try_new(JsonValue::Array(vec![])).is_ok());
    assert!(Loose::try_new(JsonValue::Object(vec![])).is_err());
    assert!(
        Loose::try_new(JsonValue::Object(vec![(
            "name".into(),
            JsonValue::Integer(1)
        )]))
        .is_err()
    );
    assert!(
        Loose::try_new(JsonValue::Object(vec![(
            "name".into(),
            JsonValue::String("ok".into())
        )]))
        .is_ok()
    );
    assert!(Choice::try_new(JsonValue::String("ok".into())).is_ok());
    assert!(Choice::try_new(JsonValue::Integer(7)).is_ok());
    assert!(Choice::try_new(JsonValue::Integer(8)).is_err());
    assert!(Choice::try_new(JsonValue::Null).is_err());
    assert!(Minimum::try_new(JsonValue::Integer(2)).is_ok());
    assert!(Minimum::try_new(JsonValue::Integer(1)).is_err());
    assert!(Minimum::try_new(JsonValue::String("not a number".into())).is_ok());
    assert!(Pattern::try_new(JsonValue::String("ok".into())).is_ok());
    assert!(Pattern::try_new(JsonValue::String("bad".into())).is_err());
    assert!(Pattern::try_new(JsonValue::Integer(4)).is_ok());
    assert!(
        Envelope { value: Field::Null }
            .into_json()
            .to_json_string()
            .is_err()
    );
    assert!(
        Envelope {
            value: Field::Default(JsonValue::Integer(8))
        }
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert_eq!(
        Envelope {
            value: Field::Missing
        }
        .into_json()
        .to_json_string()
        .unwrap(),
        "{}"
    );
    assert_eq!(
        Envelope {
            value: Field::Value(Choice::try_new(JsonValue::Integer(7)).unwrap())
        }
        .into_json()
        .to_json_string()
        .unwrap(),
        r#"{"value":7}"#
    );
    assert!(NestedValue::try_new(JsonValue::Object(vec![])).is_err());
    assert!(NestedValue::try_new(JsonValue::Bool(true)).is_ok());
    let nested = Nested {
        value: NestedValue::try_new(JsonValue::Object(vec![("n".into(), JsonValue::Integer(1))]))
            .unwrap(),
    };
    assert_eq!(
        nested.into_json().to_json_string().unwrap(),
        r#"{"value":{"n":1}}"#
    );
    let annotation: Annotation = JsonValue::Array(vec![JsonValue::Bool(true)]);
    assert_eq!(annotation.to_json_string().unwrap(), "[true]");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for expected in [r#""scalar""#, r#"{"name":"ok"}"#] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, "POST /untyped HTTP/1.1\r\n");
            let mut length = None;
            let mut content_type = None;
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
                    content_type = Some(value.trim().to_string());
                }
            }
            assert_eq!(content_type.as_deref(), Some("application/json"));
            let mut received = vec![0; length.unwrap()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(std::str::from_utf8(&received).unwrap(), expected);
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
    for value in [
        JsonValue::String("scalar".into()),
        JsonValue::Object(vec![("name".into(), JsonValue::String("ok".into()))]),
    ] {
        let mut future = std::pin::pin!(client.send_untyped(SendUntypedArgs {
            body: Loose::try_new(value).unwrap()
        }));
        let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("fixture yielded")
        };
        assert_eq!(result.unwrap().status(), 204);
    }
    server.join().unwrap();
    println!(
        "untyped schema constraints preserve all JSON kinds and reject invalid objects on real SDK paths"
    );
}
