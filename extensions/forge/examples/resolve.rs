//! Compile one document for comparison with the running Worker.
use incurs_forge::{ResolveOptions, contract_digest, resolve_document};
use serde_json::{Value, json};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: resolve OPENAPI_JSON")?;
    let document: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let contract = resolve_document(&document, ResolveOptions::new("worker-proof"))?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "digest": contract_digest(&contract)?, "contract": contract
        }))?
    );
    Ok(())
}
