use crate::artifacts::{ArtifactSet, validate_artifact_path};
use crate::model::{ForgeError, ForgeResult};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Result of publishing artifacts into a new directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishReport {
    /// Directory that was created for this publish operation.
    pub target: PathBuf,
    /// Files written, relative to the target directory.
    pub files: Vec<PathBuf>,
}

/// Write artifacts into a new directory, refusing traversal, collisions, and overwrites.
pub fn publish_artifacts(
    set: &ArtifactSet,
    target: impl AsRef<Path>,
) -> ForgeResult<PublishReport> {
    let target = target.as_ref();
    if target.exists() {
        return Err(ForgeError(format!(
            "publish target already exists: {}",
            target.display()
        )));
    }
    fs::create_dir(target)
        .map_err(|err| ForgeError(format!("create publish target {}: {err}", target.display())))?;
    let result = publish_into_created_dir(set, target);
    if result.is_err() {
        let _ = fs::remove_dir_all(target);
    }
    result
}

fn publish_into_created_dir(set: &ArtifactSet, target: &Path) -> ForgeResult<PublishReport> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for artifact in &set.artifacts {
        validate_artifact_path(&artifact.path)?;
        let relative = relative_path(&artifact.path)?;
        if !seen.insert(relative.clone()) {
            return Err(ForgeError(format!(
                "duplicate artifact path {}",
                artifact.path
            )));
        }
        let output = target.join(&relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ForgeError(format!(
                    "create artifact directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|err| ForgeError(format!("create artifact {}: {err}", output.display())))?;
        file.write_all(&artifact.bytes)
            .map_err(|err| ForgeError(format!("write artifact {}: {err}", output.display())))?;
        files.push(relative);
    }
    Ok(PublishReport {
        target: target.to_path_buf(),
        files,
    })
}

fn relative_path(path: &str) -> ForgeResult<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return Err(ForgeError(format!("invalid artifact path {path}"))),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(ForgeError(format!("invalid artifact path {path}")));
    }
    Ok(out)
}
