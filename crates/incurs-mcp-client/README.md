# incurs-mcp-client

The production `McpClient` for incurs Code Mode: it connects to MCP servers that
a developer's agents already have configured, and it does so as late as possible.

A configured server is not a running server. Constructing a client starts
nothing; the process or connection is created on the first call that genuinely
needs it. Tool schemas come from a persistent cache keyed by a configuration
fingerprint, so listing capabilities for twenty servers normally costs no
processes at all.

All downstream I/O is moved onto a bridged multi-threaded runtime, because Code
Mode's local executor runs on a current-thread runtime built without an I/O
driver.
