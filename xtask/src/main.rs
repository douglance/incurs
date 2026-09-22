use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const MCP_SCHEMA_COMMIT: &str = "fc28315bb1eb362129ab27e85f2b65ca63f2fa30";
const MCP_SCHEMA_VERSIONS: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];

#[derive(Serialize)]
struct McpSchemaManifest<'a> {
    repository: &'a str,
    commit: &'a str,
    standards: Vec<McpSchemaEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpSchemaEntry {
    version: String,
    path: String,
    sha256: String,
}

fn main() {
    let Some(command) = env::args().nth(1) else {
        eprintln!("usage: cargo xtask <release-check|mcp-schema-sync|skill-sync>");
        std::process::exit(2);
    };
    let result = match command.as_str() {
        "release-check" => release_check(),
        "mcp-schema-sync" => mcp_schema_sync(env::args().any(|arg| arg == "--check")),
        "skill-sync" => skill_sync(env::args().any(|arg| arg == "--check")),
        _ => {
            eprintln!("unknown xtask: {command}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("{command} failed: {error}");
        std::process::exit(1);
    }
}

/// Whether a release package lives in the root workspace or the extension one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workspace {
    /// The repository's own workspace.
    Root,
    /// `extensions/cloudflare`, which is its own workspace.
    Cloudflare,
}

/// Every crate this repository publishes, with the version it publishes at.
///
/// Hand-maintained, and therefore covered by a test that derives the same set
/// from the manifests: this list silently fell three versions behind, and a
/// constant someone has to remember to update is not a guard.
///
/// `incurs-remote` is a root workspace member, so `cargo package --workspace`
/// builds it. Leaving it out of this list meant it shipped without ever being
/// unpacked and compiled from its own archive.
fn release_packages() -> Vec<(Workspace, String, String)> {
    [
        (Workspace::Root, "incurs-macros", "0.6.0"),
        (Workspace::Root, "incurs", "0.10.2"),
        (Workspace::Root, "incurs-cli", "0.10.0"),
        (Workspace::Root, "incurs-extras", "0.10.0"),
        (Workspace::Root, "incurs-codemode", "0.8.0"),
        (Workspace::Root, "incurs-codemode-local", "0.8.0"),
        (Workspace::Root, "incurs-codemode-mcp", "0.8.0"),
        (Workspace::Root, "incurs-mcp-protocol", "0.2.0"),
        (Workspace::Root, "incurs-mcp-discovery", "0.1.1"),
        (Workspace::Root, "incurs-mcp-client", "0.6.0"),
        (Workspace::Root, "incurs-mcp-registry", "0.6.0"),
        (Workspace::Root, "incurs-remote", "0.6.0"),
        (Workspace::Root, "incurs-app-model", "0.5.0"),
        (Workspace::Root, "incurs-app-ratatui", "0.5.0"),
        (Workspace::Cloudflare, "incurs-codemode-cloudflare", "0.8.0"),
        (Workspace::Cloudflare, "incurs-mcp-cloudflare", "0.6.0"),
    ]
    .into_iter()
    .map(|(workspace, package, version)| (workspace, package.to_string(), version.to_string()))
    .collect()
}

fn release_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let cloudflare = root.join("extensions/cloudflare");
    let declared = release_packages();
    let packages: Vec<(&Path, &str, &str)> = declared
        .iter()
        .map(|(workspace, package, version)| {
            let package_root = match workspace {
                Workspace::Root => root,
                Workspace::Cloudflare => cloudflare.as_path(),
            };
            (package_root, package.as_str(), version.as_str())
        })
        .collect();
    for (package_root, package, version) in &packages {
        for candidate in [
            package_archive(package_root, package, version),
            package_archive(root, package, version),
        ] {
            if candidate.is_file() {
                fs::remove_file(candidate)?;
            }
        }
    }
    run(
        Command::new("cargo").current_dir(root).args([
            "package",
            "--workspace",
            "--exclude",
            "xtask",
            "--allow-dirty",
            "--no-verify",
            "--locked",
        ]),
        "package release workspace",
    )?;
    // The Cloudflare extension is its own workspace, so its members are packaged
    // separately from the root `cargo package --workspace` above.
    //
    // A path dependency that crosses a workspace boundary is resolved from the
    // registry at packaging time, so these crates cannot be packaged until the
    // `incurs` version they require is published. That is a property of cargo,
    // not a defect, and it fixes the release order: core first, extensions
    // after. Report it plainly rather than failing the check for it; every
    // other packaging failure still fails.
    let mut deferred = Vec::new();
    for package in ["incurs-codemode-cloudflare", "incurs-mcp-cloudflare"] {
        let mut command = Command::new("cargo");
        command.current_dir(&cloudflare);
        command.args([
            "package",
            "-p",
            package,
            "--allow-dirty",
            "--no-verify",
            "--locked",
        ]);
        let output = command.output()?;
        if output.status.success() {
            continue;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("failed to select a version for the requirement `incurs") {
            deferred.push(package);
            continue;
        }
        return Err(format!(
            "package {package} exited with {}: {}",
            output.status,
            stderr.trim()
        )
        .into());
    }
    if !deferred.is_empty() {
        eprintln!(
            "deferred until incurs is published: {}",
            deferred.join(", ")
        );
    }

    let temp = env::temp_dir().join(format!("incurs-release-check-{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::create_dir_all(&temp)?;
    // A deferred package has no archive to verify yet.
    let verifiable: Vec<_> = packages
        .iter()
        .filter(|(_, package, _)| !deferred.contains(package))
        .copied()
        .collect();
    let result = verify_archives(&temp, root, &verifiable);
    if result.is_ok() {
        fs::remove_dir_all(&temp)?;
    } else {
        eprintln!("kept unpacked release artifacts at {}", temp.display());
    }
    result
}

fn mcp_schema_sync(check: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let schema_root = root.join("crates/incurs-mcp-protocol/schema");
    let generated = root.join("crates/incurs-mcp-protocol/src/generated.rs");
    let mut entries = Vec::new();
    let mut registries = Vec::new();

    for version in MCP_SCHEMA_VERSIONS {
        let url = format!(
            "https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol/{MCP_SCHEMA_COMMIT}/schema/{version}/schema.json"
        );
        let output = Command::new("curl")
            .args(["--fail", "--location", "--silent", "--show-error", &url])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "download MCP schema {version} exited with {}",
                output.status
            )
            .into());
        }
        let schema: Value = serde_json::from_slice(&output.stdout)?;
        let schema = serde_json::to_vec_pretty(&schema)?;
        let path = schema_root.join(version).join("schema.json");
        sync_file(&path, &schema, check)?;

        let mut methods = Vec::new();
        collect_mcp_methods(&serde_json::from_slice(&schema)?, &mut methods);
        methods.sort();
        methods.dedup();
        registries.push((version, methods));
        entries.push(McpSchemaEntry {
            version: version.to_string(),
            path: format!("{version}/schema.json"),
            sha256: format!("{:x}", Sha256::digest(&schema)),
        });
    }

    let manifest = serde_json::to_vec_pretty(&McpSchemaManifest {
        repository: "https://github.com/modelcontextprotocol/modelcontextprotocol",
        commit: MCP_SCHEMA_COMMIT,
        standards: entries,
    })?;
    sync_file(&schema_root.join("manifest.json"), &manifest, check)?;
    let generated_source = format_rust(&generated_method_registries(&registries))?;
    sync_file(&generated, generated_source.as_bytes(), check)?;
    println!(
        "{} five MCP schemas pinned at {MCP_SCHEMA_COMMIT}",
        if check { "verified" } else { "synced" }
    );
    Ok(())
}

