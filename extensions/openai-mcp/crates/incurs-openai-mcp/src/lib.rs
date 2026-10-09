//! Server-side helper facade for OpenAI MCP Extensions.
//!
//! This crate builds OpenAI MCP extension registrations and can install them on
//! an [`incurs::cli::Cli`] through the transport-neutral `ToolCatalog` path.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCallContext, McpCommandOptions,
    McpPeer, McpPeerRequest,
};
use incurs::mcp::{
    McpResource, McpResourceContents, McpResourceRegistry, McpResultMapper, McpResultMapping,
    McpServeOptions,
};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs::tool::ToolCallOutcome;
use incurs_openai_mcp_protocol::{
    JsonObject, OPENAI_ELICITATION_EXTENSION_ID, OPENAI_ELICITATION_METHOD,
    OPENAI_EXTENSIONS_META_KEY, OPENAI_SETTINGS_CAPABILITY_KEY, OPENAI_UI_META_KEY,
    OpenAiMentionSearchParams, OpenAiMentionSearchResult, OpenAiSettingsCapability,
    ValidationError, ValidationResult, get_resource_path, validate_form_content, validate_schema,
    validate_server_schema,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value, json};

/// Pinned upstream revision implemented by this facade.
pub const OPENAI_MCP_EXTENSIONS_UPSTREAM_REVISION: &str =
    "7e1be49daea03d7ec46ed2472f410099db2743d6";

/// Result type used by this facade.
pub type OpenAiServerResult<T> = Result<T, OpenAiServerError>;
/// Boxed future returned by generated handlers.
pub type BoxToolFuture<T> = Pin<Box<dyn Future<Output = OpenAiServerResult<T>> + Send + 'static>>;
/// Handler installed for a generated tool.
pub type ToolHandler<C> = Arc<dyn Fn(Value, C) -> BoxToolFuture<ToolCallResult> + Send + Sync>;
/// Handler returning every effective settings value.
pub type SettingsReadHandler<C> = Arc<dyn Fn(C) -> BoxToolFuture<JsonObject> + Send + Sync>;
/// Handler applying a partial settings update and returning every effective value.
pub type SettingsUpdateHandler<C> =
    Arc<dyn Fn(JsonObject, C) -> BoxToolFuture<JsonObject> + Send + Sync>;
/// Handler serving OpenAI mention search.
pub type MentionSearchHandler<C> = Arc<
    dyn Fn(OpenAiMentionSearchParams, C) -> BoxToolFuture<OpenAiMentionSearchResult> + Send + Sync,
>;

/// Error produced by validation or by a host adapter.
#[derive(Debug)]
pub enum OpenAiServerError {
    /// A pinned OpenAI schema rejected a value.
    Validation(ValidationError),
    /// A client capability required by the helper is absent.
    UnsupportedCapability(&'static str),
    /// The host adapter rejected a registration or runtime operation.
    Host(String),
    /// A modern MCP client must satisfy an input request before the tool can finish.
    InputRequired {
        /// Server-assigned input request objects keyed by request identifier.
        input_requests: BTreeMap<String, Value>,
        /// Opaque state the client must echo on the retry.
        request_state: Option<String>,
        /// Namespaced metadata attached to the intermediate result.
        meta: BTreeMap<String, Value>,
    },
}

impl Display for OpenAiServerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => Display::fmt(error, f),
            Self::UnsupportedCapability(capability) => {
                write!(f, "client does not support {capability}")
            }
            Self::Host(message) => f.write_str(message),
            Self::InputRequired { .. } => f.write_str("input required"),
        }
    }
}

impl Error for OpenAiServerError {}

impl From<ValidationError> for OpenAiServerError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Host surface needed to install OpenAI helper tools.
pub trait OpenAiServer<C> {
    /// Returns true after the server has connected to a transport.
    fn is_connected(&self) -> bool;
    /// Registers one tool and returns a removable handle.
    fn register_tool(
        &mut self,
        registration: ToolRegistration<C>,
    ) -> OpenAiServerResult<ToolRegistrationHandle>;
    /// Removes a previously registered tool.
    fn remove_tool(&mut self, handle: &ToolRegistrationHandle);
    /// Advertises server capabilities.
    fn register_capabilities(
        &mut self,
        capabilities: ServerCapabilitiesPatch,
    ) -> OpenAiServerResult<()>;
}

/// OpenAI server adapter that installs helpers on an [`incurs::cli::Cli`].
pub struct IncursOpenAiServer {
    cli: Option<Cli>,
    options: McpServeOptions,
    registrations: BTreeMap<String, CommandDef>,
    base_tool_names: BTreeSet<String>,
    connected: bool,
}

impl IncursOpenAiServer {
    /// Creates an adapter for the supplied CLI.
    #[must_use]
    pub fn new(cli: Cli) -> Self {
        let base_tool_names = base_tool_names(&cli);
        Self {
            cli: Some(cli),
            options: McpServeOptions::default(),
            registrations: BTreeMap::new(),
            base_tool_names,
            connected: false,
        }
    }

    /// Returns the MCP serve options accumulated by OpenAI helper registration.
    #[must_use]
    pub fn mcp_options(&self) -> &McpServeOptions {
        &self.options
    }

    /// Returns a mutable resource registry for OpenAI Apps resources.
    pub fn resources_mut(&mut self) -> &mut McpResourceRegistry {
        &mut self.options.resources
    }

    /// Sets the peer request hook passed into MCP command contexts by incurs.
    pub fn set_peer(&mut self, peer: McpPeer) {
        self.options.peer = Some(peer);
    }

