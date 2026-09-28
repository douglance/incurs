pub mod agent_plugin;
#[cfg(feature = "agent-plugins-mcp")]
pub mod agent_plugin_runtime;
#[cfg(feature = "agent-plugins-mcp")]
mod agent_plugin_sse;
pub mod agents;
pub mod cli;
pub mod command;
pub mod completions;
pub mod config;
pub mod config_schema;
pub mod errors;
pub mod fetch;
pub mod filter;
pub mod formatter;
pub mod help;
#[cfg(feature = "http")]
pub mod http;
pub mod mcp;
#[cfg(any(feature = "http", feature = "agent-plugins-mcp"))]
pub mod mcp_client;
pub mod middleware;
pub mod openapi;
pub mod outbound;
pub mod output;
pub mod pager;
pub mod parser;
pub mod schema;
pub mod skill;
pub mod streaming;
pub mod sync_mcp;
pub mod sync_skills;
pub mod tool;

// Re-export derive macros so users can write `#[derive(incurs::Args)]`
pub use incurs_macros::{IncurArgs as Args, IncurEnv as Env, IncurOptions as Options};

/// Every process environment variable, or none on wasm32.
///
/// `std::env::vars` panics on wasm32-unknown-unknown rather than returning an
/// empty iterator, so every whole-environment read goes through here. A wasm32
/// host such as a Cloudflare Worker supplies its environment explicitly.
pub(crate) fn process_env<C: FromIterator<(String, String)>>() -> C {
    #[cfg(target_arch = "wasm32")]
    {
        std::iter::empty().collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::disallowed_methods)]
    {
        std::env::vars().collect()
    }
}

/// [`process_env`] with platform strings, for subprocess environments.
#[cfg(feature = "agent-plugins-mcp")]
pub(crate) fn process_env_os<C: FromIterator<(std::ffi::OsString, std::ffi::OsString)>>() -> C {
    #[cfg(target_arch = "wasm32")]
    {
        std::iter::empty().collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::disallowed_methods)]
    {
        std::env::vars_os().collect()
    }
}
