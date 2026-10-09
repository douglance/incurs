//! MCP App lifecycle state and high-level host requests.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fmt,
    rc::Rc,
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use crate::{AppError, AppResult, AppTransport, ListenerRegistration};

/// Method used by the neutral app bridge to initialize with the host.
pub const INITIALIZE_METHOD: &str = "ui/initialize";

/// Notification sent after the app has accepted host initialization state.
pub const INITIALIZED_NOTIFICATION_METHOD: &str = "ui/notifications/initialized";

/// Notification emitted by the host when host-context keys change.
pub const HOST_CONTEXT_CHANGED_METHOD: &str = "ui/notifications/host-context-changed";

/// Notification emitted by the host when tool input changes.
pub const TOOL_INPUT_NOTIFICATION_METHOD: &str = "ui/notifications/tool-input";

/// Notification emitted by the host when partial tool input changes.
pub const TOOL_INPUT_PARTIAL_NOTIFICATION_METHOD: &str = "ui/notifications/tool-input-partial";

/// Notification emitted by the host when the tool result changes.
pub const TOOL_RESULT_NOTIFICATION_METHOD: &str = "ui/notifications/tool-result";

/// Notification emitted by the host when a tool request is cancelled.
pub const TOOL_CANCELLED_NOTIFICATION_METHOD: &str = "ui/notifications/tool-cancelled";

/// Notification emitted by the host when a request view is torn down.
pub const REQUEST_TEARDOWN_NOTIFICATION_METHOD: &str = "ui/notifications/request-teardown";

/// Method used by the neutral app bridge to request a display-mode change.
pub const REQUEST_DISPLAY_MODE_METHOD: &str = "ui/request-display-mode";

/// Method used by the neutral app bridge to send content to the host.
pub const SEND_MESSAGE_METHOD: &str = "ui/message";

/// Method used by the neutral app bridge to update model context.
pub const MODEL_CONTEXT_UPDATE_METHOD: &str = "ui/update-model-context";

/// Method used by the neutral app bridge to read a server resource.
pub const RESOURCE_READ_METHOD: &str = "resources/read";

/// Cancellation handle shared with one or more MCP App requests.
#[derive(Clone, Default)]
pub struct RequestCancellation {
    inner: Rc<RequestCancellationState>,
}

#[derive(Default)]
struct RequestCancellationState {
    cancelled: Cell<bool>,
    next_listener: Cell<u64>,
    listeners: RefCell<BTreeMap<u64, Rc<dyn Fn()>>>,
}

impl RequestCancellation {
    /// Creates a cancellation handle in the active state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the handle cancelled and notifies current listeners once.
    pub fn cancel(&self) {
        if self.inner.cancelled.replace(true) {
            return;
        }
        let listeners = self
            .inner
            .listeners
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener();
        }
        self.inner.listeners.borrow_mut().clear();
    }

    /// Returns true after [`Self::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.get()
    }

    /// Registers a callback that runs when the handle is cancelled.
    pub fn on_cancelled(&self, listener: impl Fn() + 'static) -> ListenerRegistration {
        if self.is_cancelled() {
            listener();
            return ListenerRegistration::new(|| {});
        }
        let id = self.inner.next_listener.get() + 1;
        self.inner.next_listener.set(id);
        self.inner
            .listeners
            .borrow_mut()
            .insert(id, Rc::new(listener));
        let inner = self.inner.clone();
        ListenerRegistration::new(move || {
            inner.listeners.borrow_mut().remove(&id);
        })
    }
}

impl fmt::Debug for RequestCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Options applied to one MCP App request.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    /// Request timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Optional cancellation handle that rejects the request when cancelled.
    pub cancellation: Option<RequestCancellation>,
}

impl RequestOptions {
    /// Converts the timeout option into a duration.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_ms.map(Duration::from_millis)
    }

    /// Returns true when the attached cancellation handle has already fired.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(RequestCancellation::is_cancelled)
    }
}

/// Host capabilities captured after app initialization.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct HostCapabilities {
    /// Experimental capability namespace advertised by the host.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub experimental: Map<String, Value>,
}

impl HostCapabilities {
    /// Returns true when an experimental capability key is present.
    pub fn has_experimental(&self, key: &str) -> bool {
        self.experimental.contains_key(key)
    }
}

/// Parameters sent by an app during the `ui/initialize` handshake.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Application information advertised to the host.
    pub app_info: Value,
    /// Application capabilities advertised to the host.
    #[serde(default)]
    pub app_capabilities: Value,
    /// MCP App protocol version requested by the app.
    pub protocol_version: String,
}

impl InitializeParams {
    /// Creates initialization parameters with a simple name and version.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        protocol_version: impl Into<String>,
    ) -> Self {
        Self {
            app_info: json!({ "name": name.into(), "version": version.into() }),
            app_capabilities: json!({}),
            protocol_version: protocol_version.into(),
        }
    }
}

