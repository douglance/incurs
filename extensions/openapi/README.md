# incurs OpenAPI prototype

This standalone Rust workspace proves a shared API contract compiler before any
production importer, SDK, or transport is migrated. The Emscripten runtime proof
lives in [openapi-workers](../openapi-workers/README.md).

The compiler separates three responsibilities:

1. Resolve an OpenAPI document and explicit overlays into a contract with stable
   operation identities and HTTP bindings.
2. Compile the contract into an in-memory set of Rust SDK and documentation files.
3. Publish that set separately, then verify the generated package by building and
   invoking it.

Parameter locations, body presence, nullability, defaults, response statuses,
media types, and response headers remain explicit. External reference documents
are supplied by the caller; compilation does not fetch them implicitly.

## Verify the module

Requires a stable Rust toolchain and the `wasm32-unknown-unknown` target.

```sh
cargo test --manifest-path extensions/openapi/Cargo.toml --all-features --locked -j 2 -- --test-threads=1
cargo clippy --manifest-path extensions/openapi/Cargo.toml --all-targets --all-features --locked -- -D warnings
cargo check --manifest-path extensions/openapi/Cargo.toml --target wasm32-unknown-unknown --no-default-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path extensions/openapi/Cargo.toml --all-features --no-deps --locked
```

Run commands from the repository root. The separate Worker instructions cover a
real HTTP run, including a comparison with this native compiler.

## Generate a Rust SDK and docs

Use a new output directory; publication refuses an existing target.

```sh
cargo run --manifest-path extensions/openapi/Cargo.toml --example compile -- extensions/openapi-workers/fixtures/proof-openapi.json /tmp/incurs-openapi-proof
cargo test --manifest-path /tmp/incurs-openapi-proof/sdk/Cargo.toml --locked
cargo package --manifest-path /tmp/incurs-openapi-proof/sdk/Cargo.toml --locked
```

Open `/tmp/incurs-openapi-proof/docs/index.html` for searchable operation details.
The compiler also emits `contract.json`, a search index, and an artifact manifest.
`compile_artifacts` performs no filesystem writes; `publish_artifacts` installs
the resulting bytes separately.

The generated client has async methods and an async `Transport` trait without a
`Send` requirement. `OperationRequest::arguments_json` is the bridge to the
location-keyed HTTP binder. Optional fields distinguish `Missing`, `Null`,
`Value`, and `Default`; the binder never fills in an omitted optional parameter.
Response variants retain status, headers, and raw payloads, including binary data.
Classification selects a declared exact code first, then its declared 1XX through
5XX family, then default or Other. For example, an API declaring 200 and 4XX
returns Status4XX for HTTP 429. The optional typed-responses feature adds
schema-specific decoding while retaining the original response.

    before                         after
    +------------------+           +------------------+
    | HTTP 429         |           | HTTP 429         |
    +--------+---------+           +--------+---------+
             |                              |
             v                              v
    +------------------+           +------------------+
    | Fallback variant |           | Declared 4XX     |
    +------------------+           +------------------+

The response-status packaged consumer checks every u16 status across three
response layouts, nine loopback HTTP responses, binary payloads, and repeated
headers. Its expected classification uses arithmetic independently of generated
match arms. Confirmed mutations remove the inclusive endpoint, exact-code
precedence, and default fallback; each fails the unchanged consumer.
See [response status evidence](response-status-report.json) for artifact hashes
and the full Cloudflare and Emscripten runtime results.

Required nullable path arguments also use Field<T>. Missing fails argument
serialization; Value and Default preserve the supplied value. Null remains JSON
null. Ordinary path styles reject null; an application/json content parameter
encodes it as the literal null value. Generated smoke tests supply a value for
required nullable parameters.

Schema and operation types share one Rust name allocator. Conflicting identifiers
receive numeric suffixes, including collisions with SDK helpers and prelude
constructors. Canonical schema keys, references, operation IDs, wire field names,
and enum values remain unchanged. Separate method scopes allocate client methods
and default accessors.

## Buffered typed responses

