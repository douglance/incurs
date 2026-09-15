# Contributing to incurs

This is for working on incurs itself. If you are building a CLI *with* incurs, the
[README](README.md) and `incurs explain` are what you want.

## Gates

Everything below runs in CI on every pull request, and all of it must pass.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo test --workspace --all-features
cargo build --examples --workspace --all-features
```

Two generated files are verified rather than trusted:

```bash
cargo xtask mcp-schema-sync --check   # the pinned MCP schemas and method registries
cargo xtask skill-sync --check        # SKILL.md against the incurs command graph
```

Both have a write mode — drop `--check` — and both fail with the command that
regenerates them. `SKILL.md` is compiled from `incurs explain` and `incurs --llms-full`,
so edit the reference in `crates/incurs-cli/src/explain.rs` and regenerate; never edit
`SKILL.md` directly.

Before a release:

```bash
cargo xtask release-check   # packages every crate and compiles it from its own archive
```

## Repository layout

| Path | What |
| --- | --- |
| `crates/incurs` | The framework: parsing, help, execution, output, transports |
| `crates/incurs-macros` | The `Args`, `Options`, and `Env` derive macros |
| `crates/incurs-cli` | The `incurs` binary, itself built with incurs |
| `crates/incurs-mcp-protocol` | MCP standard registry, codecs, and negotiation policy |
| `crates/incurs-codemode*` | Code Mode lifecycle, local QuickJS executor, MCP facade |
| `crates/incurs-remote` | Remote capability protocol and runtime seam |
| `crates/incurs-extras` | Opt-in output formats |
| `extensions/*` | Standalone workspaces for heavy or platform-specific dependencies |
| `xtask` | Repository automation |

`extensions/` exists for dependencies that should not reach a normal build: `gpui`
links a native windowing backend, and the Cloudflare crates target wasm32. Each is its
own workspace with its own lockfile, and CI runs each on a runner that suits it.
Anything portable belongs in `crates/`.

## Conventions

`AGENTS.md` is the source of truth for project conventions and grows as the project
does — architecture rules, testing rules, and the lessons that earned them. `CONTEXT.md`
is the terminology registry: it classifies concepts by consumer and effect, and names
must expose which kind they are.

A few that catch people out:

- **`ToolCatalog` is the only non-CLI invocation boundary.** MCP, Code Mode, and the
  desktop surface all route through it. Adding a transport means using it, not
  reimplementing dispatch.
- **`run_to` is the one execution path.** `serve` and `serve_with` are process adapters
  over it, and `serve_to` is the buffered form tests use, so a test cannot drift from
  what a process does.
- **A command you define wins over a builtin** of the same name — `completions`, `mcp`,
  `plugin`, `skills`.

## Testing

Assert the whole observation, not selected fields. The CLI surface is pinned by golden
files recording exit code and stdout together; regenerate them deliberately:

```bash
UPDATE_GOLDEN=1 cargo test -p incurs --test cli_surface --all-features
```

Review that diff as the wire-format change it is. For output that is deterministic
apart from a measured duration or a generated id, normalize the dynamic span before
comparing rather than asserting around it.

Test a test suite by breaking what it covers. Flip a constant, invert a comparison,
delete the line that does the work. If nothing goes red, the suite is decorative — and
confirm the edit actually applied before believing either result.

## Releasing

Package versions, internal dependency requirements, all three lockfiles, and the
package list in `xtask` move together; `release-check` fails if they disagree.

Publish order follows dependencies: `incurs-macros`, then `incurs`, then the crates
that depend on it, then `incurs-cli`, and the extension workspaces last. A path
dependency that crosses a workspace boundary resolves from the registry at packaging
time, so an extension crate cannot be packaged until the `incurs` version it requires
is published. `release-check` reports those as deferred rather than failing.

Write the upgrade notes in `MIGRATION.md` for anything a consumer can hit, with a
before and after.
