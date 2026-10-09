//! Portable OpenAI MCP extension wire models and validators.
//!
//! The public types in this crate mirror the schema-bearing exports from the
//! pinned OpenAI MCP Extensions TypeScript and Python SDKs. Runtime helpers use
//! [`validate_schema`] when they need an export-name registry, and use the typed
//! models directly when Rust code owns the call boundary.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use regress::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use thiserror::Error;

/// Metadata key used for host-owned OpenAI resources.
pub const OPENAI_RESOURCE_METADATA_KEY: &str = "openai/resource";
/// Server capability key advertising OpenAI settings tools.
pub const OPENAI_SETTINGS_CAPABILITY_KEY: &str = "openai/settings";
/// App host-context key for model-context state and update metadata.
pub const OPENAI_MODEL_CONTEXT_KEY: &str = "openai/modelContext";
/// App host-context key for message send options.
pub const OPENAI_MESSAGE_KEY: &str = "openai/message";
/// App host-context key for deep-link activation state.
pub const OPENAI_DEEP_LINK_KEY: &str = "openai/deepLink";
/// App request method for opening a local host file.
pub const OPENAI_FILE_OPEN_METHOD: &str = "openai/files/open";
/// App request method for writing a host-managed resource.
pub const OPENAI_MCP_APP_RESOURCE_WRITE_METHOD: &str = "openai/resources/write";
/// Client capability key for OpenAI form elicitation.
pub const OPENAI_ELICITATION_EXTENSION_ID: &str = "openai/elicitation";
/// Client request method for OpenAI form elicitation.
pub const OPENAI_ELICITATION_METHOD: &str = "openai/elicitation/create";
/// JSON Schema extension key used by OpenAI resource and file form inputs.
pub const OPENAI_INPUT_KEY: &str = "x-openai-input";

/// Export names accepted by [`validate_schema`].
pub const SUPPORTED_SCHEMA_NAMES: &[&str] = &[
    "OpenAIFileEntrypointInputSchema",
    "OpenAIResourceToolCallMetadataSchema",
    "OpenAIMentionResourceSchema",
    "OpenAIMentionItemSchema",
    "OpenAIMentionSearchParamsSchema",
    "OpenAIMentionSearchResultSchema",
    "OpenAIUiQuickActionSchema",
    "OpenAIUiEntrypointSchema",
    "OpenAIUiToolMetadataSchema",
    "OpenAIUiResourceMetadataSchema",
    "OpenAISettingsCapabilitySchema",
    "OpenAISettingsPropertySchema",
    "OpenAISettingsToolSchema",
    "OpenAISettingsGroupSchema",
    "OpenAISettingsLayoutItemSchema",
    "OpenAISettingsFieldPresentationSchema",
    "OpenAISettingsReadResultSchema",
    "OpenAISettingsUpdateArgumentsSchema",
    "OpenAISettingsUpdateResultSchema",
    "OpenAIFileFormFieldSchema",
    "OpenAIFormFieldSchema",
    "OpenAIFormSchema",
    "OpenAIFormResultSchema",
    "OpenAIFileOpenParamsSchema",
    "OpenAIDeepLinkHostStateSchema",
    "OpenAIModelContextMetadataSchema",
    "OpenAIModelContextHostStateSchema",
    "OpenAIMessageOptionsSchema",
    "OpenAIMessageParamsSchema",
    "OpenAIResourceRepresentationSchema",
    "OpenAIResourceReadMetadataSchema",
    "OpenAIResourceMetadataSchema",
    "OpenAIResourceContentMetadataSchema",
    "OpenAIResourceWriteParamsSchema",
    "OpenAIResourceWriteResultSchema",
];

/// JSON object type used for forward-compatible MCP payload fragments.
pub type JsonObject = BTreeMap<String, Value>;

/// Error returned when a named OpenAI schema rejects a value.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{schema}: {message}")]
pub struct ValidationError {
    /// Name of the schema being validated.
    pub schema: &'static str,
    /// JSON pointer-like path to the rejected value.
    pub path: String,
    /// Human-readable validation failure.
    pub message: String,
}

impl ValidationError {
    /// Creates a validation error at a JSON-like path for a named schema.
    pub fn new(schema: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            schema,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Result alias for OpenAI extension validation.
pub type ValidationResult<T> = Result<T, ValidationError>;

/// Tool metadata key used by OpenAI extension discovery.
pub const OPENAI_EXTENSIONS_META_KEY: &str = "openai/extensions";
/// Tool and resource metadata key used by OpenAI UI hints.
pub const OPENAI_UI_META_KEY: &str = "openai/ui";

/// Compatibility alias using the server/app facade naming style.
pub type OpenAiMentionSearchParams = OpenAIMentionSearchParams;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiMentionSearchResult = OpenAIMentionSearchResult;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsCapability = OpenAISettingsCapability;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsFieldPresentation = OpenAISettingsFieldPresentation;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsGroup = OpenAISettingsGroup;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsReadResult = OpenAISettingsReadResult;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsUpdateArguments = OpenAISettingsUpdateArguments;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiSettingsUpdateResult = OpenAISettingsUpdateResult;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiDeepLinkHostState = OpenAIDeepLinkHostState;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiFileOpenParams = OpenAIFileOpenParams;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiMessageParams = OpenAIMessageParams;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiModelContextHostState = OpenAIModelContextState;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiResourceRepresentation = OpenAIResourceRepresentation;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiResourceWriteParams = OpenAIResourceWriteParams;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiResourceWriteResult = OpenAIResourceWriteResult;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiForm = OpenAIForm;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiFormField = OpenAIFormField;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiFormOption = OpenAIFormOption;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiFormRequestParams = OpenAIFormRequestParams;
/// Compatibility alias using the server/app facade naming style.
pub type OpenAiFormResult = OpenAIFormResult;
/// Python form-protocol base model equivalent for JSON object payloads.
pub type FormModel = JsonObject;
/// Python form-protocol form-field equivalent.
pub type FormField = OpenAIFormField;
/// Python form-protocol form-schema equivalent.
pub type FormSchema = OpenAIForm;
/// Python resource picker user-options equivalent.
pub type UserResourceOptions = OpenAIUserResourceOptions;
/// Python file picker user-options alias.
pub type FileUserOptions = OpenAIUserResourceOptions;

/// A minimal MCP icon object accepted by OpenAI extension metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Icon {
    /// Icon source URI.
    pub src: String,
    /// Additional MCP icon fields preserved for host-specific rendering.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: JsonObject,
}

/// A minimal MCP resource object used in resource-picker schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Resource {
    /// Resource URI.
    pub uri: String,
    /// Resource display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Resource MIME type.
    #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Additional MCP resource fields.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: JsonObject,
}

/// A minimal MCP resource link item accepted in mention search results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceLink {
    /// MCP content type discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Linked resource URI.
    pub uri: String,
    /// Optional title or name supplied by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Additional resource-link fields.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: JsonObject,
}

/// Arguments injected when an MCP App opens from a file entrypoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFileEntrypointInput {
    /// Selected file.
    pub file: OpenAIFileEntrypointFile,
}

/// Selected file supplied by an OpenAI file entrypoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFileEntrypointFile {
    /// Host display name for the selected file.
    pub name: String,
    /// Host-managed MCP resource URI for the selected file.
    #[serde(rename = "resourceUri")]
    pub resource_uri: String,
}

/// Host-owned resource context carried on tool calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceToolCallMetadata {
    /// OpenAI resource metadata, if present.
    #[serde(
        default,
        rename = "openai/resource",
        skip_serializing_if = "Option::is_none"
    )]
    pub resource: Option<OpenAIResourceToolCallPathMetadata>,
}

/// Host-owned resource path metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceToolCallPathMetadata {
    /// Path in the MCP server host filesystem namespace.
    pub path: String,
}

/// Reads the host-owned file path from tool-call metadata.
pub fn get_resource_path(meta: &Value) -> Result<Option<String>, ValidationError> {
    let metadata: OpenAIResourceToolCallMetadata =
        serde_json::from_value(meta.clone()).map_err(|error| {
            ValidationError::new(
                "OpenAIResourceToolCallMetadataSchema",
                "",
                error.to_string(),
            )
        })?;
    Ok(metadata.resource.map(|resource| resource.path))
}

/// One MCP resource returned from mention search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIMentionResource {
    /// Result discriminator.
    #[serde(rename = "type")]
    pub kind: MentionResourceKind,
    /// Referenced resource URI.
    #[serde(rename = "resourceUri")]
    pub resource_uri: String,
    /// Display title.
    pub title: String,
    /// Optional display subtitle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Optional icons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

/// Discriminator for OpenAI mention resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MentionResourceKind {
    /// OpenAI resource mention item.
    Resource,
}

/// One result returned from mention search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OpenAIMentionItem {
    /// OpenAI-specific resource mention.
    Resource(OpenAIMentionResource),
    /// Standard MCP resource link.
    ResourceLink(ResourceLink),
}

/// Request payload for OpenAI mention search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIMentionSearchParams {
    /// Raw query string, including the empty string while the user has not typed.
    pub query: String,
}

/// Response payload for OpenAI mention search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIMentionSearchResult {
    /// Search result items.
    pub items: Vec<OpenAIMentionItem>,
}

/// A sidebar shortcut that invokes a tool in the same MCP App.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIUiQuickAction {
    /// Shortcut title.
    pub title: String,
    /// One or more shortcut icons.
    pub icons: Vec<Icon>,
    /// Target tool invocation.
    pub target: OpenAIUiQuickActionToolTarget,
}

