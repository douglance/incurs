//! Provider-neutral live resource subscription primitives.
//!
//! Hosts own authentication, tenant scope, durable storage, and source-specific
//! projection. This module defines the shared event envelope, opaque delivery
//! cursor, replay contract, and bounded in-process fanout used by transports.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use futures::channel::mpsc;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current subscription event schema version.
pub const SUBSCRIPTION_SCHEMA_VERSION: u32 = 1;

/// Default maximum queued events for one subscriber.
pub const DEFAULT_SUBSCRIBER_EVENT_LIMIT: usize = 256;

/// Default maximum queued payload bytes for one subscriber.
pub const DEFAULT_SUBSCRIBER_BYTE_LIMIT: usize = 1024 * 1024;

/// A committed resource change before delivery metadata is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEnvelope {
    /// Event schema version. Version 1 is the only version accepted here.
    pub schema_version: u32,
    /// Stable durable receipt identity used for idempotent ingestion.
    pub source_id: String,
    /// Canonical resource URIs affected by the change.
    pub resource_uris: Vec<String>,
    /// Authoritative opaque business revision from the source system.
    pub revision: String,
    /// Kind of resource change.
    pub kind: ChangeKind,
    /// RFC3339 timestamp supplied by the trusted source host.
    pub occurred_at: String,
}

impl ChangeEnvelope {
    /// Validates the envelope fields that the shared core can check without
    /// knowing the source system.
    pub fn validate(&self) -> Result<(), SubscriptionError> {
        if self.schema_version != SUBSCRIPTION_SCHEMA_VERSION {
            return Err(SubscriptionError::InvalidEnvelope(
                "schema_version must be 1".to_string(),
            ));
        }
        if self.source_id.is_empty() {
            return Err(SubscriptionError::InvalidEnvelope(
                "source_id must not be empty".to_string(),
            ));
        }
        if self.resource_uris.is_empty() || self.resource_uris.iter().any(|uri| uri.is_empty()) {
            return Err(SubscriptionError::InvalidEnvelope(
                "resource_uris must contain at least one nonempty URI".to_string(),
            ));
        }
        if self.revision.is_empty() {
            return Err(SubscriptionError::InvalidEnvelope(
                "revision must not be empty".to_string(),
            ));
        }
        if self.occurred_at.is_empty() {
            return Err(SubscriptionError::InvalidEnvelope(
                "occurred_at must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Resource change kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// The resource was created or updated.
    Updated,
    /// The resource was deleted.
    Deleted,
}

/// Opaque delivery cursor assigned by a subscription store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeliveryCursor(String);

impl DeliveryCursor {
    /// Creates an opaque cursor from a store-owned string.
    pub fn new(value: impl Into<String>) -> Result<Self, SubscriptionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SubscriptionError::InvalidCursor(
                "cursor must not be empty".to_string(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the cursor string without assigning meaning to it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the cursor and returns its opaque string.
    pub fn into_string(self) -> String {
        self.0
    }
}

/// A change after the store assigns delivery order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredChange {
    /// Opaque cursor for replaying after this delivery.
    pub cursor: DeliveryCursor,
    /// Source event envelope.
    pub envelope: ChangeEnvelope,
}

/// Reset marker emitted when replay cannot bridge a retention gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionReset {
    /// Human-readable reset reason.
    pub reason: String,
    /// Resource filters the subscriber should refresh authoritatively.
    pub resource_uris: Vec<String>,
}

/// Event sent to a subscriber stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubscriptionEvent {
    /// One delivered resource change.
    Change {
        /// Opaque delivery cursor.
        cursor: DeliveryCursor,
        /// Source event envelope.
        envelope: ChangeEnvelope,
    },
    /// Replay could not cover the requested cursor, so the client must read
    /// current authoritative state before continuing.
    Reset {
        /// Reset details.
        reset: SubscriptionReset,
    },
}

impl From<DeliveredChange> for SubscriptionEvent {
    fn from(change: DeliveredChange) -> Self {
        Self::Change {
            cursor: change.cursor,
            envelope: change.envelope,
        }
    }
}

/// Replay result returned by a subscription store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionReplay {
    /// Optional reset when the requested cursor is outside retained history.
    pub reset: Option<SubscriptionReset>,
    /// Retained changes after the requested cursor.
    pub events: Vec<DeliveredChange>,
}

/// Per-subscriber buffer limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionLimits {
    /// Maximum number of queued events.
    pub max_events: usize,
    /// Maximum serialized bytes for one queued event.
    pub max_bytes: usize,
}

impl Default for SubscriptionLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_SUBSCRIBER_EVENT_LIMIT,
            max_bytes: DEFAULT_SUBSCRIBER_BYTE_LIMIT,
        }
    }
}