/// Compiles the repository's own `SKILL.md` from the `incurs` command graph.
///
/// The skill is generated, not written. Its authoring reference is carried by
/// `incurs explain` and its command list by `--llms-full`, so both reach agents
/// through the same definitions the CLI serves. A hand-maintained copy would be
/// a second thing to keep true, and the previous one drifted so far it still
/// documented a TypeScript package.
fn skill_sync(check: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();

    run(
        Command::new("cargo")
            .current_dir(root)
            .args(["build", "--quiet", "-p", "incurs-cli"]),
        "build incurs-cli",
    )?;
    let incurs = root.join("target/debug/incurs");

    let topics: Value = serde_json::from_str(&capture(&incurs, &["explain", "--format", "json"])?)?;
    let mut body = String::new();
    for topic in topics["topics"].as_array().into_iter().flatten() {
        let name = topic["name"]
            .as_str()
            .ok_or("topic name must be a string")?;
        let one: Value =
            serde_json::from_str(&capture(&incurs, &["explain", name, "--format", "json"])?)?;
        let entry = &one["topics"][0];
        let title = entry["title"]
            .as_str()
            .ok_or("topic title must be a string")?;
        let text = entry["body"]
            .as_str()
            .ok_or("topic body must be a string")?;
        body.push_str(&format!("## {title}\n\n{text}\n\n"));
    }

    let commands = capture(&incurs, &["--llms-full"])?;
    let mut document = String::new();
    document.push_str("---\n");
    document.push_str("name: incurs\n");
    document.push_str(
        "description: Rust framework for building CLIs that serve both agents and humans. \
Use when building, extending, or generating code for an incurs CLI.\n",
    );
    document.push_str("command: incurs\n");
    document.push_str("---\n\n");
    document.push_str("<!-- Generated by `cargo xtask skill-sync`; do not edit. -->\n\n");
    document.push_str("# incurs\n\n");
    document.push_str(
        "Define a command once and expose the same validated behavior through CLI, HTTP, \
MCP, OpenAPI, Agent Plugin packages, skill files, shell completions, and a native \
desktop window.\n\n",
    );
    document.push_str(&body);
    document.push_str("# The `incurs` tool\n\n");
    document.push_str(commands.trim_end());
    document.push('\n');

    sync_file_with(
        &root.join("SKILL.md"),
        document.as_bytes(),
        check,
        "skill-sync",
    )
}

