# incurs-mcp-protocol

Provider-neutral MCP standard profiles and negotiation for incurs.

The crate keeps exact modules for the five official published standards:

- `2024-11-05`
- `2025-03-26`
- `2025-06-18`
- `2025-11-25`
- `2026-07-28`

Profiles share sealed legacy and modern wire codecs while retaining their own
method registries, feature flags, and pinned official JSON Schema. Hosts choose
an ordered `McpStandardSet`; clients and servers negotiate one exact immutable
profile for each interaction.

Tasks and Apps are intentionally excluded from the generated core method
surface. They belong in optional extension crates.
