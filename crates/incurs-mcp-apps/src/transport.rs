//! Transport traits and test transports for MCP Apps.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    rc::Rc,
    time::Duration,
};

use futures::{
    channel::oneshot,
    future::{Either, select},
};
use serde_json::{Value, json};

use crate::{AppError, AppResult, RequestCancellation, RequestOptions};

/// Future returned by an MCP App transport.
pub type AppTransportFuture<T> = Pin<Box<dyn Future<Output = AppResult<T>> + 'static>>;

/// Request handler installed by a host or app transport.
pub type RequestHandler = Rc<dyn Fn(Value) -> AppTransportFuture<Value>>;

/// Notification listener installed for fire-and-forget events.
pub type Listener = Rc<dyn Fn(Value)>;

/// Provider-neutral transport for an MCP App runtime.
pub trait AppTransport: Clone + 'static {
    /// Sends a request and resolves with the host result.
    fn request(
        &self,
        method: &str,
        params: Value,
        options: RequestOptions,
    ) -> AppTransportFuture<Value>;

    /// Sends a notification.
    fn notify(&self, method: &str, params: Value) -> AppResult<()>;

    /// Registers an inbound request handler.
    fn handle(&self, method: &str, handler: RequestHandler);

    /// Registers a notification listener and returns a removal guard.
    fn on(&self, method: &str, listener: Listener) -> ListenerRegistration;

    /// Disposes the transport and rejects later work.
    fn dispose(&self);
}

/// Removes a listener when dropped or explicitly disposed.
pub struct ListenerRegistration {
    remove: Option<Box<dyn FnOnce()>>,
}

impl ListenerRegistration {
    /// Creates a listener registration from a removal callback.
    pub fn new(remove: impl FnOnce() + 'static) -> Self {
        Self {
            remove: Some(Box::new(remove)),
        }
    }

    /// Removes the listener immediately.
    pub fn dispose(mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
}

impl Drop for ListenerRegistration {
    fn drop(&mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
}

#[derive(Default)]
struct InMemoryState {
    disposed: bool,
    next_listener: u64,
    handlers: BTreeMap<String, RequestHandler>,
    listeners: BTreeMap<String, BTreeMap<u64, Listener>>,
    requests: Vec<RecordedRequest>,
    notifications: Vec<RecordedNotification>,
}

/// Request recorded by [`InMemoryTransport`].
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedRequest {
    /// Monotonic request identifier assigned by the transport.
    pub id: u64,
    /// JSON-RPC method name.
    pub method: String,
    /// JSON-RPC params value.
    pub params: Value,
    /// Requested timeout, when supplied.
    pub timeout: Option<Duration>,
}

/// Notification recorded by [`InMemoryTransport`].
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedNotification {
    /// JSON-RPC method name.
    pub method: String,
    /// JSON-RPC params value.
    pub params: Value,
}

/// Deterministic in-process transport for tests and native examples.
#[derive(Clone, Default)]
pub struct InMemoryTransport {
    next_request: Rc<Cell<u64>>,
    state: Rc<RefCell<InMemoryState>>,
}

impl InMemoryTransport {
    /// Creates an empty in-memory transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the requests observed so far.
    pub fn recorded_requests(&self) -> Vec<RecordedRequest> {
        self.state.borrow().requests.clone()
    }

    /// Returns the notifications observed so far.
    pub fn recorded_notifications(&self) -> Vec<RecordedNotification> {
        self.state.borrow().notifications.clone()
    }

    /// Emits a notification to listeners as if it came from the host.
    pub fn emit(&self, method: &str, params: Value) {
        let listeners = self
            .state
            .borrow()
            .listeners
            .get(method)
            .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener(params.clone());
        }
    }
}

impl AppTransport for InMemoryTransport {
    fn request(
        &self,
        method: &str,
        params: Value,
        options: RequestOptions,
    ) -> AppTransportFuture<Value> {
        let method = method.to_string();
        let state = self.state.clone();
        let id = self.next_request.get() + 1;
        self.next_request.set(id);
        Box::pin(async move {
            if options.is_cancelled() {
                return Err(AppError::Cancelled {
                    method: method.clone(),
                });
            }
            let handler = {
                let mut state = state.borrow_mut();
                if state.disposed {
                    return Err(AppError::Disposed);
                }
                state.requests.push(RecordedRequest {
                    id,
                    method: method.clone(),
                    params: params.clone(),
                    timeout: options.timeout(),
                });
                state.handlers.get(&method).cloned()
            };
            let Some(handler) = handler else {
                return Err(AppError::Rpc {
                    code: -32601,
                    message: format!("Unsupported app method: {method}"),
                });
            };
            run_with_cancellation(handler(params), options.cancellation.clone(), method).await
        })
    }

    fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        let mut state = self.state.borrow_mut();
        if state.disposed {
            return Err(AppError::Disposed);
        }
        state.notifications.push(RecordedNotification {
            method: method.to_string(),
            params,
        });
        Ok(())
    }

    fn handle(&self, method: &str, handler: RequestHandler) {
        self.state
            .borrow_mut()
            .handlers
            .insert(method.to_string(), handler);
    }

    fn on(&self, method: &str, listener: Listener) -> ListenerRegistration {
        let method = method.to_string();
        let state = self.state.clone();
        let id = {
            let mut state = state.borrow_mut();
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
            state
                .borrow_mut()
                .listeners
                .get_mut(&method)
                .map(|listeners| listeners.remove(&id));
        })
    }

    fn dispose(&self) {
        let mut state = self.state.borrow_mut();
        state.disposed = true;
        state.handlers.clear();
        state.listeners.clear();
    }
}

/// Creates a request handler from a synchronous value-producing function.
pub fn value_handler(function: impl Fn(Value) -> AppResult<Value> + 'static) -> RequestHandler {
    Rc::new(move |params| {
        let result = function(params);
        Box::pin(async move { result })
    })
}

/// Creates an empty JSON-RPC result value.
pub fn empty_result() -> Value {
    json!({})
}

async fn run_with_cancellation(
    future: AppTransportFuture<Value>,
    cancellation: Option<RequestCancellation>,
    method: String,
) -> AppResult<Value> {
    let Some(cancellation) = cancellation else {
        return future.await;
    };
    if cancellation.is_cancelled() {
        return Err(AppError::Cancelled { method });
    }
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(RefCell::new(Some(sender)));
    let registration = cancellation.on_cancelled({
        let sender = sender.clone();
        move || {
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(());
            }
        }
    });
    match select(future, receiver).await {
        Either::Left((result, _)) => {
            drop(registration);
            result
        }
        Either::Right((_result, _)) => Err(AppError::Cancelled { method }),
    }
}
