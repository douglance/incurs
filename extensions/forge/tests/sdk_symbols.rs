//! Generated SDK symbol collision tests.
#![cfg(not(target_arch = "wasm32"))]

use incurs_forge::{
    ArtifactOptions, ResolveOptions, compile_artifacts, publish_artifacts, resolve_document,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sdk_symbol_table_keeps_render_names_unique_without_rewriting_contract() {
    let document: Value =
        serde_json::from_str(include_str!("fixtures/sdk_symbols_openapi.json")).unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("symbol-proof")).unwrap();

    assert!(contract.schemas.contains_key("Load-BalancingOriginHealthy"));
    assert!(contract.schemas.contains_key("Load_BalancingOriginHealthy"));
    assert_eq!(
        contract.schemas["CreateThingBody"]["properties"]["origin"]["$ref"],
        "#/components/schemas/Load_BalancingOriginHealthy"
    );

    let artifacts = compile_artifacts(&contract, ArtifactOptions::new("symbol-proof-sdk")).unwrap();
    let sdk = String::from_utf8(artifacts.get("sdk/src/lib.rs").unwrap().bytes.clone()).unwrap();

    assert_no_duplicate_public_types(&sdk);
    assert!(sdk.contains("pub struct LoadBalancingOriginHealthy"));
    assert!(sdk.contains("pub struct LoadBalancingOriginHealthy2"));
    assert!(sdk.contains("pub struct LoadBalancingOriginHealthy3"));
    assert!(sdk.contains("pub struct JsonValue2"));
    assert!(sdk.contains("pub struct Some2"));
    assert!(sdk.contains("pub struct None2"));
    assert!(sdk.contains("pub struct Ok2"));
    assert!(sdk.contains("pub struct Err2"));
    assert!(sdk.contains("pub struct String2"));
    assert!(sdk.contains("pub struct CreateThingArgs2"));
    assert!(sdk.contains("pub enum CreateThingBody2"));
    assert!(sdk.contains("pub enum CreateThingResponse2"));
    assert!(sdk.contains("pub child: Field<Box<RecursiveNode>>"));
    assert!(sdk.contains("pub async fn new_operation"));
    assert!(sdk.contains("pub async fn new_operation_2"));
    assert!(sdk.contains("pub async fn into_transport_operation"));
    assert!(sdk.contains("pub fn ref_default() -> Field<String>"));
    assert!(sdk.contains("pub fn ref_default_default() -> Field<String>"));
    assert!(sdk.contains("pub fn type_default() -> Field<i64>"));
    assert!(sdk.contains("pub fn type_default_default() -> Field<i64>"));
    assert!(sdk.contains("pub fn type_2_default() -> Field<i64>"));
    assert!(!sdk.contains("pub fn ref__default"));
    assert!(!sdk.contains("pub fn type__default"));

    package_and_run_consumer(&artifacts, include_str!("fixtures/sdk_symbols_consumer.rs"));
}

fn assert_no_duplicate_public_types(sdk: &str) {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for line in sdk.lines() {
        let trimmed = line.trim_start();
        for prefix in ["pub struct ", "pub enum ", "pub type ", "pub trait "] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let name = rest
                    .split(|character: char| {
                        character == ' '
                            || character == '<'
                            || character == '('
                            || character == '{'
                            || character == '='
                    })
                    .next()
                    .unwrap()
                    .to_owned();
                if !seen.insert(name.clone()) {
                    duplicates.insert(name);
                }
            }
        }
    }
    assert!(
        duplicates.is_empty(),
        "duplicate public generated names: {duplicates:?}"
    );
}

fn package_and_run_consumer(artifacts: &incurs_forge::ArtifactSet, consumer_source: &str) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "incurs-forge-sdk-symbols-{}-{unique}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let scratch = Scratch(directory);
    publish_artifacts(artifacts, scratch.0.join("artifacts")).unwrap();

    let consumer = scratch.0.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).unwrap();
    std::fs::write(consumer.join("src/main.rs"), consumer_source).unwrap();
    let manifest = "[package]\nname = \"forge-symbol-consumer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nsdk = { package = \"symbol-proof-sdk\", path = \"../build/package/symbol-proof-sdk-0.1.0\" }\nserde_json = \"1\"\n";
    std::fs::write(consumer.join("Cargo.toml"), manifest).unwrap();

    let toolchain = Command::new("rustup")
        .args(["which", "--toolchain", "stable", "cargo"])
        .output()
        .unwrap();
    assert!(
        toolchain.status.success(),
        "rustup stable is required for consumer proof"
    );
    let cargo = PathBuf::from(String::from_utf8(toolchain.stdout).unwrap().trim());
    let binaries = cargo.parent().unwrap();
    let checks = [
        (
            scratch.0.join("artifacts/sdk"),
            vec!["test", "--offline", "--locked", "-j", "2"],
        ),
        (
            scratch.0.join("artifacts/sdk"),
            vec!["package", "--offline", "--locked", "-j", "2"],
        ),
        (consumer, vec!["run", "--offline", "--quiet", "-j", "2"]),
    ];
    for (directory, arguments) in checks {
        let output = Command::new(&cargo)
            .args(&arguments)
            .current_dir(&directory)
            .env("RUSTC", binaries.join("rustc"))
            .env("RUSTDOC", binaries.join("rustdoc"))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_TARGET_DIR", scratch.0.join("build"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
