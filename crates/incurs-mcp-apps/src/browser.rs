//! Browser `postMessage` transport for WASM MCP Apps.
//!
//! The module is available only for `wasm32` targets. JavaScript glue is limited
//! to loading the generated WASM module; request correlation and event routing
//! live here.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Duration};

use futures::{FutureExt, channel::oneshot};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{MessageEvent, Window};

use crate::{
    AppError, AppResult, AppTransport, AppTransportFuture, Listener, ListenerRegistration,
    RequestHandler, RequestOptions,
};

/// Browser `window.parent.postMessage` transport.
#[derive(Clone)]
pub struct BrowserPostMessageTransport {
    inner: Rc<RefCell<BrowserState>>,
}

struct BrowserState {
    disposed: bool,
    next_id: u64,
    window: Window,
    parent: Window,
    expected_source: Window,
    pending: BTreeMap<String, PendingRequest>,
    handlers: BTreeMap<String, RequestHandler>,
    listeners: BTreeMap<String, BTreeMap<u64, Listener>>,
    next_listener: u64,
    closure: Option<Closure<dyn FnMut(MessageEvent)>>,
}

struct PendingRequest {
    sender: oneshot::Sender<AppResult<Value>>,
    timeout_handle: Option<i32>,
    timeout_closure: Option<Closure<dyn FnMut()>>,
    cancellation_registration: Option<ListenerRegistration>,
}

impl BrowserPostMessageTransport {
    /// Creates a browser transport bound to `window.parent`.
    pub fn new() -> AppResult<Self> {
        let window =
            web_sys::window().ok_or_else(|| AppError::Transport("missing window".into()))?;
        let parent = window
            .parent()
            .map_err(|_| AppError::Transport("missing parent window".into()))?
            .ok_or_else(|| AppError::Transport("missing parent window".into()))?;
        Self::new_for_window(window, parent.clone(), parent)
    }

    /// Creates a transport with explicit windows for browser tests or custom embeddings.
    pub fn new_for_window(
        window: Window,
        parent: Window,
        expected_source: Window,
    ) -> AppResult<Self> {
        let transport = Self {
            inner: Rc::new(RefCell::new(BrowserState {
                disposed: false,
                next_id: 0,
                window,
                parent,
                expected_source,
                pending: BTreeMap::new(),
                handlers: BTreeMap::new(),
                listeners: BTreeMap::new(),
                next_listener: 0,
                closure: None,
            })),
        };
        transport.install_listener()?;
        Ok(transport)
    }

    /// Returns the number of outbound requests waiting for a host response.
    pub fn pending_request_count(&self) -> usize {
        self.inner.borrow().pending.len()
    }

    fn install_listener(&self) -> AppResult<()> {
        let inner = self.inner.clone();
        let closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            handle_message(inner.clone(), event);
        }) as Box<dyn FnMut(MessageEvent)>);
        self.inner
            .borrow()
            .window
            .add_event_listener_with_callback("message", closure.as_ref().unchecked_ref())
            .map_err(|error| AppError::Transport(format!("message listener failed: {error:?}")))?;
        self.inner.borrow_mut().closure = Some(closure);
        Ok(())
    }
}

impl AppTransport for BrowserPostMessageTransport {
    fn request(
        &self,
        method: &str,
        params: Value,
        options: RequestOptions,
    ) -> AppTransportFuture<Value> {
        let method = method.to_string();
        let inner = self.inner.clone();
        async move {
            if options.is_cancelled() {
                return Err(AppError::Cancelled {
                    method: method.clone(),
                });
            }
            let (id, receiver) = {
                let mut state = inner.borrow_mut();
                if state.disposed {
                    return Err(AppError::Disposed);
                }
                state.next_id += 1;
                let id = state.next_id.to_string();
                let (sender, receiver) = oneshot::channel();
                let (timeout_handle, timeout_closure) = install_timeout(
                    inner.clone(),
                    id.clone(),
                    method.clone(),
                    options.timeout(),
                    &state.window,
                )?;
                let cancellation_registration = install_cancellation(
                    inner.clone(),
                    id.clone(),
                    method.clone(),
                    options.cancellation.clone(),
                );
                state.pending.insert(
                    id.clone(),
                    PendingRequest {
                        sender,
                        timeout_handle,
                        timeout_closure,
                        cancellation_registration,
                    },
                );
                (id, receiver)
            };

            let message = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            });
            if let Err(error) = post_value(&inner, &message) {
                complete_pending(&inner, &id, Err(error.clone()));
                return Err(error);
            }
            receiver.await.unwrap_or(Err(AppError::Disposed))
        }
        .boxed_local()
    }

    fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        if self.inner.borrow().disposed {
            return Err(AppError::Disposed);
        }
        post_value(
            &self.inner,
            &json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
        )
    }

    fn handle(&self, method: &str, handler: RequestHandler) {
        self.inner
            .borrow_mut()
            .handlers
            .insert(method.to_string(), handler);
    }

    fn on(&self, method: &str, listener: Listener) -> ListenerRegistration {
        let method = method.to_string();
        let inner = self.inner.clone();
        let id = {
            let mut state = inner.borrow_mut();
            state.next_listener += 1;
            let id = state.next_listener;
            state
                .listeners
                .entry(method.clone())
                .or_default()
                .insert(id, listener);
            id
        };
        ListenerRegistration::new(move || {
            inner
                .borrow_mut()
                .listeners
                .get_mut(&method)
                .map(|listeners| listeners.remove(&id));
        })
    }

    fn dispose(&self) {
        let pending = {
            let mut state = self.inner.borrow_mut();
            if state.disposed {
                return;
            }
            state.disposed = true;
            state.handlers.clear();
            state.listeners.clear();
            if let Some(closure) = state.closure.take() {
                let _ = state.window.remove_event_listener_with_callback(
                    "message",
                    closure.as_ref().unchecked_ref(),
                );
            }
            std::mem::take(&mut state.pending)
        };
        for (id, request) in pending {
            finish_pending(&self.inner, id, request, Err(AppError::Disposed));
        }
    }
}