    /// Consumes the adapter and returns a CLI with all staged registrations installed.
    #[must_use]
    pub fn into_cli(mut self) -> Cli {
        self.connected = true;
        let openai_tool_names = self.registrations.keys().cloned().collect::<BTreeSet<_>>();
        self.options.result_mapper = Some(McpResultMapper::new(move |context| {
            if openai_tool_names.contains(&context.tool.name)
                && let ToolCallOutcome::Ok { data, .. } = context.outcome
            {
                if data
                    .as_object()
                    .and_then(|object| object.get(OPENAI_TOOL_RESULT_MARKER))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return McpResultMapping {
                        is_error: Some(false),
                        structured_content: Some(data["structuredContent"].clone()),
                        content: Some(data["content"].as_array().cloned().unwrap_or_else(Vec::new)),
                        meta: BTreeMap::new(),
                    };
                }
                return McpResultMapping {
                    is_error: Some(false),
                    structured_content: Some(data.clone()),
                    content: Some(Vec::new()),
                    meta: BTreeMap::new(),
                };
            }
            McpResultMapping::unchanged()
        }));
        let mut cli = self
            .cli
            .take()
            .expect("CLI is consumed once")
            .mcp(self.options);
        for (name, def) in self.registrations {
            cli = cli.command(name, def);
        }
        cli
    }
}

impl OpenAiServer<CommandContext> for IncursOpenAiServer {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn register_tool(
        &mut self,
        registration: ToolRegistration<CommandContext>,
    ) -> OpenAiServerResult<ToolRegistrationHandle> {
        if self.registrations.contains_key(&registration.name) {
            return Err(OpenAiServerError::Host(format!(
                "duplicate OpenAI tool registration: {}",
                registration.name
            )));
        }
        if self.base_tool_names.contains(&registration.name)
            || self
                .cli
                .as_ref()
                .is_some_and(|cli| cli.has_command(&registration.name))
        {
            return Err(OpenAiServerError::Host(format!(
                "OpenAI tool registration collides with existing CLI tool: {}",
                registration.name
            )));
        }
        let name = registration.name.clone();
        let command = openai_registration_command(registration);
        self.registrations.insert(name.clone(), command);
        Ok(ToolRegistrationHandle { name })
    }

    fn remove_tool(&mut self, handle: &ToolRegistrationHandle) {
        self.registrations.remove(&handle.name);
    }

    fn register_capabilities(
        &mut self,
        capabilities: ServerCapabilitiesPatch,
    ) -> OpenAiServerResult<()> {
        merge_capability_map(
            &mut self.options.capabilities,
            "extensions",
            capabilities.extensions,
        );
        merge_capability_map(
            &mut self.options.capabilities,
            "experimental",
            capabilities.experimental,
        );
        Ok(())
    }
}

/// Handle returned after a host registers a tool.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolRegistrationHandle {
    /// Registered tool name.
    pub name: String,
}

/// Tool registration produced by this facade.
pub struct ToolRegistration<C> {
    /// Tool name.
    pub name: String,
    /// Human-readable tool description.
    pub description: String,
    /// Human-readable tool title.
    pub title: Option<String>,
    /// Icon descriptors advertised on the MCP tool.
    pub icons: Vec<Value>,
    /// MCP annotations.
    pub annotations: JsonObject,
    /// Tool metadata.
    pub meta: JsonObject,
    /// Successful tool-result metadata.
    pub result_meta: JsonObject,
    /// Whether this tool is directly listed by progressive discovery hosts.
    pub direct: bool,
    /// JSON Schema for tool input.
    pub input_schema: Value,
    /// JSON Schema for structured tool output.
    pub output_schema: Value,
    /// Invocation handler.
    pub handler: ToolHandler<C>,
}

/// Structured tool result returned by generated handlers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// MCP content blocks.
    pub content: Vec<Value>,
    /// MCP structured content.
    #[serde(rename = "structuredContent")]
    pub structured_content: Value,
}

impl ToolCallResult {
    /// Builds an empty-content result.
    #[must_use]
    pub fn structured(structured_content: Value) -> Self {
        Self {
            content: Vec::new(),
            structured_content,
        }
    }
}

fn base_tool_names(cli: &Cli) -> BTreeSet<String> {
    cli.try_tool_catalog()
        .map(|catalog| {
            catalog
                .definitions()
                .into_iter()
                .map(|definition| definition.name)
                .collect()
        })
        .unwrap_or_default()
}

/// Capability patch registered for OpenAI helpers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilitiesPatch {
    /// MCP extensions capability map.
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub extensions: JsonObject,
    /// Legacy experimental capability map.
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub experimental: JsonObject,
}

/// OpenAI extension facade for one server.
#[derive(Clone)]
pub struct OpenAiExtensions<C> {
    /// Form elicitation helper.
    pub elicit_input: OpenAiElicitInput,
    /// Mention-search helper.
    pub mentions: OpenAiMentions<C>,
    /// Settings helper.
    pub settings: OpenAiSettings<C>,
}

impl<C> Default for OpenAiExtensions<C> {
    fn default() -> Self {
        Self {
            elicit_input: OpenAiElicitInput,
            mentions: OpenAiMentions::default(),
            settings: OpenAiSettings::default(),
        }
    }
}

impl<C> OpenAiExtensions<C> {
    /// Creates an OpenAI server extension facade.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Form elicitation helper.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiElicitInput;

impl OpenAiElicitInput {
    /// Builds a peer request after checking capabilities and normalizing the form.
    pub fn create_request(
        &self,
        capabilities: &Value,
        mut params: Value,
        options: Option<Value>,
    ) -> OpenAiServerResult<OpenAiPeerRequest> {
        let method = elicitation_method(capabilities)?;
        let object = params.as_object_mut().ok_or_else(|| {
            ValidationError::new("OpenAIFormRequestParams", "", "expected object")
        })?;
        if object.get("mode") != Some(&Value::String("form".to_string())) {
            return Err(
                ValidationError::new("OpenAIFormRequestParams", "mode", "expected form").into(),
            );
        }
        let requested_schema = object
            .get("requestedSchema")
            .ok_or_else(|| {
                ValidationError::new(
                    "OpenAIFormRequestParams",
                    "requestedSchema",
                    "missing form schema",
                )
            })?
            .clone();
        let requested_schema = validate_schema("OpenAIFormSchema", &requested_schema)?;
        object.insert("requestedSchema".to_string(), requested_schema.clone());
        Ok(OpenAiPeerRequest {
            method: method.to_string(),
            params,
            options,
            requested_schema,
        })
    }

