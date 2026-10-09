//! OpenAI MCP App extension helpers.
//!
//! This crate mirrors the OpenAI app SDK extension surface at upstream revision
//! `7e1be49daea03d7ec46ed2472f410099db2743d6` while keeping the runtime
//! transport provider-neutral through [`incurs_mcp_apps`].

#![deny(missing_docs)]

use incurs_mcp_apps::{AppError, AppResult, AppTransport, McpApp, RequestOptions};
use incurs_openai_mcp_protocol::validate_schema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// Pinned upstream revision used for this helper surface.
pub const OPENAI_MCP_EXTENSIONS_UPSTREAM_REVISION: &str =
    "7e1be49daea03d7ec46ed2472f410099db2743d6";

/// SHA-256 of the upstream `typescript/styles.css` file at the pinned revision.
pub const OPENAI_APP_STYLES_SHA256: &str =
    "21d09125579c5f96e5bc2650dcc69dba03e6c96a1079a70a3304aecb7593895b";

/// OpenAI MCP App stylesheet shipped with this crate.
pub const OPENAI_APP_STYLES_CSS: &str = include_str!("../styles.css");

#[cfg(target_arch = "wasm32")]
const OPENAI_APP_STYLES_ELEMENT_ID: &str = "openai-mcp-app-styles";

/// Host context key for OpenAI deep links.
pub const OPENAI_DEEP_LINK_KEY: &str = "openai/deepLink";

/// Experimental host capability key for OpenAI file helpers.
pub const OPENAI_FILES_CAPABILITY_KEY: &str = "openai/files";

/// Method that opens a host file.
pub const OPENAI_FILE_OPEN_METHOD: &str = "openai/files/open";

/// Experimental host capability key for OpenAI message helpers.
pub const OPENAI_MESSAGE_KEY: &str = "openai/message";

/// Experimental host capability key for OpenAI model-context helpers.
pub const OPENAI_MODEL_CONTEXT_KEY: &str = "openai/modelContext";

/// Experimental metadata key for OpenAI resources.
pub const OPENAI_RESOURCE_METADATA_KEY: &str = "openai/resource";

/// Host-context key for the OpenAI interaction cursor CSS state.
pub const OPENAI_INTERACTION_CURSOR_KEY: &str = "openai/interactionCursor";

/// Method that writes host-managed resources.
pub const OPENAI_MCP_APP_RESOURCE_WRITE_METHOD: &str = "openai/resources/write";

/// Deep-link state exposed by the host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiDeepLinkHostState {
    /// Current deep-link URL.
    pub url: String,
}

/// File entrypoint input supplied by OpenAI hosts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiFileEntrypointInput {
    /// File opened by the user.
    pub file: OpenAiEntrypointFile,
}

/// File value inside [`OpenAiFileEntrypointInput`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiEntrypointFile {
    /// Display name of the file.
    pub name: String,
    /// Host resource URI for the file.
    pub resource_uri: String,
}

