//! Build, package, and invoke the SDK generated from an actual OpenAPI fixture.
#![cfg(not(target_arch = "wasm32"))]
use incurs_forge::{
    ArtifactOptions, ResolveOptions, compile_artifacts, publish_artifacts, resolve_document,
};
use std::{
    path::{Path, PathBuf},
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
fn actual_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("../../forge-workers/fixtures/proof-openapi.json"),
        include_str!("fixtures/http_consumer.rs"),
    );
}

#[test]
fn text_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/text_openapi.json"),
        include_str!("fixtures/text_consumer.rs"),
    );
}

#[test]
fn schema_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/schema_openapi.json"),
        include_str!("fixtures/schema_consumer.rs"),
    );
}

#[test]
fn form_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/form_openapi.json"),
        include_str!("fixtures/form_consumer.rs"),
    );
}

#[test]
fn multipart_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/multipart_openapi.json"),
        include_str!("fixtures/multipart_consumer.rs"),
    );
}

#[test]
fn openapi32_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/openapi32.json"),
        include_str!("fixtures/openapi32_consumer.rs"),
    );
}

#[test]
fn conditional_schema_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/conditional_openapi.json"),
        include_str!("fixtures/conditional_consumer.rs"),
    );
}

#[test]
fn distinct_wire_names_build_package_and_call_http() {
    prove(
        include_str!("fixtures/names_openapi.json"),
        include_str!("fixtures/names_consumer.rs"),
    );
}

#[test]
fn nullable_paths_build_package_and_preserve_presence_over_http() {
    prove(
        include_str!("fixtures/nullable_paths_openapi.json"),
        include_str!("fixtures/nullable_paths_consumer.rs"),
    );
}

#[test]
fn numeric_defaults_build_package_and_preserve_exact_http_values() {
    prove(
        include_str!("fixtures/numeric_defaults_openapi.json"),
        include_str!("fixtures/numeric_defaults_consumer.rs"),
    );
}

#[test]
fn response_status_families_build_package_and_preserve_http_responses() {
    prove(
        include_str!("fixtures/response_status_openapi.json"),
        include_str!("fixtures/response_status_consumer.rs"),
    );
}

#[test]
fn typed_responses_build_package_and_decode_http_without_losing_raw_values() {
    prove_with_features(
        include_str!("fixtures/typed_responses_openapi.json"),
        include_str!("fixtures/typed_responses_consumer.rs"),
        &["typed-responses"],
    );
}

fn prove(document: &str, consumer_source: &str) {
    prove_with_features(document, consumer_source, &[]);
}

fn prove_with_features(document: &str, consumer_source: &str, features: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "incurs-forge-consumer-{}-{unique}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let scratch = Scratch(directory);
    let document = serde_json::from_str(document).unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("worker-proof")).unwrap();
    let artifacts = compile_artifacts(&contract, ArtifactOptions::new("http-proof-sdk")).unwrap();
    publish_artifacts(&artifacts, scratch.0.join("artifacts")).unwrap();
    let consumer = scratch.0.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).unwrap();
    std::fs::write(consumer.join("src/main.rs"), consumer_source).unwrap();
    let manifest = format!(
        "[package]\nname = \"forge-consumer-proof\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nsdk = {{ package = \"http-proof-sdk\", path = \"../build/package/http-proof-sdk-0.1.0\", features = {} }}\nincurs-forge = {{ path = {} }}\nserde_json = \"1\"\n",
        serde_json::to_string(features).unwrap(),
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
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
    for (directory, arguments) in [
        (
            scratch.0.join("artifacts/sdk"),
            vec!["test", "--all-features", "--offline", "--locked", "-j", "2"],
        ),
        (
            scratch.0.join("artifacts/sdk"),
            vec![
                "package",
                "--all-features",
                "--offline",
                "--locked",
                "-j",
                "2",
            ],
        ),
        (consumer, vec!["run", "--offline", "--quiet", "-j", "2"]),
    ] {
        let output = Command::new(&cargo)
            .args(&arguments)
            .current_dir(&directory)
            .env("RUSTC", binaries.join("rustc"))
            .env("RUSTDOC", binaries.join("rustdoc"))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
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
