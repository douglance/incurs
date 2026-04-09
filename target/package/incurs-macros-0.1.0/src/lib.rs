//! Derive macros for the incurs CLI framework.
//!
//! Provides three derive macros that generate [`incur::schema::IncurSchema`] implementations:
//!
//! - **`IncurArgs`** — positional arguments
//! - **`IncurOptions`** — named options and flags
//! - **`IncurEnv`** — environment-variable bindings
//!
//! All three macros read doc comments as field descriptions and support `#[incurs(...)]`
//! attributes for fine-grained control.

mod args;
mod common;
mod env;
mod options;

use proc_macro::TokenStream;

/// Derive `IncurSchema` for a positional-argument struct.
///
/// # Example
///
/// ```ignore
/// #[derive(IncurArgs, serde::Deserialize, schemars::JsonSchema)]
/// struct GetArgs {
///     /// The user ID to fetch
///     id: u64,
///     /// Optional format override
///     format: Option<String>,
/// }
/// ```
///
/// Fields are treated as positional args in declaration order. `Option<T>` fields
/// are optional; all others are required.
#[proc_macro_derive(IncurArgs, attributes(incurs))]
pub fn derive_incur_args(input: TokenStream) -> TokenStream {
    args::derive(input)
}

/// Derive `IncurSchema` for a named-options struct.
///
/// # Supported `#[incurs(...)]` attributes
///
/// | Attribute | Effect |
/// |-----------|--------|
/// | `alias = "x"` | Single-char short alias (e.g. `-n`) |
/// | `default = <expr>` | Default value (literal) |
/// | `count` | Marks a field as a count flag (`-vvv` → 3) |
/// | `deprecated` | Marks the option as deprecated |
///
/// # Example
///
/// ```ignore
/// #[derive(IncurOptions, serde::Deserialize, schemars::JsonSchema)]
/// struct ListOptions {
///     /// Maximum number of results
///     #[incurs(alias = "n", default = 10)]
///     limit: u32,
///     /// Include archived items
///     #[incurs(alias = "a")]
///     archived: bool,
///     /// Filter by tag (repeatable)
///     tag: Vec<String>,
///     /// Verbosity level
///     #[incurs(count)]
///     verbose: u8,
/// }
/// ```
#[proc_macro_derive(IncurOptions, attributes(incurs))]
pub fn derive_incur_options(input: TokenStream) -> TokenStream {
    options::derive(input)
}

/// Derive `IncurSchema` for an environment-variable binding struct.
///
/// # Supported `#[incurs(...)]` attributes
///
/// | Attribute | Effect |
/// |-----------|--------|
/// | `env = "VAR_NAME"` | Environment variable to read (required) |
/// | `default = <expr>` | Default value when the variable is unset |
///
/// # Example
///
/// ```ignore
/// #[derive(IncurEnv, serde::Deserialize, schemars::JsonSchema)]
/// struct AppEnv {
///     /// API token for authentication
///     #[incurs(env = "API_TOKEN")]
///     api_token: String,
///     /// Base URL
///     #[incurs(env = "BASE_URL", default = "https://api.example.com")]
///     base_url: String,
/// }
/// ```
#[proc_macro_derive(IncurEnv, attributes(incurs))]
pub fn derive_incur_env(input: TokenStream) -> TokenStream {
    env::derive(input)
}
