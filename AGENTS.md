# incurs — Agent Guidelines

> **Update after learnings or mistakes** — when a correction, new convention, or hard-won lesson emerges during development, append it to the relevant section of this file immediately. AGENTS.md is the source of truth for project conventions and should grow as the project does.

## Documentation Conventions

- **Doc comments on all public items** — every public module, function, type, field, and variant gets a `///` or `//!` comment. `cargo doc --workspace --all-features --no-deps` runs with `RUSTDOCFLAGS=-D warnings` in CI, so a missing or broken doc link fails the build. Doc-driven development: write the comment before or alongside the implementation, not after.
- **Documentation describes this implementation** — incurs is no longer a port. Do not anchor a doc comment, a test name, or a design decision to another implementation's behavior; anchor it to a test in this repository.

## Architecture Conventions

- **Use literal component names** - name the OpenAPI compiler and its Worker proof for their function; keep package, module, workspace, error, and CI names aligned.

- **Required and nullable are independent** — generated nullable path arguments retain value, null, and default states but reject missing. Convert presence before storing a required path value, and give required nullable fields a value in generated smoke tests. Ordinary path serialization rejects null; JSON-content parameters retain their explicit null encoding.

- **Generated Rust names share a symbol table** — schema types, operation argument/response/body types, and support types occupy one Rust type namespace. Allocate their identifiers once and resolve references through that map; preserve canonical schema keys and original wire names. Reserve unqualified prelude constructors such as `Some`, `None`, `Ok`, `Err` as well as helper types. Client and default-accessor methods use their own scopes. Full-corpus compilation exposed collisions that isolated operations could not.

- **Request schemas enforce the outbound boundary** — compile one version-aware graph shared by discovery and HTTP invocation, retain only reachable input definitions, and validate the selected media schema before transport. Keep raw binary separate from logical JSON values. Normalize only empty unused SDK parameter groups; never drop populated groups or body values. Preserve legacy read-only requirements and reference-sibling semantics. Prove invalid values cause zero outbound CLI/MCP HTTP and fail in the running Worker; compilation alone missed the SDK empty-group mismatch.

- **Generated validation shares one compiled graph** — retain the source OpenAPI version and declared dialect, normalize legacy nullable and reference-sibling semantics explicitly, and validate composed values through one offline graph. Preserve selected-branch checks separately from parent union checks. Reject unsupported resource scopes instead of removing their meaning; keep recursive, exact-integer, duplicate-key, and confirmed expansion-mutation controls.

- **Schema references stay a graph across consumers** — retain shared definitions and canonical references instead of copying targets into every operation. Pass the owning schema table to HTTP binding, preserve referenced metadata and SDK defaults/nullability, and restrict tool discovery to reachable definitions. Guard serialized growth with a repeated-reference fixture and a confirmed expansion mutation; compare retained component schemas directly with pinned source inputs.

- **OpenAPI servers are operation contracts** — retain effective operation/path/root server lists and variable defaults. Declared-server binding selects from that contract; an explicit host URL remains a separate override. Relative server URLs require their declaring document URL, never the process working directory.