Enable the generated crate's `typed-responses` feature to decode a raw response
with `decode()` or use its typed client methods. Status selection still prefers
an exact code, then a declared family, then default or Other. A declared error
response, such as HTTP 429, is a typed status variant when its body is valid.
Transport failures and decoding failures remain distinct.

    before
    +----------------------+
    | Raw status and bytes |
    +----------+-----------+
               |
               v
    +----------------------+
    | Caller parses body   |
    +----------------------+

    after
    +----------------------+
    | Raw status and bytes |
    +----------+-----------+
               |
               v
    +----------------------+
    | Media + schema check |
    +----------+-----------+
               |
               v
    +----------------------+
    | Typed body + raw data|
    +----------------------+

Decoded values and decoding errors retain the original status, every header
pair, and body. Read models keep unknown properties accepted by the schema,
require read-only fields when declared required, and allow required write-only
fields to be absent. They distinguish missing fields from null and do not insert
schema defaults into received data. Recursive anonymous compositions reuse a
model identity rather than expanding indefinitely.

With byte responses, JSON parsing preserves integer, decimal, and exponent
tokens in ResponseInteger and ResponseNumber. Checked conversions return None
when the requested numeric representation would change the value under their
documented conversion rule. Duplicate object keys are rejected at every depth.
The transport must supply bytes or exact JSON values to preserve this evidence;
decoding cannot recover digits already rounded by a transport.

Content-Type selects declared JSON, UTF-8/ASCII text, or raw binary media.
Ambiguous or unsupported media, invalid encodings, forbidden bodies for HEAD
and bodyless statuses, and schema violations return a decoding error with the
raw response. Binary schemas intersect byte length limits across references and allOf;
unsupported binary constraints fail explicitly. Structured XML parsing, incremental streams, and
vendor-specific envelope unwrapping are outside this decoder.

The packaged response fixture exercises recursive and composed models,
constraint-only schemas with non-object values, nullable references, reserved
field names and control characters, empty objects, exact numeric limits, media
selection, and four loopback HTTP exchanges. The [typed-response verification
report](typed-responses-report.json) records the 20-operation fixture, five
confirmed mutations, the complete 130-test OpenAPI gate, local Worker HTTP
checks, and final source and archive hashes.

All three pinned API corpora build and package with typed responses enabled.
Cloudflare exercises two typed responses over HTTP, plus three raw request-media
cases; GitHub exercises four raw HTTP cases and Stripe one. The generated
GitHub and Stripe files match their tested archives; Cloudflare was packaged
and invoked again after its response models changed. These are local fixtures,
not live vendor behavior or universal OpenAPI conformance.

## Supported proof and limits

- Resolution covers OpenAPI 3.0/3.1/3.2, replacement overlays, inherited parameters,
  supplied reference documents, request bodies, responses, and schema metadata.
  It is not a full OpenAPI conformance validator. Relative external URI handling
  is limited; recursive references remain references. Callbacks, links, and
  discriminator-driven SDK generation are outside this proof.
- SDK generation supports named scalar and array aliases, string enums, named
  and inline objects, nullable aliases and fields, null-only schemas, defaults,
  and supported recursive references. Boolean true schemas use the JSON value
  type; false schemas use an uninhabited Rust type. Generated signed integers
  retain exact decimal bytes. Numeric defaults preserve integers beyond 64 bits,
  precise decimals, and exponents without passing through floating point.
  The generated JsonValue::ExactNumber variant validates the complete number
  token before encoding. Explicit typed integer and number fields still use
  their generated Rust integer and floating-point types.
  Unsupported schema combinations return a compiler error. Generated request
  bodies preserve media alternatives through explicit variants, including binary
  byte buffers. Missing optional bodies omit both the body and media_type.
  The request-media section below describes input slots and unsupported encodings.