/// OpenAI-specific helper surface for an MCP App instance.
#[derive(Clone)]
pub struct OpenAiAppExtensions<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiAppExtensions<T>
where
    T: AppTransport,
{
    /// Creates OpenAI helpers over one neutral MCP App runtime.
    pub fn new(app: McpApp<T>) -> Self {
        Self { app }
    }

    /// Returns deep-link helpers. Deep links are host-context state and are always readable.
    pub fn deep_link(&self) -> OpenAiDeepLink<T> {
        OpenAiDeepLink {
            app: self.app.clone(),
        }
    }

    /// Returns file helpers when the host advertises a truthy `openai/files` value.
    pub fn files(&self) -> Option<OpenAiFiles<T>> {
        self.capability_value(OPENAI_FILES_CAPABILITY_KEY)
            .is_some_and(is_javascript_truthy)
            .then(|| OpenAiFiles {
                app: self.app.clone(),
            })
    }

    /// Returns message helpers when the host advertises a non-null `openai/message` value.
    pub fn message(&self) -> Option<OpenAiMessage<T>> {
        self.has_non_null_capability(OPENAI_MESSAGE_KEY)
            .then(|| OpenAiMessage {
                app: self.app.clone(),
            })
    }

    /// Returns model-context helpers when the host advertises a non-null `openai/modelContext` value.
    pub fn model_context(&self) -> Option<OpenAiModelContext<T>> {
        self.has_non_null_capability(OPENAI_MODEL_CONTEXT_KEY)
            .then(|| OpenAiModelContext {
                app: self.app.clone(),
            })
    }

    /// Returns resource helpers when the host advertises a non-null `openai/resource` value.
    pub fn resources(&self) -> Option<OpenAiResources<T>> {
        self.has_non_null_capability(OPENAI_RESOURCE_METADATA_KEY)
            .then(|| OpenAiResources {
                app: self.app.clone(),
            })
    }

    /// Applies the interaction cursor CSS variable and keeps it synced with host context.
    #[cfg(target_arch = "wasm32")]
    pub fn install_interaction_cursor_style(
        &self,
    ) -> AppResult<incurs_mcp_apps::ListenerRegistration> {
        apply_interaction_cursor_style(&self.app.host_context())?;
        let app = self.app.clone();
        Ok(app.add_host_context_listener(move |context| {
            let _ = apply_interaction_cursor_style(context);
        }))
    }

    fn capability_value(&self, key: &str) -> Option<Value> {
        self.app
            .host_capabilities()
            .and_then(|capabilities| capabilities.experimental.get(key).cloned())
    }

    fn has_non_null_capability(&self, key: &str) -> bool {
        self.capability_value(key)
            .is_some_and(|capability| !capability.is_null())
    }
}

/// Deep-link helpers for one OpenAI MCP App instance.
#[derive(Clone)]
pub struct OpenAiDeepLink<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiDeepLink<T>
where
    T: AppTransport,
{
    /// Returns the current deep-link state, including legacy host payload normalization.
    pub fn current(&self) -> Option<OpenAiDeepLinkHostState> {
        normalize_deep_link(self.app.host_context().get(OPENAI_DEEP_LINK_KEY)?)
    }
}

/// File helpers for one OpenAI MCP App instance.
#[derive(Clone)]
pub struct OpenAiFiles<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiFiles<T>
where
    T: AppTransport,
{
    /// Opens a file in the host's native viewer or editor.
    pub async fn open(&self, path: impl Into<String>) -> AppResult<EmptyResult> {
        let path = path.into();
        if path.is_empty() {
            return Err(AppError::validation("path", "path must not be empty"));
        }
        let params = validate_openai_schema("OpenAIFileOpenParamsSchema", json!({ "path": path }))?;
        self.app
            .request(OPENAI_FILE_OPEN_METHOD, params, RequestOptions::default())
            .await
    }
}

/// Empty JSON-RPC result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyResult {}

/// Message helpers for one OpenAI MCP App instance.
#[derive(Clone)]
pub struct OpenAiMessage<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiMessage<T>
where
    T: AppTransport,
{
    /// Sends content to the active or a new conversation.
    pub async fn send(
        &self,
        params: OpenAiMessageParams,
        options: RequestOptions,
    ) -> AppResult<Value> {
        let params = validate_message_params(
            serde_json::to_value(params).expect("message params serialize"),
        )?;
        self.app.send_message(params, options).await
    }
}

/// Parameters for an OpenAI message send.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiMessageParams {
    /// Message role. OpenAI hosts accept only `user` for app-originated sends.
    pub role: MessageRole,
    /// Message content blocks.
    pub content: Vec<Value>,
    /// MCP request metadata.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<OpenAiMessageMetadata>,
}

impl OpenAiMessageParams {
    /// Creates user-message params with default OpenAI send options.
    pub fn user(content: Vec<Value>) -> Self {
        Self {
            role: MessageRole::User,
            content,
            meta: Some(OpenAiMessageMetadata {
                openai_message: Some(OpenAiMessageOptions::default()),
            }),
        }
    }
}

/// Message role accepted by OpenAI app-originated messages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// User-authored message content.
    User,
}