- **Prompt and tool taxonomy is explicit** — classify concepts by consumer and effect, not by whether they are stored as text. Use the preferred terms and rules in `CONTEXT.md`: target-neutral meaning is a Capability; model-directed content is a Prompt Guide or Prompt Artifact; machine invocation belongs to Tool Contracts, Tool Bindings, and Tool Runtimes; human-only material is Documentation. Adapters convert representations, and Publishers install compiled artifacts.
- **Prompt and tool seams stay one-way** — Prompt modules may reference `CapabilityId` but never receive handlers. Tool modules must not contain prompt programs, selection heuristics, or few-shot demonstrations. Prompt changes must not alter Tool identity, policy, or contract digests, and deleting all Prompt modules must leave every Tool directly callable. Split any type that spans these kinds.
- **Agent Plugins is a publication target** — compile root `plugin.json` metadata, Agent Skills prompt artifacts, and `mcp.json` tool bindings from distinct typed inputs. Keep the Agent Plugins portable manifest closed, put client-specific behavior under reverse-domain extension namespaces, and never use a client-native plugin layout as the portable format.
- **Core stays wasm32-buildable** — Cloudflare Workers build `incurs` with `default-features = false` for `wasm32-unknown-unknown`. A dependency that needs a process, stdio, threads, or OS randomness goes in the `cfg(not(target_arch = "wasm32"))` target table, and the code that uses it is gated the same way with a coded error on wasm32. Making a dependency unconditional is not the same as making it portable; check the Cloudflare extension's wasm32 gate after any core dependency change. Compiling is not running: `std::time` clocks, `std::env::vars`/`vars_os` (they panic, they do not return empty), `std::env::temp_dir`, and `std::process::exit` compile for wasm32 and fail when called. Use `web_time`, read the process environment only through `crate::process_env` (clippy's `disallowed-methods` enforces it), take the environment from the host, and prove behaviour with `extensions/cloudflare/examples/feature-worker/smoke.sh` — a compile check and a code review both missed the `vars` panic; only the live run caught it.
- **Every feature builds alone** — CI checks each Cargo feature by itself on native and wasm32. `--all-features` hid an `http` feature that only compiled alongside `agent-plugins-mcp`.
- **Code Mode is platform-neutral** — `incurs-codemode` owns the lifecycle, dispatch, replay, approval, rollback, and shared JavaScript program contract. Executors supply isolation and host bridging. Keep provider names, dependencies, configuration, documentation, tests, and runtime assumptions inside standalone workspaces under `extensions/`.
- **`ToolCatalog` is the non-CLI invocation boundary** — MCP, Code Mode, and future transports resolve command metadata and execute through `ToolCatalog`. Preserve canonical command paths for config lookup and middleware context. `ParseMode::Flat` does not apply config defaults, so merge resolved command defaults into structured arguments before `command::execute`.
- **Facade registration preserves existing commands** — check both advertised tool names and canonical command paths before staging a registration. A unique facade-local name can still replace a base CLI command during materialization. Keep a regression that rejects the collision and invokes the original command, plus a confirmed mutation of the collision check.

- **Tool cancellation covers active commands** — race the shared `command::execute` future against `ToolCallControl::cancellation`; a pre-invocation check and stream-only cancellation do not stop an ordinary asynchronous command.
- **CLI option boundaries are absolute** — built-in and custom global extraction must stop at the first literal `--`, preserve that separator for the command parser, and never inspect later tokens.
- **Local Code Mode uses an actor boundary** — QuickJS execution is non-`Send`. Construct and drive it on a dedicated current-thread runtime, return the durable running state before the pass begins, and keep lifecycle requests responsive so cancellation can interrupt active code.
- **Approval is the only transition out of an approval pause** — a generic resume operation must reject executions with pending actions. Only the atomic approve winner may move that execution back to running before replay.
- **Code Mode dispatch requires an active pass** — never construct a fallback tool context for public dispatch. Calls and durable steps are valid only while an executor owns the active pass capability.
- **Terminal transitions are state guarded** — completion and failure only replace a running state. Rejection only applies to a paused pending action, and rollback requires a terminal execution with no unfinished actions.
- **Detached execution uses the host lifetime primitive** — adapters that return a durable running state before execution completes must register the pass with their host's request-lifetime mechanism.
- **Streamable HTTP validates before dispatch** — MCP HTTP adapters must enforce method, Origin, Accept, content type, protocol-version, JSON-RPC, and initialization requirements before invoking shared tool dispatch. Partition durable state by an authenticated tenant boundary; a singleton object is only acceptable for an explicitly local fixture.

- **MCP errors preserve machine-readable details** — retain the stable code, retryability, exit code, and field errors at the MCP boundary, with isError true. Keep the first text block valid JSON even when follow-up guidance is present; test both structured and text-only replies.

