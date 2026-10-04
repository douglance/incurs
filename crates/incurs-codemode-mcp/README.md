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

`codemode_cancel` cancels the active JavaScript pass and its incurs tool calls.
Starting or approving an execution detaches the pass from the MCP request's
cancellation token, so disconnecting that request does not abandon the durable
execution. HTTP transports also pass their request method, path, and headers
into incurs request context.

Execution responses default to the complete durable state. Use
`with_projection(ExecutionProjection::Reduced)` to withhold intermediate call
payloads and capability schemas while retaining the execution result and step
summaries.

For host-specific output, implement `ExecutionProjector` and install it with
`with_projector(Arc<dyn ExecutionProjector>)`. Its `arguments()` method adds
optional argument properties to the lifecycle tools; existing lifecycle
properties keep their definitions. Its `project()` method receives the tool
name, caller arguments, and serialized execution from successful execute, read,
approve, reject, and cancel calls. The host projector takes precedence over
`with_projection` in either builder order. Search results, retrieved artifacts,
and service errors bypass projection. Installing another projector replaces
the previous projector and its added argument properties.

Use `CodeModeMcpServer` with any compatible `rmcp` server transport, or call
`serve_stdio` for process stdio.
