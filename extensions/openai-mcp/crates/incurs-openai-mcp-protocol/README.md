# incurs-openai-mcp-protocol

Portable wire models and validators for the OpenAI MCP Extensions surface pinned in `../../parity/upstream.json`.

The crate exposes typed serde models for the Rust server/app SDKs and a schema registry through `validate_schema`. The registry names match the TypeScript exports from `typescript/src/server/index.ts` and `typescript/src/app/index.ts` so differential parity tests can call the Rust and upstream TypeScript validators with the same fixtures.

Covered schema exports:

- Server: file entrypoint input, mention items/search, UI entrypoints/quick actions/tool metadata/resource metadata, settings capabilities/layout/read/update results, form fields/forms/results/file pickers, and resource tool-call metadata.
- App: file open params, deep-link host state, model-context host state/metadata, message params/options, resource read/write metadata, resource representations, write params, and write results.

The registry returns normalized JSON after Rust parsing. Custom validators cover the constraints that are not representable by serde derives alone, including nonblank strings, strict object envelopes, settings layout references, mutually exclusive resource write bodies, form required-field checks, file/resource picker selection rules, form scalar and array constraints, and legacy thumbnail/preview aliases.

`examples/schema_oracle.rs` provides a JSONL oracle for parity harnesses. Each input line is `{ "schema": "OpenAIFormSchema", "value": ... }`; each output line is either `{ "ok": true, "value": ... }` with the normalized value or `{ "ok": false, "error": "validation_error" }` with diagnostic fields.

Form content validation applies JSON Schema `pattern` checks with an ECMAScript regular expression engine on native and wasm targets, including lookaround assertions and backreferences used by JavaScript controls.

The Python helper exports map to Rust as follows: `resource_input` and `file_input` are builders for `x-openai-input`; `UserResourceOptions` and `FileUserOptions` alias `OpenAIUserResourceOptions`; `FormField` and `FormSchema` alias `OpenAIFormField` and `OpenAIForm`; `McpAppToolTarget` and `PreviewTarget` are typed models; `prepare_field_submission`, `complete_field_submission`, `is_valid_value`, `validate_file_selections`, and `validate_form_selections` are protocol validation helpers. The asynchronous `elicit_form` helper is server-owned because it performs MCP request I/O; this crate owns its `OpenAIFormRequestParams`, `OpenAIFormResult`, and submitted-value validation contract.