- **Emscripten tooling stays scoped to its proof workspace** — use `extensions/openapi-workers/build.py` to select the pinned Rustup binaries and Cargo build layout. PATH wrappers intercepted build-script compiler probes, and the newer Cargo layout placed wasm-bindgen snippets where the pinned worker-build could not bundle them. Keep these accommodations out of the core and verify the resulting bundle through the HTTP probe.

- **Remote catalogs have independent bounds** — cap empty advancing pages and total tools, start deadlines before transport locks, bound complete discovery, and limit accumulated SSE data. Keep independent controls for empty pages, a held legacy stream lock, and a slow multi-page catalog; per-request deadlines alone do not bound discovery.
- **MCP rich content follows the negotiated standard** — audio starts in 2025-03-26 and resource links in 2025-06-18. Preserve the JSON text fallback and prove native/portable direct and progressive calls across every served era.

## Testing Conventions

- **Subscription replay and live delivery form one boundary** - register and buffer live changes before replay, then reconcile the overlap by stable source identity. A deterministic append during replay must reach the subscriber exactly once.
- **Subscriber limits bound pending data** - account for aggregate queued events and bytes, release the accounting when a reader consumes data, and clean up timers and listeners on overflow, cancellation, and error. Prove both a healthy reader beyond the event limit and a blocked reader that reaches the limit.
- **Subscription proof uses actual resource data** - a successful subscribe response, a static HTML marker, or an event metadata display does not establish delivery. Read authoritative product state, change it through an independent writer, and verify the same rendered view updates.

- **App resource wrappers retain MCP fields** — preserve request and result metadata, loose resource fields, and notification parameters across typed models. Validate the selected text or blob payload, and keep unparsed OpenAI decorations in raw metadata without blocking otherwise valid resource content. Keep independent literal controls for metadata overlays, optional decorations, and resource payload alternatives.

- **Portable peer responses require trusted session ownership** — the host supplies an authenticated client-session scope for the initiating call and peer responses. Request IDs and caller-supplied headers establish no authority. Pin anonymous and cross-session rejection plus owning-session completion through both the core server and Axum adapter.

- **Browser JSON uses the JSON codec** — arbitrary-precision serde numbers expose an internal representation to general Serde serializers. Encode the frame with serde_json before converting it to a JavaScript object, and test outgoing nested numbers and metadata against an independent JavaScript host with arbitrary precision enabled.

- **MRTR field presence stays explicit** — mutable JSON indexing can insert content: null into an intermediate result. Use non-inserting lookups and assert that InputRequired results omit content.

- **Form numbers stay exact through typed boundaries** — preserve number tokens and typed minimum/maximum values rather than passing through f64. Keep Python helper integer-token and timestamp rules separate from general form validation, with independent positive and negative controls.


- **Capture one formatter output at a time** — when migrating rustfmt output through griz, pass source on stdin with skip_children=true. Formatting a file path may print several child-module files with filename headers; never treat that concatenation as the replacement for one file. Verify the resulting diff before compilation.

- **Schema oracles distinguish schemas from factories** — an exported name ending in Schema may be a function that builds a schema. Give every compared schema a positive control, and test form-content factories with a declared form and an independent answer corpus. Include nested array elements and Unicode scalar length controls; an object-only mutator misses invalid content blocks and UTF-16 length differences.

- **Verification never rewrites source lockfiles** — use --locked in real workspaces and examples. Resolve changed manifests in a disposable mirror, then migrate the resulting lockfile through griz. A dependency change requires new package archives and a consumer built from those extractions.

- **Noninteractive searches name their roots** - pass explicit files or directories to `rg` under apoc. Its retained stdin can otherwise make a source search wait for input.

- **Signal handlers request cleanup; the main loop performs it** — set a stop flag in the handler and reap the child after its active wait returns. Calling wait recursively from a signal handler can hold the child wait lock and end in an exception. Require the expected signal exit code, empty stderr, and a closed grandchild listener in the cleanup control.

- **Parsing fixtures select their host directories explicitly** — inject an isolated VS Code user directory when testing JSONC parsing. Platform-default path discovery is a separate contract; a macOS-only directory made a parsing fixture invisible on Linux.

