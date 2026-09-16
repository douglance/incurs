# incurs-app-ratatui

Run an incurs command graph as a full-screen terminal application.

```rust,no_run
use incurs_app_ratatui::TerminalApp;

# fn example(cli: &incurs::cli::Cli) -> Result<(), Box<dyn std::error::Error>> {
TerminalApp::from_cli(cli)?.title("Todo").run()?;
# Ok(())
# }
```

```text
┌ Commands (5) ──────────┐┌ Add ──────────────────────────────────────┐
│▸ Add                   ││Add a new todo item                        │
│  Complete              ││                                           │
│  List                  ││▸ Title         [Buy milk             ] *  │
│  Watch                 ││  Done          [ ]                        │
│  Clear              ⚠  ││  Priority      ‹ low ·medium· high ›      │
│                        ││  Tags          [home, urgent         ]    │
├────────────────────────┴┴───────────────────────────────────────────┤
│ Result                                                              │
│  Id              7                                                  │
│  Title           Buy milk                                           │
└ tab focus · ↑↓ move · ^R run · ^K skills · / search · ? help ───────┘
```

Commands are listed on the left, the selected command's inputs are collected from its
schema, and every call goes through `ToolCatalog` — so validation, middleware, config
defaults, streaming, and cancellation behave as they do on the CLI.

## Keys

| Key | Does |
| --- | --- |
| `tab` / `shift-tab` | Move between the command list and the form |
| `↑` `↓` | Move within a pane |
| `←` `→` | Change a choice |
| `space` | Flip a switch, or change a choice |
| `enter` | Run the selected command |
| `^R` | Run |
| `^C` | Cancel a run, or quit when idle |
| `^K` | Install agent skills |
| `/` | Search commands |
| `?` | Help |
| `esc` | Clear the search, or quit |

## Controls

The control for each input comes from the command's schema, through
[`incurs-app-model`](../incurs-app-model), which the desktop surface shares.

| Schema | Control |
| --- | --- |
| `string` | Text field |
| `integer`, `number` | Text field coerced to a number |
| `boolean` | Checkbox |
| `enum` of strings | Cycling choice |
| `array` of scalars | Text field, entries separated by commas |
| `object` or anything else | Raw JSON text field |

## Start it from synchronous code

`run()` builds a Tokio runtime for calls, and Tokio panics when a runtime is dropped
inside another one. Call it from a `main` that is **not** `#[tokio::main]`:

```rust,no_run
# use incurs_app_ratatui::TerminalApp;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = build_cli();

    // The same binary still serves a CLI when given arguments.
    if std::env::args().nth(1).is_some() {
        return tokio::runtime::Runtime::new()?.block_on(cli.serve());
    }

    TerminalApp::from_cli(&cli)?.title("Todo").run()?;
    Ok(())
}
# fn build_cli() -> incurs::cli::Cli { unimplemented!() }
```

## The example

```sh
cargo run -p incurs-app-ratatui --example todo_tui
```

Its command graph deliberately carries a required argument, a closed choice, a switch,
a list, a repeat counter, a streaming command, a destructive one, and one that suggests
a follow-up — the shapes that expose a wrong control. A fixture holding only strings
proves nothing about a form.

## Limits

- Text editing is single-line. A list is entered as comma-separated values and a JSON
  value on one line.
- Only leaf commands visible to MCP appear.
- No mouse support.