/// Tool target for a UI quick action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIUiQuickActionToolTarget {
    /// Target discriminator.
    #[serde(rename = "type")]
    pub kind: UiQuickActionTargetKind,
    /// Tool name on the same MCP server.
    pub name: String,
    /// Fixed JSON arguments supplied with the shortcut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}

/// UI quick-action target discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UiQuickActionTargetKind {
    /// Same-server MCP tool.
    Tool,
}

/// One OpenAI app entrypoint layered onto an MCP App tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpenAIUiEntrypoint {
    /// Open an MCP App when a supported file is selected.
    File {
        /// File extensions, each beginning with a dot.
        extensions: Vec<String>,
    },
    /// Open an MCP App from the global sidebar.
    Global {
        /// Optional quick action attached to the global entrypoint.
        #[serde(
            default,
            rename = "quickAction",
            skip_serializing_if = "Option::is_none"
        )]
        quick_action: Option<OpenAIUiQuickAction>,
    },
    /// Open an MCP App from settings search.
    Settings {
        /// Search terms that surface the entrypoint in settings.
        #[serde(
            default,
            rename = "searchTerms",
            skip_serializing_if = "Option::is_none"
        )]
        search_terms: Option<Vec<String>>,
    },
    /// Open an MCP App from a conversation side panel.
    Thread,
}

/// Python form-protocol MCP App tool preview target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpAppToolTarget {
    /// Target discriminator.
    #[serde(rename = "type")]
    pub kind: McpAppToolTargetKind,
    /// MCP App tool name.
    pub name: String,
    /// Fixed tool arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}

/// MCP App tool target discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpAppToolTargetKind {
    /// MCP App tool target.
    McpAppTool,
}

/// Python form-protocol preview target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PreviewTarget {
    /// Same-app MCP tool target.
    McpAppTool(McpAppToolTarget),
    /// MCP resource link target.
    ResourceLink(ResourceLink),
}

/// OpenAI tool metadata carried in `_meta["openai/ui"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIUiToolMetadata {
    /// App entrypoints advertised by the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoints: Option<Vec<OpenAIUiEntrypoint>>,
    /// Preferred model display mode for tool output.
    #[serde(
        default,
        rename = "preferredModelDisplayMode",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_model_display_mode: Option<ModelDisplayMode>,
}

/// OpenAI resource content metadata carried in `_meta["openai/ui"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIUiResourceMetadata {
    /// Display modes supported by this resource.
    #[serde(
        default,
        rename = "availableDisplayModes",
        skip_serializing_if = "Option::is_none"
    )]
    pub available_display_modes: Option<Vec<ResourceDisplayMode>>,
    /// Preferred display mode for this resource.
    #[serde(
        default,
        rename = "preferredDisplayMode",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_display_mode: Option<ResourceDisplayMode>,
}

/// Model-context tool output display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModelDisplayMode {
    /// Inline display.
    Inline,
    /// Full-screen display.
    Fullscreen,
}

/// Resource display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceDisplayMode {
    /// Inline display.
    Inline,
    /// Full-screen display.
    Fullscreen,
    /// Picture-in-picture display.
    Pip,
}

/// Server capability extension identifying settings tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAISettingsCapability {
    /// Name of the settings read tool on the same MCP server.
    #[serde(rename = "readTool")]
    pub read_tool: String,
    /// Name of the settings update tool on the same MCP server.
    #[serde(rename = "updateTool")]
    pub update_tool: String,
}

/// A reference to a settings value field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAISettingsProperty {
    /// Layout item kind.
    pub kind: SettingsPropertyKind,
    /// Referenced settings property key.
    pub property: String,
}

/// Discriminator for a settings property item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettingsPropertyKind {
    /// Settings property reference.
    Property,
}

/// A button invoking a same-server settings tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAISettingsTool {
    /// Layout item kind.
    pub kind: SettingsToolKind,
    /// Tool name.
    pub tool: String,
    /// Button title.
    pub title: String,
    /// Button description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Discriminator for a settings tool item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettingsToolKind {
    /// Settings tool button.
    Tool,
}

/// A settings group layout item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAISettingsGroup {
    /// Layout item kind.
    pub kind: SettingsGroupKind,
    /// Group title.
    pub title: String,
    /// Ordered group contents.
    pub items: Vec<OpenAISettingsLayoutLeaf>,
}

/// Discriminator for a settings group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettingsGroupKind {
    /// Settings group.
    Group,
}

/// A settings group item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum OpenAISettingsLayoutLeaf {
    /// Field reference.
    Property {
        /// Referenced settings property.
        property: String,
    },
    /// Same-server tool button.
    Tool {
        /// Tool name.
        tool: String,
        /// Button title.
        title: String,
        /// Button description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

/// Settings layout item.
pub type OpenAISettingsLayoutItem = OpenAISettingsGroup;

/// Presentation annotations for settings fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAISettingsFieldPresentation {
    /// Display title.
    pub title: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Read tool structured content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAISettingsReadResult {
    /// JSON Schema for settings values.
    pub schema: JsonObject,
    /// Optional settings layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Vec<OpenAISettingsLayoutItem>>,
    /// Effective values.
    pub values: JsonObject,
}

/// Update tool input arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAISettingsUpdateArguments {
    /// Partial settings values to replace.
    pub set: JsonObject,
}

/// Update tool structured content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAISettingsUpdateResult {
    /// Effective values after persistence.
    pub values: JsonObject,
}

/// File open request parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIFileOpenParams {
    /// Local host path to open.
    pub path: String,
}

/// Deep-link host state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIDeepLinkHostState {
    /// Normalized deep-link URL.
    pub url: String,
}

/// Legacy host-state representation normalized into [`OpenAIDeepLinkHostState`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LegacyOpenAIDeepLinkHostState {
    /// Path segments without slashes.
    pub path: Vec<String>,
    /// Query key/value pairs.
    pub query: Vec<(String, String)>,
}

/// Model-context update metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIModelContextMetadata {
    /// OpenAI model-context metadata.
    #[serde(rename = "openai/modelContext")]
    pub model_context: OpenAIModelContextUpdateResult,
}

/// Model-context update result metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIModelContextUpdateResult {
    /// Host-assigned update identifier.
    #[serde(rename = "updateId")]
    pub update_id: String,
}

/// Model-context host state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OpenAIModelContextHostState {
    /// Active host model context.
    State(OpenAIModelContextState),
    /// Explicitly cleared host model context.
    Null(()),
}

/// Active model-context host state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIModelContextState {
    /// Optional model-visible content blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Value>>,
    /// Optional structured content.
    #[serde(
        default,
        rename = "structuredContent",
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<JsonObject>,
    /// Host update identifier.
    #[serde(rename = "updateId")]
    pub update_id: String,
}

/// Message send options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIMessageOptions {
    /// Conversation target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<OpenAIMessageTarget>,
    /// Whether the message is immediately sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<bool>,
}

/// Message target conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIMessageTarget {
    /// Active conversation.
    Active,
    /// New conversation.
    New,
}

/// Parameters for sending a user message through the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIMessageParams {
    /// Message role. OpenAI extensions only send user messages.
    pub role: String,
    /// Message content blocks.
    pub content: Vec<Value>,
    /// Optional request metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

/// Resource representation requested when reading a host-managed resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIResourceRepresentation {
    /// Binary base64 representation.
    Blob,
    /// Text representation.
    Text,
}

/// OpenAI metadata carried in params on MCP resource read requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceReadMetadata {
    /// OpenAI resource read metadata.
    #[serde(
        default,
        rename = "openai/resource",
        skip_serializing_if = "Option::is_none"
    )]
    pub resource: Option<OpenAIResourceReadOptions>,
}

/// Resource read options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceReadOptions {
    /// Requested representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<OpenAIResourceRepresentation>,
}

/// OpenAI metadata attached to a host-managed resource content item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceMetadata {
    /// Entity tag for optimistic concurrency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Whether the host accepts writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
}

/// OpenAI metadata returned on host-managed MCP resource contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceContentMetadata {
    /// OpenAI resource metadata.
    #[serde(
        default,
        rename = "openai/resource",
        skip_serializing_if = "Option::is_none"
    )]
    pub resource: Option<OpenAIResourceMetadata>,
}

/// Parameters for the OpenAI resource write request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OpenAIResourceWriteParams {
    /// Write base64 blob content.
    Blob(OpenAIBlobResourceWriteParams),
    /// Write text content.
    Text(OpenAITextResourceWriteParams),
}

/// Base64 blob resource write parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAIBlobResourceWriteParams {
    /// Target resource URI.
    pub uri: String,
    /// Optional entity tag guard.
    #[serde(default, rename = "ifMatch", skip_serializing_if = "Option::is_none")]
    pub if_match: Option<String>,
    /// Base64-encoded binary body.
    pub blob: String,
}

/// Text resource write parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenAITextResourceWriteParams {
    /// Target resource URI.
    pub uri: String,
    /// Optional entity tag guard.
    #[serde(default, rename = "ifMatch", skip_serializing_if = "Option::is_none")]
    pub if_match: Option<String>,
    /// Text body.
    pub text: String,
}

/// Result of the OpenAI resource write request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum OpenAIResourceWriteResult {
    /// Write failed due to an entity-tag conflict.
    Conflict {
        /// Current entity tag.
        etag: String,
    },
    /// Write succeeded.
    Saved {
        /// New entity tag.
        etag: String,
    },
    /// Write was rejected because the body was too large.
    TooLarge {
        /// Maximum accepted byte count.
        #[serde(rename = "maxBytes")]
        max_bytes: f64,
    },
}