- **Worker fixture cleanup owns a process group** — launch Wrangler in a dedicated session and terminate that group with bounded waits before reusing its port. Use available fixture ports and forward their URLs into the Worker; a parent-only kill left workerd listening during the authenticated restart.

- **Source fixtures compare logical newlines** — normalize CRLF in checked-in expected source before a complete generated-source comparison. Preserve full compile-failure snapshots and refresh compiler location rendering when the diagnostic format changes; Windows checkout conversion and a newer Rust diagnostic are platform differences, not reasons to weaken the assertions.

- **MCP schema relocation preserves dialect meaning** - wrapping non-object output changes reference scope. Wrap only supported declared dialects, reject unsupported relocation with a coded error before constructing either MCP server, and retain object roots unchanged. Keep declared 2019 recursive-schema rejection and 2020 positive controls, plus a confirmed guard-bypass mutation.

- **Cold caches are part of Worker proof** - the first locked native OpenAPI build must fetch its complete feature graph before later offline archive packaging. The base Worker disables native adapters and does not populate those dependencies; run both HTTP probes on CI without assuming a developer cache.

- **Generated future lints stay at their boundary** - async_trait annotates generated futures with must_use. Scope the double_must_use compatibility allowance to the affected trait declarations and retain warnings-denied Clippy across the workspace.


- **Browser proof uses an independent host** — send plain JSON-RPC objects across `postMessage`. A Rust-to-Rust fixture can accept shared serialization mistakes such as JavaScript Maps. Keep host method strings and notification envelopes independent of SDK constants, and confirm an implementation mutation makes the browser gate fail.

- **App notification listeners receive params** — decode the transport's parameter value and reconstruct the public notification envelope at the helper boundary. Injecting a full envelope into an in-memory listener masked dropped resource notifications in the browser.

- **Typed results preserve numeric schemas** — a JSON Schema number is independent of its domain label. JavaScript sends whole numbers as floating-point values; retain numeric result tokens instead of narrowing them to unsigned integers. Exercise integer, fractional, and negative host controls when the declared schema permits them.

- **Response media checks stop at the response value** — skip binary-annotation discovery for JSON media before traversing schemas. A binary child property, array item, or unused definition does not make its container a binary response. Follow only value-level references and compositions with a visited set; profiling the full Stripe document exposed repeated traversal through unrelated JSON properties.

- **Nullable read models retain their original root schema** — removing null to choose a Rust payload type must not remove null from response validation. JSON Schema type arrays may contain several non-null types; preserve that union and exercise nullable object roots as well as scalar roots.

- **Wire names remain data in generated comments** — escape control characters before embedding field or parameter names in Rust documentation comments. Preserve the original name in serializers and decoders, and compile a packaged fixture with newline, carriage return, and NUL in a JSON property name.

- **Binary response limits retain reference intersections** — a metadata view may select a codec, but replacing a referenced bound with a sibling changes validation. Intersect min/max byte lengths across references and allOf, and keep a packaged test in which a looser sibling cannot admit an oversized body.

- **Recursive response compositions reuse model identities** — anonymous allOf fields can lead back to a shared definition. Intern their schema identity instead of allocating a new Rust type for each visit, and box recursive model references. Keep a bounded identity-reuse regression and a packaged recursive response consumer; a small direct-reference fixture did not expose the full Cloudflare schema expansion.

- **Response presence starts from wire bytes** - JSON bytes containing null must become the typed null state after schema validation. Test literal null bytes as well as pre-parsed Field::Null; a pre-parsed-only control missed a string projection failure after otherwise valid JSON parsing.

- **Exact numeric fixtures start as exact text** - do not construct integers beyond 2^53 in a JavaScript number before serializing a fixture. Use exact decimal source text or an arbitrary-precision host integer, and compare the persisted boundary with its independent literal control; a rounded fixture maximum accepted the adjacent integer while the decoder was correct.

