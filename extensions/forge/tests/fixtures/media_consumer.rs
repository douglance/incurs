//! A real generated-client consumer against an isolated loopback HTTP fixture.
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
        let cases: Vec<(&str, Option<&str>, &[u8])> = vec![
            ("/binary", None, b""),
            ("/binary", Some("application/octet-stream"), b""),
            (
                "/binary",
                Some("application/octet-stream"),
                &[0, 255, 128, 13, 10],
            ),
            ("/markdown", Some("text/plain"), "snow 雪\n".as_bytes()),
            ("/markdown", Some("text/x-markdown"), "snow 雪\n".as_bytes()),
            (
                "/scripts",
                Some("application/javascript"),
                b"export default 1;",
            ),
            ("/records", Some("application/jsonl"), b"{\"id\":1}\nnull"),
            ("/ndjson", Some("application/x-ndjson"), b"{\"id\":1}\n"),
            (
                "/mixed",
                Some("application/octet-stream"),
                &[0, 255, 128, 13, 10],
            ),
            ("/mixed", Some("application/json"), b"null"),
            ("/mixed", Some("application/json"), b"null"),
            ("/config", Some("application/json"), br#"{"name":"x"}"#),
            ("/patch", Some("application/merge-patch+json"), b"null"),
            ("/patch", Some("application/json"), b"null"),
        ];
        for (path, expected_type, expected) in cases {
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
            assert_eq!(line, format!("POST {path} HTTP/1.1\r\n"));
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
            assert_eq!(length, Some(expected.len()));
            assert_eq!(content_type.as_deref(), expected_type);
            let mut received = vec![0; length.unwrap()];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, expected);
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
        UploadBinaryBody::Missing,
        UploadBinaryBody::ApplicationOctetStream(Vec::new()),
        UploadBinaryBody::ApplicationOctetStream(vec![0, 255, 128, 13, 10]),
    ] {
        assert_eq!(
            ready(client.upload_binary(UploadBinaryArgs { body }))
                .unwrap()
                .status(),
            204
        );
    }
    for body in [
        SendMarkdownBody::TextPlain(Field::Value("snow 雪\n".into())),
        SendMarkdownBody::TextXMarkdown(Field::Value("snow 雪\n".into())),
    ] {
        assert_eq!(
            ready(client.send_markdown(SendMarkdownArgs { body }))
                .unwrap()
                .status(),
            204
        );
    }
    assert_eq!(
        ready(client.send_script(SendScriptArgs {
            body: SendScriptBody::ApplicationJavascript(Field::Value("export default 1;".into()))
        }))
        .unwrap()
        .status(),
        204
    );
    assert_eq!(
        ready(client.upload_records(UploadRecordsArgs {
            body: "{\"id\":1}\nnull".into()
        }))
        .unwrap()
        .status(),
        204
    );
    assert_eq!(
        ready(client.upload_ndjson(UploadNdjsonArgs {
            body: UploadNdjsonBody::ApplicationXNdjson(b"{\"id\":1}\n".to_vec())
        }))
        .unwrap()
        .status(),
        204
    );
    for body in [
        SendMixedBody::ApplicationOctetStream(vec![0, 255, 128, 13, 10]),
        SendMixedBody::ApplicationJson(Field::Null),
        SendMixedBody::ApplicationJson(Field::Default(JsonValue::Null)),
    ] {
        assert_eq!(
            ready(client.send_mixed(SendMixedArgs { body }))
                .unwrap()
                .status(),
            204
        );
    }
    assert_eq!(
        ready(client.send_config(SendConfigArgs {
            body: SendConfigBody::ApplicationJson(Field::Value(Config {
                name: Field::Value("x".into())
            }))
        }))
        .unwrap()
        .status(),
        204
    );
    for body in [
        SendPatchBody::ApplicationMergePatchJson(Field::Null),
        SendPatchBody::ApplicationJson(Field::Default(JsonValue::Null)),
    ] {
        assert_eq!(
            ready(client.send_patch(SendPatchArgs { body }))
                .unwrap()
                .status(),
            204
        );
    }
    server.join().unwrap();
    for body in [
        SendMixedBody::ApplicationXForbiddenJson(Field::Null),
        SendMixedBody::ApplicationXForbiddenJson(Field::Default(JsonValue::Null)),
    ] {
        assert!(
            SendMixedArgs { body }
                .into_request()
                .arguments_json()
                .is_err(),
            "a false media schema must reject explicit null and defaults before transport"
        );
    }

    let missing = SendMixedArgs {
        body: SendMixedBody::ApplicationJson(Field::Missing),
    }
    .into_request();
    assert!(
        missing.arguments_json().is_err(),
        "selected media must not erase a required body"
    );
    let forbidden = SendConfigArgs {
        body: SendConfigBody::TextPlainCharsetUTF8(Field::Value(JsonValue::Object(vec![(
            "name".into(),
            JsonValue::String("x".into()),
        )]))),
    };
    assert!(
        ready(client.send_config(forbidden))
            .unwrap_err()
            .contains("text body schema")
    );
    let mut conflicting = UploadBinaryArgs {
        body: UploadBinaryBody::ApplicationOctetStream(vec![1]),
    }
    .into_request();
    conflicting.body = Field::Null;
    assert!(conflicting.arguments_json().is_err());
    println!(
        "generated media SDK -> HTTP: 14 literal wire cases, explicit media, binary bytes, framing, presence, and invalid-slot controls passed"
    );
}
