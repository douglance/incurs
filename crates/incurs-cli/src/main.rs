use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Map, Value, json};

#[derive(Debug)]
struct GenOptions {
    dir: PathBuf,
    entry: Option<String>,
    output: Option<PathBuf>,
    json_output: Option<PathBuf>,
    config_schema: bool,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = match args.first().map(String::as_str) {
        Some("gen") => parse_gen(&args[1..]).and_then(generate),
        Some("--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}")),
    };

    if let Err(error) = result {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "incurs - CLI for incurs\n\nUsage: incurs <command>\n\nCommands:\n  gen  Generate Rust command types and JSON manifests\n"
    );
}

fn parse_gen(args: &[String]) -> Result<GenOptions, String> {
    let mut options = GenOptions {
        dir: PathBuf::from("."),
        entry: None,
        output: None,
        json_output: None,
        config_schema: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                println!(
                    "Generate Rust command types and JSON manifests.\n\nUsage: incurs gen [options]\n\nOptions:\n  --dir <path>          Cargo project root (default: .)\n  --entry <name|path>   Cargo binary name or executable path\n  --output <path>       Rust output (default: src/incurs_generated.rs)\n  --json-output <path>  JSON output (default: incurs.manifest.json)\n  --config-schema       Also generate config.schema.json\n"
                );
                return Ok(options);
            }
            "--config-schema" => options.config_schema = true,
            "--dir" | "--entry" | "--output" | "--json-output" => {
                let flag = args[index].clone();
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--dir" => options.dir = PathBuf::from(value),
                    "--entry" => options.entry = Some(value.clone()),
                    "--output" => options.output = Some(PathBuf::from(value)),
                    "--json-output" => options.json_output = Some(PathBuf::from(value)),
                    _ => unreachable!(),
                }
            }
            flag => return Err(format!("unknown option: {flag}")),
        }
        index += 1;
    }
    Ok(options)
}

