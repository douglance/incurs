# incurs — Agent Guidelines

> **Update after learnings or mistakes** — when a correction, new convention, or hard-won lesson emerges during development, append it to the relevant section of this file immediately. AGENTS.md is the source of truth for project conventions and should grow as the project does.

## Documentation Conventions

- **Doc comments on all public items** — every public module, function, type, field, and variant gets a `///` or `//!` comment. `cargo doc --workspace --all-features --no-deps` runs with `RUSTDOCFLAGS=-D warnings` in CI, so a missing or broken doc link fails the build. Doc-driven development: write the comment before or alongside the implementation, not after.
- **Documentation describes this implementation** — incurs is no longer a port. Do not anchor a doc comment, a test name, or a design decision to another implementation's behavior; anchor it to a test in this repository.

## Architecture Conventions

- **Prompt and tool taxonomy is explicit** — classify concepts by consumer and effect, not by whether they are stored as text. Use the preferred terms and rules in `CONTEXT.md`: target-neutral meaning is a Capability; model-directed content is a Prompt Guide or Prompt Artifact; machine invocation belongs to Tool Contracts, Tool Bindings, and Tool Runtimes; human-only material is Documentation. Adapters convert representations, and Publishers install compiled artifacts.
- **Prompt and tool seams stay one-way** — Prompt modules may reference `CapabilityId` but never receive handlers. Tool modules must not contain prompt programs, selection heuristics, or few-shot demonstrations. Prompt changes must not alter Tool identity, policy, or contract digests, and deleting all Prompt modules must leave every Tool directly callable. Split any type that spans these kinds.
- **Agent Plugins is a publication target** — compile root `plugin.json` metadata, Agent Skills prompt artifacts, and `mcp.json` tool bindings from distinct typed inputs. Keep the Agent Plugins portable manifest closed, put client-specific behavior under reverse-domain extension namespaces, and never use a client-native plugin layout as the portable format.
- **Code Mode is platform-neutral** — `incurs-codemode` owns the lifecycle, dispatch, replay, approval, rollback, and shared JavaScript program contract. Executors supply isolation and host bridging. Keep provider names, dependencies, configuration, documentation, tests, and runtime assumptions inside standalone workspaces under `extensions/`.
- **`ToolCatalog` is the non-CLI invocation boundary** — MCP, Code Mode, and future transports resolve command metadata and execute through `ToolCatalog`. Preserve canonical command paths for config lookup and middleware context. `ParseMode::Flat` does not apply config defaults, so merge resolved command defaults into structured arguments before `command::execute`.
- **Tool cancellation covers active commands** — race the shared `command::execute` future against `ToolCallControl::cancellation`; a pre-invocation check and stream-only cancellation do not stop an ordinary asynchronous command.
- **CLI option boundaries are absolute** — built-in and custom global extraction must stop at the first literal `--`, preserve that separator for the command parser, and never inspect later tokens.
- **Local Code Mode uses an actor boundary** — QuickJS execution is non-`Send`. Construct and drive it on a dedicated current-thread runtime, return the durable running state before the pass begins, and keep lifecycle requests responsive so cancellation can interrupt active code.
- **Approval is the only transition out of an approval pause** — a generic resume operation must reject executions with pending actions. Only the atomic approve winner may move that execution back to running before replay.
- **Code Mode dispatch requires an active pass** — never construct a fallback tool context for public dispatch. Calls and durable steps are valid only while an executor owns the active pass capability.
- **Terminal transitions are state guarded** — completion and failure only replace a running state. Rejection only applies to a paused pending action, and rollback requires a terminal execution with no unfinished actions.
- **Detached execution uses the host lifetime primitive** — adapters that return a durable running state before execution completes must register the pass with their host's request-lifetime mechanism.
- **Streamable HTTP validates before dispatch** — MCP HTTP adapters must enforce method, Origin, Accept, content type, protocol-version, JSON-RPC, and initialization requirements before invoking shared tool dispatch. Partition durable state by an authenticated tenant boundary; a singleton object is only acceptable for an explicitly local fixture.

## Testing Conventions

- **The CLI surface is pinned by full-observation goldens** — `crates/incurs/tests/cli_surface.rs` records exit code and stdout together, not selected fields. Regenerate with `UPDATE_GOLDEN=1` and review the diff as the wire-format change it is.
- **Normalize dynamic values, do not assert around them** — for output that is deterministic apart from a measured duration or a generated id, replace the dynamic span with a stable marker before comparing, so the rest of the observation stays pinned.
- **Extension workspaces are gated per workspace** — `.github/workflows/rust.yml` runs each `extensions/*` workspace on its own runner with its own targets. A workspace that cannot build for `wasm32-unknown-unknown`, or that needs a platform runner, declares that in the matrix rather than being excluded from CI.
- **Builtin CLI behavior uses one active runtime path** — `serve()` and `serve_with()` are process adapters over `run_to()`. Implement and test built-in behavior through `run_to()`/`serve_to()` so process execution and integration tests cannot drift.
- **MCP HTTP tests need a valid Host** — current `rmcp` validates hosts before request dispatch. Direct requests to the Rust MCP HTTP service must include a loopback `Host` header (for example, `localhost`) unless the test is specifically exercising DNS-rebinding rejection.

## Git Conventions

- **Conventional commits** — use `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:` prefixes. Scope is optional (e.g. `feat(parser): add array coercion`).
- **Release archives are fresh, locked, and version-aligned** — bump publishable package manifests, their internal dependency requirements, lockfiles, and `xtask release-check` together. The release checker must remove each expected archive before packaging and package with `--locked` so stale archives cannot satisfy verification and release checks cannot rewrite tracked lockfiles.
