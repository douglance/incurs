//! Deterministic artifact compilation for resolved contracts.
mod docs;
mod publish;
mod sdk;

use crate::model::{OpenApiError, OpenApiResult, ResolvedOpenApi};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub use publish::{PublishReport, publish_artifacts};

/// Options for compiling artifacts from a resolved contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactOptions {
    /// Package name used by generated Rust crate artifacts.
    pub package_name: String,
    /// Version written into the generated package manifest.
    pub crate_version: String,
}

impl ArtifactOptions {
    /// Create artifact options with the default generated crate version.
    pub fn new(package_name: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            ..Self::default()
        }
    }
}

impl Default for ArtifactOptions {
    fn default() -> Self {
        Self {
            package_name: "incurs-openapi-generated".to_string(),
            crate_version: "0.1.0".to_string(),
        }
    }
}

/// One deterministic compiler output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    /// Relative output path using forward slashes.
    pub path: String,
    /// Artifact bytes.
    pub bytes: Vec<u8>,
}

/// A deterministic collection of generated artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSet {
    /// Stable contract digest for all artifacts in this set.
    pub contract_digest: String,
    /// Outputs sorted by path.
    pub artifacts: Vec<Artifact>,
}

impl ArtifactSet {
    /// Return an artifact by exact relative path.
    pub fn get(&self, path: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|artifact| artifact.path == path)
    }

    /// Return artifact paths in deterministic order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.artifacts.iter().map(|artifact| artifact.path.as_str())
    }
}

/// Compile deterministic artifacts without writing to the filesystem.
pub fn compile_artifacts(
    contract: &ResolvedOpenApi,
    options: ArtifactOptions,
) -> OpenApiResult<ArtifactSet> {
    validate_package_name(&options.package_name)?;
    validate_version(&options.crate_version)?;
    let digest = contract_digest(contract)?;
    let manifest = ArtifactManifest {
        package_name: options.package_name.clone(),
        crate_version: options.crate_version.clone(),
        namespace: contract.namespace.clone(),
        title: contract.title.clone(),
        contract_digest: digest.clone(),
        operation_count: contract.operations.len(),
    };
    let mut artifacts = BTreeMap::new();
    add_artifact(&mut artifacts, "contract.json", stable_json(contract)?)?;
    add_artifact(
        &mut artifacts,
        "artifact-manifest.json",
        stable_json(&manifest)?,
    )?;
    add_artifact(
        &mut artifacts,
        "docs/index.html",
        docs::render_index(contract, &digest)?.into_bytes(),
    )?;
    add_artifact(
        &mut artifacts,
        "docs/search.json",
        docs::render_search(contract, &digest)?,
    )?;
    for artifact in sdk::generate(contract, &options, &digest)? {
        add_artifact(&mut artifacts, &artifact.path, artifact.bytes)?;
    }
    Ok(ArtifactSet {
        contract_digest: digest,
        artifacts: artifacts
            .into_iter()
            .map(|(path, bytes)| Artifact { path, bytes })
            .collect(),
    })
}

/// Return the stable SHA-256 digest for the resolved contract bytes.
pub fn contract_digest(contract: &ResolvedOpenApi) -> OpenApiResult<String> {
    let bytes = stable_json(contract)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn add_artifact(
    artifacts: &mut BTreeMap<String, Vec<u8>>,
    path: &str,
    bytes: Vec<u8>,
) -> OpenApiResult<()> {
    validate_artifact_path(path)?;
    if artifacts.insert(path.to_string(), bytes).is_some() {
        return Err(OpenApiError(format!("duplicate artifact path {path}")));
    }
    Ok(())
}

fn stable_json(value: &impl Serialize) -> OpenApiResult<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|err| OpenApiError(format!("serialize artifact: {err}")))
}

fn validate_artifact_path(path: &str) -> OpenApiResult<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(OpenApiError(format!("invalid artifact path {path}")));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(OpenApiError(format!("invalid artifact path {path}")));
    }
    Ok(())
}

fn validate_package_name(name: &str) -> OpenApiResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(OpenApiError("package name is required".to_string()));
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(OpenApiError(format!("invalid package name {name}")));
    }
    if !chars
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        return Err(OpenApiError(format!("invalid package name {name}")));
    }
    Ok(())
}

fn validate_version(version: &str) -> OpenApiResult<()> {
    if version.is_empty()
        || !version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+')
        })
    {
        return Err(OpenApiError(format!("invalid crate version {version}")));
    }
    Ok(())
}

#[derive(Serialize)]
struct ArtifactManifest {
    package_name: String,
    crate_version: String,
    namespace: String,
    title: String,
    contract_digest: String,
    operation_count: usize,
}