/// One selectable option used by form select fields and suggestions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFormOption {
    /// Submitted value for this option.
    #[serde(rename = "const")]
    pub const_value: String,
    /// User-visible option title.
    pub title: String,
    /// Optional user-visible option description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Preferred option thumbnail.
    #[serde(
        default,
        rename = "x-openai-thumbnail",
        skip_serializing_if = "Option::is_none"
    )]
    pub thumbnail: Option<Icon>,
    /// Deprecated option preview alias accepted by the SDK.
    #[serde(
        default,
        rename = "x-openai-preview",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview: Option<Icon>,
}

/// User-selected file or directory options for an OpenAI form resource input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIUserResourceOptions {
    /// File or directory picker kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<OpenAIUserResourceKind>,
    /// HTML accept tokens for host-supplied uploads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
}

/// User resource picker kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIUserResourceKind {
    /// File picker.
    File,
    /// Directory picker.
    Directory,
}

/// OpenAI resource-picker metadata attached to a form field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIResourceInput {
    /// Input discriminator, currently `resource` or legacy `file`.
    #[serde(rename = "type")]
    pub input_type: String,
    /// Server-supplied resource options.
    pub options: Vec<Resource>,
    /// Optional host file/directory picker options.
    #[serde(
        default,
        rename = "userOptions",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_options: Option<OpenAIUserResourceOptions>,
    /// Selection mode for array resource inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<OpenAIResourceSelection>,
}

/// Resource selection mode for a resource-picker form field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIResourceSelection {
    /// User must explicitly select resources.
    Explicit,
    /// Host may implicitly provide selected resources.
    Implicit,
}

/// Item schema for an OpenAI array form field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFormArrayItems {
    /// Item type, usually `string` for OpenAI forms.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// Enum choices for legacy multi-select fields.
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// Titled choices for modern multi-select fields.
    #[serde(default, rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<OpenAIFormOption>>,
    /// Suggested string values for free-text arrays.
    #[serde(
        default,
        rename = "x-openai-suggestions",
        skip_serializing_if = "Option::is_none"
    )]
    pub suggestions: Option<Vec<OpenAIFormOption>>,
    /// Optional item format such as `uri`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Optional item minimum UTF-16 length.
    #[serde(default, rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// Optional item maximum UTF-16 length.
    #[serde(default, rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// Optional item ECMA-262 pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

/// One field in an OpenAI form schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFormField {
    /// JSON Schema field type.
    #[serde(rename = "type")]
    pub field_type: String,
    /// Optional field title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional field description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// String, numeric, or boolean enum choices.
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<Value>>,
    /// Optional legacy enum display labels.
    #[serde(default, rename = "enumNames", skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,
    /// Titled single-select choices.
    #[serde(default, rename = "oneOf", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<OpenAIFormOption>>,
    /// Suggested free-text values.
    #[serde(
        default,
        rename = "x-openai-suggestions",
        skip_serializing_if = "Option::is_none"
    )]
    pub suggestions: Option<Vec<OpenAIFormOption>>,
    /// Array item schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<OpenAIFormArrayItems>,
    /// Minimum numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Maximum numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Minimum UTF-16 string length.
    #[serde(default, rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// Maximum UTF-16 string length.
    #[serde(default, rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// ECMA-262 string pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// String format such as `email`, `uri`, `date`, or `date-time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Minimum array length.
    #[serde(default, rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    /// Maximum array length.
    #[serde(default, rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    /// Whether array items must be unique.
    #[serde(
        default,
        rename = "uniqueItems",
        skip_serializing_if = "Option::is_none"
    )]
    pub unique_items: Option<bool>,
    /// OpenAI resource-picker metadata.
    #[serde(
        default,
        rename = "x-openai-input",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_input: Option<OpenAIResourceInput>,
}

/// OpenAI file form field alias.
pub type OpenAIFileFormField = OpenAIFormField;

/// OpenAI form schema sent in form elicitation requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIForm {
    /// Optional JSON Schema URI.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema_uri: Option<String>,
    /// Form root type, always `object`.
    #[serde(rename = "type")]
    pub form_type: String,
    /// Required submitted field names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Field definitions keyed by submitted property name.
    pub properties: BTreeMap<String, OpenAIFormField>,
}

/// Parameters for an OpenAI form elicitation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpenAIFormRequestParams {
    /// Elicitation mode, always `form`.
    pub mode: String,
    /// Prompt shown by the host.
    pub message: String,
    /// Requested form schema.
    #[serde(rename = "requestedSchema")]
    pub requested_schema: OpenAIForm,
}

/// Result returned by an OpenAI form elicitation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum OpenAIFormResult {
    /// User accepted and submitted form content.
    Accept {
        /// Submitted content keyed by field name.
        content: JsonObject,
    },
    /// User canceled the request.
    Cancel,
    /// User declined the request.
    Decline,
}

/// Validate a value against an upstream TypeScript schema export name.
pub fn validate_schema(schema: &str, value: &Value) -> Result<Value, ValidationError> {
    match schema {
        "OpenAIFileEntrypointInputSchema" => validate_file_entrypoint(value),
        "OpenAIResourceToolCallMetadataSchema" => validate_resource_tool_call_metadata(value),
        "OpenAIMentionResourceSchema" => validate_mention_resource(value),
        "OpenAIMentionItemSchema" => validate_mention_item(value),
        "OpenAIMentionSearchParamsSchema" => {
            parse_model::<OpenAIMentionSearchParams>(schema_name(schema), value.clone())
        }
        "OpenAIMentionSearchResultSchema" => validate_mention_search_result(value),
        "OpenAIUiQuickActionSchema" => validate_ui_quick_action(value),
        "OpenAIUiEntrypointSchema" => validate_ui_entrypoint(value),
        "OpenAIUiToolMetadataSchema" => validate_ui_tool_metadata(value),
        "OpenAIUiResourceMetadataSchema" => validate_ui_resource_metadata(value),
        "OpenAISettingsCapabilitySchema" => validate_settings_capability(value),
        "OpenAISettingsPropertySchema" => validate_settings_property(value),
        "OpenAISettingsToolSchema" => validate_settings_tool(value),
        "OpenAISettingsGroupSchema" | "OpenAISettingsLayoutItemSchema" => {
            validate_settings_group(schema_name(schema), value)
        }
        "OpenAISettingsFieldPresentationSchema" => validate_settings_presentation(value),
        "OpenAISettingsReadResultSchema" => validate_settings_read_result(value),
        "OpenAISettingsUpdateArgumentsSchema" => validate_settings_update_arguments(value),
        "OpenAISettingsUpdateResultSchema" => {
            parse_model::<OpenAISettingsUpdateResult>(schema_name(schema), value.clone())
        }
        "OpenAIFileFormFieldSchema" => validate_file_form_field(value),
        "OpenAIFormFieldSchema" => validate_form_field(schema_name(schema), value),
        "OpenAIFormSchema" => validate_form_schema(value),
        "OpenAIFormResultSchema" => validate_form_result(value),
        "OpenAIFileOpenParamsSchema" => validate_file_open_params(value),
        "OpenAIDeepLinkHostStateSchema" => validate_deep_link_host_state(value),
        "OpenAIModelContextMetadataSchema" => validate_model_context_metadata(value),
        "OpenAIModelContextHostStateSchema" => validate_model_context_host_state(value),
        "OpenAIMessageOptionsSchema" => validate_message_options(value),
        "OpenAIMessageParamsSchema" => validate_message_params(value),
        "OpenAIResourceRepresentationSchema" => {
            parse_model::<OpenAIResourceRepresentation>(schema_name(schema), value.clone())
        }
        "OpenAIResourceReadMetadataSchema" => validate_resource_read_metadata(value),
        "OpenAIResourceMetadataSchema" => validate_resource_metadata(value),
        "OpenAIResourceContentMetadataSchema" => validate_resource_content_metadata(value),
        "OpenAIResourceWriteParamsSchema" => validate_resource_write_params(value),
        "OpenAIResourceWriteResultSchema" => validate_resource_write_result(value),
        _ => Err(ValidationError::new(
            "unknown",
            "",
            format!("unsupported schema: {schema}"),
        )),
    }
}

/// Validate a value against a server-side OpenAI schema export name.
pub fn validate_server_schema(schema: &str, value: &Value) -> ValidationResult<()> {
    validate_schema(schema, value).map(|_| ())
}

/// Validate a value against an app-side OpenAI schema export name.
pub fn validate_app_schema(schema: &str, value: &Value) -> ValidationResult<()> {
    validate_schema(schema, value).map(|_| ())
}

/// Normalize current and legacy deep-link host state.
pub fn normalize_deep_link_state(value: &Value) -> ValidationResult<OpenAIDeepLinkHostState> {
    let normalized = validate_deep_link_host_state(value)?;
    serde_json::from_value(normalized).map_err(|error| {
        ValidationError::new("OpenAIDeepLinkHostStateSchema", "", error.to_string())
    })
}

/// Build resource-picker metadata for a form field.
pub fn resource_input(
    options: Vec<Resource>,
    selection: Option<OpenAIResourceSelection>,
    user_options: Option<OpenAIUserResourceOptions>,
) -> ValidationResult<JsonObject> {
    build_resource_input("resource", options, selection, user_options)
}

/// Build legacy file-picker metadata for a form field.
pub fn file_input(
    options: Vec<Resource>,
    selection: Option<OpenAIResourceSelection>,
    user_options: Option<OpenAIUserResourceOptions>,
) -> ValidationResult<JsonObject> {
    build_resource_input("file", options, selection, user_options)
}

