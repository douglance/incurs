//! Portable contract compiler prototype, isolated from production incurs.
mod form;
mod media;
mod model;
mod multipart;
mod parameters;
mod request_schema;
mod resolver;
mod schema;
mod schema_validation;
pub use resolver::resolve_document;
#[cfg(feature = "adapters")]
/// Native adapters for existing incurs runtime boundaries.
pub mod adapters;
mod artifacts;
#[cfg(feature = "adapters")]
/// Compile resolved APIs into incurs CLI commands and tool catalogs.
pub mod catalog;
/// Portable HTTP binding and host exchange contract.
pub mod runtime;
/// Effective server selection and declared-endpoint HTTP invocation.
pub mod servers;
pub use artifacts::*;
pub use model::*;
