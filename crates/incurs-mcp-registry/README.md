# incurs-mcp-registry

Turns the MCP servers a machine already has configured into one set of incurs
Code Mode connectors.

It collapses the same server declared by several agents into a single connector,
assigns each one a JavaScript namespace that stays stable as servers come and
go, resolves approval policy from the servers' own behavioural annotations, and
keeps one failing server from taking down the rest.
