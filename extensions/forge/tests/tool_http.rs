//! Continuous OpenAPI -> automatically compiled incurs ToolCatalog -> real HTTP proof.
#![cfg(all(feature = "adapters", not(target_arch = "wasm32")))]

use incurs::{
    outbound::{HttpClient, HttpClientError, HttpRequest, HttpResponse},
    tool::{ToolCallOptions, ToolCallOutcome},
};
use incurs_forge::{
    ResolveOptions, adapters::IncursHttpTransport, catalog::compile_cli, resolve_document,
    servers::ServerSelection,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

// A bounded HTTP/1.1 transport for the loopback fixture, not a production client.
struct SocketClient {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl HttpClient for SocketClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let url = url::Url::parse(&request.url).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let mut socket = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let target = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        write!(
            socket,
            "{} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n",
            request.method
        )
        .unwrap();
        for (name, value) in request.headers {
            write!(socket, "{name}: {value}\r\n").unwrap();
        }
        let body = request.body.unwrap_or_default();
        write!(socket, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        socket.write_all(&body).unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            assert!(!line.is_empty(), "response ended before headers");
            let (name, value) = line.trim_end().split_once(':').unwrap();
            headers.push((name.to_owned(), value.trim().to_owned()));
        }
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        Ok(HttpResponse::from_bytes(status, headers, bytes))
    }
}

#[tokio::test]
async fn openapi_operation_runs_through_incurs_and_real_http() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, received) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "no HTTP request reached the server"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let mut headers = BTreeMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            assert!(!line.is_empty(), "request ended before headers");
            let (name, value) = line.trim_end().split_once(':').unwrap();
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let mut body = vec![0; headers["content-length"].parse().unwrap()];
        reader.read_exact(&mut body).unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        // Independent oracle: literal wire expectations, not binder output.
        assert_eq!(
            first,
            "POST /v1/widgets/a%2Fb%20c?dry_run=true HTTP/1.1\r\n"
        );
        assert_eq!(headers["x-trace"], "trace-123");
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(body, json!({"name":"Updated widget", "note":null}));
        let reply = serde_json::to_vec(&json!({
            "id":"a/b c", "name":body["name"], "note":body["note"],
            "receipt":format!("loopback-{}", address.port())
        }))
        .unwrap();
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Request-Id: proof-receipt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).unwrap();
        socket.write_all(&reply).unwrap();
        sent.send((first, body)).unwrap();
    });

    let mut document: Value = serde_json::from_str(include_str!(
        "../../forge-workers/fixtures/proof-openapi.json"
    ))
    .unwrap();
    document["paths"]["/widgets/{id}"]["post"]["servers"] =
        json!([{"url":format!("http://{address}/v1")}]);
    let schema = std::mem::replace(
        &mut document["paths"]["/widgets/{id}"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"],
        json!({"$ref":"#/components/schemas/UpdatePayload"}),
    );
    document["components"]["schemas"]["UpdatePayload"] = schema;
    let contract = resolve_document(&document, ResolveOptions::new("continuous-proof")).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let transport = IncursHttpTransport::new(Arc::new(SocketClient {
        calls: calls.clone(),
    }));
    let cli = compile_cli(&contract, transport, &ServerSelection::default()).unwrap();
    let catalog = cli.tool_catalog();
    assert!(catalog.get("op_updateWidget").is_some());
    assert!(catalog.get("op_listWidgets").is_some());
    assert_eq!(catalog.definitions().len(), 2);
    let schema = &catalog.get("op_updateWidget").unwrap().input_schema;
    assert_eq!(schema["properties"]["path"]["required"], json!(["id"]));
    assert_eq!(
        schema["properties"]["path"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        schema["properties"]["query"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    assert_eq!(
        schema["properties"]["body"]["$ref"],
        "#/$defs/UpdatePayload"
    );
    assert_eq!(
        schema["$defs"]["UpdatePayload"]["properties"]["note"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(schema["$defs"].as_object().unwrap().len(), 1);
    let invalid = catalog
        .call(
            "op_updateWidget",
            BTreeMap::from([
                ("path".into(), json!({})),
                ("body".into(), json!({"name":"Invalid"})),
            ]),
            ToolCallOptions::isolated(),
        )
        .await;
    match invalid {
        ToolCallOutcome::Error { code, message, .. } => {
            assert_eq!(code, "FORGE_BINDING");
            assert!(message.contains("missing path parameter: id"), "{message}");
            println!("missing id -> FORGE_BINDING; outbound calls = 0");
        }
        other => panic!("invalid input unexpectedly succeeded: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let arguments = json!({
        "path":{"id":"a/b c"},
        "query":{"dry_run":true},
        "header":{"X-Trace":"trace-123"},
        "body":{"name":"Updated widget", "note":null}
    });
    let outcome = catalog
        .call(
            "op_updateWidget",
            arguments.as_object().unwrap().clone().into_iter().collect(),
            ToolCallOptions::isolated(),
        )
        .await;
    let ToolCallOutcome::Ok { data, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(data["status"], 200);
    let bytes: Vec<u8> = serde_json::from_value(data["body"].clone()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({
            "id":"a/b c", "name":"Updated widget", "note":null,
            "receipt":format!("loopback-{}", address.port())
        })
    );
    assert!(
        data["headers"]
            .as_array()
            .unwrap()
            .contains(&json!(["X-Request-Id", "proof-receipt"]))
    );
    let (request, body) = received.recv_timeout(Duration::from_secs(3)).unwrap();
    server.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    println!("discovered tools: op_listWidgets, op_updateWidget");
    println!("server observed: {}", request.trim_end());
    println!("server received body: {body}");
    println!("incurs returned: {data}");
}