/// Result returned by the host from the `ui/initialize` handshake.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// MCP App protocol version selected by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// Host information advertised by the embedding client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_info: Option<Value>,
    /// Host capabilities advertised by the embedding client.
    #[serde(default)]
    pub host_capabilities: HostCapabilities,
    /// Initial host context object.
    #[serde(default = "empty_object")]
    pub host_context: Value,
    /// Initial tool input supplied by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    /// Initial tool result supplied by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<Value>,
}

/// Immutable snapshot of app state supplied by the host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct McpAppSnapshot {
    /// Host capabilities from app initialization.
    pub host_capabilities: Option<HostCapabilities>,
    /// Current host context object.
    pub host_context: Value,
    /// Current tool input supplied by the host.
    pub tool_input: Option<Value>,
    /// Current tool result supplied by the host.
    pub tool_result: Option<Value>,
}

/// Listener called after host context changes.
pub type HostContextListener = Rc<dyn Fn(&Value)>;

#[derive(Default)]
struct LifecycleState {
    host_capabilities: Option<HostCapabilities>,
    host_context: Value,
    tool_input: Option<Value>,
    tool_result: Option<Value>,
    next_context_listener: u64,
    host_context_listeners: Vec<(u64, HostContextListener)>,
    transport_listener_registrations: Vec<ListenerRegistration>,
}

/// Runtime wrapper for one MCP App instance.
#[derive(Clone)]
pub struct McpApp<T>
where
    T: AppTransport,
{
    transport: T,
    state: Rc<RefCell<LifecycleState>>,
}

