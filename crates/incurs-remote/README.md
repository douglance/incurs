# incurs-remote

Provider-neutral remote capability protocol and runtime seam for Incurs tools.

This crate defines serializable remote tool calls, results, errors, artifact
handles, capability manifests, and a `RemoteToolRuntime` trait. It also includes
a `ToolCatalogRemoteRuntime` adapter so an existing `incurs::tool::ToolCatalog`
can be exposed without adding provider-specific schemas, authentication, or
device assumptions.