/// Validate one submitted value against a form field without coercion.
pub fn is_valid_value(field: &OpenAIFormField, value: &Value) -> ValidationResult<bool> {
    is_valid_value_with_options(field, value, 0, &[])
}

/// Validate one submitted value with pending and completed upload context.
pub fn is_valid_value_with_options(
    field: &OpenAIFormField,
    value: &Value,
    pending_uploads: usize,
    uploaded_uris: &[String],
) -> ValidationResult<bool> {
    let field = form_field_object(field)?;
    is_valid_python_field_value_with_options(&field, value, false, pending_uploads, uploaded_uris)
}

/// Validate an answer before uploads and return accepted file filters.
pub fn prepare_field_submission(
    field: &OpenAIFormField,
    name: &str,
    content: &JsonObject,
    pending_uploads: usize,
) -> ValidationResult<Option<Vec<String>>> {
    let field_object = form_field_object(field)?;
    if pending_uploads > 0 {
        let file_options = field
            .file_input
            .as_ref()
            .and_then(|input| input.user_options.as_ref())
            .ok_or_else(|| {
                ValidationError::new(
                    "OpenAIFormContentSchema",
                    name,
                    "Form field does not accept files",
                )
            })?;
        if field.field_type == "array" {
            let empty = Value::Array(Vec::new());
            let value = content.get(name).unwrap_or(&empty);
            if !is_valid_python_field_value_with_options(
                &field_object,
                value,
                false,
                pending_uploads,
                &[],
            )? {
                return Err(ValidationError::new(
                    "OpenAIFormContentSchema",
                    name,
                    "Invalid form value",
                ));
            }
        } else if pending_uploads != 1 || content.contains_key(name) {
            return Err(ValidationError::new(
                "OpenAIFormContentSchema",
                name,
                "Invalid form fields",
            ));
        }
        return Ok(file_options.accept.clone());
    }

    if let Some(value) = content.get(name)
        && !is_valid_python_field_value_with_options(&field_object, value, false, 0, &[])?
    {
        return Err(ValidationError::new(
            "OpenAIFormContentSchema",
            name,
            "Invalid form value",
        ));
    }
    Ok(None)
}

/// Combine existing selections with uploaded URIs and validate the final answer.
pub fn complete_field_submission(
    field: &OpenAIFormField,
    name: &str,
    content: &JsonObject,
    uploaded_uris: Vec<String>,
) -> ValidationResult<Value> {
    if uploaded_uris.is_empty() {
        return Err(ValidationError::new(
            "OpenAIFormContentSchema",
            name,
            "Invalid form fields",
        ));
    }
    prepare_field_submission(field, name, content, uploaded_uris.len())?;
    let value = if field.field_type == "array" {
        let mut values = content
            .get(name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        values.extend(uploaded_uris.iter().cloned().map(Value::String));
        Value::Array(values)
    } else {
        Value::String(uploaded_uris[0].clone())
    };
    let field_object = form_field_object(field)?;
    if !is_valid_python_field_value_with_options(&field_object, &value, false, 0, &uploaded_uris)? {
        return Err(ValidationError::new(
            "OpenAIFormContentSchema",
            name,
            "Uploaded file does not match the form constraints",
        ));
    }
    Ok(value)
}

/// Check returned resource URIs before typed model validation.
pub fn validate_file_selections(schema: &OpenAIForm, content: &JsonObject) -> ValidationResult<()> {
    for (name, field) in &schema.properties {
        if field.file_input.is_some()
            && let Some(value) = content.get(name)
        {
            let field_object = form_field_object(field)?;
            if !is_valid_python_field_value_with_options(&field_object, value, true, 0, &[])? {
                return Err(ValidationError::new(
                    "OpenAIFormContentSchema",
                    name,
                    format!("Invalid resource selection for {name:?}"),
                ));
            }
        }
    }
    Ok(())
}

/// Check resource and choice constraints before typed model validation.
pub fn validate_form_selections(schema: &OpenAIForm, content: &JsonObject) -> ValidationResult<()> {
    validate_file_selections(schema, content)?;
    for (name, field) in &schema.properties {
        if field_has_form_selection_options(field) && content.contains_key(name) {
            let field_object = form_field_object(field)?;
            if !is_valid_python_field_value_with_options(
                &field_object,
                &content[name],
                false,
                0,
                &[],
            )? {
                return Err(ValidationError::new(
                    "OpenAIFormContentSchema",
                    name,
                    format!("Invalid selection for {name:?}"),
                ));
            }
        }
    }
    Ok(())
}

/// Validate submitted form content against an OpenAI form schema.
pub fn validate_form_content(form: &Value, content: &Value) -> ValidationResult<Value> {
    validate_form_schema(form)?;
    let schema = "OpenAIFormContentSchema";
    let form = as_object(schema, form)?;
    let content = as_object(schema, content)?;
    let properties = form
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| ValidationError::new(schema, "properties", "expected properties"))?;
    if let Some(required) = form.get("required").and_then(Value::as_array) {
        for name in required {
            let name = name.as_str().ok_or_else(|| {
                ValidationError::new(schema, "required", "required entries must be strings")
            })?;
            if !content.contains_key(name) {
                return Err(ValidationError::new(schema, name, "Required field"));
            }
        }
    }
    let mut normalized_content = Map::new();
    for (name, value) in content {
        let Some(field) = properties.get(name).and_then(Value::as_object) else {
            if !is_form_content_value(value) {
                return Err(ValidationError::new(schema, name, "Invalid form value"));
            }
            normalized_content.insert(name.clone(), value.clone());
            continue;
        };
        if !is_valid_field_value(field, value, true)? {
            return Err(ValidationError::new(schema, name, "Invalid form value"));
        }
        normalized_content.insert(name.clone(), normalize_form_content_value(field, value));
    }
    Ok(Value::Object(normalized_content))
}

fn build_resource_input(
    input_type: &str,
    options: Vec<Resource>,
    selection: Option<OpenAIResourceSelection>,
    user_options: Option<OpenAIUserResourceOptions>,
) -> ValidationResult<JsonObject> {
    let input = OpenAIResourceInput {
        input_type: input_type.to_string(),
        options,
        user_options,
        selection,
    };
    let mut metadata = JsonObject::new();
    metadata.insert(
        OPENAI_INPUT_KEY.to_string(),
        serde_json::to_value(input).map_err(|error| {
            ValidationError::new("OpenAIResourceInput", OPENAI_INPUT_KEY, error.to_string())
        })?,
    );
    Ok(metadata)
}

fn form_field_object(field: &OpenAIFormField) -> ValidationResult<Map<String, Value>> {
    serde_json::to_value(field)
        .map_err(|error| ValidationError::new("OpenAIFormFieldSchema", "", error.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| ValidationError::new("OpenAIFormFieldSchema", "", "expected object"))
}

fn schema_name(name: &str) -> &'static str {
    SUPPORTED_SCHEMA_NAMES
        .iter()
        .copied()
        .find(|candidate| *candidate == name)
        .unwrap_or("unknown")
}

fn parse_model<T>(schema: &'static str, value: Value) -> Result<Value, ValidationError>
where
    T: DeserializeOwned + Serialize,
{
    let parsed: T = serde_json::from_value(value)
        .map_err(|error| ValidationError::new(schema, "", error.to_string()))?;
    serde_json::to_value(parsed)
        .map_err(|error| ValidationError::new(schema, "", error.to_string()))
}

fn as_object<'a>(
    schema: &'static str,
    value: &'a Value,
) -> Result<&'a Map<String, Value>, ValidationError> {
    value
        .as_object()
        .ok_or_else(|| ValidationError::new(schema, "", "expected object"))
}

fn get_string<'a>(
    schema: &'static str,
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, ValidationError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError::new(schema, key, "expected string"))
}

fn reject_null_properties(
    schema: &'static str,
    object: &Map<String, Value>,
    keys: &[&'static str],
) -> Result<(), ValidationError> {
    for key in keys {
        if object.get(*key).is_some_and(Value::is_null) {
            return Err(ValidationError::new(schema, *key, "null is not allowed"));
        }
    }
    Ok(())
}

fn nonblank(
    schema: &'static str,
    path: impl Into<String>,
    value: &str,
) -> Result<(), ValidationError> {
    if value.chars().any(|ch| !ch.is_whitespace()) {
        Ok(())
    } else {
        Err(ValidationError::new(
            schema,
            path,
            "expected nonblank string",
        ))
    }
}

fn only_keys(
    schema: &'static str,
    object: &Map<String, Value>,
    keys: &[&str],
) -> Result<(), ValidationError> {
    for key in object.keys() {
        if !keys.contains(&key.as_str()) {
            return Err(ValidationError::new(schema, key, "unknown field"));
        }
    }
    Ok(())
}

fn array<'a>(
    schema: &'static str,
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Vec<Value>, ValidationError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ValidationError::new(schema, key, "expected array"))
}

fn validate_file_entrypoint(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIFileEntrypointInputSchema";
    let object = as_object(schema, value)?;
    let file = object
        .get("file")
        .ok_or_else(|| ValidationError::new(schema, "file", "missing file"))?;
    let file = as_object(schema, file)?;
    nonblank(schema, "file.name", get_string(schema, file, "name")?)?;
    nonblank(
        schema,
        "file.resourceUri",
        get_string(schema, file, "resourceUri")?,
    )?;
    parse_model::<OpenAIFileEntrypointInput>(schema, value.clone())
}

