//! Target-neutral contracts and explicit HTTP bindings.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A compiler result with a descriptive, deterministic failure.
pub type OpenApiResult<T> = Result<T, OpenApiError>;

/// A contract could not be compiled.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OpenApiError(pub String);

/// Inputs used to resolve a document without ambient network or filesystem access.
#[derive(Clone, Debug)]
pub struct ResolveOptions {
    /// Stable namespace used to qualify operation identities.
    pub namespace: String,
    /// URI identifying the root document.
    pub document_uri: String,
    /// Explicitly supplied external documents, keyed by URI.
    pub documents: BTreeMap<String, Value>,
    /// Ordered replacements applied to JSON pointers before resolution.
    pub overlays: Vec<Overlay>,
}

impl ResolveOptions {
    /// Start a resolution with an explicit namespace and no external documents.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            document_uri: "root.json".into(),
            documents: BTreeMap::new(),
            overlays: Vec::new(),
        }
    }
}

/// An explicit document modification applied before compilation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Overlay {
    /// JSON pointer selecting an existing value in the root document.
    pub pointer: String,
    /// Replacement for that value.
    pub value: Value,
}

/// A deterministic contract consumed by artifact compilers and transport bindings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResolvedOpenApi {
    /// OpenAPI version that determines the source Schema Object semantics.
    pub openapi_version: String,
    /// Default JSON Schema dialect explicitly declared by the source document.
    pub json_schema_dialect: Option<String>,
    /// Stable application namespace.
    pub namespace: String,
    /// Human-readable API title.
    pub title: String,
    /// First root server URL, unexpanded; use each operation's servers for execution.
    pub server_url: Option<String>,
    /// Operations in stable identity order.
    pub operations: Vec<Operation>,
    /// Named JSON schemas, including definitions needed by recursive references.
    pub schemas: BTreeMap<String, Value>,
}

/// A declared server URL with substitution variables.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Server {
    /// Absolute or document-relative URL template.
    pub url: String,
    /// Human-readable endpoint description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Variables indexed by their exact placeholder name.
    #[serde(default)]
    pub variables: BTreeMap<String, ServerVariable>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            url: "/".into(),
            description: None,
            variables: BTreeMap::new(),
        }
    }
}

/// A server URL variable, whose default participates in URL construction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServerVariable {
    /// Required substitution when the caller does not supply an alternative.
    pub default: String,
    /// Allowed substitutions, when the document restricts this variable.
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    /// Human-readable variable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An independently callable operation and its explicit HTTP binding.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Operation {
    /// Stable namespace-qualified identity.
    pub id: String,
    /// Unqualified operation name.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// HTTP method with its exact wire capitalization.
    pub method: String,
    /// URI path template.
    pub path: String,
    /// Effective operation/path/root server declarations, in source order.
    #[serde(default)]
    pub servers: Vec<Server>,
    /// Parameters retain their wire location and serialization rules.
    pub parameters: Vec<Parameter>,
    /// Optional request body contract.
    pub request_body: Option<RequestBody>,
    /// All declared responses, keyed by status or status pattern.
    pub responses: BTreeMap<String, ApiResponse>,
    /// Effective security requirements, inherited from the document when absent.
    pub security: Value,
}

/// One HTTP parameter; locations prevent collisions between equal wire names.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    /// Wire name.
    pub name: String,
    /// One of path, query, querystring, header, or cookie.
    pub location: String,
    /// Whether omission is forbidden.
    pub required: bool,
    /// Resolved JSON schema, retaining default and nullability information.
    pub schema: Value,
    /// OpenAPI serialization style.
    pub style: String,
    /// Whether arrays and objects expand into multiple wire values.
    pub explode: bool,
    /// Media-based serialization; schema remains the effective content schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ParameterContent>,
}

/// The single media representation declared by a content-based parameter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ParameterContent {
    /// Declared media type.
    pub media_type: String,
    /// Property encodings for URL-encoded form content.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub encoding: BTreeMap<String, Encoding>,
}

/// Body presence and media types are separate from the schema's nullability.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RequestBody {
    /// Whether the body must be present.
    pub required: bool,
    /// Schema per declared media type.
    pub content: BTreeMap<String, Value>,
    /// Explicit property encodings per media type; absence differs from explicit defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub encoding: BTreeMap<String, BTreeMap<String, Encoding>>,
}

/// A property's media representation or explicit query-style form serialization.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Encoding {
    /// Property media type when no style, explode, or allowReserved field is explicit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Explicit serialization style; absent defaults to form when style-based encoding is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Explicit expansion choice; presence selects style-based encoding even when false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    /// Reserved expansion choice; form delimiters remain escaped in values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_reserved: Option<bool>,
    /// Part headers, retained for multipart consumers and ignored for URL-encoded bodies.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, Value>,
    /// Additional fields, including specification extensions.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Response status, content, and headers are retained without selecting only one success.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ApiResponse {
    /// Human-readable response description.
    pub description: String,
    /// Schema per declared response media type.
    pub content: BTreeMap<String, Value>,
    /// Header definitions, keyed by wire name.
    pub headers: BTreeMap<String, Value>,
}
