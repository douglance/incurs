//! A real generated-client consumer against an isolated loopback HTTP fixture.
use incurs_forge::runtime::{ForgeHttpRequest, ForgeHttpResponse, ForgeTransport};
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
    binding: incurs_forge::runtime::HttpBinding,
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
            let response = self
                .binding
                .invoke(
                    request.operation_id,
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

fn all_parts(response: AllStatusesResponse) -> (&'static str, OperationResponse) {
    match response {
        AllStatusesResponse::Status1XX(raw) => ("1XX", raw),
        AllStatusesResponse::Status2XX(raw) => ("2XX", raw),
        AllStatusesResponse::Status3XX(raw) => ("3XX", raw),
        AllStatusesResponse::Status4XX(raw) => ("4XX", raw),
        AllStatusesResponse::Status5XX(raw) => ("5XX", raw),
        AllStatusesResponse::Status201(raw) => ("201", raw),
        AllStatusesResponse::Status404(raw) => ("404", raw),
        AllStatusesResponse::Default(raw) => ("default", raw),
        AllStatusesResponse::Other(raw) => ("other", raw),
    }
}

fn sparse_parts(response: SparseStatusesResponse) -> (&'static str, OperationResponse) {
    match response {
        SparseStatusesResponse::Status2XX(raw) => ("2XX", raw),
        SparseStatusesResponse::Status4XX(raw) => ("4XX", raw),
        SparseStatusesResponse::Status418(raw) => ("418", raw),
        SparseStatusesResponse::Other(raw) => ("other", raw),
    }
}

fn fallback_parts(response: FallbackStatusesResponse) -> (&'static str, OperationResponse) {
    match response {
        FallbackStatusesResponse::Status2XX(raw) => ("2XX", raw),
        FallbackStatusesResponse::Default(raw) => ("default", raw),
        FallbackStatusesResponse::Other(raw) => ("other", raw),
    }
}

fn response(status: u16) -> OperationResponse {
    OperationResponse {
        status,
        headers: vec![
            ("set-cookie".into(), "a=1".into()),
            ("set-cookie".into(), "b=2".into()),
        ],
        body: Field::Value(JsonValue::Bytes(vec![0, 255, 1])),
    }
}

fn main() {
    for status in 0..=u16::MAX {
        let raw = response(status);
        let family = match status / 100 {
            1 => "1XX",
            2 => "2XX",
            3 => "3XX",
            4 => "4XX",
            5 => "5XX",
            _ => "default",
        };
        let expected = match status {
            201 => "201",
            404 => "404",
            _ => family,
        };
        let classified = AllStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = all_parts(classified);
        assert_eq!(label, expected, "all families: status {status}");
        assert_eq!(retained, raw);

        let expected = if status == 418 {
            "418"
        } else {
            match status / 100 {
                2 => "2XX",
                4 => "4XX",
                _ => "other",
            }
        };
        let classified = SparseStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = sparse_parts(classified);
        assert_eq!(label, expected, "sparse families: status {status}");
        assert_eq!(retained, raw);

        let expected = if status / 100 == 2 { "2XX" } else { "default" };
        let classified = FallbackStatusesResponse::from_response(raw.clone());
        assert_eq!(classified.status(), status);
        let (label, retained) = fallback_parts(classified);
        assert_eq!(label, expected, "family with default: status {status}");
        assert_eq!(retained, raw);
    }
    let cases = [
        ("/all", 201, "201"),
        ("/all", 202, "2XX"),
        ("/all", 404, "404"),
        ("/all", 418, "4XX"),
        ("/all", 503, "5XX"),
        ("/sparse", 201, "2XX"),
        ("/sparse", 418, "418"),
        ("/sparse", 503, "other"),
        ("/fallback", 503, "default"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for (path, status, _) in cases {
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
            assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
            }
            write!(stream, "HTTP/1.1 {status} Proof\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\nConnection: close\r\n\r\n").unwrap();
            stream.write_all(&[0, 255, 1]).unwrap();
        }
    });
    let contract = serde_json::from_str(include_str!("../../artifacts/contract.json")).unwrap();
    let client = Client::new(Bridge {
        binding: incurs_forge::runtime::HttpBinding::new(&contract).unwrap(),
        exchange: Exchange { address },
    });
    for (path, status, expected) in cases {
        let (label, raw) = match path {
            "/all" => all_parts(ready(client.all_statuses(AllStatusesArgs {})).unwrap()),
            "/sparse" => {
                sparse_parts(ready(client.sparse_statuses(SparseStatusesArgs {})).unwrap())
            }
            "/fallback" => {
                fallback_parts(ready(client.fallback_statuses(FallbackStatusesArgs {})).unwrap())
            }
            _ => unreachable!(),
        };
        assert_eq!(label, expected, "HTTP status {status}");
        assert_eq!(raw.status, status);
        assert_eq!(raw.body, Field::Value(JsonValue::Bytes(vec![0, 255, 1])));
        let cookies: Vec<_> = raw
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(cookies, ["a=1", "b=2"]);
    }
    server.join().unwrap();
    println!(
        "RESPONSE_STATUS_ARCHIVE_HTTP_PASSED: 196608 exhaustive classifications and nine real HTTP responses; exact > family > fallback; raw body and duplicate headers preserved"
    );
}