/// OpenAI message metadata wrapper.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiMessageMetadata {
    /// OpenAI message options under the exact `openai/message` key.
    #[serde(
        rename = "openai/message",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub openai_message: Option<OpenAiMessageOptions>,
}

/// Options for routing an app-originated message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiMessageOptions {
    /// Which conversation receives the content.
    #[serde(default = "default_message_target")]
    pub target: OpenAiMessageTarget,
    /// Whether to send the content immediately.
    #[serde(default = "default_send_message")]
    pub send: bool,
}

impl Default for OpenAiMessageOptions {
    fn default() -> Self {
        Self {
            target: default_message_target(),
            send: default_send_message(),
        }
    }
}

/// Target conversation for message sends.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiMessageTarget {
    /// Send to the active conversation.
    Active,
    /// Send to a new conversation.
    New,
}

fn default_message_target() -> OpenAiMessageTarget {
    OpenAiMessageTarget::Active
}

fn default_send_message() -> bool {
    true
}

/// Model-context helpers for one OpenAI MCP App instance.
#[derive(Clone)]
pub struct OpenAiModelContext<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiModelContext<T>
where
    T: AppTransport,
{
    /// Returns the current host-provided model context state.
    pub fn current(&self) -> Option<OpenAiModelContextHostState> {
        let value = self
            .app
            .host_context()
            .get(OPENAI_MODEL_CONTEXT_KEY)?
            .clone();
        let value = validate_openai_schema("OpenAIModelContextHostStateSchema", value).ok()?;
        serde_json::from_value(value).ok()
    }

    /// Updates model context and returns the OpenAI update identifier when present.
    pub async fn update(
        &self,
        params: Value,
        options: RequestOptions,
    ) -> AppResult<Option<OpenAiModelContextUpdateResult>> {
        let result: Value = self.app.update_model_context(params, options).await?;
        let Some(meta) = result.get("_meta") else {
            return Ok(None);
        };
        if meta.get(OPENAI_MODEL_CONTEXT_KEY).is_none() {
            return Ok(None);
        }
        let meta = validate_openai_schema("OpenAIModelContextMetadataSchema", meta.clone())?;
        meta.get(OPENAI_MODEL_CONTEXT_KEY)
            .cloned()
            .map(|value| decode_validated(incurs_mcp_apps::MODEL_CONTEXT_UPDATE_METHOD, value))
            .transpose()
    }
}

/// Host state for current model context. `None` means the host explicitly cleared context.
pub type OpenAiModelContextHostState = Option<OpenAiModelContextValue>;

/// Non-null model context state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiModelContextValue {
    /// Optional content blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Value>>,
    /// Optional structured content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Map<String, Value>>,
    /// Host update identifier.
    pub update_id: String,
}

/// Result metadata returned after a model-context update.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiModelContextUpdateResult {
    /// Host update identifier.
    pub update_id: String,
}

/// Resource helpers for one OpenAI MCP App instance.
#[derive(Clone)]
pub struct OpenAiResources<T>
where
    T: AppTransport,
{
    app: McpApp<T>,
}