    /// Sends a form elicitation request through an incurs MCP peer and validates the reply.
    pub async fn request_peer(
        &self,
        peer: &McpPeer,
        capabilities: &Value,
        params: Value,
        meta: Option<Value>,
    ) -> OpenAiServerResult<Value> {
        let request = self.create_request(capabilities, params, meta)?;
        let result = peer
            .request(McpPeerRequest {
                method: request.method.clone(),
                params: Some(request.params.clone()),
                meta: request.options.clone(),
            })
            .await
            .map_err(|error| OpenAiServerError::Host(error.message))?;
        self.validate_response(&request, result)
    }

    /// Sends a form elicitation request using the peer and capabilities in a command context.
    pub async fn request_from_context(
        &self,
        context: &CommandContext,
        params: Value,
        meta: Option<Value>,
    ) -> OpenAiServerResult<Value> {
        let mcp = context
            .mcp
            .as_ref()
            .ok_or(OpenAiServerError::UnsupportedCapability(
                OPENAI_ELICITATION_EXTENSION_ID,
            ))?;
        let capabilities = mcp.client_capabilities.clone().unwrap_or(Value::Null);
        let request = self.create_request(&capabilities, params.clone(), meta.clone())?;
        let request_state = elicitation_request_state(&request)?;
        if let Some(input_responses) = &mcp.input_responses {
            if mcp.request_state.as_deref() != Some(request_state.as_str()) {
                return Err(ValidationError::new(
                    "OpenAIFormResultSchema",
                    "requestState",
                    "request state does not match the form request",
                )
                .into());
            }
            let response = input_responses
                .get(OPENAI_ELICITATION_INPUT_ID)
                .ok_or_else(|| {
                    ValidationError::new(
                        "OpenAIFormResultSchema",
                        OPENAI_ELICITATION_INPUT_ID,
                        "missing input response",
                    )
                })?
                .clone();
            return self.validate_response(&request, response);
        }
        if supports_mrtr(mcp.protocol_version.as_deref()) {
            return Err(OpenAiServerError::InputRequired {
                input_requests: BTreeMap::from([(
                    OPENAI_ELICITATION_INPUT_ID.to_string(),
                    elicitation_input_request(&request),
                )]),
                request_state: Some(request_state),
                meta: BTreeMap::new(),
            });
        }
        let peer = mcp
            .peer
            .clone()
            .ok_or(OpenAiServerError::UnsupportedCapability(
                OPENAI_ELICITATION_EXTENSION_ID,
            ))?;
        self.request_peer(&peer, &capabilities, params, meta).await
    }

    /// Validates a peer response against the original form schema.
    pub fn validate_response(
        &self,
        request: &OpenAiPeerRequest,
        result: Value,
    ) -> OpenAiServerResult<Value> {
        let mut normalized = validate_schema("OpenAIFormResultSchema", &result)?;
        if normalized.get("action") == Some(&Value::String("accept".to_string())) {
            let content = normalized
                .get("content")
                .ok_or_else(|| {
                    ValidationError::new("OpenAIFormResultSchema", "content", "missing content")
                })?
                .clone();
            let content = validate_form_content(&request.requested_schema, &content)?;
            normalized
                .as_object_mut()
                .expect("form result object")
                .insert("content".to_string(), content);
        }
        Ok(normalized)
    }
}

/// Peer request for client-side form elicitation.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiPeerRequest {
    /// Request method.
    pub method: String,
    /// Request parameters.
    pub params: Value,
    /// Transport-specific metadata preserved for the host request.
    pub options: Option<Value>,
    /// Normalized requested schema used for response validation.
    pub requested_schema: Value,
}

/// Mention-search registration builder.
#[derive(Clone)]
pub struct OpenAiMentions<C> {
    handler: Arc<Mutex<Option<MentionSearchHandler<C>>>>,
    registered: Arc<AtomicBool>,
}

