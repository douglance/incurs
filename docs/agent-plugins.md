# Agent Plugins compatibility

Incurs implements the Agent Plugins 1.0 Working Draft package format as both a publisher and a client. It recognizes only the canonical 1.0 schema identifiers and bundles the official JSON schemas for offline use. Loading never fetches a schema URL.

## Package conformance

| Requirement | Incurs behavior |
| --- | --- |
| Root manifest | Loads root `plugin.json`; a fatal manifest failure returns no components. |
| Closed manifest fields | Diagnoses and ignores unknown root fields. |
| Extensions | Preserves object-valued extension namespaces; diagnoses and ignores non-object values. |
| Agent Skills | Scans only immediate `skills/<name>/SKILL.md` children, validates Agent Skills frontmatter and directory-name agreement, and skips invalid skills independently. |
| MCP document | Loads only root `mcp.json`; a top-level MCP document failure disables MCP without invalidating the plugin or skills. |
| MCP servers | Validates each closed server entry independently and skips only invalid entries. |
| Filesystem containment | Canonicalizes package files and plugin-root cwd paths, rejects escapes, and rechecks data-directory cwd containment before launch. |
| Publisher | Writes deterministic `plugin.json`, Agent Skill directories including authored resources, and `mcp.json` with any mix of the three portable transports. `--bundle-cli` also copies the current target-specific executable into `bin/`. |

## Package taxonomy

| Kind | Package representation | Consumer and effect |
| --- | --- | --- |
| Prompt Artifact | `skills/<name>/SKILL.md` and its resources | Model-directed guidance; it may reference capabilities but cannot invoke a handler. |
| Tool Binding | Root `mcp.json` | Machine-readable MCP connection configuration. |
| Tool Runtime | `bin/<command>` plus `io.github.douglance.incurs.toolRuntime` metadata | Native executable installed for one operating system and architecture. |

Deleting every Prompt Artifact leaves the Tool Binding and Tool Runtime directly callable. Changing prompt text does not change tool identity, policy, or contract digests.

## One-install bundles

Build a self-contained package from a CLI that exposes the built-in plugin command:

```bash
my-cli plugin build --bundle-cli --output ./dist/my-cli-plugin
```

The standalone generator forwards the same behavior with `--plugin-bundle-cli`. A bundled directory has this shape:

```text
my-cli-plugin/
|-- plugin.json
|-- mcp.json
|-- bin/
|   `-- my-cli
`-- skills/
    `-- my-cli/
        `-- SKILL.md
```

The bundle is target-specific. Its private manifest extension records the runtime path, shell command, operating system, and architecture. Incurs validates those fields and every portable Agent Plugins component before changing an installation.

```bash
incurs plugin install ./dist/my-cli-plugin
incurs plugin uninstall my-cli
incurs plugin uninstall my-cli --purge
```

Install accepts a local directory. It rejects symlinks, special files, platform mismatches, malformed runtime metadata, and command collisions. `--force` may replace only the same managed plugin and never an unrelated command. The installer stages replacement files and restores the prior installation if commit fails.

Installed files use platform user directories. Set `INCURS_DATA_HOME` or `INCURS_BIN_DIR` to choose explicit roots. The JSON install result includes the plugin root, persistent data root, command path, and `pathReady`. If the command directory is absent from `PATH`, Incurs emits an exact warning but does not edit shell profiles. Uninstall verifies the managed command digest before removal and preserves plugin data unless `--purge` is passed.

Incurs does not rewrite Claude, Codex, or other legacy agent configuration. Agent clients that implement Agent Plugins remain responsible for discovering the installed plugin directory.

## MCP transport conformance

| Transport | Launch or connection behavior |
| --- | --- |
| `stdio` | Accepts one bare executable token or a contained `./` path. Launches direct argv without a shell. Expands `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` once in args, env values, and cwd. Applies client base env, plugin overlay, then reserved variables. Creates and preserves `PLUGIN_DATA`. |
| `streamable-http` | Requires an absolute safe HTTP(S) URL, with HTTPS outside loopback. Applies validated visible headers while MCP client headers win. Redirects are disabled so configured headers cannot cross origins. |
| `sse` | Implements the MCP 2024-11-05 GET event stream plus POST endpoint flow. Supports relative endpoint events. Configured headers follow only same-origin endpoint events; unsafe endpoint URLs fail that server. |

Connection, authentication, startup, and MCP handshake failures are recorded per server. Connected tools are grouped under the `mcpServers` key before Incurs builds the final `ToolCatalog`, so equal tool names from different servers do not collide.

## Failure boundaries

```text
plugin.json fatal
  -> no plugin, skills, extensions, or MCP servers

plugin.json valid
  +-> each invalid skill is skipped
  +-> invalid mcp.json top level disables MCP only
  +-> each invalid MCP server is skipped
  +-> each runtime connection failure skips that server only
  `-> connected server tools remain callable
```

## Client policy outside the portable format

The portable specification does not standardize installation, update discovery, trust prompts, process sandboxing, secret storage, or OAuth configuration. Incurs therefore keeps installer metadata in its namespaced manifest extension and keeps the remaining concerns out of `plugin.json` and `mcp.json`:

- Callers choose the dedicated persistent data directory.
- The runtime accepts an explicit base subprocess environment; its convenience default inherits the current process environment.
- Configured MCP headers are treated as visible values and are never described as secrets.
- No portable OAuth behavior is inferred from remote server configuration.
- Subprocess containment protects path resolution; it is not an operating-system sandbox.

Use `incurs plugin validate <path> --data-dir <path>` for the structured load report, `incurs plugin tools <path> --data-dir <path>` for per-server connection results plus namespaced tool definitions, and `incurs plugin call <path> <tool> --arguments <json> --data-dir <path>` to execute one tool through the shared `ToolCatalog`.