/// Request for a replaying live subscription.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionRequest {
    /// Resource URI filters. Empty means all resources permitted by the trusted scope.
    pub resource_uris: Vec<String>,
    /// Last delivered cursor observed by the client.
    pub after: Option<DeliveryCursor>,
    /// Subscriber queue limits.
    pub limits: SubscriptionLimits,
}

impl SubscriptionRequest {
    /// Returns whether the request's resource filter includes the change.
    pub fn matches(&self, envelope: &ChangeEnvelope) -> bool {
        self.resource_uris.is_empty()
            || envelope
                .resource_uris
                .iter()
                .any(|uri| self.resource_uris.iter().any(|filter| filter == uri))
    }
}

/// Trusted host authorization scope for one subscriber.
pub trait SubscriptionScope: Send + Sync {
    /// Returns whether the subscriber may observe this resource URI.
    fn permits_resource(&self, resource_uri: &str) -> bool;

    /// Returns whether the subscriber may observe the complete envelope.
    fn permits_envelope(&self, envelope: &ChangeEnvelope) -> bool {
        envelope
            .resource_uris
            .iter()
            .all(|uri| self.permits_resource(uri))
    }
}

impl SubscriptionScope for () {
    fn permits_resource(&self, _resource_uri: &str) -> bool {
        true
    }
}

/// Durable storage backend supplied by the host.
pub trait SubscriptionStore: Send + Sync {
    /// Appends or deduplicates one validated envelope and returns its delivery cursor.
    fn append<'a>(
        &'a self,
        envelope: ChangeEnvelope,
    ) -> BoxFuture<'a, Result<DeliveredChange, SubscriptionError>>;

    /// Replays retained changes after the request cursor.
    fn replay<'a>(
        &'a self,
        request: &'a SubscriptionRequest,
    ) -> BoxFuture<'a, Result<SubscriptionReplay, SubscriptionError>>;
}

/// In-process subscription hub over a host-supplied store.
pub struct SubscriptionHub<S> {
    store: Arc<S>,
    inner: Arc<Mutex<HubInner>>,
    next_subscriber_id: AtomicU64,
}

impl<S: SubscriptionStore> SubscriptionHub<S> {
    /// Creates a hub around an owned store.
    pub fn new(store: S) -> Self {
        Self::from_shared(Arc::new(store))
    }

    /// Creates a hub around a shared store.
    pub fn from_shared(store: Arc<S>) -> Self {
        Self {
            store,
            inner: Arc::new(Mutex::new(HubInner::default())),
            next_subscriber_id: AtomicU64::new(1),
        }
    }

    /// Validates, stores, and fans out one source envelope.
    pub async fn ingest(
        &self,
        envelope: ChangeEnvelope,
    ) -> Result<DeliveredChange, SubscriptionError> {
        envelope.validate()?;
        let delivered = self.store.append(envelope).await?;
        self.publish(&delivered);
        Ok(delivered)
    }

    /// Creates a replaying live stream for one trusted subscriber scope.
    pub async fn subscribe<A>(
        &self,
        scope: A,
        request: SubscriptionRequest,
    ) -> Result<SubscriptionStream, SubscriptionError>
    where
        A: SubscriptionScope + 'static,
    {
        if request.limits.max_events == 0 || request.limits.max_bytes == 0 {
            return Err(SubscriptionError::InvalidRequest(
                "subscription limits must be nonzero".to_string(),
            ));
        }
        for uri in &request.resource_uris {
            if !scope.permits_resource(uri) {
                return Err(SubscriptionError::UnauthorizedResource { uri: uri.clone() });
            }
        }
        let replay = self.store.replay(&request).await?;
        let id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel(request.limits.max_events);
        let subscriber = Subscriber {
            scope: Arc::new(scope),
            filters: request.resource_uris.clone(),
            limits: request.limits,
            sender,
        };
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| SubscriptionError::StateUnavailable)?;
            inner.subscribers.insert(id, subscriber);
            if let Some(reset) = replay.reset {
                inner.send_to(id, SubscriptionEvent::Reset { reset });
            }
            for event in replay.events {
                inner.send_to(id, event.into());
            }
        }
        Ok(SubscriptionStream {
            id,
            hub: Arc::downgrade(&self.inner),
            receiver,
        })
    }

    fn publish(&self, delivered: &DeliveredChange) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.publish(delivered);
        }
    }
}