impl<C> Default for OpenAiMentions<C> {
    fn default() -> Self {
        Self {
            handler: Arc::new(Mutex::new(None)),
            registered: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<C: Send + 'static> OpenAiMentions<C> {
    /// Installs or replaces the mention search handler.
    pub fn set_handler<S: OpenAiServer<C>>(
        &self,
        server: &mut S,
        handler: MentionSearchHandler<C>,
    ) -> OpenAiServerResult<()> {
        if !self.registered.load(Ordering::SeqCst) {
            let state = Arc::clone(&self.handler);
            server.register_tool(ToolRegistration {
                name: "search_mentions".to_string(),
                description: "Search OpenAI mention resources.".to_string(),
                title: Some("Search mentions".to_string()),
                icons: Vec::new(),
                annotations: BTreeMap::from([("readOnlyHint".to_string(), Value::Bool(true))]),
                meta: BTreeMap::from([
                    (OPENAI_EXTENSIONS_META_KEY.to_string(), json!({"mentions/search": {}})),
                    ("ui".to_string(), json!({"visibility": ["app"]})),
                ]),
                result_meta: JsonObject::new(),
                direct: true,
                input_schema: object_schema(BTreeMap::from([(
                    "query".to_string(),
                    json!({"type":"string"}),
                )])),
                output_schema: json!({"type":"object","properties":{"items":{"type":"array"}},"required":["items"],"additionalProperties":false}),
                handler: Arc::new(move |params, extra| {
                    let state = Arc::clone(&state);
                    Box::pin(async move {
                        let normalized = validate_schema("OpenAIMentionSearchParamsSchema", &params)?;
                        let params: OpenAiMentionSearchParams = serde_json::from_value(normalized).map_err(|error| ValidationError::new("OpenAIMentionSearchParamsSchema", "", error.to_string()))?;
                        let next = state.lock().expect("mention handler mutex").clone();
                        let result = match next {
                            Some(handler) => handler(params, extra).await?,
                            None => OpenAiMentionSearchResult { items: Vec::new() },
                        };
                        let value = serde_json::to_value(&result).map_err(|error| OpenAiServerError::Host(error.to_string()))?;
                        validate_server_schema("OpenAIMentionSearchResultSchema", &value)?;
                        Ok(ToolCallResult::structured(value))
                    })
                }),
            })?;
            self.registered.store(true, Ordering::SeqCst);
        }
        *self.handler.lock().expect("mention handler mutex") = Some(handler);
        Ok(())
    }
}

/// Settings registration builder.
#[derive(Clone)]
pub struct OpenAiSettings<C> {
    registered: Arc<AtomicBool>,
    _marker: std::marker::PhantomData<C>,
}

impl<C> Default for OpenAiSettings<C> {
    fn default() -> Self {
        Self {
            registered: Arc::new(AtomicBool::new(false)),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<C: Send + 'static> OpenAiSettings<C> {
    /// Registers settings read and update tools and advertises both capability maps.
    pub fn register<S: OpenAiServer<C>>(
        &self,
        server: &mut S,
        registration: OpenAiSettingsRegistration<C>,
    ) -> OpenAiServerResult<SettingsTools> {
        if self.registered.load(Ordering::SeqCst) {
            return Err(OpenAiServerError::Host(
                "Settings are already registered on this server.".to_string(),
            ));
        }
        if server.is_connected() {
            return Err(OpenAiServerError::Host(
                "Register settings before connecting the server.".to_string(),
            ));
        }
        let plan = SettingsPlan::new(registration)?;
        let read_handle = server.register_tool(plan.read_tool())?;
        let update_handle = match server.register_tool(plan.update_tool()) {
            Ok(handle) => handle,
            Err(error) => {
                server.remove_tool(&read_handle);
                return Err(error);
            }
        };
        let capabilities = plan.capabilities();
        if let Err(error) = server.register_capabilities(capabilities.clone()) {
            server.remove_tool(&update_handle);
            server.remove_tool(&read_handle);
            return Err(error);
        }
        self.registered.store(true, Ordering::SeqCst);
        Ok(SettingsTools {
            read: read_handle,
            update: update_handle,
            capability: plan.capability,
            capabilities,
        })
    }
}

/// One OpenAI settings field declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiSettingsField {
    /// Field JSON Schema.
    pub schema: Value,
    /// Field title.
    pub title: String,
    /// Optional field description.
    pub description: Option<String>,
}

impl OpenAiSettingsField {
    /// Creates a settings field with a native JSON Schema and title.
    #[must_use]
    pub fn new(schema: Value, title: impl Into<String>) -> Self {
        Self {
            schema,
            title: title.into(),
            description: None,
        }
    }

    /// Adds a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Settings registration request.
pub struct OpenAiSettingsRegistration<C> {
    /// Read tool name, defaulting to settings.read.
    pub read_tool: Option<String>,
    /// Update tool name, defaulting to settings.update.
    pub update_tool: Option<String>,
    /// Native value fields keyed by property name.
    pub fields: BTreeMap<String, OpenAiSettingsField>,
    /// Optional layout groups.
    pub layout: Option<Vec<Value>>,
    /// Read handler returning all effective values.
    pub read: SettingsReadHandler<C>,
    /// Update handler returning all effective values after persistence.
    pub update: SettingsUpdateHandler<C>,
}

/// Registered settings tools and capability patch.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsTools {
    /// Read tool handle.
    pub read: ToolRegistrationHandle,
    /// Update tool handle.
    pub update: ToolRegistrationHandle,
    /// Advertised OpenAI settings capability.
    pub capability: OpenAiSettingsCapability,
    /// Patch containing extensions and experimental maps.
    pub capabilities: ServerCapabilitiesPatch,
}

/// Builds OpenAI UI metadata for an app tool.
pub fn openai_ui_tool_metadata(ui: Value) -> ValidationResult<JsonObject> {
    Ok(BTreeMap::from([(
        OPENAI_UI_META_KEY.to_string(),
        validate_schema("OpenAIUiToolMetadataSchema", &ui)?,
    )]))
}

/// Builds OpenAI UI metadata for an app resource.
pub fn openai_ui_resource_metadata(ui: Value) -> ValidationResult<JsonObject> {
    Ok(BTreeMap::from([(
        OPENAI_UI_META_KEY.to_string(),
        validate_schema("OpenAIUiResourceMetadataSchema", &ui)?,
    )]))
}

/// Builds an HTML MCP Apps resource descriptor with OpenAI UI metadata.
pub fn openai_html_resource(
    uri: impl Into<String>,
    name: impl Into<String>,
    title: impl Into<String>,
    ui: Value,
) -> ValidationResult<McpResource> {
    Ok(McpResource {
        uri: uri.into(),
        name: name.into(),
        title: Some(title.into()),
        description: None,
        mime_type: Some("text/html".to_string()),
        size: None,
        icons: Vec::new(),
        meta: openai_ui_resource_metadata(ui)?,
    })
}

/// Builds a text/html MCP resource content item with OpenAI content metadata.
#[must_use]
pub fn openai_html_resource_contents(
    uri: impl Into<String>,
    html: impl Into<String>,
    meta: JsonObject,
) -> McpResourceContents {
    McpResourceContents::Text {
        uri: uri.into(),
        mime_type: Some("text/html".to_string()),
        text: html.into(),
        meta,
    }
}

/// Reads an OpenAI server resource path from tool metadata.
pub fn resource_path_from_tool_meta(meta: &Value) -> ValidationResult<Option<String>> {
    get_resource_path(meta)
}

/// OpenAI onboarding metadata for portable Agent Plugin publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiOnboardingMetadata {
    /// Onboarding title.
    pub title: String,
    /// Optional onboarding description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl OpenAiOnboardingMetadata {
    /// Serializes metadata under a reverse-domain OpenAI extension key.
    pub fn into_extension_metadata(self) -> OpenAiServerResult<JsonObject> {
        nonblank("OpenAiOnboardingMetadata", "title", self.title.clone())?;
        Ok(BTreeMap::from([(
            "com.openai/onboarding".to_string(),
            serde_json::to_value(self).expect("metadata serializes"),
        )]))
    }
}

struct SettingsPlan<C> {
    read_tool: String,
    update_tool: String,
    schema: Value,
    layout: Option<Vec<Value>>,
    fields: BTreeMap<String, OpenAiSettingsField>,
    read: SettingsReadHandler<C>,
    update: SettingsUpdateHandler<C>,
    capability: OpenAiSettingsCapability,
}

impl<C: Send + 'static> SettingsPlan<C> {
    fn new(registration: OpenAiSettingsRegistration<C>) -> OpenAiServerResult<Self> {
        let read_tool = nonblank(
            "OpenAISettingsCapabilitySchema",
            "readTool",
            registration
                .read_tool
                .unwrap_or_else(|| "settings.read".to_string()),
        )?;
        let update_tool = nonblank(
            "OpenAISettingsCapabilitySchema",
            "updateTool",
            registration
                .update_tool
                .unwrap_or_else(|| "settings.update".to_string()),
        )?;
        if read_tool == update_tool {
            return Err(ValidationError::new(
                "OpenAISettingsCapabilitySchema",
                "updateTool",
                "settings read and update tools must differ",
            )
            .into());
        }
        let capability = OpenAiSettingsCapability {
            read_tool: read_tool.clone(),
            update_tool: update_tool.clone(),
        };
        validate_server_schema(
            "OpenAISettingsCapabilitySchema",
            &serde_json::to_value(&capability).expect("capability serializes"),
        )?;
        let schema = settings_schema(&registration.fields)?;
        validate_server_schema(
            "OpenAISettingsReadResultSchema",
            &json!({"schema": schema, "layout": registration.layout, "values": {}}),
        )?;
        Ok(Self {
            read_tool,
            update_tool,
            schema,
            layout: registration.layout,
            fields: registration.fields,
            read: registration.read,
            update: registration.update,
            capability,
        })
    }

    fn read_tool(&self) -> ToolRegistration<C> {
        let read = Arc::clone(&self.read);
        let fields = self.fields.clone();
        let schema = self.schema.clone();
        let layout = self.layout.clone();
        ToolRegistration {
            name: self.read_tool.clone(),
            description: "Read current OpenAI extension settings.".to_string(),
            title: Some("Read settings".to_string()),
            icons: Vec::new(),
            annotations: BTreeMap::from([("readOnlyHint".to_string(), Value::Bool(true))]),
            meta: JsonObject::new(),
            result_meta: JsonObject::new(),
            direct: true,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            output_schema: json!({"type":"object","properties":{"schema":{"type":"object"},"layout":{"type":"array"},"values":schema},"required":["schema","values"],"additionalProperties":false}),
            handler: Arc::new(move |params, extra| {
                let read = Arc::clone(&read);
                let fields = fields.clone();
                let schema = schema.clone();
                let layout = layout.clone();
                Box::pin(async move {
                    ensure_empty(&params)?;
                    let values = read(extra).await?;
                    validate_complete_values(&fields, &values)?;
                    let mut result = BTreeMap::from([
                        ("schema".to_string(), schema),
                        (
                            "values".to_string(),
                            serde_json::to_value(values).expect("values serialize"),
                        ),
                    ]);
                    if let Some(layout) = layout {
                        result.insert("layout".to_string(), Value::Array(layout));
                    }
                    let structured = serde_json::to_value(result).expect("result serializes");
                    validate_server_schema("OpenAISettingsReadResultSchema", &structured)?;
                    Ok(ToolCallResult::structured(structured))
                })
            }),
        }
    }

    fn update_tool(&self) -> ToolRegistration<C> {
        let update = Arc::clone(&self.update);
        let fields = self.fields.clone();
        ToolRegistration {
            name: self.update_tool.clone(),
            description: "Update OpenAI extension settings.".to_string(),
            title: Some("Update settings".to_string()),
            icons: Vec::new(),
            annotations: JsonObject::new(),
            meta: JsonObject::new(),
            result_meta: JsonObject::new(),
            direct: true,
            input_schema: settings_update_input_schema(&fields),
            output_schema: json!({"type":"object","properties":{"values":settings_schema(&fields).unwrap_or_else(|_| json!({"type":"object"}))},"required":["values"],"additionalProperties":false}),
            handler: Arc::new(move |params, extra| {
                let update = Arc::clone(&update);
                let fields = fields.clone();
                Box::pin(async move {
                    validate_server_schema("OpenAISettingsUpdateArgumentsSchema", &params)?;
                    let set = btree(
                        params
                            .get("set")
                            .and_then(Value::as_object)
                            .ok_or_else(|| {
                                ValidationError::new(
                                    "OpenAISettingsUpdateArgumentsSchema",
                                    "set",
                                    "expected object",
                                )
                            })?
                            .clone(),
                    );
                    validate_partial_values(&fields, &set)?;
                    let values = update(set, extra).await?;
                    validate_complete_values(&fields, &values)?;
                    let structured = json!({"values": values});
                    validate_server_schema("OpenAISettingsUpdateResultSchema", &structured)?;
                    Ok(ToolCallResult::structured(structured))
                })
            }),
        }
    }

    fn capabilities(&self) -> ServerCapabilitiesPatch {
        let value = serde_json::to_value(&self.capability).expect("capability serializes");
        ServerCapabilitiesPatch {
            extensions: BTreeMap::from([(
                OPENAI_SETTINGS_CAPABILITY_KEY.to_string(),
                value.clone(),
            )]),
            experimental: BTreeMap::from([(OPENAI_SETTINGS_CAPABILITY_KEY.to_string(), value)]),
        }
    }
}

struct OpenAiToolCommand {
    handler: ToolHandler<CommandContext>,
}

#[async_trait::async_trait]
impl CommandHandler for OpenAiToolCommand {
    async fn run(&self, context: CommandContext) -> CommandResult {
        let params = context.options.clone();
        match (self.handler)(params, context).await {
            Ok(result) => {
                let data = if result.content.is_empty()
                    && !structured_content_has_result_marker(&result.structured_content)
                {
                    result.structured_content
                } else {
                    json!({
                        OPENAI_TOOL_RESULT_MARKER: true,
                        "content": result.content,
                        "structuredContent": result.structured_content,
                    })
                };
                CommandResult::Ok {
                    data,
                    cta: None,
                    exit_code: None,
                }
            }
            Err(OpenAiServerError::InputRequired {
                input_requests,
                request_state,
                meta,
            }) => CommandResult::InputRequired {
                input_requests,
                request_state,
                meta,
            },
            Err(error) => CommandResult::Error {
                code: "OPENAI_EXTENSION_ERROR".to_string(),
                message: error.to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

const OPENAI_ELICITATION_INPUT_ID: &str = "openai_elicitation";
const OPENAI_TOOL_RESULT_MARKER: &str = "__openaiToolResult";

fn structured_content_has_result_marker(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get(OPENAI_TOOL_RESULT_MARKER))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn elicitation_request_state(request: &OpenAiPeerRequest) -> OpenAiServerResult<String> {
    serde_json::to_string(&json!({
        "method": request.method,
        "params": request.params,
        "options": request.options,
    }))
    .map_err(|error| OpenAiServerError::Host(error.to_string()))
}

fn elicitation_input_request(request: &OpenAiPeerRequest) -> Value {
    let mut object = Map::new();
    object.insert("method".to_string(), Value::String(request.method.clone()));
    object.insert("params".to_string(), request.params.clone());
    if let Some(options) = &request.options {
        object.insert("_meta".to_string(), options.clone());
    }
    Value::Object(object)
}

fn openai_registration_command(registration: ToolRegistration<CommandContext>) -> CommandDef {
    let input_schema = registration.input_schema.clone();
    let output_schema = registration.output_schema.clone();
    let fields = input_fields(&input_schema);
    let description = registration.description.clone();
    let mcp_options = McpCommandOptions {
        name: Some(registration.name.clone()),
        description: Some(description.clone()),
        title: registration.title.clone(),
        icons: registration.icons.clone(),
        meta: registration.meta.clone(),
        result_meta: registration.result_meta.clone(),
        direct: registration.direct,
        annotations: annotations_from_map(&registration.annotations),
        input_schema: Some(input_schema),
        ..McpCommandOptions::default()
    };
    let mut command = CommandDef::build(
        registration.name,
        OpenAiToolCommand {
            handler: registration.handler,
        },
    )
    .description(description)
    .mcp(mcp_options)
    .done();
    command.options_fields = fields;
    command.output_schema = Some(output_schema);
    command
}

fn input_fields(schema: &Value) -> Vec<FieldMeta> {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| FieldMeta {
                    name: leak_string(name),
                    cli_name: name.clone(),
                    description: None,
                    field_type: field_type(schema),
                    required: required.contains(name.as_str()),
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn field_type(schema: &Value) -> FieldType {
    match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => FieldType::Boolean,
        Some("number" | "integer") => FieldType::Number,
        Some("string") => schema
            .get("enum")
            .and_then(Value::as_array)
            .map(|values| {
                FieldType::Enum(
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect(),
                )
            })
            .unwrap_or(FieldType::String),
        Some("array") => FieldType::Array(Box::new(FieldType::Value)),
        _ => FieldType::Value,
    }
}

fn annotations_from_map(map: &JsonObject) -> Option<McpAnnotations> {
    if map.is_empty() {
        return None;
    }
    Some(McpAnnotations {
        title: map
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        read_only_hint: map.get("readOnlyHint").and_then(Value::as_bool),
        destructive_hint: map.get("destructiveHint").and_then(Value::as_bool),
        idempotent_hint: map.get("idempotentHint").and_then(Value::as_bool),
        open_world_hint: map.get("openWorldHint").and_then(Value::as_bool),
    })
}

fn merge_capability_map(target: &mut BTreeMap<String, Value>, key: &str, patch: JsonObject) {
    if patch.is_empty() {
        return;
    }
    let entry = target
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    let object = entry.as_object_mut().expect("capability object");
    for (name, value) in patch {
        object.insert(name, value);
    }
}

fn leak_string(value: &str) -> &'static str {
    Box::leak(value.to_string().into_boxed_str())
}

fn elicitation_method(capabilities: &Value) -> OpenAiServerResult<&'static str> {
    if cap(
        capabilities,
        &["extensions", OPENAI_ELICITATION_EXTENSION_ID, "form"],
    ) {
        Ok(OPENAI_ELICITATION_METHOD)
    } else {
        Err(OpenAiServerError::UnsupportedCapability(
            OPENAI_ELICITATION_EXTENSION_ID,
        ))
    }
}

fn supports_mrtr(protocol_version: Option<&str>) -> bool {
    matches!(protocol_version, Some("2026-07-28"))
}

fn cap(value: &Value, path: &[&str]) -> bool {
    let mut cursor = value;
    for key in path {
        match cursor.get(*key) {
            Some(next) => cursor = next,
            None => return false,
        }
    }
    cursor.is_object()
}

fn object_schema(properties: BTreeMap<String, Value>) -> Value {
    let required = properties
        .keys()
        .cloned()
        .map(Value::String)
        .collect::<Vec<_>>();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn settings_schema(fields: &BTreeMap<String, OpenAiSettingsField>) -> OpenAiServerResult<Value> {
    let mut properties = BTreeMap::new();
    for (name, field) in fields {
        validate_field(name, field)?;
        let mut schema = field_object(field)?;
        schema.insert("title".to_string(), Value::String(field.title.clone()));
        if let Some(description) = &field.description {
            schema.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        properties.insert(name.clone(), Value::Object(schema));
    }
    Ok(object_schema(properties))
}

fn settings_update_input_schema(fields: &BTreeMap<String, OpenAiSettingsField>) -> Value {
    let properties = fields
        .iter()
        .filter_map(|(name, field)| {
            field_object(field)
                .ok()
                .map(|schema| (name.clone(), Value::Object(schema)))
        })
        .collect::<BTreeMap<_, _>>();
    json!({"type":"object","properties":{"set":{"type":"object","properties":properties,"minProperties":1,"additionalProperties":false}},"required":["set"],"additionalProperties":false})
}

fn validate_field(name: &str, field: &OpenAiSettingsField) -> OpenAiServerResult<()> {
    let presentation = match &field.description {
        Some(description) => json!({"title":field.title,"description":description}),
        None => json!({"title":field.title}),
    };
    validate_server_schema("OpenAISettingsFieldPresentationSchema", &presentation)?;
    let object = field_object(field)?;
    for (key, value) in &object {
        match key.as_str() {
            "type" | "title" | "description" | "enum" | "const" => {}
            "minLength" | "maxLength"
                if object.get("type").and_then(Value::as_str) == Some("string") =>
            {
                length_keyword(name, key, value)?;
            }
            "pattern" | "format"
                if object.get("type").and_then(Value::as_str) == Some("string") =>
            {
                string_keyword(name, key, value)?;
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf"
                if matches!(
                    object.get("type").and_then(Value::as_str),
                    Some("number" | "integer")
                ) =>
            {
                number_keyword(name, key, value)?;
            }
            _ => {
                return Err(ValidationError::new(
                    "OpenAISettingsFieldSchema",
                    name,
                    "unsupported native setting keyword",
                )
                .into());
            }
        }
    }
    let ty = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError::new("OpenAISettingsFieldSchema", name, "missing type"))?;
    if !matches!(ty, "boolean" | "string" | "number" | "integer") {
        return Err(ValidationError::new(
            "OpenAISettingsFieldSchema",
            name,
            "unsupported native setting type",
        )
        .into());
    }
    if let Some(values) = object.get("enum") {
        if ty != "string" {
            return Err(ValidationError::new(
                "OpenAISettingsFieldSchema",
                name,
                "enum settings must be strings",
            )
            .into());
        }
        let values = values.as_array().ok_or_else(|| {
            ValidationError::new("OpenAISettingsFieldSchema", name, "enum must be an array")
        })?;
        if values.is_empty() || values.iter().any(|value| value.as_str().is_none()) {
            return Err(ValidationError::new(
                "OpenAISettingsFieldSchema",
                name,
                "enum must contain strings",
            )
            .into());
        }
    }
    if let Some(value) = object.get("const") {
        validate_const_keyword(name, ty, value)?;
    }
    Ok(())
}

fn validate_complete_values(
    fields: &BTreeMap<String, OpenAiSettingsField>,
    values: &JsonObject,
) -> OpenAiServerResult<()> {
    for key in values.keys() {
        if !fields.contains_key(key) {
            return Err(ValidationError::new(
                "OpenAISettingsReadResultSchema",
                key,
                "unknown setting",
            )
            .into());
        }
    }
    for (name, field) in fields {
        let value = values.get(name).ok_or_else(|| {
            ValidationError::new("OpenAISettingsReadResultSchema", name, "missing setting")
        })?;
        validate_setting_value(name, field, value)?;
    }
    Ok(())
}

fn validate_partial_values(
    fields: &BTreeMap<String, OpenAiSettingsField>,
    values: &JsonObject,
) -> OpenAiServerResult<()> {
    for (name, value) in values {
        let field = fields.get(name).ok_or_else(|| {
            ValidationError::new(
                "OpenAISettingsUpdateArgumentsSchema",
                name,
                "unknown setting",
            )
        })?;
        validate_setting_value(name, field, value)?;
    }
    Ok(())
}

fn validate_setting_value(
    name: &str,
    field: &OpenAiSettingsField,
    value: &Value,
) -> OpenAiServerResult<()> {
    let object = field_object(field)?;
    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return Err(
            ValidationError::new("OpenAISettingsValue", name, "value is not in enum").into(),
        );
    }
    if let Some(expected) = object.get("const") {
        let ty = object.get("type").and_then(Value::as_str).ok_or_else(|| {
            ValidationError::new("OpenAISettingsValue", name, "field has no type")
        })?;
        if !const_matches(ty, expected, value) {
            return Err(ValidationError::new(
                "OpenAISettingsValue",
                name,
                "value does not match const",
            )
            .into());
        }
    }
    match object.get("type").and_then(Value::as_str) {
        Some("boolean") if value.is_boolean() => Ok(()),
        Some("string") => match value.as_str() {
            Some(text) => validate_string_constraints(name, &object, text),
            None => Err(
                ValidationError::new("OpenAISettingsValue", name, "value has wrong type").into(),
            ),
        },
        Some("number") if value.is_number() => validate_number_constraints(name, &object, value),
        Some("integer") if value.as_i64().is_some() || value.as_u64().is_some() => {
            validate_number_constraints(name, &object, value)
        }
        Some(_) => {
            Err(ValidationError::new("OpenAISettingsValue", name, "value has wrong type").into())
        }
        None => Err(ValidationError::new("OpenAISettingsValue", name, "field has no type").into()),
    }
}

fn validate_const_keyword(name: &str, ty: &str, value: &Value) -> OpenAiServerResult<()> {
    let supported = match ty {
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => false,
    };
    if supported {
        Ok(())
    } else {
        Err(ValidationError::new(
            "OpenAISettingsFieldSchema",
            name,
            "const must match the setting type",
        )
        .into())
    }
}

fn const_matches(ty: &str, expected: &Value, actual: &Value) -> bool {
    match ty {
        "number" | "integer" => match (expected.as_number(), actual.as_number()) {
            (Some(expected), Some(actual)) => numeric_json_equal(expected, actual),
            _ => false,
        },
        _ => expected == actual,
    }
}

fn numeric_json_equal(left: &Number, right: &Number) -> bool {
    normalized_json_number(left) == normalized_json_number(right)
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct NormalizedJsonNumber {
    negative: bool,
    digits: String,
    scale: usize,
}

fn normalized_json_number(number: &Number) -> Option<NormalizedJsonNumber> {
    normalize_decimal_number(&number.to_string())
}

fn normalize_decimal_number(text: &str) -> Option<NormalizedJsonNumber> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => {
            let exponent = unsigned[index + 1..].parse::<i64>().ok()?;
            (&unsigned[..index], exponent)
        }
        None => (unsigned, 0),
    };
    let (integer, fraction) = match mantissa.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (mantissa, ""),
    };
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    if !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{integer}{fraction}");
    let mut scale = fraction.len() as i64 - exponent;
    if scale < 0 {
        digits.extend(std::iter::repeat_n('0', (-scale) as usize));
        scale = 0;
    }
    let Some(first_non_zero) = digits.find(|ch| ch != '0') else {
        return Some(NormalizedJsonNumber {
            negative: false,
            digits: "0".to_string(),
            scale: 0,
        });
    };
    digits.drain(..first_non_zero);
    let mut scale = scale as usize;
    while scale > 0 && digits.ends_with('0') {
        digits.pop();
        scale -= 1;
    }
    Some(NormalizedJsonNumber {
        negative,
        digits,
        scale,
    })
}

fn validate_string_constraints(
    name: &str,
    object: &Map<String, Value>,
    text: &str,
) -> OpenAiServerResult<()> {
    let length = text.encode_utf16().count();
    if let Some(value) = object.get("minLength") {
        let minimum = length_keyword(name, "minLength", value)?;
        if length < minimum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "string is too short").into(),
            );
        }
    }
    if let Some(value) = object.get("maxLength") {
        let maximum = length_keyword(name, "maxLength", value)?;
        if length > maximum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "string is too long").into(),
            );
        }
    }
    validate_string_pattern_and_format(name, object, text)?;
    Ok(())
}

