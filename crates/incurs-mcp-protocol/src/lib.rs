//! Provider-neutral MCP standard profiles, wire codecs, and negotiation.
//!
//! The crate models published MCP standards as exact, simultaneously supported
//! profiles. It deliberately contains no transport provider or hosting runtime.

pub mod core;
mod generated;
pub mod negotiation;
pub mod standards;
pub mod structured;
pub mod wire;

pub use core::{McpClientMetadata, McpLifecycleFamily, McpVersion};
pub use negotiation::{
    McpNegotiationDecision, McpNegotiationError, McpNegotiationEvidence, McpNegotiationScope,
    McpPriorNegotiation, McpStandardSet, negotiate,
};
pub use standards::{
    McpStandardFeatures, McpStandardProfile, UNSUPPORTED_PROTOCOL_VERSION, known_standard,
    known_standards,
};
pub use wire::{McpValidation, McpWireCodec, McpWireCodecKind, codec};
