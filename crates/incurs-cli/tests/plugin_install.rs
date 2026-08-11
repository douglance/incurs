#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "incurs-plugin-install-e2e-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    root
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn installed_bundle_launches_and_calls_its_mcp_runtime() {
    let root = temp_root();
    let source = root.join("source");
    let runtime = source.join("bin/demo-tools");
    write(
        &source.join("skills/demo-tools/SKILL.md"),
        "---\nname: demo-tools\ndescription: Call the demo tools.\n---\n\nCall the ping tool.\n",
    );
    write(
        &runtime,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n -E 's/.*"id":("[^"]*"|-?[0-9]+).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","supportedVersions":["2026-07-28","2025-03-26"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}}\n' "$id"
      ;;
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{"tools":{}},"protocolVersion":"2025-03-26","serverInfo":{"name":"fixture","version":"1.0.0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"description":"Return pong","inputSchema":{"type":"object","properties":{}},"name":"ping"}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"{\\"message\\":\\"pong\\"}"}],"isError":false,"structuredContent":{"message":"pong"}}}\n' "$id"
      ;;
  esac
done
"#,
    );
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    write(
        &source.join("plugin.json"),
        &serde_json::to_string_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": "demo-tools",
            "version": "1.0.0",
            "extensions": {
                "io.github.douglance.incurs": {
                    "toolRuntime": {
                        "arch": std::env::consts::ARCH,
                        "os": std::env::consts::OS,
                        "path": "./bin/demo-tools",
                        "shellCommand": "demo-tools"
                    }
                }
            }
        }))
        .unwrap(),
    );
    write(
        &source.join("mcp.json"),
        &serde_json::to_string_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {
                "fixture": {
                    "type": "stdio",
                    "command": "./bin/demo-tools"
                }
            }
        }))
        .unwrap(),
    );

    let data_home = root.join("data-home");
    let bin_dir = root.join("commands");
    let install = Command::new(env!("CARGO_BIN_EXE_incurs"))
        .args(["plugin", "install"])
        .arg(&source)
        .arg("--bin-dir")
        .arg(&bin_dir)
        .env("INCURS_DATA_HOME", &data_home)
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let installed: Value = serde_json::from_slice(&install.stdout).unwrap();
    let plugin_root = installed["pluginRoot"].as_str().unwrap();
    assert!(
        Path::new(plugin_root)
            .join("skills/demo-tools/SKILL.md")
            .is_file()
    );

    let call = Command::new(env!("CARGO_BIN_EXE_incurs"))
        .args(["plugin", "call", plugin_root, "fixture_ping"])
        .args(["--arguments", "{}", "--data-dir"])
        .arg(root.join("runtime-data"))
        .output()
        .unwrap();
    assert!(
        call.status.success(),
        "{}",
        String::from_utf8_lossy(&call.stderr)
    );
    let outcome: Value = serde_json::from_slice(&call.stdout).unwrap();
    assert_eq!(outcome["data"], json!({ "message": "pong" }));

    let _ = fs::remove_dir_all(root);
}
