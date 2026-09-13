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

## Structured output

The `structured` module projects Incurs values into MCP's object-shaped
`outputSchema` and `structuredContent` contracts. Explicit object schemas remain
unchanged. Other schemas and values use a `data` property with the versioned
`io.incurs.outputProjection` metadata marker. Incurs MCP import restores the
original schema and value only when that marker and wrapper shape match.

CLI output, Code Mode, ToolCatalog, and remote runtime values retain their original
shape. The projection preserves schema references, embedded resource boundaries,
and root dialect declarations. Unmarked third-party objects containing `data`
are ordinary objects and are never implicitly unwrapped.
