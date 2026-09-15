//! Rust and JSON generation from a target CLI's command manifest.
//!
//! The target is asked for its own manifest (`--llms-full --format json`) and
//! its own config schema (`--config-schema --format json`), so this module
//! never reimplements the command model — it only lowers a manifest the target
//! produced into Rust source a caller can compile against.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incurs::command::{TypedContext, TypedResult};
use incurs::schema::to_kebab;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Named options for `incurs gen`.
#[derive(Deserialize, incurs::Options)]
pub struct GenOptions {
    /// Cargo project root.
    #[incurs(default = ".")]
    pub dir: String,
    /// Cargo binary name or executable path.
    pub entry: Option<String>,
    /// Rust output path, relative to `--dir` unless absolute.
    pub output: Option<String>,
    /// JSON manifest output path, relative to `--dir` unless absolute.
    pub json_output: Option<String>,
    /// Also generate `config.schema.json`.
    pub config_schema: bool,
    /// Also build an Agent Plugins 1.0 directory through the target CLI.
    pub plugin_output: Option<String>,
    /// Skill grouping depth for the generated Agent Plugin.
    #[incurs(default = 1)]
    pub plugin_skill_depth: u32,
    /// Skip `mcp.json` generation.
    pub plugin_no_mcp: bool,
    /// Bundle the target CLI as the plugin Tool Runtime.
    pub plugin_bundle_cli: bool,
    /// Replace existing owned plugin artifacts.
    pub plugin_force: bool,
}

/// Paths written by one `incurs gen` run.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenOutput {
    /// Resolved Cargo project root.
    pub dir: PathBuf,
    /// Generated Rust module.
    pub output: PathBuf,
    /// Generated canonical JSON manifest.
    pub manifest: PathBuf,
    /// Generated config schema, when `--config-schema` was passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<PathBuf>,
    /// Generated Agent Plugin directory, when `--plugin-output` was passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PathBuf>,
}

/// Runs `incurs gen`.
pub async fn run(ctx: TypedContext<(), GenOptions, ()>) -> TypedResult<GenOutput> {
    match generate(&ctx.options) {
        Ok(output) => TypedResult::ok(output),
        Err(error) => TypedResult::error("GEN_FAILED", error),
    }
}

/// Generates every artifact the options ask for.
fn generate(options: &GenOptions) -> Result<GenOutput, String> {
    let requested = PathBuf::from(&options.dir);
    let dir = requested
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", requested.display()))?;
    let manifest = load_manifest(&dir, options.entry.as_deref())?;
    validate_manifest(&manifest)?;

    let rust_output = resolve_output(
        &dir,
        options.output.as_deref().map(PathBuf::from),
        PathBuf::from("src/incurs_generated.rs"),
    );
    let json_output = resolve_output(
        &dir,
        options.json_output.as_deref().map(PathBuf::from),
        PathBuf::from("incurs.manifest.json"),
    );
    write(&rust_output, &rust_source(&manifest)?)?;
    write(
        &json_output,
        &(serde_json::to_string_pretty(&canonicalize(manifest.clone()))
            .map_err(|error| error.to_string())?
            + "\n"),
    )?;

    let config_schema = if options.config_schema {
        Some(write_config_schema(&dir, options.entry.as_deref())?)
    } else {
        None
    };

    let plugin = match options.plugin_output.as_deref() {
        Some(plugin_output) => Some(build_agent_plugin(&dir, options, plugin_output)?),
        None => None,
    };

    Ok(GenOutput {
        dir,
        output: rust_output,
        manifest: json_output,
        config_schema,
        plugin,
    })
}

/// Asks the target for its config schema and writes it beside the manifest.
fn write_config_schema(dir: &Path, entry: Option<&str>) -> Result<PathBuf, String> {
    let output = run_target(dir, entry, &["--config-schema", "--format", "json"])?;
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
    Ok(schema_output)
}

