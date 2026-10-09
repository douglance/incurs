# incurs-mcp-apps

Provider-neutral MCP App primitives for Rust and browser-hosted WASM clients.

The crate keeps the MCP App lifecycle and transport shape separate from OpenAI-specific helpers. Host-specific extensions can layer on top of `McpApp` without owning JSON-RPC request correlation, lifecycle state, listener disposal, or host-context updates.
