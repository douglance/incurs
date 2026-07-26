# incurs Code Mode MCP

`incurs-codemode-mcp` exposes the generic Code Mode lifecycle through a real
`rmcp::ServerHandler` with exactly five tools:

- `codemode_search`
- `codemode_execute`
- `codemode_execution`
- `codemode_decide`
- `codemode_cancel`

`codemode_execution` accepts an optional `artifact_id` to retrieve an oversized
value owned by the named execution. Search results include the TypeScript
declarations or snippet source needed to use each match. Execution snapshots
keep oversized values as artifact references, so responses remain bounded and
expose the identifier needed for retrieval.

MCP cancellation is propagated into the active JavaScript pass and then into
incurs tool calls. HTTP transports also pass their request method, path, and
headers into incurs request context.

Use `CodeModeMcpServer` with any compatible `rmcp` server transport, or call
`serve_stdio` for process stdio.