fn validate_resource_tool_call_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceToolCallMetadataSchema";
    let object = as_object(schema, value)?;
    if let Some(resource) = object.get(OPENAI_RESOURCE_METADATA_KEY) {
        let resource = as_object(schema, resource)?;
        get_string(schema, resource, "path")?;
    }
    parse_model::<OpenAIResourceToolCallMetadata>(schema, value.clone())
}

fn validate_mention_resource(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIMentionResourceSchema";
    let object = as_object(schema, value)?;
    only_keys(
        schema,
        object,
        &["type", "resourceUri", "title", "subtitle", "icons"],
    )?;
    if get_string(schema, object, "type")? != "resource" {
        return Err(ValidationError::new(schema, "type", "expected resource"));
    }
    nonblank(
        schema,
        "resourceUri",
        get_string(schema, object, "resourceUri")?,
    )?;
    nonblank(schema, "title", get_string(schema, object, "title")?)?;
    if let Some(subtitle) = object.get("subtitle") {
        nonblank(
            schema,
            "subtitle",
            subtitle
                .as_str()
                .ok_or_else(|| ValidationError::new(schema, "subtitle", "expected string"))?,
        )?;
    }
    parse_model::<OpenAIMentionResource>(schema, value.clone())
}

fn validate_mention_item(value: &Value) -> Result<Value, ValidationError> {
    if value.get("type") == Some(&Value::String("resource".to_string())) {
        validate_mention_resource(value)
    } else {
        let schema = "OpenAIMentionItemSchema";
        let link: ResourceLink = serde_json::from_value(value.clone())
            .map_err(|error| ValidationError::new(schema, "", error.to_string()))?;
        nonblank(schema, "uri", &link.uri)?;
        Ok(serde_json::to_value(link).expect("resource link serializes"))
    }
}

fn validate_mention_search_result(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIMentionSearchResultSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["items"])?;
    for item in array(schema, object, "items")? {
        validate_mention_item(item)?;
    }
    parse_model::<OpenAIMentionSearchResult>(schema, value.clone())
}

fn validate_ui_quick_action(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIUiQuickActionSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["title", "icons", "target"])?;
    nonblank(schema, "title", get_string(schema, object, "title")?)?;
    if array(schema, object, "icons")?.is_empty() {
        return Err(ValidationError::new(
            schema,
            "icons",
            "expected at least one icon",
        ));
    }
    let target = as_object(
        schema,
        object
            .get("target")
            .ok_or_else(|| ValidationError::new(schema, "target", "missing target"))?,
    )?;
    only_keys(schema, target, &["type", "name", "arguments"])?;
    if get_string(schema, target, "type")? != "tool" {
        return Err(ValidationError::new(schema, "target.type", "expected tool"));
    }
    nonblank(schema, "target.name", get_string(schema, target, "name")?)?;
    parse_model::<OpenAIUiQuickAction>(schema, value.clone())
}

fn validate_ui_entrypoint(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIUiEntrypointSchema";
    let object = as_object(schema, value)?;
    let mut normalized = value.clone();
    match get_string(schema, object, "type")? {
        "file" => {
            only_keys(schema, object, &["type", "extensions"])?;
            let mut extensions = Vec::new();
            for (index, extension) in array(schema, object, "extensions")?.iter().enumerate() {
                let extension = extension.as_str().ok_or_else(|| {
                    ValidationError::new(schema, format!("extensions/{index}"), "expected string")
                })?;
                let extension = extension.trim();
                if !extension.starts_with('.') {
                    return Err(ValidationError::new(
                        schema,
                        format!("extensions/{index}"),
                        "file extension must start with dot",
                    ));
                }
                extensions.push(Value::String(extension.to_string()));
            }
            normalized["extensions"] = Value::Array(extensions);
        }
        "global" => {
            only_keys(schema, object, &["type", "quickAction"])?;
            if let Some(action) = object.get("quickAction") {
                let action = validate_ui_quick_action(action)?;
                normalized["quickAction"] = action;
            }
        }
        "settings" => {
            only_keys(schema, object, &["type", "searchTerms"])?;
            if let Some(search_terms) = object.get("searchTerms") {
                let terms = search_terms
                    .as_array()
                    .ok_or_else(|| ValidationError::new(schema, "searchTerms", "expected array"))?;
                let mut normalized_terms = Vec::new();
                for (index, term) in terms.iter().enumerate() {
                    let term = term.as_str().ok_or_else(|| {
                        ValidationError::new(
                            schema,
                            format!("searchTerms/{index}"),
                            "expected string",
                        )
                    })?;
                    let term = term.trim();
                    if term.is_empty() {
                        return Err(ValidationError::new(
                            schema,
                            format!("searchTerms/{index}"),
                            "expected nonempty search term",
                        ));
                    }
                    normalized_terms.push(Value::String(term.to_string()));
                }
                normalized["searchTerms"] = Value::Array(normalized_terms);
            }
        }
        "thread" => only_keys(schema, object, &["type"])?,
        other => {
            return Err(ValidationError::new(
                schema,
                "type",
                format!("unknown entrypoint type {other}"),
            ));
        }
    }
    parse_model::<OpenAIUiEntrypoint>(schema, normalized)
}

fn validate_ui_tool_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIUiToolMetadataSchema";
    let object = as_object(schema, value)?;
    only_keys(
        schema,
        object,
        &["entrypoints", "preferredModelDisplayMode"],
    )?;
    reject_null_properties(
        schema,
        object,
        &["entrypoints", "preferredModelDisplayMode"],
    )?;
    if let Some(entrypoints) = object.get("entrypoints").and_then(Value::as_array) {
        for entrypoint in entrypoints {
            validate_ui_entrypoint(entrypoint)?;
        }
    }
    parse_model::<OpenAIUiToolMetadata>(schema, value.clone())
}

fn validate_ui_resource_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIUiResourceMetadataSchema";
    let object = as_object(schema, value)?;
    reject_null_properties(
        schema,
        object,
        &["availableDisplayModes", "preferredDisplayMode"],
    )?;
    parse_model::<OpenAIUiResourceMetadata>(schema, value.clone())
}

fn validate_settings_capability(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsCapabilitySchema";
    let object = as_object(schema, value)?;
    nonblank(schema, "readTool", get_string(schema, object, "readTool")?)?;
    nonblank(
        schema,
        "updateTool",
        get_string(schema, object, "updateTool")?,
    )?;
    parse_model::<OpenAISettingsCapability>(schema, value.clone())
}

fn validate_settings_property(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsPropertySchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["kind", "property"])?;
    if get_string(schema, object, "kind")? != "property" {
        return Err(ValidationError::new(schema, "kind", "expected property"));
    }
    parse_model::<OpenAISettingsProperty>(schema, value.clone())
}

fn validate_settings_tool(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsToolSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["kind", "tool", "title", "description"])?;
    if get_string(schema, object, "kind")? != "tool" {
        return Err(ValidationError::new(schema, "kind", "expected tool"));
    }
    nonblank(schema, "tool", get_string(schema, object, "tool")?)?;
    nonblank(schema, "title", get_string(schema, object, "title")?)?;
    parse_model::<OpenAISettingsTool>(schema, value.clone())
}

fn validate_settings_group(schema: &'static str, value: &Value) -> Result<Value, ValidationError> {
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["kind", "title", "items"])?;
    if get_string(schema, object, "kind")? != "group" {
        return Err(ValidationError::new(schema, "kind", "expected group"));
    }
    nonblank(schema, "title", get_string(schema, object, "title")?)?;
    for item in array(schema, object, "items")? {
        match item.get("kind").and_then(Value::as_str) {
            Some("property") => {
                validate_settings_property(item)?;
            }
            Some("tool") => {
                validate_settings_tool(item)?;
            }
            _ => {
                return Err(ValidationError::new(
                    schema,
                    "items",
                    "unknown layout item kind",
                ));
            }
        }
    }
    parse_model::<OpenAISettingsGroup>(schema, value.clone())
}

fn validate_settings_presentation(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsFieldPresentationSchema";
    let object = as_object(schema, value)?;
    reject_null_properties(schema, object, &["description"])?;
    nonblank(schema, "title", get_string(schema, object, "title")?)?;
    parse_model::<OpenAISettingsFieldPresentation>(schema, value.clone())
}

fn validate_settings_read_result(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsReadResultSchema";
    let object = as_object(schema, value)?;
    let settings_schema = as_object(
        schema,
        object
            .get("schema")
            .ok_or_else(|| ValidationError::new(schema, "schema", "missing schema"))?,
    )?;
    if settings_schema.get("type") != Some(&Value::String("object".to_string())) {
        return Err(ValidationError::new(
            schema,
            "schema.type",
            "expected object schema",
        ));
    }
    let mut seen = BTreeSet::new();
    let properties =
        if let Some(properties) = settings_schema.get("properties") {
            Some(properties.as_object().ok_or_else(|| {
                ValidationError::new(schema, "schema.properties", "expected object")
            })?)
        } else {
            None
        };
    if let Some(required) = settings_schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| ValidationError::new(schema, "schema.required", "expected array"))?;
        for (index, name) in required.iter().enumerate() {
            name.as_str().ok_or_else(|| {
                ValidationError::new(
                    schema,
                    format!("schema.required/{index}"),
                    "required entries must be strings",
                )
            })?;
        }
    }
    if let Some(layout) = object.get("layout").and_then(Value::as_array) {
        for group in layout {
            validate_settings_group(schema, group)?;
            for item in group
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if item.get("kind") == Some(&Value::String("property".to_string())) {
                    let property = item
                        .get("property")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if properties.is_none_or(|props| !props.contains_key(property))
                        || !seen.insert(property.to_string())
                    {
                        return Err(ValidationError::new(
                            schema,
                            "layout",
                            format!("Unknown or duplicate settings key: {property}"),
                        ));
                    }
                }
            }
        }
    }
    parse_model::<OpenAISettingsReadResult>(schema, value.clone())
}

