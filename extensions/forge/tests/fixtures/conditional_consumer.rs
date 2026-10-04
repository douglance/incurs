//! Constraint-only composition consumer based on a published schema failure.
use incurs_forge::{
    ResolvedOpenApi,
    runtime::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport, invoke_http},
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

fn fields(items: Vec<(&str, JsonValue)>) -> JsonValue {
    JsonValue::Object(
        items
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}
fn main() {
    let valid = Conditional::Value(fields(vec![
        ("left", 7_i64.into_json()),
        ("name", "proof".to_owned().into_json()),
    ]));
    assert_eq!(
        valid.clone().into_json().to_json_string().unwrap(),
        "{\"left\":7,\"name\":\"proof\"}"
    );
    assert_eq!(
        Conditional::Value2(fields(vec![
            ("name", "proof".to_owned().into_json()),
            ("right", "ok".to_owned().into_json())
        ]))
        .into_json()
        .to_json_string()
        .unwrap(),
        "{\"name\":\"proof\",\"right\":\"ok\"}"
    );
    for payload in [
        fields(vec![("name", "proof".to_owned().into_json())]),
        fields(vec![
            ("name", "proof".to_owned().into_json()),
            ("left", 7_i64.into_json()),
            ("right", "ok".to_owned().into_json()),
        ]),
        fields(vec![
            ("name", 1_i64.into_json()),
            ("left", 7_i64.into_json()),
        ]),
        fields(vec![
            ("name", "proof".to_owned().into_json()),
            ("left", "wrong".to_owned().into_json()),
        ]),
        fields(vec![
            ("name", "proof".to_owned().into_json()),
            ("left", 7_i64.into_json()),
            ("extra", true.into_json()),
        ]),
        fields(vec![
            ("name", String::new().into_json()),
            ("left", 7_i64.into_json()),
        ]),
        "not an object".to_owned().into_json(),
    ] {
        assert!(
            Conditional::Value(payload)
                .into_json()
                .to_json_string()
                .is_err()
        );
    }
    assert!(
        Conditional::Value2(fields(vec![
            ("name", "proof".to_owned().into_json()),
            ("left", 7_i64.into_json())
        ]))
        .into_json()
        .to_json_string()
        .is_err()
    );
    assert_eq!(
        UntypedRequired::Value("not an object".to_owned().into_json())
            .into_json()
            .to_json_string()
            .unwrap(),
        "\"not an object\""
    );
    assert!(
        UntypedRequired::Value(fields(vec![]))
            .into_json()
            .to_json_string()
            .is_err()
    );
    assert_eq!(
        UntypedRequired::Value(fields(vec![("left", 7_i64.into_json())]))
            .into_json()
            .to_json_string()
            .unwrap(),
        "{\"left\":7}"
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
        assert_eq!(line, "POST /conditional HTTP/1.1\r\n");
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
                content_type = Some(value.trim().to_owned());
            }
        }
        assert_eq!(content_type.as_deref(), Some("application/json"));
        let mut body = vec![0; length.unwrap()];
        reader.read_exact(&mut body).unwrap();
        assert_eq!(body, b"{\"left\":7,\"name\":\"proof\"}");
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    let mut future = std::pin::pin!(client.send_conditional(SendConditionalArgs { body: valid }));
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("fixture yielded")
    };
    assert_eq!(result.unwrap().status(), 204);
    server.join().unwrap();
    println!(
        "constraint-only oneOf branches preserve parent restrictions and reach exact HTTP bytes"
    );
}