- **Response families need executable dispatch proof** - emitting a Status4XX variant does not make it reachable. Generated classification must prefer exact codes, then inclusive 1XX through 5XX ranges, then default or Other. Exercise the complete u16 domain against an independent arithmetic oracle through packaged native and Worker consumers; keep literal HTTP response controls and confirmed endpoint, precedence, and fallback mutations.

- **Media projections preserve complete constraints** — discovering a text alternative in a composed binary schema must not flatten or overwrite its validation keywords. Intersect the original schema with the adapter representation and verify conflicting branch constraints with an independent validator.

- **Request media chooses the codec before schema annotations** — JSON and text media keep their wire semantics even with a binary annotation. Raw binary and binary NDJSON use an exclusive base64 adapter slot; JSON Lines must reject blank records. Prove these through the public HTTP binder and a packaged generated consumer, since codec-only tests missed precedence, slot, and framing defects.

- **Generated field names are allocated once** — reserve natural names and request-body fields before choosing collision suffixes. Reuse the same allocation in declarations, defaults, serialization, and generated smoke tests; keep original wire names and parameter locations. Prove punctuation, keyword, Unicode, cross-location, and body-name collisions through a packaged consumer and a confirmed wrong-field wire mutation.

- **Bound nested Cargo test builds** — generated-consumer tests launch their own Cargo builds. Run the OpenAPI suite with --test-threads=1 under a process-family memory limit; parallel consumer builds exceeded 4 GiB even though each child used -j2. Preserve all consumer assertions when bounding concurrency.

- **Accepted test sockets must set their intended mode** — a nonblocking listener can produce nonblocking accepted sockets on macOS. Set the accepted stream to blocking before applying read/write timeouts; otherwise a scheduling gap can turn a valid HTTP exchange into WouldBlock.

- **Constraint-only composition branches do not imply a type** — a required-only schema accepts non-objects unless another schema constrains the type. Generated oneOf serialization must validate both the selected branch and the complete parent schema. Keep independent positive and negative controls, including a mutation that disables parent-property validation.

- **HTTP method capitalization is part of the contract** — OpenAPI additionalOperations keys carry their exact wire capitalization. Preserve it through resolution, generated SDKs, ToolCatalog, and HTTP; an uppercase normalization mutation must fail a literal request-line check.

- **Composition types preserve validation semantics** — selecting a oneOf enum variant does not prove exactly one schema matches, and merging allOf fields does not remove branch-local restrictions. Validate the serialized value against the selected branch and the original composition, preserve exact integer comparisons, reject unsupported constraints, and include overlapping-branch and closed-object negative controls in a packaged generated consumer.

- **Multipart proof includes a separate parser** — check actual HTTP bytes with an independent MIME parser as well as literal wire expectations. Preserve referenced Encoding Object headers, avoid payload boundary collisions, and keep transfer-encoded text separate from JSON serialization.

- **Form encodings preserve field presence** — retain Encoding Object metadata separately from schemas. An explicit style, explode, or allowReserved field selects style-based serialization; replacing absent fields with defaults changes content-based JSON properties. Prove encoded wire bytes through both generated consumers and the automatic tool path.

- **Tool discovery is a schema artifact** — validate emitted input and output schemas independently of successful invocation. Empty allOf/anyOf/oneOf lists are invalid schemas; omit unused combinators. CLI and MCP process probes must follow the interface the running server advertises, including progressive search/inspect/call routing.

- **Generated numeric defaults retain their number text** — a fallback through f64 rounded defaults beyond i64/u64 and rejected large exponents. Preserve numeric tokens recursively, validate the complete JSON number grammar before encoding, and keep packaged HTTP and live Worker controls for precise decimals, extreme exponents, malformed tokens, and exact schema bounds.

- **Generated integers must reach the wire exactly** — exercise signed limits and IDs above 2^53 through a packaged generated consumer. A floating-point intermediary can silently round valid IDs even when source generation and compilation pass.

- **Parallel test scratch paths need an independent sequence** — wall-clock nanoseconds can repeat across threads. Combine the process and timestamp with an atomic sequence, and install cleanup ownership only after directory creation succeeds.