/// Runs one command and returns its stdout, failing on a non-zero status.
fn capture(program: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "{} {:?} exited with {}: {}",
            program.display(),
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn format_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut rustfmt = Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", "2024"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    rustfmt
        .stdin
        .take()
        .expect("piped rustfmt stdin")
        .write_all(source.as_bytes())?;
    let output = rustfmt.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt generated MCP registries exited with {}",
            output.status
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn sync_file(path: &Path, expected: &[u8], check: bool) -> Result<(), Box<dyn std::error::Error>> {
    sync_file_with(path, expected, check, "mcp-schema-sync")
}

/// Writes one generated file, or verifies it matches, naming the task that
/// regenerates it.
fn sync_file_with(
    path: &Path,
    expected: &[u8],
    check: bool,
    task: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if check {
        let actual = fs::read(path).map_err(|error| {
            format!(
                "{} is missing; run cargo xtask {task}: {error}",
                path.display()
            )
        })?;
        if actual != expected {
            return Err(
                format!("{} is out of date; run cargo xtask {task}", path.display()).into(),
            );
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, expected)?;
    Ok(())
}

fn collect_mcp_methods(value: &Value, methods: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(method) = object
                .get("method")
                .and_then(Value::as_object)
                .and_then(|method| method.get("const"))
                .and_then(Value::as_str)
            {
                methods.push(method.to_string());
            }
            for value in object.values() {
                collect_mcp_methods(value, methods);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_mcp_methods(value, methods);
            }
        }
        _ => {}
    }
}

fn generated_method_registries(registries: &[(&str, Vec<String>)]) -> String {
    let mut output =
        String::from("// @generated by `cargo xtask mcp-schema-sync`; do not edit.\n\n");
    for (version, methods) in registries {
        let name = format!("V_{}", version.replace('-', "_"));
        output.push_str(&format!("pub(super) const {name}_REQUESTS: &[&str] = &[\n"));
        for method in methods
            .iter()
            .filter(|method| is_core_mcp_method(method))
            .filter(|method| !method.starts_with("notifications/"))
        {
            output.push_str(&format!("    {method:?},\n"));
        }
        output.push_str("];\n");
        output.push_str(&format!(
            "pub(super) const {name}_NOTIFICATIONS: &[&str] = &[\n"
        ));
        for method in methods
            .iter()
            .filter(|method| is_core_mcp_method(method))
            .filter(|method| method.starts_with("notifications/"))
        {
            output.push_str(&format!("    {method:?},\n"));
        }
        output.push_str("];\n\n");
    }
    output
}

fn is_core_mcp_method(method: &str) -> bool {
    !method.starts_with("tasks/")
        && !method.starts_with("notifications/tasks/")
        && !method.starts_with("apps/")
        && !method.starts_with("notifications/apps/")
}