impl<T> OpenAiResources<T>
where
    T: AppTransport,
{
    /// Registers a host resource update listener.
    pub fn add_update_handler(
        &self,
        handler: impl Fn(ResourceUpdatedNotification) + 'static,
    ) -> incurs_mcp_apps::ListenerRegistration {
        self.app.transport().on(
            "notifications/resources/updated",
            std::rc::Rc::new(move |value| {
                if let Ok(notification) = serde_json::from_value(value) {
                    handler(notification);
                }
            }),
        )
    }

    /// Reads a host-managed resource and parses OpenAI metadata on each content item.
    pub async fn read(
        &self,
        mut params: OpenAiResourceReadParams,
        options: RequestOptions,
    ) -> AppResult<OpenAiResourceReadResult> {
        if let Some(representation) = params.representation.take() {
            params
                .meta
                .get_or_insert_with(OpenAiResourceReadMetadata::default)
                .openai_resource
                .get_or_insert_with(OpenAiResourceReadPreference::default)
                .representation = Some(representation);
        }
        let value = serde_json::to_value(params).expect("resource params serialize");
        if let Some(meta) = value.get("_meta") {
            validate_openai_schema("OpenAIResourceReadMetadataSchema", meta.clone())?;
        }
        let result: Value = self.app.read_server_resource(value, options).await?;
        let result: OpenAiRawResourceReadResult =
            decode_validated(incurs_mcp_apps::RESOURCE_READ_METHOD, result)?;
        Ok(OpenAiResourceReadResult {
            contents: result
                .contents
                .into_iter()
                .map(OpenAiResourceContent::try_from)
                .collect::<AppResult<Vec<_>>>()?,
        })
    }

    /// Subscribes to updates for one host-managed resource URI.
    pub async fn subscribe(
        &self,
        uri: impl Into<String>,
        options: RequestOptions,
    ) -> AppResult<EmptyResult> {
        let uri = non_blank("uri", uri.into())?;
        self.app
            .request("resources/subscribe", json!({ "uri": uri }), options)
            .await
    }

    /// Unsubscribes from updates for one host-managed resource URI.
    pub async fn unsubscribe(
        &self,
        uri: impl Into<String>,
        options: RequestOptions,
    ) -> AppResult<EmptyResult> {
        let uri = non_blank("uri", uri.into())?;
        self.app
            .request("resources/unsubscribe", json!({ "uri": uri }), options)
            .await
    }

    /// Writes a host-managed resource.
    pub async fn write(
        &self,
        uri: impl Into<String>,
        content: OpenAiResourceWriteOptions,
        options: RequestOptions,
    ) -> AppResult<OpenAiResourceWriteResult> {
        let params = OpenAiResourceWriteParams::new(uri.into(), content)?;
        let result: Value = self
            .app
            .request(
                OPENAI_MCP_APP_RESOURCE_WRITE_METHOD,
                serde_json::to_value(params).expect("resource write params serialize"),
                options,
            )
            .await?;
        let result = validate_openai_schema("OpenAIResourceWriteResultSchema", result)?;
        decode_validated(OPENAI_MCP_APP_RESOURCE_WRITE_METHOD, result)
    }
}

/// Resource representation requested from the host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiResourceRepresentation {
    /// Request base64 blob content.
    Blob,
    /// Request UTF-8 text content.
    Text,
}

/// Metadata carried in `params._meta` on `resources/read` requests.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiResourceReadMetadata {
    /// OpenAI resource preference under the exact `openai/resource` key.
    #[serde(
        rename = "openai/resource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub openai_resource: Option<OpenAiResourceReadPreference>,
}

/// OpenAI resource read preference.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiResourceReadPreference {
    /// Requested representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<OpenAiResourceRepresentation>,
}

/// Parameters for reading a host-managed resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiResourceReadParams {
    /// Resource URI.
    pub uri: String,
    /// MCP metadata for read preferences.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<OpenAiResourceReadMetadata>,
    /// Convenience representation override merged into `_meta` before transport.
    #[serde(skip)]
    pub representation: Option<OpenAiResourceRepresentation>,
}

/// Metadata returned with resource content.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiResourceMetadata {
    /// Entity tag for optimistic writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Whether the host allows writes to this content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
}

/// Resource content with parsed OpenAI metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiResourceContent {
    /// Resource URI.
    pub uri: String,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Text representation, when returned by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64 blob representation, when returned by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// Raw MCP metadata.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
    /// Parsed OpenAI metadata.
    #[serde(skip)]
    pub openai_metadata: Option<OpenAiResourceMetadata>,
}

impl TryFrom<OpenAiRawResourceContent> for OpenAiResourceContent {
    type Error = AppError;

    fn try_from(content: OpenAiRawResourceContent) -> AppResult<Self> {
        if let Some(meta) = &content.meta {
            validate_openai_schema("OpenAIResourceContentMetadataSchema", meta.clone())?;
        }
        let openai_metadata = content
            .meta
            .as_ref()
            .and_then(|meta| meta.get(OPENAI_RESOURCE_METADATA_KEY))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        Ok(Self {
            uri: content.uri,
            mime_type: content.mime_type,
            text: content.text,
            blob: content.blob,
            meta: content.meta,
            openai_metadata,
        })
    }
}