fn validate_settings_update_arguments(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAISettingsUpdateArgumentsSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["set"])?;
    let set = as_object(
        schema,
        object
            .get("set")
            .ok_or_else(|| ValidationError::new(schema, "set", "missing set"))?,
    )?;
    if set.is_empty() {
        return Err(ValidationError::new(
            schema,
            "set",
            "Set at least one setting.",
        ));
    }
    parse_model::<OpenAISettingsUpdateArguments>(schema, value.clone())
}

fn validate_file_form_field(value: &Value) -> Result<Value, ValidationError> {
    validate_form_field("OpenAIFileFormFieldSchema", value)?;
    let object = as_object("OpenAIFileFormFieldSchema", value)?;
    if !object.contains_key("x-openai-input") {
        return Err(ValidationError::new(
            "OpenAIFileFormFieldSchema",
            "x-openai-input",
            "missing resource input",
        ));
    }
    parse_model::<OpenAIFormField>("OpenAIFileFormFieldSchema", value.clone())
}

fn validate_form_field(schema: &'static str, value: &Value) -> Result<Value, ValidationError> {
    let object = as_object(schema, value)?;
    let field_type = get_string(schema, object, "type")?;
    match field_type {
        "string" | "number" | "integer" | "boolean" | "array" => {}
        _ => {
            return Err(ValidationError::new(
                schema,
                "type",
                "unsupported form field type",
            ));
        }
    }
    reject_null_properties(
        schema,
        object,
        &[
            "title",
            "description",
            "default",
            "enum",
            "enumNames",
            "oneOf",
            "x-openai-suggestions",
            "items",
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
            "pattern",
            "format",
            "minItems",
            "maxItems",
            "uniqueItems",
            "x-openai-input",
        ],
    )?;
    validate_field_constraints(schema, object, field_type)?;
    if let Some(items) = object.get("items") {
        let items = as_object(schema, items)?;
        validate_array_items(schema, items)?;
    }
    validate_enum_choices(schema, object)?;
    if let Some(input) = object.get("x-openai-input") {
        validate_resource_input(schema, field_type, object, input)?;
    }
    validate_default_type(schema, field_type, object)?;
    let mut value = value.clone();
    if object.get("enum").is_none()
        && let Some(object) = value.as_object_mut()
    {
        object.remove("enumNames");
    }
    parse_model::<OpenAIFormField>(schema, value)
}

fn validate_field_constraints(
    schema: &'static str,
    object: &Map<String, Value>,
    field_type: &str,
) -> Result<(), ValidationError> {
    let string_constraints = [
        "minLength",
        "maxLength",
        "pattern",
        "format",
        "oneOf",
        "x-openai-suggestions",
        "enumNames",
    ];
    let number_constraints = ["minimum", "maximum"];
    let array_constraints = ["items", "minItems", "maxItems", "uniqueItems"];
    for key in string_constraints {
        if object.contains_key(key) && field_type != "string" {
            return Err(ValidationError::new(
                schema,
                key,
                "constraint does not apply",
            ));
        }
    }
    for key in number_constraints {
        if object.contains_key(key) && field_type != "number" && field_type != "integer" {
            return Err(ValidationError::new(
                schema,
                key,
                "constraint does not apply",
            ));
        }
    }
    for key in array_constraints {
        if object.contains_key(key) && field_type != "array" {
            return Err(ValidationError::new(
                schema,
                key,
                "constraint does not apply",
            ));
        }
    }
    if field_type == "array" && !object.contains_key("items") {
        return Err(ValidationError::new(
            schema,
            "items",
            "array field requires items",
        ));
    }
    if let Some(format) = object.get("format").and_then(Value::as_str)
        && !matches!(format, "email" | "uri" | "date" | "date-time")
    {
        return Err(ValidationError::new(schema, "format", "unsupported format"));
    }
    if let Some(options) = object
        .get("oneOf")
        .or_else(|| object.get("x-openai-suggestions"))
    {
        validate_options(schema, options)?;
    }
    Ok(())
}

fn validate_array_items(
    schema: &'static str,
    items: &Map<String, Value>,
) -> Result<(), ValidationError> {
    reject_null_properties(
        schema,
        items,
        &[
            "type",
            "enum",
            "anyOf",
            "x-openai-suggestions",
            "format",
            "minLength",
            "maxLength",
            "pattern",
        ],
    )?;
    match items.get("type") {
        Some(Value::String(item_type)) if item_type == "string" => {}
        Some(_) => {
            return Err(ValidationError::new(
                schema,
                "items.type",
                "expected string item type",
            ));
        }
        None if !items.contains_key("anyOf") => {
            return Err(ValidationError::new(
                schema,
                "items.type",
                "missing item type",
            ));
        }
        None => {}
    }
    if let Some(pattern) = items.get("pattern")
        && !pattern.is_string()
    {
        return Err(ValidationError::new(
            schema,
            "items.pattern",
            "pattern must be a string",
        ));
    }
    if let Some(format) = items.get("format").and_then(Value::as_str)
        && !matches!(format, "email" | "uri" | "date" | "date-time")
    {
        return Err(ValidationError::new(
            schema,
            "items.format",
            "unsupported format",
        ));
    }
    if let Some(options) = items.get("anyOf") {
        validate_options(schema, options)?;
    }
    if let Some(options) = items.get("x-openai-suggestions") {
        validate_options(schema, options)?;
    }
    if let Some(choices) = items.get("enum").and_then(Value::as_array) {
        if choices.iter().any(|choice| !choice.is_string()) {
            return Err(ValidationError::new(
                schema,
                "items.enum",
                "string choices must be strings",
            ));
        }
        let unique: BTreeSet<&str> = choices.iter().filter_map(Value::as_str).collect();
        if unique.len() != choices.len() {
            return Err(ValidationError::new(
                schema,
                "items.enum",
                "choices must be unique",
            ));
        }
    }
    Ok(())
}

fn validate_enum_choices(
    schema: &'static str,
    object: &Map<String, Value>,
) -> Result<(), ValidationError> {
    if let Some(enum_names) = object.get("enumNames")
        && !enum_names.is_array()
    {
        return Err(ValidationError::new(
            schema,
            "enumNames",
            "expected enum labels",
        ));
    }
    if let Some(choices) = object.get("enum").and_then(Value::as_array) {
        for choice in choices {
            if !matches!(choice, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                return Err(ValidationError::new(
                    schema,
                    "enum",
                    "enum choice must be scalar",
                ));
            }
        }
    }
    Ok(())
}

fn validate_resource_input(
    schema: &'static str,
    field_type: &str,
    field: &Map<String, Value>,
    input: &Value,
) -> Result<(), ValidationError> {
    let input = as_object(schema, input)?;
    match input.get("type").and_then(Value::as_str) {
        Some("resource" | "file") => {}
        _ => {
            return Err(ValidationError::new(
                schema,
                "x-openai-input.type",
                "expected resource or file",
            ));
        }
    }
    let options = input
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ValidationError::new(schema, "x-openai-input.options", "expected options")
        })?;
    let mut uris = BTreeSet::new();
    for option in options {
        let uri = option.get("uri").and_then(Value::as_str).ok_or_else(|| {
            ValidationError::new(schema, "x-openai-input.options.uri", "expected uri")
        })?;
        if !uris.insert(uri.to_string()) {
            return Err(ValidationError::new(
                schema,
                "x-openai-input.options",
                "duplicate resource choice",
            ));
        }
    }
    let resource_field = if field_type == "array" {
        field
            .get("items")
            .and_then(Value::as_object)
            .ok_or_else(|| ValidationError::new(schema, "items", "expected item schema"))?
    } else {
        field
    };
    if resource_field.get("type") != Some(&Value::String("string".to_string()))
        || resource_field.get("format") != Some(&Value::String("uri".to_string()))
    {
        return Err(ValidationError::new(
            schema,
            "x-openai-input",
            "resource input requires URI string or array",
        ));
    }
    if input.contains_key("selection") && field_type != "array" {
        return Err(ValidationError::new(
            schema,
            "x-openai-input.selection",
            "selection mode requires array",
        ));
    }
    if input.get("selection") == Some(&Value::String("implicit".to_string()))
        && field.contains_key("default")
    {
        return Err(ValidationError::new(
            schema,
            "default",
            "implicit selection cannot specify a default",
        ));
    }
    if let Some(default) = field.get("default") {
        let defaults: Vec<&Value> = default
            .as_array()
            .map(|values| values.iter().collect())
            .unwrap_or_else(|| vec![default]);
        for default in defaults {
            if default.as_str().is_none_or(|uri| !uris.contains(uri)) {
                return Err(ValidationError::new(
                    schema,
                    "default",
                    "defaults must name supplied resources",
                ));
            }
        }
    }
    Ok(())
}

fn validate_default_type(
    schema: &'static str,
    field_type: &str,
    object: &Map<String, Value>,
) -> Result<(), ValidationError> {
    let Some(default) = object.get("default") else {
        return Ok(());
    };
    let valid = match field_type {
        "string" => default.is_string(),
        "boolean" => default.is_boolean(),
        "integer" => default.as_i64().is_some() || default.as_u64().is_some(),
        "number" => default.is_number() && !default.is_boolean(),
        "array" => default.is_array(),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(ValidationError::new(
            schema,
            "default",
            "default does not match field type",
        ))
    }
}

