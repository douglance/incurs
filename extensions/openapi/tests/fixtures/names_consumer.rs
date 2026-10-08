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
    let body = NamePayload {
        choices: vec![
            sdk::WireChoice::Value2,
            sdk::WireChoice::Value1000Errors,
            sdk::WireChoice::Value500Errors,
            sdk::WireChoice::ValueSelf,
            sdk::WireChoice::Value22,
            sdk::WireChoice::FooBar,
            sdk::WireChoice::FooBar3,
            sdk::WireChoice::FooBar2,
            sdk::WireChoice::Generated,
            sdk::WireChoice::Generated2,
        ],
        _1: 1,
        _1_3: 2,
        _1_2: 3,
        foo_bar: 4,
        foo_bar_3: 5,
        foo_bar_2: 6,
        type_: 7,
        type__2: 8,
        value: 9,
        value_2: 10,
        value_3: 11,
    };
    let Field::Default(default) = NamePayload::_1_3_default() else {
        panic!("default presence lost")
    };
    assert_eq!(default.to_json_string().unwrap(), "2");
    assert_eq!(
        Simple {
            count: 1,
            label: "unchanged".into()
        }
        .into_json()
        .to_json_string()
        .unwrap(),
        r#"{"count":1,"label":"unchanged"}"#
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
        let words: Vec<_> = line.trim_end().split(' ').collect();
        assert_eq!(words[0], "POST");
        assert_eq!(words[2], "HTTP/1.1");
        let (path, query) = words[1].split_once('?').unwrap();
        assert_eq!(path, "/names/7");
        let mut pairs: Vec<_> = query.split('&').collect();
        pairs.sort_unstable();
        assert_eq!(pairs, ["%2B1=11", "-1=12", "body=9", "body_2=10", "id=8"]);
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
            r#"{"+1":1,"-1":2,"_1_2":3,"choices":["2","1000_errors","500_errors","self","value2","foo-bar","foo_bar","foo_bar_2","","♥"],"foo-bar":4,"foo_bar":5,"foo_bar_2":6,"type":7,"type_":8,"value":9,"value_2":10,"♥":11}"#
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
    let args = SendNamesArgs {
        id: 7,
        id_2: 8,
        body_3: 9,
        body_2: 10,
        _1: 11,
        _1_2: SendNamesArgs::_1_2_default(),
        body,
    };
    let mut future = std::pin::pin!(client.send_names(args));
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("fixture yielded")
    };
    assert_eq!(result.unwrap().status(), 204);
    server.join().unwrap();
    println!("generated SDK distinct Rust fields preserve literal JSON and parameter wire names");
}
