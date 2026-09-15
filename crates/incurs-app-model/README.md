# incurs-app-model

The toolkit-neutral interaction model behind incurs' interactive surfaces.

A desktop window and a terminal application need the same things: the commands a
[`ToolCatalog`](https://docs.rs/incurs) exposes, a form lowered from each command's
input schema, the collected values coerced back to the types the contract declares, a
result flattened into labelled rows, and the state of the call in flight.

This crate is all of that and no user-interface dependency at all.

| Module | Responsibility |
| --- | --- |
| `form` | Lowers an input JSON Schema to an ordered form model, and coerces control state back to contract-typed JSON |
| `rows` | Flattens a result value into labelled `DisplayRow`s |
| `runtime` | Runs calls on a dedicated Tokio runtime and streams ordered updates over a channel a UI thread can drain |
| `session` | `AppSession` — the command list, selection, values, and run state machine |
| `skills` | Installs the Agent Skills the command graph generates |

## Using it

```rust,no_run
# use std::sync::Arc;
# use incurs_app_model::{AppSession, CallEnvironment, ToolRunner};
# fn example(catalog: incurs::tool::ToolCatalog) -> std::io::Result<()> {
let runner = Arc::new(ToolRunner::new(catalog, CallEnvironment::Host)?);
let mut session = AppSession::new(runner);

session.select_command("greet");
session.state_mut().value_mut("name").text = "Ada".to_string();

if let Ok(handle) = session.start_run() {
    // Pump `handle.updates` and feed each one to `session.receive(...)`
    // until you observe `RunUpdate::Finished`.
}
# Ok(())
# }
```

A view owns its own controls and its own event pump. It drives the session and draws
what it reads back.

## One rule worth knowing

**The view owns text; the model does not.** A text control is the source of truth for
its own buffer, and the view writes it into the form state immediately before starting
a call. A model that also held text would let the two disagree — and a test that set
both would pass while proving nothing.

Toggles and choices have no text buffer, so they live in the form state and are driven
through `state_mut()`.