/// Resource read result with parsed OpenAI content metadata.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenAiResourceReadResult {
    /// Returned resource contents.
    pub contents: Vec<OpenAiResourceContent>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiRawResourceReadResult {
    contents: Vec<OpenAiRawResourceContent>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiRawResourceContent {
    uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blob: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    meta: Option<Value>,
}

/// Options for writing resource content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenAiResourceWriteOptions {
    /// Optional ETag for optimistic concurrency.
    pub if_match: Option<String>,
    /// Resource content payload.
    pub content: OpenAiResourceWriteContent,
}

/// Resource content payload for writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenAiResourceWriteContent {
    /// Base64 encoded blob content.
    Blob(String),
    /// UTF-8 text content.
    Text(String),
}

/// Wire parameters for `openai/resources/write`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiResourceWriteParams {
    /// Resource URI.
    pub uri: String,
    /// Optional ETag for optimistic concurrency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_match: Option<String>,
    /// Base64 blob content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// Text content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl OpenAiResourceWriteParams {
    /// Builds and validates resource write parameters.
    pub fn new(uri: String, options: OpenAiResourceWriteOptions) -> AppResult<Self> {
        let uri = non_blank("uri", uri)?;
        let if_match = options
            .if_match
            .map(|value| non_blank("ifMatch", value))
            .transpose()?;
        let (blob, text) = match options.content {
            OpenAiResourceWriteContent::Blob(blob) => (Some(blob), None),
            OpenAiResourceWriteContent::Text(text) => (None, Some(text)),
        };
        let params = Self {
            uri,
            if_match,
            blob,
            text,
        };
        let value = validate_openai_schema(
            "OpenAIResourceWriteParamsSchema",
            serde_json::to_value(&params).expect("resource write params serialize"),
        )?;
        decode_validated(OPENAI_MCP_APP_RESOURCE_WRITE_METHOD, value)
    }
}

/// Result of an OpenAI resource write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum OpenAiResourceWriteResult {
    /// The resource was saved.
    Saved {
        /// New ETag for the saved content.
        etag: String,
    },
    /// The write conflicted with the current resource ETag.
    Conflict {
        /// Current resource ETag.
        etag: String,
    },
    /// The host rejected the write because the content was too large.
    TooLarge {
        /// Maximum accepted payload size in bytes.
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
}

/// Resource update notification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceUpdatedNotification {
    /// Notification method.
    pub method: String,
    /// Notification params.
    pub params: ResourceUpdatedParams,
}

/// Resource update notification parameters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceUpdatedParams {
    /// Updated resource URI.
    pub uri: String,
}

fn validate_openai_schema(schema: &'static str, value: Value) -> AppResult<Value> {
    validate_schema(schema, &value).map_err(|error| AppError::validation(schema, error.to_string()))
}

fn decode_validated<T>(method: impl Into<String>, value: Value) -> AppResult<T>
where
    T: DeserializeOwned,
{
    let method = method.into();
    serde_json::from_value(value).map_err(|error| AppError::Decode {
        method,
        message: error.to_string(),
    })
}

fn validate_message_params(value: Value) -> AppResult<Value> {
    let value = validate_openai_schema("OpenAIMessageParamsSchema", value)?;
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::validation("content", "expected content array"))?;
    for (index, block) in content.iter().enumerate() {
        validate_content_block(index, block)?;
    }
    Ok(value)
}

fn validate_content_block(index: usize, block: &Value) -> AppResult<()> {
    let object = block.as_object().ok_or_else(|| {
        AppError::validation("content", format!("content[{index}] must be an object"))
    })?;
    match object.get("type").and_then(Value::as_str) {
        Some("text") => require_string_field(index, object, "text"),
        Some("image") | Some("audio") => {
            require_string_field(index, object, "data")?;
            require_string_field(index, object, "mimeType")
        }
        Some("resource") => {
            if object.get("resource").is_some_and(Value::is_object) {
                Ok(())
            } else {
                Err(AppError::validation(
                    "content",
                    format!("content[{index}].resource must be an object"),
                ))
            }
        }
        Some("resource_link") => {
            require_string_field(index, object, "uri")?;
            require_string_field(index, object, "name")
        }
        Some(kind) => Err(AppError::validation(
            "content",
            format!("content[{index}].type is not supported: {kind}"),
        )),
        None => Err(AppError::validation(
            "content",
            format!("content[{index}].type must be a string"),
        )),
    }
}

