# incurs GPUI extension

Ship an incurs command graph as a native desktop application.

A command defined once already serves a CLI, HTTP, MCP, and Code Mode. This
workspace adds one more surface: a window. The same commands, schemas,
validation, middleware, config defaults, and cancellation reach a person who
will never open a terminal.

```text
typed command definitions
          |
          v
shared command graph + schemas
  |       |       |       |              |
 CLI     HTTP     MCP   tool catalog   generated artifacts
                           |
                           +-- generic Code Mode
                           |
                           `-- [desktop window] (this workspace)
```

## Using it

```toml
[dependencies]
incurs = "0.8"
incurs-app-gpui = { path = "extensions/gpui/crates/incurs-app-gpui" }
```

```rust
use incurs_app_gpui::DesktopApp;

fn main() -> std::io::Result<()> {
    let cli = build_cli(); // your existing incurs CLI

    DesktopApp::from_cli(&cli)
        .expect("command names are unique")
        .title("Todo")
        .run()
}
```

One binary can serve both audiences. Open a window when started with no
arguments, and fall back to the CLI when given any:

```rust
if std::env::args().nth(1).is_some() {
    return tokio::runtime::Runtime::new()?.block_on(cli.serve());
}
DesktopApp::from_cli(&cli).unwrap().title("Todo").run()
```

## What the window shows

Commands are listed on the left, and the selected command's inputs are
collected from its Tool Contract's JSON Schema:

| Schema | Control |
| --- | --- |
| `string` | Text field |
| `integer`, `number` | Text field coerced to a number |
| `boolean` | Switch |
| `enum` of strings | Choice buttons |
| `array` of scalars | Text field, entries separated by commas |
| `object` or anything else | Raw JSON text field |

Required properties appear first, in the order the schema's `required` array
declares them, which preserves the authoring order of positional arguments. `$ref` pointers and
nullable `anyOf` unions are resolved, so `schemars`-derived typed commands get
the same controls as hand-written field metadata.

Results are rendered as labelled rows rather than JSON, with booleans as
Yes/No, empty collections named rather than shown as brackets, and the raw
value one click away. A command's follow-up suggestions render as next steps,
and a contract marked destructive shows a warning before it is run.

Streaming commands report progress and collected chunks while they run, and
`Stop` cancels through the same cooperative token the CLI uses.

## Agent skills

A command graph reaches coding agents through skill files. Shipping it as a
window should not cost it that, so the sidebar offers to install the same
skills the CLI's `skills` command generates, to the same detected agents.

`DesktopApp::from_cli` configures this automatically. Installation is offered
rather than performed, because it writes into the person's agent configuration
directories:

```rust
DesktopApp::from_cli(&cli)?
    .title("Todo")
    .install_skills_on_launch(true)   // optional; off by default
    .run()
```

Use `skills(None)` to hide it, or supply a publisher with a different scope:

```rust
use incurs_app_gpui::{SkillPublisher, SkillScope};

DesktopApp::from_cli(&cli)?
    .skills(Some(
        SkillPublisher::from_cli(&cli)
            .scope(SkillScope::Project)
            .directory("."),
    ))
    .run()
```

Skills are generated from the command graph at run time, so a shipped
application always installs skills that match the commands it actually has.

## Shipping to a non-technical person

A compiled binary is not installable by someone who does not use a terminal.
`bundle::MacBundle` wraps a built executable in the macOS `.app` layout:

```rust
use incurs_app_gpui::bundle::MacBundle;

MacBundle::new("Todo", "com.example.todo", "target/release/todo-desktop")
    .version("1.0.0")
    .icon("assets/todo.icns")
    .write("dist")?;
```

That produces `dist/Todo.app`, which opens from Finder and the Dock.
Distributing it beyond your own machine also requires code signing and
notarization, which are account-bound and deliberately left to you.

Only macOS bundling is implemented. Linux and Windows packaging are not.

## Running the example

```sh
cd extensions/gpui
cargo run -p todo-desktop              # opens the window
cargo run -p todo-desktop -- list --json
```

## Known limits

- Text editing is single-line. A list is entered as comma-separated values, and
  a JSON field is entered on one line. Multi-line editing would need wrapping
  and vertical caret motion, which this crate does not implement.
- Only leaf commands that are visible to MCP appear. A command whose MCP
  exposure is disabled is not shown.
- The window is built for one catalog. It does not host several applications.
