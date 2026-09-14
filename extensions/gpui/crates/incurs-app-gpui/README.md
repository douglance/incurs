# incurs-app-gpui

`incurs-app-gpui` presents any [`incurs::tool::ToolCatalog`] as a native desktop
window built on [GPUI](https://crates.io/crates/gpui).

Every call goes through `ToolCatalog`, so schema validation, middleware, config
defaults, streaming, and cooperative cancellation behave exactly as they do on
the CLI. The crate owns presentation only — it decides which control a JSON
Schema property gets and how a result is laid out, and nothing else.

```rust
use incurs_app_gpui::DesktopApp;

DesktopApp::from_cli(&cli)?.title("Todo").run()
```

## What it contains

| Module | Responsibility |
| --- | --- |
| `form` | Lowers a Tool Contract's input schema to a form model, and coerces control state back to contract-typed JSON. No GPUI types. |
| `render` | Flattens a result value into labelled `DisplayRow`s. No GPUI types. |
| `runtime` | Bridges GPUI's foreground executor to a dedicated Tokio runtime, and streams `ToolEvent`s and the terminal outcome over one channel. |
| `skills` | Installs the same Agent Skills the CLI's `skills` command generates, through `incurs::sync_skills`. |
| `bundle` | Writes the macOS `.app` directory layout around a built executable. |
| `text_field` | A single-line text control with IME, grapheme-aware motion, and horizontal scrolling. |
| `theme` | Light and dark palettes, installed as a GPUI `Global`. |

`Workbench` is the root view. It owns the text controls and is the sole source
of truth for their contents; form state is written from the controls immediately
before arguments are collected, so a control and a command can never disagree.

## Schema to control

| Schema | Control |
| --- | --- |
| `string` | Text field |
| `integer`, `number` | Text field coerced to a number |
| `boolean` | Switch |
| `enum` of strings | Choice buttons |
| `array` of scalars | Text field, entries separated by commas |
| `object` or anything else | Raw JSON text field |

Required properties appear first, in `required` array order, which preserves the
authoring order of positional arguments. Local `$ref` pointers and nullable
`anyOf` unions are resolved, so `schemars`-derived typed commands get the same
controls as hand-written field metadata.

## Platform

GPUI is a large platform-specific dependency, which is why this crate lives in a
standalone workspace and the root workspace does not depend on it. Only macOS
bundling is implemented; Linux and Windows packaging are not.

See [the workspace README](../../README.md) for the full guide, the worked
example, and the current limits.
