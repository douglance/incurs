# Migrating from incurs 0.5 to 0.6

Version 0.6 builds the `incurs` CLI with incurs itself. That exercise found
three defects that no test in the repository could see, because no CLI in it
had ever used the affected paths. Two of the fixes change behavior, and one
changes an emitted schema.

## Prerequisites

incurs 0.6 requires Rust 1.88 or newer, unchanged from 0.5.

```bash
rustup update stable
rustc --version
```

## Package versions

Update the packages that share the core incurs API together:

```toml
[dependencies]
incurs = "0.6"
incurs-macros = "0.5"  # only when depended on directly
incurs-extras = "0.6"  # only when using Rust-only formats
```

Code Mode packages exchange incurs types through their public APIs, so upgrade
them as a set:

```toml
[dependencies]
incurs-codemode = "0.3"
incurs-codemode-local = "0.3"
incurs-codemode-mcp = "0.3"
```

Cloudflare integrations use `incurs-codemode-cloudflare = "0.3"` and
`incurs-mcp-cloudflare = "0.2"`. The remote capability seam is
`incurs-remote = "0.2"`. `incurs-mcp-protocol` stays at `0.1`.

## Rename a command that shadows a builtin

`completions`, `mcp`, `plugin` and `skills` are builtin command names. Until
now a builtin claimed its name unconditionally, and every builtin dispatcher
treats an unrecognized subcommand as success — so a CLI that registered its own
command under one of those names saw builtin help and a zero exit, with no
diagnostic and no way to reach its handler.

A registered command now wins:

```rust
// In 0.5 this command was unreachable; `app plugin check` printed builtin
// help and exited 0. In 0.6 it runs.
Cli::create("app").command("plugin", plugin_command())
```

Audit any CLI that defines one of those four names. If it relied on the builtin
being reachable under that name, rename the registered command — the builtin is
shadowed only when a command of the same name exists.

## Expect a declared default on boolean options

A `bool` field in a struct deriving `incurs::Options` is optional, but declared
no default. `bool` has no serde default either, so omitting the flag failed to
parse:

```console
$ my-cli ship
Error (VALIDATION_ERROR): Failed to parse options: missing field `force`
```

An absent boolean flag now declares `false`. Omitting the flag works, and the
default appears in every published schema:

```json
{ "force": { "type": "boolean", "default": false } }
```

Tests or clients that asserted a boolean option had **no** `default` key must
be updated. Nothing else about boolean parsing changed.

## Declare both destructive fields

`McpCommandOptions::destructive` gates skill confirmation, while the MCP
`destructive_hint` annotation is what MCP clients read. They are separate
fields carrying one fact, and setting only one leaves the other surface
unwarned. Until they are unified, declare both:

```rust
.mcp(McpCommandOptions {
    destructive: true,
    annotations: Some(McpAnnotations {
        destructive_hint: Some(true),
        ..Default::default()
    }),
    ..Default::default()
})
```

## Update `incurs plugin call` consumers

`incurs plugin call` printed the raw `{status, data}` tool outcome, which
nested one envelope inside the framework's own. It now prints the tool's data,
like every other command:

```console
# 0.5
{"status":"ok","data":{"message":"pong"}}

# 0.6
{"message":"pong"}
```

A failure is the standard error envelope with a non-zero exit, so branch on the
exit status rather than reading `status` from the payload. Pass `--full-output`
when the `{ok, data, meta}` envelope is wanted.

`incurs` help and error text are now generated rather than hardcoded. `gen` and
the `plugin` commands keep JSON as their default output format, so scripts that
parse stdout are unaffected.

## Read the reference from the CLI

The repository's `SKILL.md` documented a TypeScript package. It is now compiled
from the command graph by `cargo xtask skill-sync`, and the same content is
available from the CLI:

```bash
incurs explain                 # list topics
incurs explain typed-commands  # read one
```

## Validate the migration

Run the checks that match the features the application uses:

```bash
cargo check --all-features
cargo test --all-features
cargo doc --all-features --no-deps
```

For a CLI that defines a command named `completions`, `mcp`, `plugin` or
`skills`, also run that command and confirm it reaches the intended handler.