/// Asks the target to build its own Agent Plugin directory.
fn build_agent_plugin(
    dir: &Path,
    options: &GenOptions,
    plugin_output: &str,
) -> Result<PathBuf, String> {
    let plugin_root = resolve_output(
        dir,
        Some(PathBuf::from(plugin_output)),
        PathBuf::from("agent-plugin"),
    );
    let mut args = vec![
        "plugin".to_string(),
        "build".to_string(),
        "--output".to_string(),
        plugin_root.to_string_lossy().to_string(),
        "--depth".to_string(),
        options.plugin_skill_depth.to_string(),
    ];
    if options.plugin_no_mcp {
        args.push("--no-mcp".to_string());
    }
    if options.plugin_bundle_cli {
        args.push("--bundle-cli".to_string());
    }
    if options.plugin_force {
        args.push("--force".to_string());
    }
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_target(dir, options.entry.as_deref(), &arg_refs)?;
    if !output.status.success() {
        return Err(format!(
            "target could not build Agent Plugin: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(plugin_root)
}

/// Resolves an output path against the project root unless it is absolute.
fn resolve_output(dir: &Path, value: Option<PathBuf>, default: PathBuf) -> PathBuf {
    let output = value.unwrap_or(default);
    if output.is_absolute() {
        output
    } else {
        dir.join(output)
    }
}

/// Asks the target CLI for its own command manifest.
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

/// Runs the target CLI, either as a built executable or through Cargo.
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

/// Finds the sole binary target of a Cargo manifest.
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

/// Rejects a manifest this generator does not understand.
fn validate_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["version"] != "incur.v1" {
        return Err("target manifest version must be incur.v1".to_string());
    }
    if !manifest["commands"].is_array() {
        return Err("target manifest must contain a commands array".to_string());
    }
    Ok(())
}

/// Writes one generated file, creating parent directories as needed.
fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Sorts every object key so the embedded manifest is byte-stable.
pub fn canonicalize(value: Value) -> Value {
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

/// Lowers a command manifest into a Rust module of typed CTA helpers.
pub fn rust_source(manifest: &Value) -> Result<String, String> {
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

/// Appends one struct field per schema property.
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

/// Appends the rendering statements that turn one field into CTA tokens.
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
                    to_kebab(name)
                ));
            } else if property["type"] == "array" {
                let values = if required.contains(&name.as_str()) {
                    format!("&options.{ident}")
                } else {
                    format!("options.{ident}.as_deref().unwrap_or_default()")
                };
                source.push_str(&format!("        for value in {values} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&value.to_string())); }}\n", to_kebab(name)));
            } else if required.contains(&name.as_str()) {
                let render = render_value(&format!("options.{ident}"), property);
                source.push_str(&format!("        parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render}));\n", to_kebab(name)));
            } else {
                source.push_str(&format!("        if let Some(value) = &options.{ident} {{ parts.push(\"--{}\".to_string()); parts.push(super::quote(&{render})); }}\n", to_kebab(name)));
            }
        }
    }
}

/// Maps one JSON Schema fragment onto a Rust type.
fn rust_type(schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("boolean") => "bool".to_string(),
        Some("integer") => "i64".to_string(),
        Some("number") => "f64".to_string(),
        Some("array") => format!("Vec<{}>", rust_type(&schema["items"])),
        _ => "String".to_string(),
    }
}

/// Renders one field value as a CTA token expression.
fn render_value(value: &str, schema: &Value) -> String {
    match schema["type"].as_str() {
        Some("string") | None => format!("{value}.to_string()"),
        Some("array") => {
            format!("{value}.iter().map(ToString::to_string).collect::<Vec<_>>().join(\",\")")
        }
        _ => format!("{value}.to_string()"),
    }
}

/// Sanitizes a name into a Rust identifier.
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

