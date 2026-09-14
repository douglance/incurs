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
        eprintln!("usage: cargo xtask <release-check|mcp-schema-sync>");
        std::process::exit(2);
    };
    let result = match command.as_str() {
        "release-check" => release_check(),
        "mcp-schema-sync" => mcp_schema_sync(env::args().any(|arg| arg == "--check")),
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

fn release_check() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let cloudflare = root.join("extensions/cloudflare");
    let packages = vec![
        (root, "incurs-macros", "0.4.0"),
        (root, "incurs", "0.5.3"),
        (root, "incurs-cli", "0.5.1"),
        (root, "incurs-extras", "0.5.0"),
        (root, "incurs-codemode", "0.2.0"),
        (root, "incurs-codemode-local", "0.2.1"),
        (root, "incurs-codemode-mcp", "0.2.0"),
        (root, "incurs-mcp-protocol", "0.1.0"),
        (cloudflare.as_path(), "incurs-codemode-cloudflare", "0.2.0"),
        (cloudflare.as_path(), "incurs-mcp-cloudflare", "0.1.0"),
    ];
    for (package_root, package, version) in &packages {
        let archive = package_archive(package_root, package, version);
        if archive.is_file() {
            fs::remove_file(archive)?;
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
        run(&mut command, &format!("package {package}"))?;
    }

    let temp = env::temp_dir().join(format!("incurs-release-check-{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::create_dir_all(&temp)?;
    let result = verify_archives(&temp, &packages);
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
    if check {
        let actual = fs::read(path).map_err(|error| {
            format!(
                "{} is missing; run cargo xtask mcp-schema-sync: {error}",
                path.display()
            )
        })?;
        if actual != expected {
            return Err(format!(
                "{} differs from the pinned MCP schema; run cargo xtask mcp-schema-sync",
                path.display()
            )
            .into());
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
    packages: &[(&Path, &str, &str)],
) -> Result<(), Box<dyn std::error::Error>> {
    for (root, package, version) in packages {
        let archive = package_archive(root, package, version);
        if !archive.is_file() {
            return Err(format!("missing package archive {}", archive.display()).into());
        }
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
