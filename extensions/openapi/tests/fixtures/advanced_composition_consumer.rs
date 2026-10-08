//! A real generated-client consumer for oneOf/allOf SDK types.
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
    assert_eq!(
        AnyNumeric::Integer(3).into_json().to_json_string().unwrap(),
        "3"
    );
    assert_eq!(
        AnyNumeric::Number(3.0)
            .into_json()
            .to_json_string()
            .unwrap(),
        "3"
    );
    assert_eq!(
        Bounded::try_new(JsonValue::Integer(4))
            .unwrap()
            .into_json()
            .to_json_string()
            .unwrap(),
        "4"
    );
    assert!(Bounded::try_new(JsonValue::Integer(0)).is_err());
    assert!(Bounded::try_new(JsonValue::Integer(6)).is_err());
    assert!(Bounded::try_new(JsonValue::String("4".into())).is_err());
    assert!(PatternIntersection::try_new(JsonValue::String("AB".into())).is_ok());
    assert!(PatternIntersection::try_new(JsonValue::String("Ab".into())).is_err());
    assert!(PatternIntersection::try_new(JsonValue::String("ABCD".into())).is_err());
    for value in [JsonValue::Null, JsonValue::Object(vec![])] {
        assert!(Impossible::try_new(value).is_err());
    }
    for id in [JsonValue::Integer(1), JsonValue::String("one".into())] {
        assert!(Conflict::try_new(JsonValue::Object(vec![("id".into(), id)])).is_err());
    }
    assert_eq!(
        PatternChoice::String("AB".into())
            .into_json()
            .to_json_string()
            .unwrap(),
        "\"AB\""
    );
    assert!(
        PatternChoice::String("ab".into())
            .into_json()
            .to_json_string()
            .is_err()
    );
    assert_eq!(
        PatternChoice::Integer(6)
            .into_json()
            .to_json_string()
            .unwrap(),
        "6"
    );
    assert!(
        PatternChoice::Integer(7)
            .into_json()
            .to_json_string()
            .is_err()
    );
    let tree = RecursiveAll {
        value: 1,
        next: Field::Value(Box::new(RecursiveAll {
            value: 2,
            next: Field::Missing,
        })),
    };
    assert_eq!(
        tree.into_json().to_json_string().unwrap(),
        "{\"next\":{\"value\":2},\"value\":1}"
    );
    assert!(
        RecursiveAll {
            value: 1,
            next: Field::Default(JsonValue::String("bad".into()))
        }
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert!(
        Conditional {
            enabled: Field::Value(true),
            value: Field::Missing
        }
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert!(
        Conditional {
            enabled: Field::Value(true),
            value: Field::Value(2)
        }
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert!(
        Conditional {
            enabled: Field::Value(true),
            value: Field::Value(3)
        }
        .into_json()
        .to_json_string()
        .is_ok()
    );
    assert!(
        TupleIntersection::try_new(JsonValue::Array(vec![
            JsonValue::Integer(1),
            JsonValue::String("two".into())
        ]))
        .is_ok()
    );
    assert!(
        TupleIntersection::try_new(JsonValue::Array(vec![
            JsonValue::String("one".into()),
            JsonValue::Integer(2)
        ]))
        .is_err()
    );
    assert!(ExactUnsigned::try_new(JsonValue::Unsigned(u64::MAX)).is_ok());
    assert!(ExactUnsigned::try_new(JsonValue::Unsigned(u64::MAX - 1)).is_err());
    assert!(Decimal::try_new(JsonValue::Number(0.3)).is_ok());
    assert!(Decimal::try_new(JsonValue::Number(0.31)).is_err());
    assert!(
        OpenMap::Value(JsonValue::Object(vec![
            ("id".into(), JsonValue::Integer(1)),
            ("id".into(), JsonValue::Integer(2))
        ]))
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert!(
        OpenMap::Value(JsonValue::Object(vec![(
            "id".into(),
            JsonValue::Bytes(vec![])
        )]))
        .into_json()
        .to_json_string()
        .is_err()
    );

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "POST /compose HTTP/1.1\r\n");
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
        let mut body = vec![0; length.unwrap()];
        reader.read_exact(&mut body).unwrap();
        assert_eq!(body, b"4");
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    let response = block_on(client.submit(SubmitArgs {
        body: Bounded::try_new(JsonValue::Integer(4)).unwrap(),
    }))
    .unwrap();
    assert_eq!(response.status(), 204);
    server.join().unwrap();
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture yielded"),
    }
}