/// Maps a schema property key onto a Rust field identifier.
///
/// Routes through [`to_kebab`] so a snake_case key and its kebab-case spelling
/// produce the same field, and so the flag rendered beside it cannot disagree.
fn rust_field_ident(value: &str) -> String {
    rust_ident(&to_kebab(value).replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A real `--llms-full` manifest, captured from the built `todoapp` example.
    ///
    /// Regenerate with:
    ///
    /// ```sh
    /// cargo build --examples -p incurs --all-features
    /// target/debug/examples/todoapp --llms-full --format json \
    ///   > crates/incurs-cli/fixtures/todoapp.manifest.json
    /// ```
    const TODOAPP_MANIFEST: &str = include_str!("../fixtures/todoapp.manifest.json");

    /// The Rust `incurs gen` produces from [`TODOAPP_MANIFEST`].
    const TODOAPP_GENERATED: &str = include_str!("../fixtures/todoapp.generated.rs.txt");

    /// Code generation is pinned against a real manifest, not a hand-written one.
    ///
    /// The fixture beside this test is the actual output of a built `incurs`
    /// CLI, so this asserts what a user of `incurs gen` receives rather than
    /// what a fixture author imagined. `source_contains_typed_command_modules`
    /// covers shapes the todoapp fixture happens not to contain; this covers
    /// every byte of one that it does.
    #[test]
    fn generated_source_matches_a_real_manifest() {
        let manifest: Value = serde_json::from_str(TODOAPP_MANIFEST).expect("fixture is JSON");

        let source = rust_source(&manifest).expect("the real manifest generates");

        assert_eq!(
            source, TODOAPP_GENERATED,
            "`incurs gen` output changed for the todoapp manifest. If that is \
             intended, regenerate crates/incurs-cli/fixtures/todoapp.generated.rs.txt \
             and review the diff as the wire-format change it is."
        );
        syn::parse_file(&source).expect("generated source parses");
    }

    /// An array option must generate its declared item type, not `Vec<String>`.
    ///
    /// `rust_type` reads `schema["items"]`, so an emitter that omits `items`
    /// silently degrades every array to `Vec<String>` with no error. Asserting
    /// both branches here means that degradation fails a test rather than
    /// reaching a user's generated code.
    #[test]
    fn an_array_generates_its_declared_item_type() {
        assert_eq!(
            rust_type(&json!({ "type": "array", "items": { "type": "number" } })),
            "Vec<f64>"
        );
        assert_eq!(
            rust_type(&json!({ "type": "array", "items": { "type": "boolean" } })),
            "Vec<bool>"
        );
        assert_eq!(
            rust_type(&json!({ "type": "array", "items": { "type": "string" } })),
            "Vec<String>"
        );
        // Today's MCP emitter omits `items` entirely. Pinning the degraded
        // result makes the Phase 9a fix visible as a test change.
        assert_eq!(rust_type(&json!({ "type": "array" })), "Vec<String>");
    }

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

    /// A snake_case option key renders a kebab-case flag.
    ///
    /// The previous local `kebab` helper left `_` untouched, so a manifest key
    /// of `dry_run` generated `--dry_run`, which no incurs CLI accepts. Routing
    /// through `incurs::schema::to_kebab` is what makes that unrepresentable.
    #[test]
    fn an_underscored_option_key_renders_a_kebab_flag() {
        let manifest = json!({
            "version": "incur.v1",
            "commands": [{
                "name": "ship",
                "schema": {
                    "options": {
                        "type": "object",
                        "properties": { "dry_run": { "type": "boolean" } }
                    }
                }
            }]
        });

        let source = rust_source(&manifest).unwrap();

        assert!(source.contains("--dry-run"), "flag must be kebab-case");
        assert!(
            !source.contains("--dry_run"),
            "an underscored flag reaches no incurs CLI"
        );
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

    #[test]
    fn a_manifest_of_an_unknown_version_is_rejected() {
        let error = validate_manifest(&json!({ "version": "other.v9", "commands": [] }))
            .expect_err("an unknown manifest version must not generate");

        assert_eq!(error, "target manifest version must be incur.v1");
    }
}