fn validate_number_constraints(
    name: &str,
    object: &Map<String, Value>,
    value: &Value,
) -> OpenAiServerResult<()> {
    let number = value
        .as_f64()
        .ok_or_else(|| ValidationError::new("OpenAISettingsValue", name, "value has wrong type"))?;
    if let Some(value) = object.get("minimum") {
        let minimum = number_keyword(name, "minimum", value)?;
        if number < minimum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "number is too small").into(),
            );
        }
    }
    if let Some(value) = object.get("maximum") {
        let maximum = number_keyword(name, "maximum", value)?;
        if number > maximum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "number is too large").into(),
            );
        }
    }
    if let Some(value) = object.get("exclusiveMinimum") {
        let minimum = number_keyword(name, "exclusiveMinimum", value)?;
        if number <= minimum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "number is too small").into(),
            );
        }
    }
    if let Some(value) = object.get("exclusiveMaximum") {
        let maximum = number_keyword(name, "exclusiveMaximum", value)?;
        if number >= maximum {
            return Err(
                ValidationError::new("OpenAISettingsValue", name, "number is too large").into(),
            );
        }
    }
    if let Some(value) = object.get("multipleOf") {
        let multiple = number_keyword(name, "multipleOf", value)?;
        if multiple <= 0.0 || !multiple.is_finite() {
            return Err(ValidationError::new(
                "OpenAISettingsFieldSchema",
                name,
                "multipleOf must be positive",
            )
            .into());
        }
        let quotient = number / multiple;
        if (quotient - quotient.round()).abs() > f64::EPSILON * quotient.abs().max(1.0) {
            return Err(ValidationError::new(
                "OpenAISettingsValue",
                name,
                "number is not a valid multiple",
            )
            .into());
        }
    }
    Ok(())
}

