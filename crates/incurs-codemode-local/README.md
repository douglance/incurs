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

`LocalExecutorOptions` bounds execution time, QuickJS heap usage, and stack
size. Replace `MemoryStore` with another `RuntimeStore` when local executions
must survive process restarts.
