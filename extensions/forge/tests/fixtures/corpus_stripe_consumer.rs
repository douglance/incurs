//! Loopback proof for the complete pinned Stripe SDK; no Stripe API calls.
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

fn main() {
    let contract_path = std::env::args_os()
        .nth(1)
        .expect("pass the generated contract.json path");
    let contract: ResolvedOpenApi =
        serde_json::from_slice(&std::fs::read(contract_path).unwrap()).unwrap();
    let operation = contract
        .operations
        .iter()
        .find(|operation| operation.name == "GetBalance")
        .unwrap();
    let unsupported = incurs_forge::runtime::build_http_request(
        operation,
        &contract.schemas,
        "http://127.0.0.1:1",
        &serde_json::json!({"query":{"expand":["available"]}}),
    );
    assert!(
        unsupported.is_err(),
        "deepObject array was silently reinterpreted"
    );
    println!("DECLARED_EXPAND_LIMIT {}", unsupported.unwrap_err());
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
        assert_eq!(line, "GET /v1/balance HTTP/1.1\r\n");
        let mut length = None;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            assert!(!line.is_empty());
            let (name, value) = line.trim_end().split_once(':').unwrap();
            assert!(!name.eq_ignore_ascii_case("content-type"));
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        assert_eq!(length, Some(0));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let client = Client::new(Bridge {
        contract,
        exchange: Exchange { address },
    });
    let mut future = std::pin::pin!(client.get_balance(GetBalanceArgs {
        expand: Field::Missing,
        body: Field::Missing
    }));
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("fixture yielded")
    };
    let response = result.unwrap();
    assert_eq!(response.status(), 200);
    match response {
        GetBalanceResponse::Status200(value) => {
            assert_eq!(value.body, Field::Value(JsonValue::Bytes(b"{}".to_vec())))
        }
        _ => panic!("unexpected generated response variant"),
    }
    server.join().unwrap();
    println!("FULL_STRIPE_SDK_LOOPBACK_HTTP_PASSED GET /v1/balance; optional expand unsupported");
}
