//! Generated OpenAPI 3.2 consumer with independent literal HTTP expectations.
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

fn complete<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("loopback transport unexpectedly yielded"),
    }
}
fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for (line_expected, cookie, meta) in [
            (
                "QUERY /find?labels=red&labels=blue&term=a+%2B+b%2F%C3%A9 HTTP/1.1\r\n",
                Some("mode=dark; token=a%2Fb"),
                None,
            ),
            (
                "x-Copy /copy/a%2Fb HTTP/1.1\r\n",
                None,
                Some("{\"id\":9007199254740993}"),
            ),
            ("GET /raw? HTTP/1.1\r\n", None, None),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, line_expected);
            let mut headers = std::collections::BTreeMap::new();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
                let (name, value) = line.trim_end().split_once(':').unwrap();
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
            assert_eq!(headers.get("cookie").map(String::as_str), cookie);
            assert_eq!(headers.get("x-meta").map(String::as_str), meta);
            assert_eq!(headers.get("content-length").map(String::as_str), Some("0"));
            assert!(!headers.contains_key("content-type"));
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
    let filter = JsonValue::Object(vec![
        ("term".into(), JsonValue::String("a + b/é".into())),
        (
            "labels".into(),
            vec!["red".to_owned(), "blue".to_owned()].into_json(),
        ),
    ]);
    let prefs = JsonValue::Object(vec![
        ("mode".into(), JsonValue::String("dark".into())),
        ("token".into(), JsonValue::String("a%2Fb".into())),
    ]);
    assert_eq!(
        complete(client.find_items(FindItemsArgs {
            filter,
            prefs: Field::Value(prefs)
        }))
        .unwrap()
        .status(),
        204
    );
    let x_meta = JsonValue::Object(vec![(
        "id".into(),
        JsonValue::Unsigned(9_007_199_254_740_993),
    )]);
    assert_eq!(
        complete(client.copy_item(CopyItemArgs {
            id: "a/b".into(),
            x_meta
        }))
        .unwrap()
        .status(),
        204
    );
    assert_eq!(
        complete(client.read_raw(ReadRawArgs {
            raw: Field::Value(String::new())
        }))
        .unwrap()
        .status(),
        204
    );
    server.join().unwrap();
    println!(
        "generated OpenAPI 3.2 SDK -> HTTP: QUERY, custom method case, whole query and cookie bytes passed"
    );
}