fn validate_string_pattern_and_format(
    name: &str,
    object: &Map<String, Value>,
    text: &str,
) -> OpenAiServerResult<()> {
    let mut field = Map::new();
    field.insert("type".to_string(), Value::String("string".to_string()));
    if let Some(pattern) = object.get("pattern") {
        field.insert("pattern".to_string(), pattern.clone());
    }
    if let Some(format) = object.get("format") {
        field.insert("format".to_string(), format.clone());
    }
    if field.len() == 1 {
        return Ok(());
    }
    let form = json!({
        "type":"object",
        "properties":{"value": Value::Object(field)},
        "required":["value"],
        "additionalProperties":false,
    });
    validate_form_content(&form, &json!({"value": text})).map_err(|error| {
        OpenAiServerError::from(ValidationError::new(
            "OpenAISettingsValue",
            name,
            error.to_string(),
        ))
    })?;
    Ok(())
}

fn string_keyword(name: &str, keyword: &str, value: &Value) -> OpenAiServerResult<String> {
    value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
        ValidationError::new(
            "OpenAISettingsFieldSchema",
            name,
            format!("{keyword} must be a string"),
        )
        .into()
    })
}

fn length_keyword(name: &str, keyword: &str, value: &Value) -> OpenAiServerResult<usize> {
    let length = value.as_u64().and_then(|value| usize::try_from(value).ok());
    length.ok_or_else(|| {
        ValidationError::new(
            "OpenAISettingsFieldSchema",
            name,
            format!("{keyword} must be an integer"),
        )
        .into()
    })
}

