//! Cloudflare WorkerLoader and Durable Object adapters for incurs Code Mode.
//!
//! The public API remains Rust. This crate emits a small internal JavaScript
//! module because Dynamic Workers currently accept JavaScript or Python source,
//! not a nested Rust/Wasm Worker.

mod module;

pub use module::{DynamicWorkerOptions, build_executor_module};

#[cfg(target_arch = "wasm32")]
mod executor;
#[cfg(target_arch = "wasm32")]
mod sql_store;

#[cfg(target_arch = "wasm32")]
pub use executor::{CloudflareClock, DynamicWorkerExecutor, WorkerLoader};
#[cfg(target_arch = "wasm32")]
pub use sql_store::DurableSqlStore;