fn generate(options: GenOptions) -> Result<(), String> {
    let dir = options
        .dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", options.dir.display()))?;
    let manifest = load_manifest(&dir, options.entry.as_deref())?;
    validate_manifest(&manifest)?;

    let rust_output = resolve_output(
        &dir,
        options.output,
        PathBuf::from("src/incurs_generated.rs"),
    );
    let json_output = resolve_output(
        &dir,
        options.json_output,
        PathBuf::from("incurs.manifest.json"),
    );
    write(&rust_output, &rust_source(&manifest)?)?;
    write(
        &json_output,
        &(serde_json::to_string_pretty(&canonicalize(manifest.clone()))
            .map_err(|error| error.to_string())?
            + "\n"),
    )?;

    let mut result = Map::from_iter([
        ("dir".to_string(), json!(dir)),
        ("output".to_string(), json!(rust_output)),
        ("manifest".to_string(), json!(json_output)),
    ]);
    if options.config_schema {
        let output = run_target(
            &dir,
            options.entry.as_deref(),
            &["--config-schema", "--format", "json"],
        )?;
        if !output.status.success() {
            return Err(format!(
                "target could not generate config schema: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let schema: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("target returned invalid config schema: {error}"))?;
        let schema_output = dir.join("config.schema.json");
        write(
            &schema_output,
            &(serde_json::to_string_pretty(&canonicalize(schema))
                .map_err(|error| error.to_string())?
                + "\n"),
        )?;
        result.insert("configSchema".to_string(), json!(schema_output));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(result)).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn resolve_output(dir: &Path, value: Option<PathBuf>, default: PathBuf) -> PathBuf {
    let output = value.unwrap_or(default);
    if output.is_absolute() {
        output
    } else {
        dir.join(output)
    }
}

fn load_manifest(dir: &Path, entry: Option<&str>) -> Result<Value, String> {
    let output = run_target(dir, entry, &["--llms-full", "--format", "json"])?;
    if !output.status.success() {
        return Err(format!(
            "target could not export its command manifest: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("target returned an invalid command manifest: {error}"))
}

fn run_target(dir: &Path, entry: Option<&str>, args: &[&str]) -> Result<Output, String> {
    if let Some(entry) = entry {
        let path = Path::new(entry);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            dir.join(path)
        };
        if path.is_file() {
            return Command::new(path)
                .current_dir(dir)
                .args(args)
                .output()
                .map_err(|error| error.to_string());
        }
    }

    let manifest = dir.join("Cargo.toml");
    let bin = match entry {
        Some(entry) => entry.to_string(),
        None => discover_bin(&manifest)?,
    };
    Command::new("cargo")
        .current_dir(dir)
        .args([
            "run",
            "--quiet",
            "--manifest-path",
            manifest
                .to_str()
                .ok_or_else(|| "Cargo.toml path is not UTF-8".to_string())?,
            "--bin",
            &bin,
            "--",
        ])
        .args(args)
        .output()
        .map_err(|error| error.to_string())
}

fn discover_bin(manifest: &Path) -> Result<String, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            manifest
                .to_str()
                .ok_or_else(|| "Cargo.toml path is not UTF-8".to_string())?,
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let manifest = manifest
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
    let mut bins = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|path| Path::new(path).canonicalize().ok())
                .as_ref()
                == Some(&manifest)
        })
        .flat_map(|package| package["targets"].as_array().into_iter().flatten())
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| target["name"].as_str().map(ToString::to_string))
        .collect::<Vec<_>>();
    bins.sort();
    match bins.as_slice() {
        [bin] => Ok(bin.clone()),
        [] => Err("no binary target found; pass --entry <bin>".to_string()),
        _ => Err(format!(
            "multiple binary targets found ({}); pass --entry <bin>",
            bins.join(", ")
        )),
    }
}

fn validate_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["version"] != "incur.v1" {
        return Err("target manifest version must be incur.v1".to_string());
    }
    if !manifest["commands"].is_array() {
        return Err("target manifest must contain a commands array".to_string());
    }
    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

fn rust_source(manifest: &Value) -> Result<String, String> {
    let commands = manifest["commands"]
        .as_array()
        .ok_or_else(|| "manifest commands must be an array".to_string())?;
    let manifest_json = serde_json::to_string(&canonicalize(manifest.clone()))
        .map_err(|error| error.to_string())?;
    let mut source = String::from(
        "// @generated by `incurs gen`; do not edit.\n\n\
         /// Canonical command manifest used to generate this module.\n",
    );
    source.push_str(&format!(
        "pub const MANIFEST_JSON: &str = {:?};\n\n",
        manifest_json
    ));
    source.push_str(
        "fn quote(value: &str) -> String {\n    if value.is_empty() || value.chars().any(char::is_whitespace) {\n        format!(\"{:?}\", value)\n    } else {\n        value.to_string()\n    }\n}\n\n",
    );

    for command in commands {
        let name = command["name"]
            .as_str()
            .ok_or_else(|| "command name must be a string".to_string())?;
        let module = rust_ident(&name.replace(' ', "_"));
        source.push_str(&format!(
            "/// Typed CTA helpers for `{name}`.\npub mod {module} {{\n"
        ));
        source.push_str(&format!(
            "    /// Canonical command name.\n    pub const NAME: &str = {name:?};\n\n"
        ));
        source.push_str("    /// Positional arguments for this command.\n    #[derive(Clone, Debug, Default)]\n    pub struct Args {\n");
        append_fields(&mut source, command.pointer("/schema/args"), true);
        source.push_str("    }\n\n    /// Named options for this command.\n    #[derive(Clone, Debug, Default)]\n    pub struct Options {\n");
        append_fields(&mut source, command.pointer("/schema/options"), false);
        source.push_str("    }\n\n");
        source.push_str("    /// Renders this typed invocation as a CTA entry.\n    pub fn cta(args: &Args, options: &Options) -> incurs::output::CtaEntry {\n        let mut parts = vec![NAME.to_string()];\n");
        append_render(&mut source, command.pointer("/schema/args"), true);
        append_render(&mut source, command.pointer("/schema/options"), false);
        source
            .push_str("        incurs::output::CtaEntry::Simple(parts.join(\" \"))\n    }\n}\n\n");
    }
    Ok(source)
}

fn append_fields(source: &mut String, schema: Option<&Value>, _positional: bool) {
    let required = schema
        .and_then(|schema| schema["required"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if let Some(properties) = schema.and_then(|schema| schema["properties"].as_object()) {
        for (name, property) in properties {
            let ident = rust_field_ident(name);
            let ty = rust_type(property);
            let optional = !required.contains(&name.as_str());
            let ty = if optional {
                format!("Option<{ty}>")
            } else {
                ty
            };
            source.push_str(&format!(
                "        /// Value for `{name}`.\n        pub {ident}: {ty},\n"
            ));
        }
    }
}

fn append_render(source: &mut String, schema: Option<&Value>, positional: bool) {
    let required = schema
        .and_then(|schema| schema["required"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if let Some(properties) = schema.and_then(|schema| schema["properties"].as_object()) {
        for (name, property) in properties {
            let ident = rust_field_ident(name);
            let is_required = required.contains(&name.as_str()) && positional;
            let render = render_value("value", property);
            if positional {
                if property["type"] == "array" {
                    let values = if is_required {
                        format!("&args.{ident}")
                    } else {
                        format!("args.{ident}.as_deref().unwrap_or_default()")
                    };
                    source.push_str(&format!("        for value in {values} {{ parts.push(super::quote(&value.to_string())); }}\n"));
                } else if is_required {
                    let render = render_value(&format!("args.{ident}"), property);
                    source.push_str(&format!("        parts.push(super::quote(&{render}));\n"));
                } else {
                    source.push_str(&format!("        if let Some(value) = &args.{ident} {{ parts.push(super::quote(&{render})); }}\n"));
                }
            } else if property["type"] == "boolean" {
                let condition = if required.contains(&name.as_str()) {
                    format!("options.{ident}")
                } else {
                    format!("options.{ident} == Some(true)")
                };
                source.push_str(&format!(
                    "        if {condition} {{ parts.push(\"--{}\".to_string()); }}\n",
                    kebab(name)
                ));
            } else if property["type"] == "array" {
                let values = if required.contains(&name.as_str()) {
                    format!("&options.{ident}")
                } else {
                    format!("options.{ident}.as_deref().unwrap_or_default()")
                };
                source.push_str(&format!("        for value in {values} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&value.to_string())); }}\n", kebab(name)));
            } else if required.contains(&name.as_str()) {
                let render = render_value(&format!("options.{ident}"), property);
                source.push_str(&format!("        parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render}));\n", kebab(name)));
            } else {
                source.push_str(&format!("        if let Some(value) = &options.{ident} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render})); }}\n", kebab(name)));
            }
        }
    }
}

fn rust_type(schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("boolean") => "bool".to_string(),
        Some("integer") => "i64".to_string(),
        Some("number") => "f64".to_string(),
        Some("array") => format!("Vec<{}>", rust_type(&schema["items"])),
        _ => "String".to_string(),
    }
}

fn render_value(value: &str, schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("string") | None => format!("{value}.to_string()"),
        Some("array") => {
            format!("{value}.iter().map(ToString::to_string).collect::<Vec<_>>().join(\",\")")
        }
        _ => format!("{value}.to_string()"),
    }
}

fn rust_ident(value: &str) -> String {
    let mut result = value
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if character.is_ascii_alphanumeric() || character == '_' {
                if index == 0 && character.is_ascii_digit() {
                    format!("_{character}")
                } else {
                    character.to_string()
                }
            } else {
                "_".to_string()
            }
        })
        .collect::<String>();
    if [
        "type", "match", "mod", "self", "crate", "super", "use", "pub", "fn",
    ]
    .contains(&result.as_str())
    {
        result = format!("r#{result}");
    }
    result
}

fn rust_field_ident(value: &str) -> String {
    rust_ident(&kebab(value).replace('-', "_"))
}

fn kebab(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('-');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_contains_typed_command_modules() {
        let manifest = json!({
            "version": "incur.v1",
            "commands": [{
                "name": "user create",
                "schema": {
                    "args": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    },
                    "options": {
                        "type": "object",
                        "properties": { "dryRun": { "type": "boolean" } }
                    }
                }
            }]
        });
        let source = rust_source(&manifest).unwrap();
        assert!(source.contains("pub mod user_create"));
        assert!(source.contains("pub name: String"));
        assert!(source.contains("pub dry_run: Option<bool>"));
        assert!(source.contains("--dry-run"));
        syn::parse_file(&source).unwrap();
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let value = canonicalize(json!({ "z": 1, "a": { "z": 2, "a": 3 } }));
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"a":{"a":3,"z":2},"z":1}"#
        );
    }
}