fn verify_archives(
    temp: &Path,
    workspace_root: &Path,
    packages: &[(&Path, &str, &str)],
) -> Result<(), Box<dyn std::error::Error>> {
    for (root, package, version) in packages {
        let Some(archive) = locate_archive(root, workspace_root, package, version) else {
            return Err(format!(
                "missing package archive for {package} {version}: looked in {} and {}",
                package_archive(root, package, version).display(),
                package_archive(workspace_root, package, version).display()
            )
            .into());
        };
        run(
            Command::new("tar")
                .args(["-xzf"])
                .arg(&archive)
                .arg("-C")
                .arg(temp),
            &format!("unpack {package}"),
        )?;
    }

    let patch = packages
        .iter()
        .map(|(_, package, version)| {
            format!(
                "{package} = {{ path = {:?} }}",
                temp.join(format!("{package}-{version}"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (_, package, version) in packages {
        let manifest = temp.join(format!("{package}-{version}/Cargo.toml"));
        let mut contents = fs::read_to_string(&manifest)?;
        contents.push_str("\n[patch.crates-io]\n");
        contents.push_str(&patch);
        contents.push('\n');
        fs::write(&manifest, contents)?;
        run(
            Command::new("cargo")
                .arg("check")
                .arg("--manifest-path")
                .arg(&manifest)
                .arg("--target-dir")
                .arg(temp.join("target"))
                .arg("--all-features"),
            &format!("check packaged {package}"),
        )?;
        println!("verified {package} {version}");
    }
    Ok(())
}

fn package_archive(root: &Path, package: &str, version: &str) -> PathBuf {
    root.join("target/package")
        .join(format!("{package}-{version}.crate"))
}

/// Finds a package archive, wherever cargo decided to put it.
///
/// A nested workspace does not reliably get its own `target/`: the directory
/// depends on `CARGO_TARGET_DIR` and on whatever build wrapper is in front of
/// cargo, and the Cloudflare extension's archives land in the root workspace's
/// `target/package` on at least one common setup. Looking in both places is
/// correct under either layout, where hard-coding one silently failed the whole
/// release check on a path that was never the interesting part.
fn locate_archive(
    root: &Path,
    workspace_root: &Path,
    package: &str,
    version: &str,
) -> Option<PathBuf> {
    [
        package_archive(root, package, version),
        package_archive(workspace_root, package, version),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn run(command: &mut Command, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{label} exited with {status}").into());
    }
    Ok(())
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live below the workspace root")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the `version` from a manifest without a TOML parser.
    ///
    /// The field is the first bare `version = "…"` before any `[section]` that
    /// follows `[package]`, which is enough for these manifests and avoids a
    /// dependency whose only user would be this test.
    fn manifest_version(path: &Path) -> Option<String> {
        let text = fs::read_to_string(path).ok()?;
        let mut in_package = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_package = line == "[package]";
                continue;
            }
            if in_package && let Some(rest) = line.strip_prefix("version") {
                let rest = rest.trim_start().strip_prefix('=')?.trim();
                return rest.trim_matches('"').split('"').next().map(str::to_string);
            }
        }
        None
    }

    /// Returns every path a workspace manifest lists as a member.
    fn members(root: &Path) -> Vec<PathBuf> {
        let text = fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
        let Some(start) = text.find("members = [") else {
            return Vec::new();
        };
        let body = &text[start..];
        let end = body.find(']').expect("members list closes");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim().trim_end_matches(',').trim_matches('"');
                (!line.is_empty() && !line.starts_with("members")).then(|| root.join(line))
            })
            .filter(|path| path.join("Cargo.toml").is_file())
            .collect()
    }

    /// The release list is a hand-maintained copy of every crate's version, and
    /// it silently fell three versions behind. A list someone has to remember to
    /// update is not a guard, so this derives the expectation from the manifests
    /// instead of restating it.
    #[test]
    fn every_publishable_member_is_release_checked_at_its_manifest_version() {
        let root = workspace_root();
        let cloudflare = root.join("extensions/cloudflare");
        let pinned = release_packages();

        let mut missing = Vec::new();
        let mut drifted = Vec::new();
        for workspace in [root.to_path_buf(), cloudflare] {
            for member in members(&workspace) {
                let manifest = member.join("Cargo.toml");
                let text = fs::read_to_string(&manifest).expect("member manifest");
                if text.contains("publish = false") {
                    continue;
                }
                let name = member
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("member directory name")
                    .to_string();
                let version = manifest_version(&manifest).expect("a version");
                match pinned.iter().find(|(_, package, _)| *package == name) {
                    None => missing.push(name),
                    Some((_, _, pinned_version)) if *pinned_version != version => {
                        drifted.push(format!(
                            "{name}: pinned {pinned_version}, manifest {version}"
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        assert!(
            missing.is_empty(),
            "publishable members absent from the release list: {missing:?}"
        );
        assert!(
            drifted.is_empty(),
            "the release list disagrees with the manifests: {drifted:?}"
        );
    }
}
