# Forge Worker proof

A separate workspace for testing the Forge extension on
`wasm32-unknown-emscripten`. Its patched dependencies do not participate in
the core or existing Cloudflare workspaces. It does not change the default
Workers target.

## Run locally

Requires Rustup, Python 3, Node.js and npm.

```sh
cd extensions/forge-workers
python3 build.py
npm exec --yes --package=wrangler@4.143.0 -- wrangler dev --ip 127.0.0.1 --port 8797
```

From a second terminal:

```sh
python3 probe.py http://127.0.0.1:8797
```

The probe checks the actual HTTP runtime: Emscripten target identification,
temporary-file isolation and cleanup, Tokio task spawning and timers, concurrent
requests, and input limits. It also sends the shared OpenAPI fixture to `POST /resolve`,
builds the native `incurs-forge` resolver, and compares the complete contract and
SHA-256 digest. Independent assertions check parameter locations, defaults,
response statuses, media types, headers, and required bodies. The bounded
`POST /bind` route constructs a request without sending it; the probe checks
operation/path/root server precedence, server variable defaults and overrides,
invalid selections, matrix/label paths, pipe/space query collections, headers,
and JSON and UTF-8 text body bytes, including empty and omitted text bodies,
inside Emscripten. It also checks 64-bit integer body preservation and rejects all
JSON value kinds under a false body schema. URL-encoded form bytes and multipart
frames are also checked; an independent MIME parser validates repeated fields,
resolved part headers, base64 transfer content, and boundary collision handling.
Media checks also cover arbitrary binary bytes, JSON Lines, NDJSON, both Markdown
media types, JavaScript text, null/default media selection, and false media schemas.
Invalid body slots, unsupported text schemas, and malformed record framing are
rejected before a request can be sent.
These binding routes construct requests without making outbound calls. It returns a nonzero
exit code on a mismatch. Native Cargo uses a separate target directory.

The compiler and binding routes accept JSON inputs up to 128 KiB and use the explicit
`worker-proof` namespace. Invalid documents return 400; oversized documents return
413. The runtime route limits tokens to 256 bytes and delays to 1,000 ms.

Files are ephemeral. The proof does not claim persistence, process support,
or production parity. `default_switch_eligible` stays false because passing this
probe alone does not establish the migration gate.

## Run generated SDKs inside the Worker

After build.py has installed the pinned builder, run this from the repository
root. Choose an output directory that does not already exist.

```sh
python3 extensions/forge-workers/build_generated_sdk.py /tmp/incurs-sdk-proof
cd /tmp/incurs-sdk-proof/worker
npm exec --yes --package=wrangler@4.143.0 -- wrangler dev --ip 127.0.0.1 --port 18988 --inspector-port 18989
```

From a second terminal at the repository root:

```sh
python3 extensions/forge-workers/probe_generated_sdk.py http://127.0.0.1:18988
```

The builder generates SDKs from the composition, request-media, numeric-default,
response-status, and typed-response OpenAPI fixtures, verifies all five package
archives with every feature enabled, and compiles the retained
[consumer](fixtures/generated_sdk_media_consumer.rs) against those archives.
The Worker converts typed SDK requests into binder arguments and validates them
through a cached HttpBinding before producing request bytes. The base Worker's
/bind endpoint uses the same validated binding for each supplied document.

The HTTP probe checks 12 media bindings, three SDK rejection controls, recursive
and composed values, exact u64 integers, and eight concurrent requests. Python's
standard base64 implementation independently checks every byte value and all
three nonempty padding remainders. Missing and empty bodies remain distinct.
Request validation checks reject 18 invalid nested/parameter/media inputs and
preserve two valid media selections. These checks repeat during eight concurrent
HTTP requests. Numeric checks compare literal query and body bytes for integers beyond 64 bits,
precise decimals, extreme exponents, and nested defaults. They accept an exact
schema bound, reject the adjacent integer, and reject a malformed number token.
Response classification checks all 65,536 u16 values across three response
layouts: every family with exact overrides and default, sparse families with
Other, and a family with default. It checks raw body and repeated-header
preservation before caching the result for the isolate. The HTTP probe verifies
196,608 observed classifications and the same result under concurrent requests.
Typed response checks decode a generated client result, preserve its exact
integer and decimal values, distinguish nullable fields, retain unknown fields
and duplicate headers, and exercise JSON, media, and schema failures. The probe
compares the returned values with independent literals.
These are local binding and validation checks; the Worker sends no vendor request.

## Pinned build

| Component | Pin |
| --- | --- |
| Rust | beta-2026-09-29 |
| workers-rs and worker-build | b57ba6ef8198c65499c2f92b1845cc2412dd6e8c |
| Tokio and tokio-macros | 7227f2d72739c3bc9821074cc704e7582be92907 |
| Mio | 0788dbb67883cb6191503af999471def54e3fb77 |
| libc | ab79e74db7f0f063de9d479a8593d88dfdf7f2eb |
| Wrangler | 4.143.0 |

The pinned worker-build installs Emscripten 6.0.10 and its matching patches in its
cache. The build script selects Rustup binaries directly, disables compiler wrappers
for this invocation, uses locked dependencies and two build jobs by default, and opts into Cargo's legacy build-directory layout because
this builder stages wasm-bindgen snippets from `deps/snippets`. Without that scoped
setting, the dated Cargo beta emits snippets in a different directory and bundling
fails. See [Cargo's migration issue](https://github.com/rust-lang/cargo/issues/17182). The builder binary is installed in `target/toolchain`, not over the
machine's existing builder. `rust-toolchain.toml` selects the dated compiler only
for this workspace.

## Automated local runtime gate

From the repository root, run both real HTTP probes with owned process cleanup:

```sh
python3 extensions/forge-workers/smoke.py
```

The harness builds the base Worker, starts pinned Wrangler on available loopback
ports, runs `probe.py`, and stops that process group. It then builds the generated
SDK Worker from verified package archives in a new temporary directory and runs
`probe_generated_sdk.py` against its HTTP listener. Both probe reports must contain
`passed: true`; the final JSON retains their measured results. Builds, startup,
and probes have separate deadlines. The harness cleans only its own Wrangler
process groups and temporary proof directory.

This command runs on Ubuntu and macOS with Rustup, Python 3, Node.js, and npm.
Use `--skip-install` only when this workspace already contains the pinned builder.
A local runtime pass does not establish the promotion gate below.

## Promotion gate

Integrating this target into the existing Cloudflare extension is a separate
step. It requires the existing feature and durable Code Mode suites on both
targets, hosted runtime evidence, platform-limit checks, and no more than a 10%
increase in median or p95 startup and request latency. No target switch follows
automatically from this proof.
