# incurs-mcp-discovery

Reads the MCP server configuration a developer's coding agents already have, and
normalizes it into one representation with stable identity.

This crate is deliberately inert. It reads files, and that is all: it never
starts a process, resolves a `PATH` entry, opens a network connection, or reads
an environment variable outside [`HostPaths::from_env`]. Everything it returns is
a pure function of file bytes plus that one resolved path set, which is what
makes it testable against a fixture tree.

Configured credentials are separated from configuration at parse time. A value
classified as a secret is held in a `SecretValue`, which implements neither
`Serialize` nor `Deserialize`, so persisting one is a compile error rather than a
review finding.