#[derive(Default)]
struct HubInner {
    subscribers: BTreeMap<u64, Subscriber>,
}

impl HubInner {
    fn publish(&mut self, delivered: &DeliveredChange) {
        let event = SubscriptionEvent::from(delivered.clone());
        let ids = self.subscribers.keys().copied().collect::<Vec<_>>();
        for id in ids {
            self.send_to(id, event.clone());
        }
    }

    fn send_to(&mut self, id: u64, event: SubscriptionEvent) {
        let Some(subscriber) = self.subscribers.get_mut(&id) else {
            return;
        };
        if !subscriber.accepts(&event) {
            return;
        }
        if !subscriber.try_send(event) {
            self.subscribers.remove(&id);
        }
    }
}

struct Subscriber {
    scope: Arc<dyn SubscriptionScope>,
    filters: Vec<String>,
    limits: SubscriptionLimits,
    sender: mpsc::Sender<SubscriptionEvent>,
}

impl Subscriber {
    fn accepts(&self, event: &SubscriptionEvent) -> bool {
        match event {
            SubscriptionEvent::Change { envelope, .. } => {
                (self.filters.is_empty()
                    || envelope
                        .resource_uris
                        .iter()
                        .any(|uri| self.filters.iter().any(|filter| filter == uri)))
                    && self.scope.permits_envelope(envelope)
            }
            SubscriptionEvent::Reset { reset } => reset
                .resource_uris
                .iter()
                .all(|uri| self.scope.permits_resource(uri)),
        }
    }

    fn try_send(&mut self, event: SubscriptionEvent) -> bool {
        let Ok(bytes) = serde_json::to_vec(&event) else {
            return false;
        };
        if bytes.len() > self.limits.max_bytes {
            return false;
        }
        self.sender.try_send(event).is_ok()
    }
}

/// A live subscription stream. Dropping it unsubscribes from hub fanout.
pub struct SubscriptionStream {
    id: u64,
    hub: Weak<Mutex<HubInner>>,
    receiver: mpsc::Receiver<SubscriptionEvent>,
}

impl Stream for SubscriptionStream {
    type Item = SubscriptionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

impl Drop for SubscriptionStream {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade()
            && let Ok(mut inner) = hub.lock()
        {
            inner.subscribers.remove(&self.id);
        }
    }
}

/// In-memory store intended for local fixtures and tests.
pub struct InMemorySubscriptionStore {
    inner: Mutex<MemoryInner>,
    max_retained: usize,
}

impl InMemorySubscriptionStore {
    /// Creates a store retaining at most `max_retained` delivered changes.
    pub fn new(max_retained: usize) -> Self {
        Self {
            inner: Mutex::new(MemoryInner::default()),
            max_retained,
        }
    }
}

impl Default for InMemorySubscriptionStore {
    fn default() -> Self {
        Self::new(100_000)
    }
}

impl SubscriptionStore for InMemorySubscriptionStore {
    fn append<'a>(
        &'a self,
        envelope: ChangeEnvelope,
    ) -> BoxFuture<'a, Result<DeliveredChange, SubscriptionError>> {
        async move {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| SubscriptionError::StateUnavailable)?;
            if let Some(existing) = inner.by_source_id.get(&envelope.source_id) {
                return Ok(existing.clone());
            }
            inner.next_cursor += 1;
            let delivered = DeliveredChange {
                cursor: DeliveryCursor::new(format!("mem:{}", inner.next_cursor))?,
                envelope,
            };
            inner
                .by_source_id
                .insert(delivered.envelope.source_id.clone(), delivered.clone());
            inner.events.push_back(delivered.clone());
            while inner.events.len() > self.max_retained {
                if let Some(removed) = inner.events.pop_front() {
                    inner.by_source_id.remove(&removed.envelope.source_id);
                }
            }
            Ok(delivered)
        }
        .boxed()
    }

    fn replay<'a>(
        &'a self,
        request: &'a SubscriptionRequest,
    ) -> BoxFuture<'a, Result<SubscriptionReplay, SubscriptionError>> {
        async move {
            let inner = self
                .inner
                .lock()
                .map_err(|_| SubscriptionError::StateUnavailable)?;
            let first = inner.events.front().and_then(|event| parse_mem_cursor(&event.cursor));
            let after = match &request.after {
                Some(cursor) => match parse_mem_cursor(cursor) {
                    Some(value) => Some(value),
                    None => {
                        return Ok(reset_replay(request));
                    }
                },
                None => None,
            };
            if let (Some(first), Some(after)) = (first, after)
                && after + 1 < first
            {
                return Ok(reset_replay(request));
            }
            let events = inner
                .events
                .iter()
                .filter(|event| after.is_none_or(|after| {
                    parse_mem_cursor(&event.cursor).is_some_and(|cursor| cursor > after)
                }))
                .filter(|event| request.matches(&event.envelope))
                .cloned()
                .collect();
            Ok(SubscriptionReplay {
                reset: None,
                events,
            })
        }
        .boxed()
    }
}

