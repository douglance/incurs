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

// Type-check that the false schema has no inhabitant.
#[allow(dead_code)]
fn impossible(value: Nothing) -> ! {
    match value {}
}

fn main() {
    let label: Label = "雪".to_string();
    let id: Identifier = i64::MAX;
    let enabled: Enabled = true;
    let amount: Ratio = 1.25;
    let ids: Identifiers = vec![9_007_199_254_740_993];
    let anything: Anything = JsonValue::Object(vec![(
        "valid".into(),
        JsonValue::Array(vec![
            JsonValue::Bool(true),
            JsonValue::Null,
            3_i64.into_json(),
        ]),
    )]);
    let nullable: NullableLabel = None;
    assert_eq!(nullable.into_json().to_json_string().unwrap(), "null");
    let nullable: NullableLabel = Some("value".into());
    assert_eq!(nullable.into_json().to_json_string().unwrap(), "\"value\"");
    let null: OnlyNull = ();
    assert_eq!(null.into_json().to_json_string().unwrap(), "null");
    let Field::Default(default) = Payload::id_default() else {
        panic!("default presence lost")
    };
    assert_eq!(default.to_json_string().unwrap(), "9223372036854775807");
    let Field::Default(default) = Payload::ceiling_default() else {
        panic!("default presence lost")
    };
    assert_eq!(default.to_json_string().unwrap(), "18446744073709551615");
    assert!(JsonValue::Bytes(vec![0]).to_json_string().is_err());
    assert!(JsonValue::Number(f64::NAN).to_json_string().is_err());
    let body = Payload {
        id,
        minimum: i64::MIN,
        label,
        enabled,
        amount,
        ids,
        anything,
        ceiling: Field::Missing,
    };
    let metadata = ReferenceMetadata {
        nullable: Field::Null,
        fallback: ReferenceMetadata::fallback_default(),
    };
    assert_eq!(
        metadata.into_json().to_json_string().unwrap(),
        "{\"fallback\":\"fallback\",\"nullable\":null}"
    );
    let graph = GraphNodeA {
        name: "root".into(),
        next: Field::Value(Box::new(GraphNodeB {
            previous: Field::Missing,
        })),
    };
    assert_eq!(
        graph.into_json().to_json_string().unwrap(),
        "{\"name\":\"root\",\"next\":{}}"
    );
    let request = CheckMetadataArgs {
        tag: CheckMetadataArgs::tag_default(),
    }
    .into_request();
    assert!(
        request
            .arguments_json()
            .unwrap()
            .contains("\"tag\":\"fallback\"")
    );
    let request = SendAliasTextArgs {
        body: "text".into(),
    }
    .into_request();
    assert!(
        request
            .arguments_json()
            .unwrap()
            .contains("\"body\":\"text\"")
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "POST /schema HTTP/1.1\r\n");
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
        assert_eq!(
            std::str::from_utf8(&received).unwrap(),
            "{\"amount\":1.25,\"anything\":{\"valid\":[true,null,3]},\"enabled\":true,\"id\":9223372036854775807,\"ids\":[9007199254740993],\"label\":\"雪\",\"minimum\":-9223372036854775808}"
        );
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    let mut future = std::pin::pin!(client.send_schema(SendSchemaArgs { body }));
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("fixture yielded")
    };
    assert_eq!(result.unwrap().status(), 204);
    server.join().unwrap();
    println!(
        "generated scalar SDK -> HTTP: exact signed integer boundaries, defaults and schema aliases passed"
    );
}
