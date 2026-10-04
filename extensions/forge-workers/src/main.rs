//! Isolated Emscripten runtime proof; never changes the production Workers target.
use serde::Deserialize;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use worker::{Context, Env, Error, Request, Response, Result, event};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn main() {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    token: String,
    delay_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindProbe {
    document: serde_json::Value,
    operation: String,
    arguments: serde_json::Value,
    #[serde(default)]
    server_index: usize,
    #[serde(default)]
    server_variables: std::collections::BTreeMap<String, String>,
    document_url: Option<String>,
}

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn rust_error(error: impl std::fmt::Display) -> Error {
    Error::RustError(error.to_string())
}

#[event(fetch)]
async fn fetch(mut request: Request, _env: Env, _context: Context) -> Result<Response> {
    match (request.method(), request.path().as_str()) {
        (worker::Method::Get, "/health") => Response::from_json(&json!({
            "ok": true, "target": std::env::consts::OS, "durable_files": false
        })),
        (worker::Method::Post, "/resolve") => {
            let bytes = request.bytes().await?;
            if bytes.len() > 131_072 {
                return Response::error("document exceeds 128 KiB", 413);
            }
            let document: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(document) => document,
                Err(_) => return Response::error("invalid JSON document", 400),
            };
            let contract = match incurs_forge::resolve_document(
                &document,
                incurs_forge::ResolveOptions::new("worker-proof"),
            ) {
                Ok(contract) => contract,
                Err(error) => return Response::error(error.to_string(), 400),
            };
            let digest = incurs_forge::contract_digest(&contract).map_err(rust_error)?;
            Response::from_json(&json!({ "contract": contract, "digest": digest }))
        }
        (worker::Method::Post, "/bind") => {
            let bytes = request.bytes().await?;
            if bytes.len() > 131_072 {
                return Response::error("binding input exceeds 128 KiB", 413);
            }
            let input: BindProbe = match serde_json::from_slice(&bytes) {
                Ok(input) => input,
                Err(_) => return Response::error("invalid binding input", 400),
            };
            let contract = match incurs_forge::resolve_document(
                &input.document,
                incurs_forge::ResolveOptions::new("worker-proof"),
            ) {
                Ok(contract) => contract,
                Err(error) => return Response::error(error.to_string(), 400),
            };
            let Some(operation) = contract
                .operations
                .iter()
                .find(|op| op.name == input.operation)
            else {
                return Response::error("unknown operation", 400);
            };
            let selection = incurs_forge::servers::ServerSelection {
                index: input.server_index,
                variables: input.server_variables,
                document_url: input.document_url,
            };
            let server_url = match incurs_forge::servers::select_server_url(operation, &selection) {
                Ok(server_url) => server_url,
                Err(error) => return Response::error(error.to_string(), 400),
            };
            let binding = match incurs_forge::runtime::HttpBinding::new(&contract) {
                Ok(binding) => binding,
                Err(error) => return Response::error(error.to_string(), 400),
            };
            match binding.build_request(&operation.id, &server_url, &input.arguments) {
                Ok(bound) => Response::from_json(&json!({
                    "method":bound.method, "url":bound.url, "headers":bound.headers, "body":bound.body
                })),
                Err(error) => Response::error(error.to_string(), 400),
            }
        }
        (worker::Method::Post, "/prove") => {
            let probe: Probe = match request.json().await {
                Ok(probe) => probe,
                Err(_) => return Response::error("invalid probe", 400),
            };
            if probe.token.len() > 256 || probe.delay_ms > 1000 {
                return Response::error("probe exceeds limits", 400);
            }
            let start = Instant::now();
            let path = std::env::temp_dir().join(format!(
                "incurs-forge-probe-{}",
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = TemporaryFile(path);
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary.0)
                .map_err(rust_error)?;
            file.write_all(probe.token.as_bytes()).map_err(rust_error)?;
            drop(file);
            let first = tokio::spawn(async { 2_u32 });
            let second = tokio::spawn(async { 3_u32 });
            tokio::time::sleep(Duration::from_millis(probe.delay_ms)).await;
            let task_sum = first.await.map_err(rust_error)? + second.await.map_err(rust_error)?;
            let content = std::fs::read_to_string(&temporary.0).map_err(rust_error)?;
            let file_roundtrip = content == probe.token;
            std::fs::remove_file(&temporary.0).map_err(rust_error)?;
            Response::from_json(&json!({
                "token": content,
                "file_roundtrip": file_roundtrip,
                "file_removed": !temporary.0.exists(),
                "task_sum": task_sum,
                "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0,
                "target": std::env::consts::OS,
            }))
        }
        _ => Response::error("not found", 404),
    }
}