fn validate_options(schema: &'static str, options: &Value) -> Result<(), ValidationError> {
    let options = options
        .as_array()
        .ok_or_else(|| ValidationError::new(schema, "options", "expected options"))?;
    let mut seen = BTreeSet::new();
    for option in options {
        let object = as_object(schema, option)?;
        let constant = get_string(schema, object, "const")?;
        if !seen.insert(constant.to_string()) {
            return Err(ValidationError::new(
                schema,
                "options",
                "selection options must be unique",
            ));
        }
        get_string(schema, object, "title")?;
        for key in ["x-openai-thumbnail", "x-openai-preview"] {
            if let Some(icon) = object.get(key) {
                let icon = as_object(schema, icon)?;
                let src = get_string(schema, icon, "src")?;
                if !(src.starts_with("https://") || src.starts_with("data:image/")) {
                    return Err(ValidationError::new(
                        schema,
                        key,
                        "thumbnail requires HTTPS or image data URI",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_form_schema(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIFormSchema";
    let object = as_object(schema, value)?;
    if object.get("type") != Some(&Value::String("object".to_string())) {
        return Err(ValidationError::new(schema, "type", "expected object"));
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| ValidationError::new(schema, "properties", "expected properties"))?;
    for field in properties.values() {
        validate_form_field("OpenAIFormFieldSchema", field)?;
    }
    if let Some(required) = object.get("required").and_then(Value::as_array) {
        for name in required {
            name.as_str().ok_or_else(|| {
                ValidationError::new(schema, "required", "required entries must be strings")
            })?;
        }
    }
    let mut normalized = parse_model::<OpenAIForm>(schema, value.clone())?;
    if let Some(required) = object.get("required")
        && let Some(object) = normalized.as_object_mut()
    {
        object.insert("required".to_string(), required.clone());
    }
    Ok(normalized)
}

fn validate_form_result(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIFormResultSchema";
    let object = as_object(schema, value)?;
    match get_string(schema, object, "action")? {
        "accept" => {
            let content = object
                .get("content")
                .ok_or_else(|| ValidationError::new(schema, "content", "missing content"))?;
            let content = as_object(schema, content)?;
            for value in content.values() {
                if !(value.is_string()
                    || value.is_number()
                    || value.is_boolean()
                    || value
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string)))
                {
                    return Err(ValidationError::new(
                        schema,
                        "content",
                        "unsupported form content value",
                    ));
                }
            }
        }
        "cancel" | "decline" => {}
        _ => {
            return Err(ValidationError::new(
                schema,
                "action",
                "unknown form action",
            ));
        }
    }
    parse_model::<OpenAIFormResult>(schema, value.clone())
}

fn is_form_content_value(value: &Value) -> bool {
    value.is_string()
        || value.is_number()
        || value.is_boolean()
        || value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
}

fn normalize_form_content_value(field: &Map<String, Value>, value: &Value) -> Value {
    if field.get("type").and_then(Value::as_str) == Some("integer")
        && let Some(number) = value.as_f64()
        && number.fract() == 0.0
        && number >= i64::MIN as f64
        && number <= i64::MAX as f64
    {
        return Value::Number(serde_json::Number::from(number as i64));
    }
    value.clone()
}

fn is_valid_field_value(
    field: &Map<String, Value>,
    value: &Value,
    allow_user_files: bool,
) -> Result<bool, ValidationError> {
    is_valid_field_value_with_options(field, value, allow_user_files, 0, &[], false)
}

fn is_valid_python_field_value_with_options(
    field: &Map<String, Value>,
    value: &Value,
    allow_user_files: bool,
    pending_uploads: usize,
    uploaded_uris: &[String],
) -> Result<bool, ValidationError> {
    if field_uses_pattern(field) {
        return Err(ValidationError::new(
            "OpenAIFormContentSchema",
            "pattern",
            "patterns are not supported by the Python helper API",
        ));
    }
    is_valid_field_value_with_options(
        field,
        value,
        allow_user_files,
        pending_uploads,
        uploaded_uris,
        true,
    )
}

fn field_uses_pattern(field: &Map<String, Value>) -> bool {
    field.get("pattern").is_some()
        || field
            .get("items")
            .and_then(Value::as_object)
            .is_some_and(|items| items.get("pattern").is_some())
}

fn field_has_form_selection_options(field: &OpenAIFormField) -> bool {
    field
        .one_of
        .as_ref()
        .is_some_and(|values| !values.is_empty())
        || field.items.as_ref().is_some_and(|items| {
            items
                .any_of
                .as_ref()
                .is_some_and(|values| !values.is_empty())
        })
}

fn is_valid_field_value_with_options(
    field: &Map<String, Value>,
    value: &Value,
    allow_user_files: bool,
    pending_uploads: usize,
    uploaded_uris: &[String],
    python_string_lengths: bool,
) -> Result<bool, ValidationError> {
    let field_type = field
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(input) = field.get("x-openai-input").and_then(Value::as_object)
        && !(allow_user_files && input.contains_key("userOptions"))
    {
        let mut choices: BTreeSet<String> = input
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                option
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect();
        choices.extend(uploaded_uris.iter().cloned());
        let selected: Vec<&Value> = value
            .as_array()
            .map(|items| items.iter().collect())
            .unwrap_or_else(|| vec![value]);
        if selected
            .iter()
            .any(|item| item.as_str().is_none_or(|uri| !choices.contains(uri)))
        {
            return Ok(false);
        }
    }
    if pending_uploads == 0
        && let Some(choices) = field.get("enum").and_then(Value::as_array)
        && !choices
            .iter()
            .any(|choice| choice == value && choice.is_boolean() == value.is_boolean())
    {
        return Ok(false);
    }
    Ok(match field_type {
        "string" => {
            let Some(string) = value.as_str() else {
                return Ok(false);
            };
            let constraints_match = if python_string_lengths {
                python_string_constraints_match(field, string)
            } else {
                string_constraints_match(field, string)
            }?;
            constraints_match
                && field
                    .get("oneOf")
                    .and_then(Value::as_array)
                    .is_none_or(|options| {
                        options.iter().any(|option| {
                            option.get("const") == Some(&Value::String(string.to_string()))
                        })
                    })
        }
        "boolean" => value.is_boolean(),
        "integer" => numeric_constraints_match(field, value, true, !python_string_lengths),
        "number" => numeric_constraints_match(field, value, false, true),
        "array" => array_constraints_match(field, value, pending_uploads, python_string_lengths)?,
        _ => false,
    })
}

fn string_constraints_match(
    field: &Map<String, Value>,
    value: &str,
) -> Result<bool, ValidationError> {
    if let Some(pattern) = field.get("pattern").and_then(Value::as_str)
        && !pattern_matches(pattern, value)?
    {
        return Ok(false);
    }
    let len = value.chars().count() as u64;
    if field
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|min| len < min)
    {
        return Ok(false);
    }
    if field
        .get("maxLength")
        .and_then(Value::as_u64)
        .is_some_and(|max| len > max)
    {
        return Ok(false);
    }
    if let Some(format) = field.get("format").and_then(Value::as_str)
        && !format_matches(format, value)
    {
        return Ok(false);
    }
    Ok(true)
}

fn python_string_constraints_match(
    field: &Map<String, Value>,
    value: &str,
) -> Result<bool, ValidationError> {
    let len = value.chars().count() as u64;
    if field
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|min| len < min)
    {
        return Ok(false);
    }
    if field
        .get("maxLength")
        .and_then(Value::as_u64)
        .is_some_and(|max| len > max)
    {
        return Ok(false);
    }
    if let Some(format) = field.get("format").and_then(Value::as_str)
        && !format_matches(format, value)
    {
        return Ok(false);
    }
    Ok(true)
}

fn pattern_matches(pattern: &str, value: &str) -> Result<bool, ValidationError> {
    Regex::new(pattern)
        .map(|regex| regex.find(value).is_some())
        .map_err(|error| {
            ValidationError::new(
                "OpenAIFormContentSchema",
                "pattern",
                format!("unsupported pattern: {error}"),
            )
        })
}

fn format_matches(format: &str, value: &str) -> bool {
    match format {
        "email" => value.contains('@') && !value.starts_with('@') && !value.ends_with('@'),
        "uri" => valid_uri(value),
        "date" => valid_date(value),
        "date-time" => valid_date_time(value),
        _ => true,
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !(bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit()))
    {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
    };
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400)
}

fn valid_uri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' => true,
            b'0'..=b'9' if index > 0 => true,
            b'+' | b'.' | b'-' if index > 0 => true,
            _ => false,
        })
        && rest.strip_prefix("//").is_some_and(|tail| !tail.is_empty())
}

fn valid_date_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    let timezone = value
        .get(value.len().saturating_sub(6)..)
        .unwrap_or_default();
    bytes.len() >= 20
        && valid_date(&value[..10])
        && matches!(bytes[10], b'T' | b't')
        && bytes[13] == b':'
        && bytes[16] == b':'
        && (value.ends_with('Z')
            || value.ends_with('z')
            || timezone.contains('+')
            || timezone.contains('-'))
}

