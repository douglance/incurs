//! Stage-specific audit of a published OpenAPI document; no API requests are sent.
use incurs_openapi::{
    ArtifactOptions, ResolveOptions, compile_artifacts, publish_artifacts, resolve_document,
};
use serde_json::{Value, json};
use std::{process::ExitCode, time::Instant};

struct ByteCount(usize);
impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn emit(report: &Value) {
    println!("{}", serde_json::to_string(report).unwrap());
}
fn fail(
    mut report: Value,
    stage: &str,
    error: impl std::fmt::Display,
    started: Instant,
) -> ExitCode {
    report[stage] = json!({"status":"failed","error":error.to_string(),"elapsed_ms":started.elapsed().as_millis()});
    emit(&report);
    ExitCode::FAILURE
}
fn main() -> ExitCode {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let resolve_only = args.last().is_some_and(|arg| arg == "--resolve-only");
    if resolve_only {
        args.pop();
    }
    if !(2..=3).contains(&args.len()) {
        eprintln!(
            "usage: audit DOCUMENT_JSON_OR_STDIN_DASH DECLARING_URI [NEW_OUTPUT_DIRECTORY] [--resolve-only]"
        );
        return ExitCode::from(2);
    }
    let mut report = json!({
        "scope":"published schema resolution and artifact compilation; no HTTP behavior proof",
        "parse":{"status":"not_run"},"resolution":{"status":"not_run"},
        "artifact_compilation":{"status":"not_run"},"publication":{"status":if args.len()==3 {"not_run"} else {"not_requested"}}
    });
    let started = Instant::now();
    let bytes = if args[0] == "-" {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes).map(|_| bytes)
    } else {
        std::fs::read(&args[0])
    };
    let document: Value = match bytes
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
    {
        Ok(value) => value,
        Err(error) => return fail(report, "parse", error, started),
    };
    let source_operations = document
        .get("paths")
        .and_then(Value::as_object)
        .map(|paths| {
            paths
                .values()
                .map(|item| {
                    [
                        "get", "put", "post", "delete", "options", "head", "patch", "trace",
                        "query",
                    ]
                    .iter()
                    .filter(|method| item.get(**method).is_some())
                    .count()
                        + item
                            .get("additionalOperations")
                            .and_then(Value::as_object)
                            .map_or(0, serde_json::Map::len)
                })
                .sum::<usize>()
        })
        .unwrap_or(0);
    report["parse"] = json!({"status":"passed","openapi":document.get("openapi"),"path_operations":source_operations,"elapsed_ms":started.elapsed().as_millis()});
    report["resolution"] = json!({"status":"running"});
    emit(&report);
    let started = Instant::now();
    let mut options = ResolveOptions::new("corpus");
    options.document_uri = args[1].clone();
    let contract = match resolve_document(&document, options) {
        Ok(value) => value,
        Err(error) => return fail(report, "resolution", error, started),
    };
    let mut resolved_bytes = ByteCount(0);
    if let Err(error) = serde_json::to_writer(&mut resolved_bytes, &contract) {
        return fail(report, "resolution", error, started);
    }
    report["resolution"] = json!({"status":"passed","operations":contract.operations.len(),"schemas":contract.schemas.len(),"resolved_json_bytes":resolved_bytes.0,"elapsed_ms":started.elapsed().as_millis()});
    let mut schema_bytes = ByteCount(0);
    serde_json::to_writer(&mut schema_bytes, &contract.schemas).unwrap();
    let mut operation_bytes = ByteCount(0);
    serde_json::to_writer(&mut operation_bytes, &contract.operations).unwrap();
    let root_schemas = document
        .pointer("/components/schemas")
        .and_then(Value::as_object);
    report["resolution"]["root_component_schemas_equal_input"] =
        json!(root_schemas.is_none_or(|schemas| {
            schemas
                .iter()
                .all(|(name, source)| contract.schemas.get(name) == Some(source))
        }));
    report["resolution"]["schema_json_bytes"] = json!(schema_bytes.0);
    report["resolution"]["operation_json_bytes"] = json!(operation_bytes.0);
    if resolve_only {
        emit(&report);
        return ExitCode::SUCCESS;
    }
    report["artifact_compilation"] = json!({"status":"running"});
    emit(&report);
    let started = Instant::now();
    let artifacts = match compile_artifacts(&contract, ArtifactOptions::new("corpus-proof-sdk")) {
        Ok(value) => value,
        Err(error) => return fail(report, "artifact_compilation", error, started),
    };
    report["artifact_compilation"] =
        json!({"status":"passed","elapsed_ms":started.elapsed().as_millis()});
    if let Some(target) = args.get(2) {
        report["publication"] = json!({"status":"running"});
        emit(&report);
        let started = Instant::now();
        match publish_artifacts(&artifacts, target) {
            Ok(value) => {
                report["publication"] = json!({"status":"passed","files":value.files.len(),"elapsed_ms":started.elapsed().as_millis()})
            }
            Err(error) => return fail(report, "publication", error, started),
        }
    }
    emit(&report);
    ExitCode::SUCCESS
}
