use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use regex_lite::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Compare {
    Json,
    JsonLines,
    Text,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    args: Vec<String>,
    compare: Compare,
}

#[derive(Debug, Deserialize)]
struct Inventory {
    expected_tests: usize,
    files: Vec<InventoryFile>,
}

#[derive(Debug, Deserialize)]
struct InventoryFile {
    file: String,
    classification: String,
    rust_target: String,
    reason: String,
}

#[derive(Debug, PartialEq)]
struct Observation {
    code: Option<i32>,
    stdout: ObservedOutput,
    stderr: String,
}

#[derive(Debug, PartialEq)]
enum ObservedOutput {
    Json(Value),
    JsonLines(Vec<Value>),
    Text(String),
}

fn main() {
    let command = env::args().nth(1).unwrap_or_else(|| "parity".to_string());
    let result = match command.as_str() {
        "parity" => parity(),
        "release-check" => release_check(),
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
    let packages = [
        ("incurs-macros", "0.4.0"),
        ("incurs", "0.4.0"),
        ("incurs-cli", "0.4.0"),
        ("incurs-extras", "0.4.0"),
        ("incurs-codemode", "0.1.0"),
        ("incurs-codemode-local", "0.1.0"),
        ("incurs-codemode-mcp", "0.1.0"),
    ];
    run(
        Command::new("cargo").current_dir(root).args([
            "package",
            "--workspace",
            "--exclude",
            "xtask",
            "--allow-dirty",
            "--no-verify",
        ]),
        "package release workspace",
    )?;

    let temp = env::temp_dir().join(format!("incurs-release-check-{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::create_dir_all(&temp)?;
    let result = verify_archives(root, &temp, &packages);
    if result.is_ok() {
        fs::remove_dir_all(&temp)?;
    } else {
        eprintln!("kept unpacked release artifacts at {}", temp.display());
    }
    result
}

fn verify_archives(
    root: &Path,
    temp: &Path,
    packages: &[(&str, &str)],
) -> Result<(), Box<dyn std::error::Error>> {
    for (package, version) in packages {
        let archive = root
            .join("target/package")
            .join(format!("{package}-{version}.crate"));
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
        .map(|(package, version)| {
            format!(
                "{package} = {{ path = {:?} }}",
                temp.join(format!("{package}-{version}"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (package, version) in packages {
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

fn parity() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let inventory_count = validate_inventory(root)?;
    println!("classified {inventory_count} TypeScript oracle tests");
    let cases: Vec<Case> =
        serde_json::from_str(&fs::read_to_string(root.join("tests/parity/cases.json"))?)?;

    let status = Command::new("cargo")
        .current_dir(root)
        .args([
            "build",
            "-q",
            "-p",
            "incurs",
            "--example",
            "todoapp",
            "--all-features",
        ])
        .status()?;
    if !status.success() {
        return Err("failed to build the Rust parity fixture".into());
    }

    let rust = rust_fixture(root);
    let mut failed = Vec::new();
    for case in &cases {
        let ts = observe(run_ts(root, &case.args)?, &case.compare)?;
        let rs = observe(run_rust(root, &rust, &case.args)?, &case.compare)?;
        if ts == rs {
            println!("PASS {}", case.name);
        } else {
            println!("FAIL {}", case.name);
            println!("  TypeScript: {ts:#?}");
            println!("  Rust:       {rs:#?}");
            failed.push(case.name.as_str());
        }
    }

    println!(
        "\n{} passed, {} failed",
        cases.len() - failed.len(),
        failed.len()
    );
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("mismatched cases: {}", failed.join(", ")).into())
    }
}

fn validate_inventory(root: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let inventory: Inventory = serde_json::from_str(&fs::read_to_string(
        root.join("tests/parity/inventory.json"),
    )?)?;
    let mut discovered = fs::read_dir(root.join("src"))?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".test.ts") || name.ends_with(".test-d.ts"))
        .map(|name| format!("src/{name}"))
        .collect::<Vec<_>>();
    discovered.sort();
    let mut classified = inventory
        .files
        .iter()
        .map(|entry| entry.file.clone())
        .collect::<Vec<_>>();
    classified.sort();
    if discovered != classified {
        return Err(format!(
            "parity inventory drifted\n  discovered: {discovered:?}\n  classified: {classified:?}"
        )
        .into());
    }

    let test = Regex::new(r"(?m)^\s*(?:test|it)\(").expect("valid test regex");
    let mut count = 0;
    for entry in &inventory.files {
        if !matches!(
            entry.classification.as_str(),
            "shared" | "rust_native" | "typescript_only"
        ) || entry.rust_target.trim().is_empty()
            || entry.reason.trim().is_empty()
        {
            return Err(format!("invalid inventory entry for {}", entry.file).into());
        }
        count += test
            .find_iter(&fs::read_to_string(root.join(&entry.file))?)
            .count();
    }
    if count != inventory.expected_tests {
        return Err(format!(
            "expected {} TypeScript tests, discovered {count}",
            inventory.expected_tests
        )
        .into());
    }
    Ok(count)
}

fn rust_fixture(root: &Path) -> PathBuf {
    root.join("target")
        .join("debug")
        .join("examples")
        .join(if cfg!(windows) {
            "todoapp.exe"
        } else {
            "todoapp"
        })
}

fn run_ts(root: &Path, args: &[String]) -> std::io::Result<Output> {
    Command::new("node")
        .current_dir(root)
        .args(["--import", "tsx", "examples/todoapp.ts"])
        .args(args)
        .output()
}

fn run_rust(root: &Path, executable: &Path, args: &[String]) -> std::io::Result<Output> {
    Command::new(executable)
        .current_dir(root)
        .args(args)
        .output()
}

fn observe(output: Output, compare: &Compare) -> Result<Observation, Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout)?;
    let stdout = match compare {
        Compare::Json => ObservedOutput::Json(normalize_value(serde_json::from_str(&stdout)?)),
        Compare::JsonLines => ObservedOutput::JsonLines(
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(normalize_value)
                .collect(),
        ),
        Compare::Text => ObservedOutput::Text(normalize_text(&stdout)),
    };
    Ok(Observation {
        code: output.status.code(),
        stdout,
        stderr: normalize_text(&String::from_utf8(output.stderr)?),
    })
}

fn normalize_value(mut value: Value) -> Value {
    match &mut value {
        Value::Array(values) => {
            for value in values {
                *value = normalize_value(value.take());
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                *value = normalize_value(value.take());
            }
        }
        Value::String(value) => {
            let duration = Regex::new(r"^\d+(?:\.\d+)?ms$").expect("valid duration regex");
            if duration.is_match(value) {
                *value = "<duration>".to_string();
            }
        }
        _ => {}
    }
    value
}

fn normalize_text(value: &str) -> String {
    let duration = Regex::new(r"\b\d+(?:\.\d+)?ms\b").expect("valid duration regex");
    duration
        .replace_all(&value.replace("\r\n", "\n"), "<duration>")
        .into_owned()
}