fn numeric_constraints_match(
    field: &Map<String, Value>,
    value: &Value,
    integer: bool,
    allow_float_integer: bool,
) -> bool {
    if value.is_boolean() || !value.is_number() {
        return false;
    }
    if integer
        && !(value.as_i64().is_some()
            || value.as_u64().is_some()
            || (allow_float_integer && value.as_f64().is_some_and(|number| number.fract() == 0.0)))
    {
        return false;
    }
    let Some(number) = value.as_f64() else {
        return false;
    };
    if field
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| number < minimum)
    {
        return false;
    }
    if field
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| number > maximum)
    {
        return false;
    }
    true
}

fn array_constraints_match(
    field: &Map<String, Value>,
    value: &Value,
    pending_uploads: usize,
    python_helper: bool,
) -> Result<bool, ValidationError> {
    let Some(items) = value.as_array() else {
        return Ok(false);
    };
    let len = (items.len() + pending_uploads) as u64;
    if field
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|min| len < min)
    {
        return Ok(false);
    }
    if field
        .get("maxItems")
        .and_then(Value::as_u64)
        .is_some_and(|max| len > max)
    {
        return Ok(false);
    }
    if field
        .get("uniqueItems")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let unique: BTreeSet<String> = items.iter().map(Value::to_string).collect();
        if unique.len() != items.len() {
            return Ok(false);
        }
    }
    let Some(item_schema) = field.get("items").and_then(Value::as_object) else {
        return Ok(false);
    };
    if python_helper && (item_schema.get("enum").is_some() || item_schema.get("anyOf").is_some()) {
        let unique: BTreeSet<String> = items.iter().map(Value::to_string).collect();
        if unique.len() != items.len() {
            return Ok(false);
        }
    }
    for item in items {
        if let Some(choices) = item_schema.get("enum").and_then(Value::as_array)
            && !choices.contains(item)
        {
            return Ok(false);
        }
        let Some(value) = item.as_str() else {
            return Ok(false);
        };
        if let Some(options) = item_schema.get("anyOf").and_then(Value::as_array)
            && !options
                .iter()
                .any(|option| option.get("const") == Some(&Value::String(value.to_string())))
        {
            return Ok(false);
        }
        if !string_constraints_match(item_schema, value)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_file_open_params(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIFileOpenParamsSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["path"])?;
    nonblank(schema, "path", get_string(schema, object, "path")?)?;
    parse_model::<OpenAIFileOpenParams>(schema, value.clone())
}

fn validate_deep_link_host_state(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIDeepLinkHostStateSchema";
    if value.get("url").is_some() {
        parse_model::<OpenAIDeepLinkHostState>(schema, value.clone())
    } else {
        let legacy: LegacyOpenAIDeepLinkHostState = serde_json::from_value(value.clone())
            .map_err(|error| ValidationError::new(schema, "", error.to_string()))?;
        let mut url = format!("/{}", legacy.path.join("/"));
        if !legacy.query.is_empty() {
            let query = legacy
                .query
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&");
            url.push('?');
            url.push_str(&query);
        }
        Ok(serde_json::json!({ "url": url }))
    }
}

fn validate_model_context_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIModelContextMetadataSchema";
    let object = as_object(schema, value)?;
    let inner = as_object(
        schema,
        object.get(OPENAI_MODEL_CONTEXT_KEY).ok_or_else(|| {
            ValidationError::new(
                schema,
                OPENAI_MODEL_CONTEXT_KEY,
                "missing model context metadata",
            )
        })?,
    )?;
    nonblank(
        schema,
        "openai/modelContext.updateId",
        get_string(schema, inner, "updateId")?,
    )?;
    parse_model::<OpenAIModelContextMetadata>(schema, value.clone())
}

fn validate_model_context_host_state(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIModelContextHostStateSchema";
    if value.is_null() {
        return Ok(Value::Null);
    }
    let object = as_object(schema, value)?;
    nonblank(schema, "updateId", get_string(schema, object, "updateId")?)?;
    if let Some(content) = object.get("content") {
        let content = content
            .as_array()
            .ok_or_else(|| ValidationError::new(schema, "content", "expected array"))?;
        for (index, block) in content.iter().enumerate() {
            validate_content_block(schema, format!("content/{index}"), block)?;
        }
    }
    if let Some(structured) = object.get("structuredContent") {
        as_object(schema, structured)?;
    }
    parse_model::<OpenAIModelContextState>(schema, value.clone())
}

fn validate_content_block(
    schema: &'static str,
    path: impl Into<String>,
    value: &Value,
) -> Result<(), ValidationError> {
    let path = path.into();
    let object = value
        .as_object()
        .ok_or_else(|| ValidationError::new(schema, &path, "expected content block object"))?;
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            get_string(schema, object, "text")?;
        }
        Some("image" | "audio") => {
            get_string(schema, object, "data")?;
            get_string(schema, object, "mimeType")?;
        }
        Some("resource_link") => {
            get_string(schema, object, "uri")?;
        }
        Some("resource") => {
            let resource = object
                .get("resource")
                .ok_or_else(|| ValidationError::new(schema, &path, "missing embedded resource"))?;
            let resource = as_object(schema, resource)?;
            get_string(schema, resource, "uri")?;
            if !(resource.get("text").is_some_and(Value::is_string)
                || resource.get("blob").is_some_and(Value::is_string))
            {
                return Err(ValidationError::new(
                    schema,
                    &path,
                    "expected text or blob resource content",
                ));
            }
        }
        Some(_) | None => {
            return Err(ValidationError::new(
                schema,
                &path,
                "unknown content block type",
            ));
        }
    }
    Ok(())
}

fn validate_message_options(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIMessageOptionsSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["target", "send"])?;
    reject_null_properties(schema, object, &["target"])?;
    if let Some(send) = object.get("send")
        && send != &Value::Bool(true)
    {
        return Err(ValidationError::new(
            schema,
            "send",
            "send may only be true",
        ));
    }
    parse_model::<OpenAIMessageOptions>(schema, value.clone())
}

fn validate_message_params(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIMessageParamsSchema";
    let object = as_object(schema, value)?;
    reject_null_properties(schema, object, &["_meta"])?;
    if get_string(schema, object, "role")? != "user" {
        return Err(ValidationError::new(schema, "role", "expected user"));
    }
    if !object.get("content").is_some_and(Value::is_array) {
        return Err(ValidationError::new(
            schema,
            "content",
            "expected content array",
        ));
    }
    if let Some(meta) = object.get("_meta").and_then(Value::as_object)
        && let Some(options) = meta.get(OPENAI_MESSAGE_KEY)
    {
        validate_message_options(options)?;
    }
    parse_model::<OpenAIMessageParams>(schema, value.clone())
}

fn validate_resource_read_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceReadMetadataSchema";
    let object = as_object(schema, value)?;
    if let Some(resource) = object.get(OPENAI_RESOURCE_METADATA_KEY) {
        let resource = as_object(schema, resource)?;
        reject_null_properties(schema, resource, &["representation"])?;
    }
    parse_model::<OpenAIResourceReadMetadata>(schema, value.clone())
}

fn validate_resource_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceMetadataSchema";
    let object = as_object(schema, value)?;
    reject_null_properties(schema, object, &["etag", "writable"])?;
    parse_model::<OpenAIResourceMetadata>(schema, value.clone())
}

fn validate_resource_content_metadata(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceContentMetadataSchema";
    let object = as_object(schema, value)?;
    if let Some(resource) = object.get(OPENAI_RESOURCE_METADATA_KEY) {
        as_object(schema, resource)?;
        validate_resource_metadata(resource)?;
    }
    parse_model::<OpenAIResourceContentMetadata>(schema, value.clone())
}

fn validate_resource_write_params(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceWriteParamsSchema";
    let object = as_object(schema, value)?;
    only_keys(schema, object, &["uri", "ifMatch", "blob", "text"])?;
    nonblank(schema, "uri", get_string(schema, object, "uri")?)?;
    if let Some(if_match) = object.get("ifMatch") {
        nonblank(
            schema,
            "ifMatch",
            if_match
                .as_str()
                .ok_or_else(|| ValidationError::new(schema, "ifMatch", "expected string"))?,
        )?;
    }
    match (object.get("blob"), object.get("text")) {
        (Some(blob), None) => {
            let blob = blob
                .as_str()
                .ok_or_else(|| ValidationError::new(schema, "blob", "expected base64 string"))?;
            if !is_base64(blob) {
                return Err(ValidationError::new(
                    schema,
                    "blob",
                    "expected base64 string",
                ));
            }
        }
        (None, Some(text)) if text.is_string() => {}
        _ => {
            return Err(ValidationError::new(
                schema,
                "",
                "expected exactly one of blob or text",
            ));
        }
    }
    parse_model::<OpenAIResourceWriteParams>(schema, value.clone())
}

fn is_base64(value: &str) -> bool {
    if !value.len().is_multiple_of(4) {
        return false;
    }
    let bytes = value.as_bytes();
    let mut seen_padding = false;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' if !seen_padding => {}
            b'=' => {
                seen_padding = true;
                if index < bytes.len().saturating_sub(2) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

fn validate_resource_write_result(value: &Value) -> Result<Value, ValidationError> {
    let schema = "OpenAIResourceWriteResultSchema";
    let object = as_object(schema, value)?;
    match get_string(schema, object, "outcome")? {
        "conflict" | "saved" => {
            get_string(schema, object, "etag")?;
        }
        "too-large" => {
            if !object.get("maxBytes").is_some_and(Value::is_number) {
                return Err(ValidationError::new(schema, "maxBytes", "expected number"));
            }
        }
        _ => return Err(ValidationError::new(schema, "outcome", "unknown outcome")),
    }
    parse_model::<OpenAIResourceWriteResult>(schema, value.clone())
}