impl<T> McpApp<T>
where
    T: AppTransport,
{
    /// Creates an app runtime over a provider-neutral transport.
    pub fn new(transport: T) -> Self {
        let app = Self {
            transport,
            state: Rc::new(RefCell::new(LifecycleState {
                host_context: json!({}),
                ..LifecycleState::default()
            })),
        };
        app.install_standard_listeners();
        app
    }

    /// Returns the underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Performs the `ui/initialize` handshake and stores host lifecycle state.
    pub async fn initialize(
        &self,
        params: InitializeParams,
        options: RequestOptions,
    ) -> AppResult<InitializeResult> {
        let result: InitializeResult = self
            .request(
                INITIALIZE_METHOD,
                serde_json::to_value(params).expect("initialize params serialize"),
                options,
            )
            .await?;
        self.connect(
            result.host_capabilities.clone(),
            result.host_context.clone(),
            result.tool_input.clone(),
            result.tool_result.clone(),
        );
        self.notify(INITIALIZED_NOTIFICATION_METHOD, json!({}))?;
        Ok(result)
    }

    /// Applies the initial host state received during app initialization.
    pub fn connect(
        &self,
        host_capabilities: HostCapabilities,
        host_context: Value,
        tool_input: Option<Value>,
        tool_result: Option<Value>,
    ) {
        let listeners = {
            let mut state = self.state.borrow_mut();
            state.host_capabilities = Some(host_capabilities);
            state.host_context = host_context.clone();
            state.tool_input = tool_input;
            state.tool_result = tool_result;
            state
                .host_context_listeners
                .iter()
                .map(|(_, listener)| listener.clone())
                .collect::<Vec<_>>()
        };
        for listener in listeners {
            listener(&host_context);
        }
    }

    /// Returns a snapshot of lifecycle state.
    pub fn snapshot(&self) -> McpAppSnapshot {
        let state = self.state.borrow();
        McpAppSnapshot {
            host_capabilities: state.host_capabilities.clone(),
            host_context: state.host_context.clone(),
            tool_input: state.tool_input.clone(),
            tool_result: state.tool_result.clone(),
        }
    }

    /// Returns current host capabilities.
    pub fn host_capabilities(&self) -> Option<HostCapabilities> {
        self.state.borrow().host_capabilities.clone()
    }

    /// Returns the current host context object.
    pub fn host_context(&self) -> Value {
        self.state.borrow().host_context.clone()
    }

    /// Returns the current tool input decoded as a concrete type.
    pub fn tool_input<D>(&self) -> AppResult<Option<D>>
    where
        D: DeserializeOwned,
    {
        self.state
            .borrow()
            .tool_input
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| AppError::Decode {
                method: "tool_input".to_string(),
                message: error.to_string(),
            })
    }

    /// Returns the current tool result decoded as a concrete type.
    pub fn tool_result<D>(&self) -> AppResult<Option<D>>
    where
        D: DeserializeOwned,
    {
        self.state
            .borrow()
            .tool_result
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| AppError::Decode {
                method: "tool_result".to_string(),
                message: error.to_string(),
            })
    }

    /// Replaces host context and notifies registered listeners.
    pub fn replace_host_context(&self, host_context: Value) {
        replace_host_context_state(&self.state, host_context);
    }

    /// Merges host-context keys and notifies registered listeners.
    pub fn merge_host_context(&self, host_context_update: Value) {
        merge_host_context_state(&self.state, host_context_update);
    }

    /// Replaces current tool input.
    pub fn replace_tool_input(&self, tool_input: Value) {
        self.state.borrow_mut().tool_input = Some(tool_input);
    }

    /// Replaces current tool result.
    pub fn replace_tool_result(&self, tool_result: Value) {
        self.state.borrow_mut().tool_result = Some(tool_result);
    }

    /// Registers a host-context listener.
    pub fn add_host_context_listener(
        &self,
        listener: impl Fn(&Value) + 'static,
    ) -> ListenerRegistration {
        let listener = Rc::new(listener) as HostContextListener;
        let state = self.state.clone();
        let id = {
            let mut state = state.borrow_mut();
            state.next_context_listener += 1;
            let id = state.next_context_listener;
            state.host_context_listeners.push((id, listener));
            id
        };
        ListenerRegistration::new(move || {
            state
                .borrow_mut()
                .host_context_listeners
                .retain(|(candidate, _)| *candidate != id);
        })
    }

    /// Sends a raw request over the app transport.
    pub async fn request<R>(
        &self,
        method: &str,
        params: Value,
        options: RequestOptions,
    ) -> AppResult<R>
    where
        R: DeserializeOwned,
    {
        let value = self.transport.request(method, params, options).await?;
        serde_json::from_value(value).map_err(|error| AppError::Decode {
            method: method.to_string(),
            message: error.to_string(),
        })
    }

    /// Sends a raw notification over the app transport.
    pub fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        self.transport.notify(method, params)
    }

    /// Sends a host message request.
    pub async fn send_message<R>(&self, params: Value, options: RequestOptions) -> AppResult<R>
    where
        R: DeserializeOwned,
    {
        self.request(SEND_MESSAGE_METHOD, params, options).await
    }

    /// Updates model context through the host.
    pub async fn update_model_context<R>(
        &self,
        params: Value,
        options: RequestOptions,
    ) -> AppResult<R>
    where
        R: DeserializeOwned,
    {
        self.request(MODEL_CONTEXT_UPDATE_METHOD, params, options)
            .await
    }

    /// Reads a server resource through the host.
    pub async fn read_server_resource<R>(
        &self,
        params: Value,
        options: RequestOptions,
    ) -> AppResult<R>
    where
        R: DeserializeOwned,
    {
        self.request(RESOURCE_READ_METHOD, params, options).await
    }

    /// Requests a display-mode change through the host.
    pub async fn request_display_mode<R>(
        &self,
        params: Value,
        options: RequestOptions,
    ) -> AppResult<R>
    where
        R: DeserializeOwned,
    {
        self.request(REQUEST_DISPLAY_MODE_METHOD, params, options)
            .await
    }

    /// Disposes the app transport and standard lifecycle listeners.
    pub fn dispose(&self) {
        self.transport.dispose();
        self.state
            .borrow_mut()
            .transport_listener_registrations
            .clear();
    }

    fn install_standard_listeners(&self) {
        let state = self.state.clone();
        let host_context = self.transport.on(
            HOST_CONTEXT_CHANGED_METHOD,
            Rc::new(move |value| merge_host_context_state(&state, value)),
        );

        let state = self.state.clone();
        let tool_input = self.transport.on(
            TOOL_INPUT_NOTIFICATION_METHOD,
            Rc::new(move |value| state.borrow_mut().tool_input = Some(value)),
        );

        let state = self.state.clone();
        let tool_result = self.transport.on(
            TOOL_RESULT_NOTIFICATION_METHOD,
            Rc::new(move |value| state.borrow_mut().tool_result = Some(value)),
        );

        self.state.borrow_mut().transport_listener_registrations =
            vec![host_context, tool_input, tool_result];
    }
}

fn replace_host_context_state(state: &Rc<RefCell<LifecycleState>>, host_context: Value) {
    let listeners = {
        let mut state = state.borrow_mut();
        state.host_context = host_context.clone();
        state
            .host_context_listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect::<Vec<_>>()
    };
    for listener in listeners {
        listener(&host_context);
    }
}

fn merge_host_context_state(state: &Rc<RefCell<LifecycleState>>, host_context_update: Value) {
    let (host_context, listeners) = {
        let mut state = state.borrow_mut();
        let host_context = match (&mut state.host_context, host_context_update) {
            (Value::Object(current), Value::Object(update)) => {
                for (key, value) in update {
                    current.insert(key, value);
                }
                Value::Object(current.clone())
            }
            (current, replacement) => {
                *current = replacement;
                current.clone()
            }
        };
        let listeners = state
            .host_context_listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect::<Vec<_>>();
        (host_context, listeners)
    };
    for listener in listeners {
        listener(&host_context);
    }
}

fn empty_object() -> Value {
    json!({})
}
