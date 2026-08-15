# incurs Code Mode local executor

`incurs-codemode-local` runs the generic `incurs-codemode::CodeMode` lifecycle in
an isolated QuickJS runtime. It uses the same connector, approval, replay,
rollback, search, and value-encoding contracts as remote executors.

```rust
use std::sync::Arc;

use incurs::cli::Cli;
use incurs_codemode::{
    CodeMode, ExecutionState, IncurConnector, MemoryStore,
};
use incurs_codemode_local::LocalExecutor;

async fn execute(cli: &Cli) -> Result<ExecutionState, String> {
    let codemode = CodeMode::new(
        Arc::new(MemoryStore::default()),
        LocalExecutor::default(),
        vec![Arc::new(IncurConnector::new(cli.tool_catalog()))],
    );
    codemode.execute("tools.lookup({ id: 42 })").await
}
```

Run the complete local example:

```sh
cargo run -p incurs-codemode-local --example local
```

`LocalExecutorOptions` bounds uninterrupted JavaScript execution time, QuickJS
heap usage, and stack size. The local executor renews the JavaScript timeout
immediately before a host dispatch and after every host completion, so slow
connector work does not consume the QuickJS runaway-loop budget. Replace
`MemoryStore` with another `RuntimeStore` when local executions must survive
process restarts.

Runaway JavaScript terminates the pass with this stable error contract:
`CODE_EXECUTION_TIMEOUT: maximum uninterrupted JavaScript execution interval exceeded`.