fn handle_message(inner: Rc<RefCell<BrowserState>>, event: MessageEvent) {
    if inner.borrow().disposed {
        return;
    }
    let Some(source) = event.source() else {
        return;
    };
    let source: JsValue = source.into();
    let expected: JsValue = inner.borrow().expected_source.clone().into();
    if !source.eq(&expected) {
        return;
    }
    let Ok(message) = serde_wasm_bindgen::from_value::<Value>(event.data()) else {
        return;
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return;
    }
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message
            .get("params")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        if let Some(id) = message.get("id").cloned() {
            handle_request(inner, id, method.to_string(), params);
        } else {
            emit_notification(&inner, method, params);
        }
        return;
    }
    if let Some(id) = message.get("id").and_then(json_rpc_id_to_string) {
        let result = if let Some(error) = message.get("error") {
            Err(AppError::Rpc {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("JSON-RPC error")
                    .to_string(),
            })
        } else {
            Ok(message.get("result").cloned().unwrap_or(Value::Null))
        };
        complete_pending(&inner, &id, result);
    }
}

fn handle_request(inner: Rc<RefCell<BrowserState>>, id: Value, method: String, params: Value) {
    let handler = inner.borrow().handlers.get(&method).cloned();
    let Some(handler) = handler else {
        let _ = post_value(
            &inner,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Unsupported app method: {method}") },
            }),
        );
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let response = match handler(params).await {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": error.to_string() },
            }),
        };
        let _ = post_value(&inner, &response);
    });
}

fn emit_notification(inner: &Rc<RefCell<BrowserState>>, method: &str, params: Value) {
    let listeners = inner
        .borrow()
        .listeners
        .get(method)
        .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for listener in listeners {
        listener(params.clone());
    }
}

type InstalledTimeout = (Option<i32>, Option<Closure<dyn FnMut()>>);

fn install_timeout(
    inner: Rc<RefCell<BrowserState>>,
    id: String,
    method: String,
    timeout: Option<Duration>,
    window: &Window,
) -> AppResult<InstalledTimeout> {
    let Some(timeout) = timeout else {
        return Ok((None, None));
    };
    let milliseconds = timeout.as_millis().min(i32::MAX as u128) as i32;
    let timeout_inner = inner.clone();
    let timeout_id = id.clone();
    let closure = Closure::wrap(Box::new(move || {
        let pending = timeout_inner.borrow_mut().pending.remove(&timeout_id);
        if let Some(request) = pending {
            finish_pending(
                &timeout_inner,
                timeout_id.clone(),
                request,
                Err(AppError::Timeout {
                    method: method.clone(),
                }),
            );
        }
    }) as Box<dyn FnMut()>);
    let handle = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            closure.as_ref().unchecked_ref(),
            milliseconds,
        )
        .map_err(|error| AppError::Transport(format!("setTimeout failed: {error:?}")))?;
    Ok((Some(handle), Some(closure)))
}

fn install_cancellation(
    inner: Rc<RefCell<BrowserState>>,
    id: String,
    method: String,
    cancellation: Option<crate::RequestCancellation>,
) -> Option<ListenerRegistration> {
    cancellation.map(|cancellation| {
        let cancellation_inner = inner.clone();
        let cancellation_id = id.clone();
        cancellation.on_cancelled(move || {
            let pending = cancellation_inner
                .borrow_mut()
                .pending
                .remove(&cancellation_id);
            if let Some(request) = pending {
                finish_pending(
                    &cancellation_inner,
                    cancellation_id.clone(),
                    request,
                    Err(AppError::Cancelled {
                        method: method.clone(),
                    }),
                );
            }
        })
    })
}

fn complete_pending(inner: &Rc<RefCell<BrowserState>>, id: &str, result: AppResult<Value>) {
    let pending = inner.borrow_mut().pending.remove(id);
    if let Some(request) = pending {
        finish_pending(inner, id.to_string(), request, result);
    }
}

fn finish_pending(
    inner: &Rc<RefCell<BrowserState>>,
    _id: String,
    mut request: PendingRequest,
    result: AppResult<Value>,
) {
    if let Some(handle) = request.timeout_handle.take() {
        inner.borrow().window.clear_timeout_with_handle(handle);
    }
    request.timeout_closure.take();
    request.cancellation_registration.take();
    let _ = request.sender.send(result);
}

fn post_value(inner: &Rc<RefCell<BrowserState>>, value: &Value) -> AppResult<()> {
    let message =
        serde::Serialize::serialize(value, &serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(|error| AppError::Transport(error.to_string()))?;
    inner
        .borrow()
        .parent
        .post_message(&message, "*")
        .map_err(|error| AppError::Transport(format!("postMessage failed: {error:?}")))
}

fn json_rpc_id_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