- HTTP binding supports simple/form values, matrix/label paths, pipe/space query
  collections, and deepObject query objects for their defined combinations.
  JSON request bodies retain omission and explicit null. `text/plain` bodies
  encode strings as UTF-8, distinguish empty from omitted bodies, and reject
  non-string values. A false schema at the body root rejects every supplied JSON
  value. The compiled runtime::HttpBinding validates logical request values
  against the source-version-aware schema before transport; the low-level
  serialization functions do not have source-version context.
  URL-encoded form bodies retain Encoding Object
  metadata and support content-based JSON/scalar values and explicit form,
  deepObject, pipeDelimited, and spaceDelimited styles. Unsupported property
  media types and undefined styles fail explicitly. Multipart form bodies preserve
  repeated array parts, JSON/scalar content, declared transfer-encoded strings,
  and resolved part headers. Boundaries are selected after checking the serialized
  parts for collisions. Raw binary, text alternatives, JSON Lines, and NDJSON
  use the shared request-media codec described below.
  Undefined combinations, nulls in style-based parameters, and empty collections
  fail explicitly. Authentication remains the host's responsibility.
  For pipe/space/deepObject delimiters inside values, API-defined pre-escaping
  remains necessary for an unambiguous roundtrip; the binder follows the wire
  encoding rules in [OpenAPI Appendix E.5](https://spec.openapis.org/oas/v3.1.1.html#appendix-e-percent-encoding-and-form-media-types).
- The `adapters` feature bridges incurs outbound HTTP, `RemoteToolRuntime`,
  and `CodeModeService`, and exposes `catalog::compile_cli`. It automatically
  registers resolved operations with nested discovery schemas and generic HTTP
  handlers. The host supplies authentication and network policy. A native
  process probe covers CLI HTTP and progressive MCP discovery/invocation;
  a concrete Code Mode executor remains outside this extension proof.

The full test command includes generated SDK builds, packaging, and consumers
that compile against the extracted package archives. That test requires Rustup's stable toolchain and uses a separate Cargo
target directory, so nested builds do not share the parent build lock. Run these
tests with --test-threads=1: each consumer starts its own Cargo build, and running
many at once can exceed a bounded runner's memory.

## Prove an incurs tool makes the HTTP call

Run the continuous integration proof with its request/response transcript:

```sh
cargo test --manifest-path extensions/openapi/Cargo.toml --all-features --locked --test tool_http -- --nocapture
```

This test resolves the OpenAPI fixture, compiles both operations into an incurs
CLI with `catalog::compile_cli`, obtains its `ToolCatalog`, and invokes
`op_updateWidget`. It contains no per-operation registration loop or handler.
A loopback HTTP server independently checks the POST method, encoded path,
query option, header, and JSON body including explicit null. The incurs caller
receives the server's unique receipt and response header. A missing required path
parameter returns an error with zero outbound calls.

The compiler publishes location objects, required parameters, body alternatives,
media choices, and recursive schema definitions. Responses retain status,
repeated header pairs, and raw body bytes, including HTTP errors and binary data.
Consumers choose how to decode those bytes. Cancellation drops an active HTTP
future through the shared ToolCatalog execution boundary. These checks remain
bounded fixtures, not universal OpenAPI conformance.

## Run an imported CLI or MCP server

Build the standalone Rust example with native HTTP enabled:

```sh
cargo build --manifest-path extensions/openapi/Cargo.toml --features native-cli --example openapi --locked
```

Inspect a local JSON specification without making API requests:

```sh
extensions/openapi/target/debug/examples/openapi ./api.json https://api.example.test/openapi.json --describe-tools
```

The second argument is the declaring document URL, used to resolve relative
servers. Declared absolute servers still control requests. Tool/command names
start with `op_`; non-alphanumeric UTF-8 bytes are escaped, and long names use a
digest. Use discovery instead of guessing names. A host can also mount the
returned `Cli` or call its `ToolCatalog` directly.

Location flags such as `--path` accept JSON objects. `--body` preserves a CLI
string, while `--body-json` explicitly decodes JSON text. Raw binary uses
`--body-base64`; the three body slots are mutually exclusive. For multiple
request media types, supply `--media-type`. Start the same
imported CLI with `--mcp` to use incurs' progressive `search_tools`,
`get_tool_details`, and `call_write_tool` interface. Imported commands are
unclassified by default, so they use the write route even for GET operations.

Run the bounded process proof after building:

```sh
python3 extensions/openapi/probe_cli.py
```

The standard-library Python harness invokes the Rust executable against loopback
HTTP, then performs MCP initialization, search, inspection, invocation, and a
rejected call. It checks complete response values, binary bytes, duplicate
headers, empty/omitted bodies, and zero HTTP calls for malformed input. A form
operation also runs through MCP and checks exact URL-encoded bytes, including
Unicode, reserved characters, JSON-valued fields, arrays, and large integers.
A multipart operation runs through an independent MIME parser, checking repeated
parts, resolved part headers, base64-decoded bytes, and a boundary collision.
It closes the owned server and child process. The probe does not authenticate
against a remote API or prove every MCP protocol version.

## Request media and raw bytes

The selected media type determines the wire codec. A binary schema annotation does
not turn JSON into raw bytes or turn JavaScript text into a binary argument.

| Declared media | Adapter input | Wire representation |
| --- | --- | --- |
| application/json or a +json suffix | body or body_json | JSON, including explicit null |
| text/* or application/javascript | body string or body_json containing a string | UTF-8 text; declared US-ASCII rejects non-ASCII characters |
| application/octet-stream | body_base64 | Decoded bytes; empty bytes remain distinct from an omitted body |
| application/jsonl | String body, or body_base64 for a binary schema | Supplied JSON Lines records, without array-to-lines conversion |
| application/x-ndjson | String body, or body_base64 for a binary schema | Supplied records with a final newline |
| application/x-www-form-urlencoded | Object body | Existing form encoding rules |
| multipart/form-data | Object body | Existing multipart encoding rules |

Raw binary uses the explicit body_base64 slot. An octet-stream schema with an
explicit non-binary string alternative also permits a UTF-8 body string.
body, body_json, and body_base64 are mutually exclusive. Multiple declared media
types require media_type. The CLI spellings are --body, --body-json,
--body-base64, and --media-type.

JSON Lines and NDJSON inputs must be UTF-8, contain one valid JSON value per
record, and contain neither blank records nor a byte-order mark. JSON Lines may
omit its final newline; NDJSON may not. Empty streams are accepted. These codecs
preserve supplied bytes; they do not infer framing from an object or array.

Generated multi-media and binary request bodies expose an enum. Its variant owns
the media choice, so a Null or Default value cannot lose its Content-Type.
Optional enums have a Missing variant; binary variants carry Vec<u8>. The
transport-neutral request stores those bytes separately and encodes them into
body_base64 when arguments_json is called. False media schemas reject supplied
values, including null and defaults.

    before
    +-----------------------+
    | Generated SDK request |
    +-----------+-----------+
                |
                v
    +-----------------------+
    | One selected medium   |
    +-----------------------+

    after
    +-----------------------+
    | Explicit media choice |
    +-----------+-----------+
                |
                v
    +-----------------------+
    | Shared body codec     |
    +-----------+-----------+
                |
                v
    +-----------------------+
    | Literal HTTP bytes    |
    +-----------------------+

The packaged media consumer checks binary bytes, both Markdown media types,
JavaScript, JSON Lines, NDJSON, omitted and empty bodies, nulls, defaults, and
invalid slot combinations. Native CLI/MCP and the Emscripten Worker run separate
protocol probes. Confirmed mutations corrupted SDK byte encoding and replaced
enum wire strings with Rust identifiers; the unchanged HTTP consumers failed.

An object schema under text/plain does not define an object-to-text encoding.
That alternative stays declared and fails when selected; use its JSON alternative
when the API supplies one. Low-level serialization functions do not enforce
nested schemas; HttpBinding validates logical values before serialization is
returned to a caller. Discovery keeps the complete source constraints when
projecting a text alternative instead of flattening composed keywords.

## Multipart body inputs

Supply a JSON object under `body`; array properties become repeated parts.
Content-based objects use JSON, scalars use text, and explicit encoding styles
produce unescaped multipart values. A declared `contentEncoding` supplies the
transfer header and expects the caller's already-encoded string. The binder does
not automatically encode or validate that string. Raw string content is UTF-8;
arbitrary unencoded byte buffers are not yet represented by this JSON argument
interface.

Part headers can use schema `const`, `default`, or a single `enum` value.
Optional headers without a bound value are omitted; required headers without one
fail. Per-call part-header values, selection among multiple part media types,
and style-based collections with encoded items are not yet supported. Header
control characters are rejected, and field names are quoted without allowing
header injection. These cases are exercised by `tests/multipart_body.rs` and the
native SDK/MCP consumers. The encoding distinction follows the
[OpenAPI Encoding Object](https://spec.openapis.org/oas/v3.1.1.html#encoding-object).

## Shared schema references

The resolved contract keeps a shared schema table and canonical local references
from named schemas, parameters, bodies, and responses. Referenced external schemas
receive deterministic names in that table. Reference siblings and instance data
inside defaults or examples remain in the contract.

HTTP binding and invocation now take the owning contract's schemas table alongside
the operation. Multipart serialization follows references for media hints, transfer
encoding, array items, and header values. This metadata lookup does not replace
general schema validation. Tool discovery includes only definitions reachable from
that operation's input schemas.

Generated SDKs follow references for type selection, defaults, and nullability.
Mutually recursive object fields use indirection. A referenced object request body
can now use its named Rust type where eager expansion previously exposed JsonValue;
construct that generated type before passing the request to the client.

The graph regression checks a shared definition with 128 uses. Reintroducing copies
produced 548,579 bytes and failed its size bound. Disabling reference metadata lookup
failed the literal multipart request check. Both implementations were restored
exactly, and the graph and generated-consumer tests passed afterward. The continuous
ToolCatalog-to-HTTP test and live Emscripten Worker probe also exercise references.

## Typed schema composition

Generated oneOf and anyOf enums carry typed branch values. Serialization validates
the selected branch and the complete parent schema. oneOf requires exactly one
matching branch; anyOf permits overlap. Constraint-only branches retain JSON
values because a keyword such as required does not itself constrain the type.

Object allOf schemas produce a combined struct when their declarations can be
represented together. Other intersections produce a named JSON wrapper with
try_new, as_json, and IntoJson. try_new rejects values that violate any original
constraint. Conflicting declarations remain intersections; they do not become
unions or nullable alternatives. An intersection with no valid value rejects
every attempted construction.

Composed SDKs include one shared schemas.json graph and use pinned jsonschema
0.58.2 with default features disabled and arbitrary-precision number validation.
Each thread compiles one offline validator and dispatches named values through
references into that graph. No validator fetches network or file references.
Ordinary SDKs without compositions retain their dependency-free runtime.

    before
    ┌───────────────────────┐
    │ Each composed type    │
    └───────────┬───────────┘
                ▼
    ┌───────────────────────┐
    │ Copied schema tree    │
    │ Handwritten validator │
    └───────────────────────┘

    after
    ┌───────────────────────┐
    │ Composed SDK values   │
    └───────────┬───────────┘
                ▼
    ┌───────────────────────┐
    │ One schema graph      │
    │ Offline validator     │
    └───────────────────────┘

The resolved contract retains openapi_version and json_schema_dialect. For
OpenAPI 3.0, nullable adds null only when type is present in the same Schema
Object; other constraints still apply. Reference siblings follow 3.0 Reference
Object semantics. In 3.1 and 3.2, nullable is an annotation. The generator rejects
unrecognized dialects and resource-scoping keywords it cannot preserve
($id, $anchor, $dynamicAnchor, and $dynamicRef).

Packaged consumers check recursive intersections, patterns, multipleOf,
conditionals, tuple constraints, overlapping unions, impossible intersections,
duplicate keys, and exact integers including u64::MAX. They also send literal
expected JSON bytes to loopback HTTP servers. Separate controls distinguish
3.0 from 3.1 nullability and reference siblings.

Confirmed mutations disabled selected-branch validation, bypassed instance
validation, and expanded shared allOf references. The unchanged tests failed
for the wrong enum variant, an out-of-range integer, and graph growth beyond
40 KB respectively. The expanded graph measured 1,078,151 bytes. All three
implementations were restored exactly and their checks passed afterward.

Generated composed types and compiled HTTP request bindings share source-version
normalization. Format remains an annotation, discriminators do not select branches,
and the typed representation does not cover every valid schema shape. These checks
do not establish complete JSON Schema or OpenAPI conformance.

## OpenAPI 3.2 operations and parameters

The resolver includes QUERY and additionalOperations. Custom HTTP methods retain
the exact capitalization declared by the API. Invalid method tokens and attempts
to redefine a fixed method through additionalOperations are rejected.

Content-based parameters retain their single media type, schema, and form
encodings. The binder supports JSON, text/plain, and URL-encoded form content.
Referenced Media Type Objects retain the resolved schema and encodings.

A querystring parameter describes the entire query, so it cannot coexist with a
query parameter or a second querystring parameter. Its name identifies the
argument under the querystring location but is absent from the serialized URL:

    {"querystring":{"filter":{"term":"a + b"}}}

With URL-encoded form content, that value produces term=a+%2B+b. With text/plain
content, callers supply an already URI-encoded string. The binder preserves valid
percent escapes and rejects fragments, malformed escapes, raw Unicode, spaces,
and control characters. An explicit empty string produces a trailing question
mark; omission produces no query.

The cookie style uses literal Cookie header syntax. It preserves percent-looking
text without encoding or decoding it and rejects values that require
application-defined escaping. Expanded objects use their property names, so the
container name is not serialized. This differs from the older form style.

The OpenAPI 3.2 fixture is independently validated. A packaged generated SDK and
native CLI/MCP consumers check actual HTTP request lines and headers; the Worker
probe checks these bindings through its HTTP endpoint. Forcing custom methods to
uppercase was observed to fail an unchanged test before exact restoration.

Sequential itemSchema and positional prefixEncoding/itemEncoding contracts
remain unsupported and are rejected explicitly. These operation and parameter
checks do not establish complete OpenAPI 3.2 conformance.

## Select declared servers

Each resolved operation retains its effective `servers` list. Operation-level
declarations override path-level declarations, which override the root list.
Use `servers::build_operation_request` or `servers::invoke_operation` with a
`ServerSelection` to choose the server index and variables. Omitted server
variables use their declared defaults; overrides must satisfy a declared enum.

Relative URLs, including the default `/`, require an explicit absolute
`ServerSelection::document_url`. This is the URL of the document that declares
the selected server, not the current process directory. Hosts loading local
files must supply that URL. The lower-level `runtime::build_http_request` and
`runtime::invoke_http` continue to accept an explicit endpoint override.

## Audit compatibility

Run the bounded compatibility diagnostic:

```sh
cargo run --manifest-path extensions/openapi/Cargo.toml --locked --example compatibility
```

Exit 1 means at least one case failed. The report separates resolution, SDK
source generation, and selected HTTP request checks. A passing row does not
establish that the generated SDK builds or that a request was executed.
These are selected feature cases, not a representative percentage of all APIs.

Print any case as a complete OpenAPI document for an independent validator:

```sh
cargo run --manifest-path extensions/openapi/Cargo.toml --locked --example compatibility -- --document body-text
```

The [initial 2026-09-29 report](compatibility-report.json) records 19 documents accepted
by openapi-spec-validator 0.9.0: six passed the probe and 13 failed. Failures
include OpenAPI 3.2, label/matrix/pipe/space parameter styles, text/form/multipart
bodies, named scalar/composed/boolean schemas, and an operation-level server
override discarded by the original resolver. The server-selection tests now
cover that defect, including a real ToolCatalog-to-HTTP call. The initial report
is retained as historical evidence. The [current report](compatibility-current.json)
uses the same input hashes: all 19 cases pass, with no regressions.
The 13 newly passing cases cover the basic OpenAPI 3.2 document, four parameter
styles, operation server selection, text/form/multipart bodies, and
scalar/composed/boolean schemas. Separate consumer tests build and package the
JSON, text, form, multipart, scalar-schema, and composition SDKs and check real
loopback requests; the text consumer checks
UTF-8, empty bodies, and omitted bodies. Mutating text bytes or omitted-body media
selection was observed to fail the corresponding unchanged test. The schema
consumer checks signed integer limits and IDs above 2^53 against literal HTTP
bytes; restoring a floating-point conversion was observed to corrupt those
values and fail the unchanged test. A separate negative control rejects every
JSON kind under a false body schema. These counts describe this bounded audit, not all OpenAPI inputs.

The independent validator was used as temporary verification tooling; it is
not a project dependency. The Rust probe and implementation remain standalone.
The report includes source hashes and per-document hashes to identify its scope.
Supported request cases also use literal wire expectations, and changing the
binder to send GET was observed to break the JSON POST control before restoration.

## Audit published API documents

The [pinned source manifest](corpus-sources.json) records repository commits,
original URLs, byte counts, and SHA-256 hashes. The [initial audit](corpus-report.json),
[shared-graph audit](corpus-graph-report.json), and
[composition audit](corpus-composition-report.json) retain historical results.
The [request-media and SDK audit](corpus-request-media-report.json) records the
then-current source hashes, terminal verification results, archives, and limits.
The later [typed-response report](typed-responses-report.json) records the current
response decoder and its separate verification.

| Pinned input | Operations | Resolved bytes | SDK build/package | Local HTTP cases | Typed HTTP cases |
| --- | ---: | ---: | --- | ---: | ---: |
| GitHub | 1,231 | 4,787,312 | Passed | 4 | 0 |
| Cloudflare | 3,627 | 12,123,837 | Passed | 5 | 2 |
| Stripe | 644 | 4,620,179 | Passed | 1 | 0 |

All 9,622 component schemas compare equal to their original source definitions.
Resolved byte counts describe the shared contract representation; they are not
peak memory measurements or controlled performance benchmarks.

The consumers compile against verified package archives and send literal HTTP
requests to isolated local servers. The retained consumers cover:

- [GitHub](tests/fixtures/corpus_github_consumer.rs): GET /zen, both Markdown
  media types, and binary release-asset upload.
- [Cloudflare](tests/fixtures/corpus_cloudflare_consumer.rs): typed user lookup
  responses for HTTP 200 and 429,
  binary DLP upload, a JSON Lines usage report, and NDJSON Vectorize insertion.
- [Stripe](tests/fixtures/corpus_stripe_consumer.rs): GET /v1/balance with
  omitted optional arguments. Explicit expand remains rejected because the
  source declares deepObject for an array.

Those ten request cases cover eight operations. They do not establish every
vendor operation's business behavior. GitHub and Stripe pass independent input
validation. Cloudflare's pinned source has default "type" outside the enum at
/components/schemas/dns-settings_order; the source is preserved unchanged.

In the earlier request-media audit, Cloudflare's full SDK crossed the 4 GiB
process-family limit during a
default debug build. The identical generated source then passed tests, package
verification, and its HTTP consumer with one build job and an 8 GiB limit.
The report records this retry separately from successful artifact generation.

The request-media audit passed 113 OpenAPI tests, strict Clippy, formatting, strict
rustdoc, and each declared feature separately on native and
wasm32-unknown-unknown. The bounded compatibility matrix still passes all 19
documents. The final native CLI/MCP probe passed 20 media requests and 23
rejection checks, alongside its existing protocol and encoding controls.

That audit also passed the local Emscripten runtime, parity, and media probes.
A separate [generated-SDK Worker proof](../openapi-workers/README.md#run-generated-sdks-inside-the-worker)
builds current package archives into the Worker and checks typed request binding,
composition validation, all byte values, and base64 padding against Python's
standard implementation.

Five confirmed mutations corrupted SDK byte encoding and enum wire strings,
bypassed name allocation and reference lookup, or accepted a missing required
path as null. Each unchanged regression failed after the source edit was
confirmed. The implementation was restored exactly before the final green gate.
Earlier graph and composition mutation controls remain in the historical reports.

Build the stage-specific auditor, then pass a JSON document and its declaring URL:

```sh
cargo build --manifest-path extensions/openapi/Cargo.toml --locked --release --example audit
extensions/openapi/target/release/examples/audit ./api.json https://example.test/openapi.json
```

An optional third argument publishes artifacts into a new directory. A dash as
the first argument reads JSON from stdin. Append --resolve-only to measure the
contract without compiling artifacts. Each stdout line is a JSON stage snapshot.
Root component comparison checks literal schema retention; it is separate from
document validation and does not prove all schema semantics.

Exit 1 means a compiler stage failed. Record an external timeout or memory
cancellation as an aborted stage, including any earlier completed stages.
The auditor does not independently validate documents, build SDKs, or send API
requests. Universal OpenAPI conformance, every-operation HTTP compatibility,
hosted production parity, and comparative performance remain unproven.

## Request validation before HTTP

Use runtime::HttpBinding::new with the owning ResolvedOpenApi contract, then
reuse its build_request or invoke method. The binding owns the contract and
compiled offline validator, so an operation cannot be paired with a validator
from a different document. build_request accepts an explicit host URL; declared
server selection remains available through servers::select_server_url.

ToolCatalog, CLI, MCP, and the Worker proof use this boundary. Tool discovery and
runtime enforcement consume the same normalized request schemas. Invalid logical
values return OPENAPI_VALIDATION before transport; serialization failures retain
OPENAPI_BINDING. Error messages identify the operation and schema/instance pointers
without echoing the supplied values.

```
before                         after
+--------------------+         +--------------------+
| Logical arguments  |         | Logical arguments  |
+---------+----------+         +---------+----------+
          |                              |
          v                              v
+--------------------+         +--------------------+
| Wire serialization |         | Wire serialization |
+---------+----------+         +---------+----------+
          |                              |
          v                              v
+--------------------+         +--------------------+
| HTTP transport     |         | Compiled schema    |
+--------------------+         +---------+----------+
                                         |
                                         v
                               +--------------------+
                               | HTTP transport     |
                               +--------------------+
```

The shared graph retains each reachable input definition once. Validation uses
the selected media type; a value matching another declared media schema cannot
bypass that selection. Raw binary remains a separate decoded byte slot, rather
than being validated as a logical JSON string. Empty unused parameter groups
emitted by generated SDKs normalize to omission; populated undeclared groups,
unknown locations, and invalid bodies still fail.

OpenAPI 3.0 nullable and reference-sibling rules remain distinct from 3.1.
A legacy read-only property is not required in requests, but its constraints still
apply when supplied. Unsupported dialects and resource scopes fail explicitly.
Response-only definitions do not enter the request validation graph.

Run the native process proof after building the openapi example:

```sh
python3 extensions/openapi/probe_request_validation.py
```

The [request-validation report](request-validation-report.json) records native,
Worker, full-corpus construction, mutation, and integrated verification.
Low-level build_http_request and servers::build_operation_request remain
serialization APIs; callers requiring schema enforcement use HttpBinding.

## Exact numeric defaults

The [numeric-defaults report](numeric-defaults-report.json) records the current
numeric fix and its source and artifact hashes. A generated consumer previously
rounded 18446744073709551617 to 18446744073709552000 and rejected 1e+400.

Defaults now retain their numeric text, including inside arrays and objects.
The complete JSON number token is checked before encoding or schema validation.
The packaged HTTP regression checks literal query and body bytes, 16 valid
number tokens, 33 malformed tokens, and adjacent values at an exact schema bound.
Confirmed default-corruption and token-validation mutations both failed the
unchanged regression.

The restored integrated suite passed 114 tests, strict Clippy, formatting,
strict rustdoc, and the all-features wasm32 check. The original reproduction also
passed from a dependency-free package archive. A current Emscripten Worker built
from three SDK archives passed the numeric checks, existing media and composition
checks, and eight concurrent HTTP requests. This proof is local and sends no
vendor requests. Earlier full-vendor corpus reports retain their original hashes;
those complete vendor SDKs were not regenerated for this numeric change.

## Scope

This is an experimental module, not a published package. Existing incurs APIs and
the default Cloudflare target retain their current behavior. A passing local
proof does not establish hosted production parity or authorize a target switch.

The design draws on Cloudflare's [Forge](https://github.com/cloudflare/forge)
contract-first generation pipeline and its
[Rust Workers Emscripten work](https://blog.cloudflare.com/rust-workers-emscripten-target/).
The SDK implementation language is Rust.