- **Model migrations target the enclosing type** — common field names such as path appear in unrelated types. Restrict added fields to the intended struct constructor and compile all targets; a focused integration test can miss a broken artifact fixture.

- **OpenAPI acceptance is stage-specific** — validate compatibility fixtures independently, check resolution, SDK generation, and wire semantics separately, and keep positive controls. A resolver or generator accepting a document does not prove preservation of server overrides, serialization, or schema semantics.

- **OpenAPI-to-tool proof must be continuous** — a generated SDK calling HTTP and a separate adapter calling a handwritten constant-result handler do not prove OpenAPI-to-incurs execution. Exercise the resolved operation through ToolCatalog to a real server, assert the observed wire request independently, and distinguish explicit host registration from generated integration.

- **Packaged consumer proof uses the archive** — point the consumer at the package extraction directory, not the original generated source. Locate the toolchain through Rustup so the same gate runs in CI; a developer-specific executable path is not portable evidence.

- **SDK proof starts from OpenAPI** — build, package, and invoke a generated consumer from a real source fixture. Hand-built contracts did not expose inline request-body generation failures.
- **Parallel verification preserves other owners' edits** - never restore files outside the assigned slice to make a tree look clean. Unexpected changes may be deliberate work by another actor; inspect the journal and coordinate with the owner. Verify the actual source fingerprint after a mutation and before its gate.

- **Mutation probes change implementation** — keep test expectations unchanged, verify the source edit, and coordinate the mutation window with other verification work. Restore the implementation before reporting the final gate.

- **Bounded logs are excerpts** — read a file separately or inspect its final lines before concluding content was removed. A combined source-and-manifest log cutoff once made intact Cargo dependencies look missing.

- **The CLI surface is pinned by full-observation goldens** — `crates/incurs/tests/cli_surface.rs` records exit code and stdout together, not selected fields. Regenerate with `UPDATE_GOLDEN=1` and review the diff as the wire-format change it is.
- **Normalize dynamic values, do not assert around them** — for output that is deterministic apart from a measured duration or a generated id, replace the dynamic span with a stable marker before comparing, so the rest of the observation stays pinned.
- **Extension workspaces are gated per workspace** — `.github/workflows/rust.yml` runs each `extensions/*` workspace on its own runner with its own targets. A workspace that cannot build for `wasm32-unknown-unknown`, or that needs a platform runner, declares that in the matrix rather than being excluded from CI.
- **Builtin CLI behavior uses one active runtime path** — `serve()` and `serve_with()` are process adapters over `run_to()`. Implement and test built-in behavior through `run_to()`/`serve_to()` so process execution and integration tests cannot drift.
- **MCP HTTP tests need a valid Host** — current `rmcp` validates hosts before request dispatch. Direct requests to the Rust MCP HTTP service must include a loopback `Host` header (for example, `localhost`) unless the test is specifically exercising DNS-rebinding rejection.

- **Extension CI resolves locked dependencies** — run extension tests, Clippy, and target checks with --locked. An unlocked GPUI source build silently refreshed a stale dependency graph while its release archive refused the tracked lockfile. Verify fresh package archives after their new core dependencies are published.

## Git Conventions

- **Self test dependencies stay local** — when a crate enables its own testing feature through a dev-dependency with `path = "."`, omit the version. Cargo otherwise resolves the unpublished self-version from the registry during packaging. Keep versions on ordinary dependencies.

- **Archive deferral is limited to release dependencies** — before publication, the release checker recognizes missing registry dependencies listed in its release catalog, including newly introduced package names. External dependency errors and missing local paths remain failures. After publishing the dependencies, rerun archive verification and require no deferred packages.


- **Conventional commits** — use `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:` prefixes. Scope is optional (e.g. `feat(parser): add array coercion`).
- **Release archives are fresh, locked, and version-aligned** — bump publishable package manifests, their internal dependency requirements, lockfiles, and `xtask release-check` together. The release checker must remove each expected archive before packaging and package with `--locked` so stale archives cannot satisfy verification and release checks cannot rewrite tracked lockfiles.