#[derive(Default)]
struct MemoryInner {
    next_cursor: u64,
    events: VecDeque<DeliveredChange>,
    by_source_id: BTreeMap<String, DeliveredChange>,
}

fn reset_replay(request: &SubscriptionRequest) -> SubscriptionReplay {
    SubscriptionReplay {
        reset: Some(SubscriptionReset {
            reason: "requested cursor is outside retained subscription history".to_string(),
            resource_uris: request.resource_uris.clone(),
        }),
        events: Vec::new(),
    }
}

fn parse_mem_cursor(cursor: &DeliveryCursor) -> Option<u64> {
    cursor.as_str().strip_prefix("mem:")?.parse().ok()
}

/// Subscription operation failure.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubscriptionError {
    /// Envelope fields are malformed.
    #[error("invalid subscription envelope: {0}")]
    InvalidEnvelope(String),
    /// Cursor text is malformed.
    #[error("invalid subscription cursor: {0}")]
    InvalidCursor(String),
    /// Request fields are malformed.
    #[error("invalid subscription request: {0}")]
    InvalidRequest(String),
    /// Trusted scope does not allow one requested resource.
    #[error("subscription scope does not permit resource {uri}")]
    UnauthorizedResource {
        /// Resource URI rejected by the trusted scope.
        uri: String,
    },
    /// Shared state could not be locked.
    #[error("subscription state is unavailable")]
    StateUnavailable,
    /// Store-specific failure.
    #[error("subscription store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{FutureExt, StreamExt};

    fn envelope(source_id: &str, uri: &str) -> ChangeEnvelope {
        ChangeEnvelope {
            schema_version: SUBSCRIPTION_SCHEMA_VERSION,
            source_id: source_id.to_string(),
            resource_uris: vec![uri.to_string()],
            revision: format!("rev-{source_id}"),
            kind: ChangeKind::Updated,
            occurred_at: "2026-10-09T00:00:00Z".to_string(),
        }
    }

    struct PrefixScope(&'static str);

    impl SubscriptionScope for PrefixScope {
        fn permits_resource(&self, resource_uri: &str) -> bool {
            resource_uri.starts_with(self.0)
        }
    }

    #[test]
    fn validates_version_and_required_fields() {
        let mut change = envelope("s1", "memory://one");
        change.schema_version = 2;
        assert!(matches!(
            change.validate(),
            Err(SubscriptionError::InvalidEnvelope(_))
        ));
    }

    #[test]
    fn deduplicates_by_source_id_and_replays_after_cursor() {
        futures::executor::block_on(async {
            let hub = SubscriptionHub::new(InMemorySubscriptionStore::default());
            let first = hub.ingest(envelope("s1", "memory://one")).await.unwrap();
            let duplicate = hub.ingest(envelope("s1", "memory://one")).await.unwrap();
            assert_eq!(duplicate.cursor, first.cursor);
            let second = hub.ingest(envelope("s2", "memory://one")).await.unwrap();

            let mut stream = hub
                .subscribe(
                    (),
                    SubscriptionRequest {
                        resource_uris: vec!["memory://one".to_string()],
                        after: Some(first.cursor),
                        limits: SubscriptionLimits::default(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(
                stream.next().await,
                Some(SubscriptionEvent::from(second.clone()))
            );
            drop(stream);
        });
    }

    #[test]
    fn authorizes_each_event_before_delivery() {
        futures::executor::block_on(async {
            let hub = SubscriptionHub::new(InMemorySubscriptionStore::default());
            let mut stream = hub
                .subscribe(PrefixScope("memory://allowed"), SubscriptionRequest::default())
                .await
                .unwrap();
            let delivered = hub
                .ingest(envelope("s1", "memory://allowed/one"))
                .await
                .unwrap();
            hub.ingest(envelope("s2", "memory://denied/two"))
                .await
                .unwrap();
            assert_eq!(stream.next().await, Some(SubscriptionEvent::from(delivered)));
            assert!(stream.next().now_or_never().is_none());
        });
    }
}
