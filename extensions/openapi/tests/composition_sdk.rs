//! Generated SDK composition proofs.
#![cfg(not(target_arch = "wasm32"))]
use incurs_openapi::{
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
fn composition_openapi_sdk_builds_packages_and_calls_http() {
    prove(
        include_str!("fixtures/composition_openapi.json"),
        include_str!("fixtures/composition_consumer.rs"),
    );
}

#[test]
fn advanced_compositions_build_package_and_preserve_wire_values() {
    prove(
        include_str!("fixtures/advanced_composition_openapi.json"),
        include_str!("fixtures/advanced_composition_consumer.rs"),
    );
}

#[test]
fn untyped_constraints_build_package_and_preserve_json_domains() {
    prove(
        include_str!("fixtures/untyped_openapi.json"),
        include_str!("fixtures/untyped_consumer.rs"),
    );
}

#[test]
fn raw_and_framed_media_build_package_and_preserve_http_bytes() {
    prove(
        include_str!("fixtures/media_openapi.json"),
        include_str!("fixtures/media_consumer.rs"),
    );
}

fn prove(document: &str, consumer_source: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "incurs-openapi-composition-consumer-{}-{unique}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let scratch = Scratch(directory);
    let document = serde_json::from_str(document).unwrap();
    let contract = resolve_document(&document, ResolveOptions::new("composition-proof")).unwrap();
    let artifacts =
        compile_artifacts(&contract, ArtifactOptions::new("composition-proof-sdk")).unwrap();
    publish_artifacts(&artifacts, scratch.0.join("artifacts")).unwrap();
    let consumer = scratch.0.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).unwrap();
    std::fs::write(consumer.join("src/main.rs"), consumer_source).unwrap();
    let manifest = format!(
        "[package]\nname = \"openapi-composition-consumer-proof\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nsdk = {{ package = \"composition-proof-sdk\", path = \"../build/package/composition-proof-sdk-0.1.0\" }}\nincurs-openapi = {{ path = {} }}\nserde_json = \"1\"\n",
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
            vec!["test", "--offline", "--locked", "-j", "2"],
        ),
        (
            scratch.0.join("artifacts/sdk"),
            vec!["package", "--offline", "--locked", "-j", "2"],
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

#[test]
fn composition_compiles_patterns_and_conflicting_intersections_without_erasing_them() {
    let original: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/composition_openapi.json")).unwrap();
    let contract = resolve_document(&original, ResolveOptions::new("composition-proof")).unwrap();
    assert!(compile_artifacts(&contract, ArtifactOptions::new("composition-proof-sdk")).is_ok());
    let mut document = original.clone();
    document["components"]["schemas"]["Pet"]["oneOf"][2]["pattern"] = serde_json::json!("^valid$");
    let contract = resolve_document(&document, ResolveOptions::new("composition-proof")).unwrap();
    assert!(compile_artifacts(&contract, ArtifactOptions::new("composition-proof-sdk")).is_ok());
    let mut document = original;
    document["components"]["schemas"]["Event"]["allOf"][2]["properties"]["id"] =
        serde_json::json!({"type":"string"});
    let contract = resolve_document(&document, ResolveOptions::new("composition-proof")).unwrap();
    let artifacts =
        compile_artifacts(&contract, ArtifactOptions::new("composition-proof-sdk")).unwrap();
    let schema = artifacts.get("sdk/src/schemas.json").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&schema.bytes).unwrap();
    assert_eq!(
        value["$defs"]["Event"]["allOf"][2]["properties"]["id"]["type"],
        "string"
    );
}

#[test]
fn source_dialect_controls_nullable_and_reference_siblings() {
    use serde_json::json;
    for (version, permits_null, permits_negative_sibling) in
        [("3.0.3", true, true), ("3.1.0", false, false)]
    {
        let document = json!({
            "openapi":version,"info":{"title":"Dialect proof","version":"1"},"paths":{},
            "components":{"schemas":{
                "Integer":{"type":"integer"},
                "Nullable":{"allOf":[{"type":"string","nullable":true}]},
                "UntypedNullable":{"anyOf":[{"type":"string"}],"nullable":true},
                "Siblings":{"allOf":[{"$ref":"#/components/schemas/Integer","minimum":0}]}
            }}
        });
        let contract = resolve_document(&document, ResolveOptions::new("dialect")).unwrap();
        assert_eq!(contract.openapi_version, version);
        let artifacts =
            compile_artifacts(&contract, ArtifactOptions::new("dialect-proof")).unwrap();
        let graph: serde_json::Value =
            serde_json::from_slice(&artifacts.get("sdk/src/schemas.json").unwrap().bytes).unwrap();
        let validator = jsonschema::draft202012::options()
            .offline()
            .build(&graph)
            .unwrap();
        assert_eq!(
            validator.is_valid(&json!({"#/$defs/Nullable":null})),
            permits_null
        );
        assert!(validator.is_valid(&json!({"#/$defs/Nullable":"value"})));
        assert!(!validator.is_valid(&json!({"#/$defs/UntypedNullable":null})));
        assert_eq!(
            validator.is_valid(&json!({"#/$defs/Siblings":-1})),
            permits_negative_sibling
        );
        assert!(validator.is_valid(&json!({"#/$defs/Siblings":1})));
    }
}

#[test]
fn unresolved_dialect_and_resource_semantics_are_not_silently_erased() {
    use serde_json::json;
    let original = json!({
        "openapi":"3.1.0","info":{"title":"Dialect proof","version":"1"},"paths":{},
        "components":{"schemas":{"Number":{"allOf":[{"type":"integer"}]}}}
    });
    let mut document = original.clone();
    document["jsonSchemaDialect"] = json!("https://example.invalid/custom-dialect");
    let contract = resolve_document(&document, ResolveOptions::new("dialect")).unwrap();
    assert_eq!(
        contract.json_schema_dialect.as_deref(),
        Some("https://example.invalid/custom-dialect")
    );
    assert!(
        compile_artifacts(&contract, ArtifactOptions::new("dialect-proof"))
            .unwrap_err()
            .to_string()
            .contains("unsupported schema dialect")
    );
    for keyword in ["$id", "$anchor", "$dynamicAnchor", "$dynamicRef"] {
        let mut document = original.clone();
        document["components"]["schemas"]["Number"][keyword] =
            json!("https://example.invalid/schema");
        let contract = resolve_document(&document, ResolveOptions::new("dialect")).unwrap();
        assert!(
            compile_artifacts(&contract, ArtifactOptions::new("dialect-proof"))
                .unwrap_err()
                .to_string()
                .contains("schema resource keyword is unsupported")
        );
    }
}

#[test]
fn generated_validation_keeps_repeated_references_in_one_bounded_graph() {
    use serde_json::json;
    let mut schemas = serde_json::Map::new();
    schemas.insert(
        "Leaf".into(),
        json!({"type":"object","description":"x".repeat(8192),
        "properties":{"id":{"type":"integer"}},"required":["id"]}),
    );
    for index in 0..128 {
        schemas.insert(
            format!("Wrapper{index}"),
            json!({"allOf":[{"$ref":"#/components/schemas/Leaf"}]}),
        );
    }
    let document = json!({"openapi":"3.1.0","info":{"title":"Shared","version":"1"},"paths":{},
        "components":{"schemas":schemas}});
    let contract = resolve_document(&document, ResolveOptions::new("shared")).unwrap();
    let artifacts = compile_artifacts(&contract, ArtifactOptions::new("shared-sdk")).unwrap();
    let graph = artifacts.get("sdk/src/schemas.json").unwrap();
    assert!(
        graph.bytes.len() < 40_000,
        "shared graph expanded to {} bytes",
        graph.bytes.len()
    );
    let value: serde_json::Value = serde_json::from_slice(&graph.bytes).unwrap();
    assert_eq!(
        value["$defs"]["Wrapper127"]["allOf"][0],
        json!({"$ref":"#/$defs/Leaf"})
    );
    assert_eq!(
        value["$defs"]["Leaf"]["description"]
            .as_str()
            .unwrap()
            .len(),
        8192
    );
    let source = std::str::from_utf8(&artifacts.get("sdk/src/lib.rs").unwrap().bytes).unwrap();
    assert_eq!(source.matches("static SCHEMA_VALIDATOR:").count(), 1);
    assert!(!source.contains(&"x".repeat(128)));
}

#[test]
fn schema_keywords_inside_examples_do_not_enable_generated_validation() {
    let document = serde_json::json!({
        "openapi":"3.1.0","info":{"title":"Annotation data","version":"1"},"paths":{},
        "components":{"schemas":{"Payload":{
            "type":"object",
            "examples":[{"oneOf":[],"properties":{"name":{"required":["not-a-schema"]}}}],
            "default":{"minimum":2}
        }}}
    });
    let contract = resolve_document(&document, ResolveOptions::new("annotations")).unwrap();
    let artifacts = compile_artifacts(&contract, ArtifactOptions::new("annotations-sdk")).unwrap();
    assert!(artifacts.get("sdk/src/schemas.json").is_none());
    let manifest = std::str::from_utf8(&artifacts.get("sdk/Cargo.toml").unwrap().bytes).unwrap();
    assert!(manifest.contains(
        "jsonschema = { version = \"=0.58.2\", optional = true, default-features = false"
    ));
    assert!(!manifest.contains("default = ["));
}