fn require_string_field(
    index: usize,
    object: &Map<String, Value>,
    name: &'static str,
) -> AppResult<()> {
    if object.get(name).is_some_and(Value::is_string) {
        Ok(())
    } else {
        Err(AppError::validation(
            "content",
            format!("content[{index}].{name} must be a string"),
        ))
    }
}

fn is_javascript_truthy(value: Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        Value::Bool(true) | Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_interaction_cursor_style(context: &Value) -> AppResult<()> {
    install_openai_app_styles()?;
    let cursor = match context
        .get(OPENAI_INTERACTION_CURSOR_KEY)
        .and_then(Value::as_str)
    {
        Some("default") => "default",
        Some("pointer") => "pointer",
        _ => "pointer",
    };
    let window = web_sys::window().ok_or_else(|| AppError::Transport("missing window".into()))?;
    let document = window
        .document()
        .ok_or_else(|| AppError::Transport("missing document".into()))?;
    let root = document
        .document_element()
        .ok_or_else(|| AppError::Transport("missing document element".into()))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| AppError::Transport("document element is not an HtmlElement".into()))?;
    root.style()
        .set_property("--cursor-interaction", cursor)
        .map_err(|error| AppError::Transport(format!("cursor style update failed: {error:?}")))
}

#[cfg(target_arch = "wasm32")]
fn install_openai_app_styles() -> AppResult<()> {
    let window = web_sys::window().ok_or_else(|| AppError::Transport("missing window".into()))?;
    let document = window
        .document()
        .ok_or_else(|| AppError::Transport("missing document".into()))?;
    if document
        .get_element_by_id(OPENAI_APP_STYLES_ELEMENT_ID)
        .is_some()
    {
        return Ok(());
    }
    let style = document
        .create_element("style")
        .map_err(|error| AppError::Transport(format!("style create failed: {error:?}")))?;
    style
        .set_attribute("id", OPENAI_APP_STYLES_ELEMENT_ID)
        .map_err(|error| AppError::Transport(format!("style id failed: {error:?}")))?;
    style.set_text_content(Some(OPENAI_APP_STYLES_CSS));
    document
        .document_element()
        .ok_or_else(|| AppError::Transport("missing document element".into()))?
        .append_child(&style)
        .map_err(|error| AppError::Transport(format!("style append failed: {error:?}")))?;
    Ok(())
}

fn normalize_deep_link(value: &Value) -> Option<OpenAiDeepLinkHostState> {
    if let Some(url) = value.get("url").and_then(Value::as_str) {
        return Some(OpenAiDeepLinkHostState {
            url: url.to_string(),
        });
    }
    let path = value.get("path")?.as_array()?;
    let query = value.get("query")?.as_array()?;
    let path = path
        .iter()
        .map(|segment| segment.as_str().map(percent_encode))
        .collect::<Option<Vec<_>>>()?
        .join("/");
    let query = query
        .iter()
        .map(|entry| {
            let pair = entry.as_array()?;
            let key = percent_encode(pair.first()?.as_str()?);
            let value = percent_encode(pair.get(1)?.as_str()?);
            Some(format!("{key}={value}"))
        })
        .collect::<Option<Vec<_>>>()?
        .join("&");
    Some(OpenAiDeepLinkHostState {
        url: format!(
            "/{path}{}",
            if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            }
        ),
    })
}

fn non_blank(field: &'static str, value: String) -> AppResult<String> {
    if value.chars().any(|character| !character.is_whitespace()) {
        Ok(value)
    } else {
        Err(AppError::validation(
            field,
            "value must contain a non-whitespace character",
        ))
    }
}

fn percent_encode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
