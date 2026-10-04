//! Resolve an OpenAPI document and publish a new Rust SDK artifact directory.
use incurs_forge::{
    ArtifactOptions, ResolveOptions, compile_artifacts, publish_artifacts, resolve_document,
};
use serde_json::Value;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = args
        .next()
        .ok_or("usage: compile OPENAPI_JSON NEW_OUTPUT_DIRECTORY")?;
    let output = args
        .next()
        .ok_or("usage: compile OPENAPI_JSON NEW_OUTPUT_DIRECTORY")?;
    let crate_name = args
        .next()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "SDK crate name must be UTF-8")
        })
        .transpose()?
        .unwrap_or_else(|| "worker-proof-sdk".to_string());
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    let document: Value = serde_json::from_slice(&std::fs::read(source)?)?;
    let contract = resolve_document(&document, ResolveOptions::new("worker-proof"))?;
    let artifacts = compile_artifacts(&contract, ArtifactOptions::new(crate_name))?;
    let report = publish_artifacts(&artifacts, output)?;
    println!(
        "published {} files to {}",
        report.files.len(),
        report.target.display()
    );
    Ok(())
}