fn number_keyword(name: &str, keyword: &str, value: &Value) -> OpenAiServerResult<f64> {
    value.as_f64().ok_or_else(|| {
        ValidationError::new(
            "OpenAISettingsFieldSchema",
            name,
            format!("{keyword} must be a number"),
        )
        .into()
    })
}

fn field_object(field: &OpenAiSettingsField) -> OpenAiServerResult<Map<String, Value>> {
    field.schema.as_object().cloned().ok_or_else(|| {
        ValidationError::new("OpenAISettingsFieldSchema", "", "expected object").into()
    })
}

fn ensure_empty(params: &Value) -> OpenAiServerResult<()> {
    match params.as_object() {
        Some(object) if object.is_empty() => Ok(()),
        _ => Err(
            ValidationError::new("OpenAISettingsReadArguments", "", "expected empty object").into(),
        ),
    }
}

fn nonblank(
    schema: &'static str,
    path: impl Into<String>,
    value: String,
) -> OpenAiServerResult<String> {
    if value.chars().any(|ch| !ch.is_whitespace()) {
        Ok(value)
    } else {
        Err(ValidationError::new(schema, path, "expected nonblank string").into())
    }
}

fn btree(map: Map<String, Value>) -> JsonObject {
    map.into_iter().collect()
}

fn _keep_mcp_call_context_public(_: Option<McpCallContext>) {}
