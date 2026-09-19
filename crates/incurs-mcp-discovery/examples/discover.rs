//! Prints the MCP servers configured on this machine.
//!
//! Reading only: no server is started and no credential is printed.

use std::collections::BTreeMap;

use incurs_mcp_discovery::{HostPaths, McpTransport, discover};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = HostPaths::from_env()?;
    if let Some(project) = std::env::args().nth(1) {
        paths = paths.with_project(project);
    }
    let found = discover(&paths);

    let mut by_id: BTreeMap<String, Vec<&incurs_mcp_discovery::DiscoveredMcpServer>> =
        BTreeMap::new();
    for server in &found.servers {
        by_id
            .entry(server.id.as_str().to_string())
            .or_default()
            .push(server);
    }

    println!(
        "{} entries -> {} distinct servers",
        found.servers.len(),
        by_id.len()
    );
    for (id, group) in &by_id {
        let first = group[0];
        let kind = match &first.transport {
            McpTransport::Stdio(stdio) => format!("stdio {:?}", stdio.program),
            McpTransport::StreamableHttp(http) => format!("http {}", http.canonical_url),
            McpTransport::Sse(http) => format!("sse {}", http.canonical_url),
        };
        let hosts: Vec<&str> = group
            .iter()
            .map(|server| server.source.discovery.slug())
            .collect();
        println!(
            "  {}  {:<28} {:<62} [{}]{}",
            &id[..6],
            first.local_name,
            kind,
            hosts.join(", "),
            if first.enabled { "" } else { " DISABLED" }
        );
    }
    if !found.diagnostics.is_empty() {
        println!("\ndiagnostics:");
        for diagnostic in &found.diagnostics {
            println!(
                "  {:?} {} {}",
                diagnostic.severity, diagnostic.code, diagnostic.path
            );
        }
    }
    if !found.unreadable.is_empty() {
        println!("\nunreadable:");
        for (path, reason) in &found.unreadable {
            println!("  {} — {reason}", path.display());
        }
    }
    Ok(())
}
