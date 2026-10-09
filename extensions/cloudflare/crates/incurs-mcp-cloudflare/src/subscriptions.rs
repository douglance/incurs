use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use incurs::subscriptions as core_subscriptions;
use serde_json::json;

pub const DEFAULT_RETENTION_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_MAX_RETAINED_EVENTS: usize = 100_000;
pub const DEFAULT_MAX_SUBSCRIBER_EVENTS: usize = 256;
pub const DEFAULT_MAX_SUBSCRIBER_BYTES: usize = 1024 * 1024;
pub const DEFAULT_HEARTBEAT_MS: u64 = 15_000;
const SOURCE_PAGE_LIMIT: usize = 500;
type CoreStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, core_subscriptions::SubscriptionError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionScope {
    pub tenant_id: String,
    pub repository_id: String,
}

impl SubscriptionScope {
    pub fn new(tenant_id: impl Into<String>, repository_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            repository_id: repository_id.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceEventKind {
    Updated,
    Deleted,
}

impl ResourceEventKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEventInput {
    pub schema_version: u16,
    pub source_id: String,
    pub resource_uris: Vec<String>,
    pub revision: String,
    pub kind: ResourceEventKind,
    pub occurred_at: String,
    pub occurred_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendReceipt {
    pub cursor: u64,
    pub source_id: String,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdPageRequest {
    pub after: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdPage {
    pub source_ids: Vec<String>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryFrame {
    Change {
        cursor: u64,
        event: ResourceEventInput,
    },
    Reset {
        cursor: u64,
    },
    Heartbeat {
        cursor: u64,
    },
}

impl DeliveryFrame {
    pub fn to_sse(&self) -> String {
        let (event, data) = match self {
            Self::Change { cursor, event } => (
                "change",
                json!({
                    "cursor": format_cursor(*cursor),
                    "event": event_json(event),
                }),
            ),
            Self::Reset { cursor } => ("reset", json!({ "cursor": format_cursor(*cursor) })),
            Self::Heartbeat { cursor } => {
                ("heartbeat", json!({ "cursor": format_cursor(*cursor) }))
            }
        };
        format!("event: {event}\ndata: {data}\n\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionError {
    UnsupportedSchemaVersion(u16),
    EmptySourceId,
    EmptyResourceUris,
    EmptyResourceUri,
    EmptyRevision,
    EmptyOccurredAt,
    ConflictingSourcePayload,
}

impl std::fmt::Display for SubscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "unsupported resource event schema version {version}")
            }
            Self::EmptySourceId => f.write_str("resource event source_id is required"),
            Self::EmptyResourceUris => f.write_str("resource event resource_uris must be nonempty"),
            Self::EmptyResourceUri => {
                f.write_str("resource event resource_uris cannot contain an empty URI")
            }
            Self::EmptyRevision => f.write_str("resource event revision is required"),
            Self::EmptyOccurredAt => f.write_str("resource event occurred_at is required"),
            Self::ConflictingSourcePayload => {
                f.write_str("source_id already exists with different payload")
            }
        }
    }
}

impl std::error::Error for SubscriptionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionStoreConfig {
    pub retention_window_ms: u64,
    pub max_retained_events: usize,
    pub max_subscriber_events: usize,
    pub max_subscriber_bytes: usize,
    pub heartbeat_ms: u64,
}

impl Default for SubscriptionStoreConfig {
    fn default() -> Self {
        Self {
            retention_window_ms: DEFAULT_RETENTION_WINDOW_MS,
            max_retained_events: DEFAULT_MAX_RETAINED_EVENTS,
            max_subscriber_events: DEFAULT_MAX_SUBSCRIBER_EVENTS,
            max_subscriber_bytes: DEFAULT_MAX_SUBSCRIBER_BYTES,
            heartbeat_ms: DEFAULT_HEARTBEAT_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriberId(u64);

#[derive(Debug, Clone)]
pub struct SubscriptionStore {
    config: SubscriptionStoreConfig,
    next_subscriber_id: u64,
    scopes: BTreeMap<SubscriptionScope, ScopeState>,
}

impl Default for SubscriptionStore {
    fn default() -> Self {
        Self::new(SubscriptionStoreConfig::default())
    }
}

impl SubscriptionStore {
    pub fn new(config: SubscriptionStoreConfig) -> Self {
        Self {
            config,
            next_subscriber_id: 0,
            scopes: BTreeMap::new(),
        }
    }

    pub fn append(
        &mut self,
        scope: &SubscriptionScope,
        event: ResourceEventInput,
    ) -> Result<AppendReceipt, SubscriptionError> {
        validate_event(&event)?;
        let state = self.scopes.entry(scope.clone()).or_default();
        if let Some(cursor) = state.source_cursors.get(&event.source_id).copied() {
            if state
                .source_events
                .get(&event.source_id)
                .is_some_and(|stored| event_json(stored) != event_json(&event))
            {
                return Err(SubscriptionError::ConflictingSourcePayload);
            }
            return Ok(AppendReceipt {
                cursor,
                source_id: event.source_id,
                deduplicated: true,
            });
        }

        state.next_cursor += 1;
        let cursor = state.next_cursor;
        let source_id = event.source_id.clone();
        state.source_cursors.insert(source_id.clone(), cursor);
        state.source_events.insert(source_id.clone(), event.clone());
        state.retained.push_back(StoredEvent {
            cursor,
            event: event.clone(),
        });
        prune_retention(state, &self.config, event.occurred_at_ms);

        let frame = DeliveryFrame::Change { cursor, event };
        let dropped = state
            .subscribers
            .iter_mut()
            .filter_map(|(id, subscriber)| {
                if subscriber.matches(&frame) && !subscriber.push(frame.clone(), &self.config) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for id in dropped {
            state.subscribers.remove(&id);
        }

        Ok(AppendReceipt {
            cursor,
            source_id,
            deduplicated: false,
        })
    }

    pub fn known_source_ids(
        &self,
        scope: &SubscriptionScope,
        request: SourceIdPageRequest,
    ) -> SourceIdPage {
        let limit = request.limit.clamp(1, SOURCE_PAGE_LIMIT);
        let Some(state) = self.scopes.get(scope) else {
            return SourceIdPage {
                source_ids: Vec::new(),
                next_after: None,
            };
        };
        let mut source_ids = state
            .source_cursors
            .keys()
            .filter(|source_id| {
                request
                    .after
                    .as_ref()
                    .is_none_or(|after| *source_id > after)
            })
            .take(limit + 1)
            .cloned()
            .collect::<Vec<_>>();
        let next_after = if source_ids.len() > limit {
            source_ids.truncate(limit);
            source_ids.last().cloned()
        } else {
            None
        };
        SourceIdPage {
            source_ids,
            next_after,
        }
    }

    pub fn replay(
        &self,
        scope: &SubscriptionScope,
        resource_uris: &[String],
        cursor: Option<u64>,
    ) -> Vec<DeliveryFrame> {
        let Some(state) = self.scopes.get(scope) else {
            return cursor
                .filter(|cursor| *cursor > 0)
                .map(|_| vec![DeliveryFrame::Reset { cursor: 0 }])
                .unwrap_or_default();
        };
        let from_cursor = cursor.unwrap_or(0);
        if let Some(first) = state.retained.front()
            && from_cursor < first.cursor.saturating_sub(1)
        {
            return vec![DeliveryFrame::Reset {
                cursor: state.next_cursor,
            }];
        }
        state
            .retained
            .iter()
            .filter(|stored| {
                stored.cursor > from_cursor && event_matches(&stored.event, resource_uris)
            })
            .map(|stored| DeliveryFrame::Change {
                cursor: stored.cursor,
                event: stored.event.clone(),
            })
            .collect()
    }

    pub fn subscribe(
        &mut self,
        scope: &SubscriptionScope,
        resource_uris: Vec<String>,
        cursor: Option<u64>,
    ) -> SubscriberId {
        self.next_subscriber_id += 1;
        let id = SubscriberId(self.next_subscriber_id);
        let initial = self.replay(scope, &resource_uris, cursor);
        let state = self.scopes.entry(scope.clone()).or_default();
        let mut subscriber = SubscriberState::new(resource_uris);
        for frame in initial {
            if !subscriber.push(frame, &self.config) {
                break;
            }
        }
        state.subscribers.insert(id.clone(), subscriber);
        id
    }

    pub fn take_pending(&mut self, id: &SubscriberId) -> Option<Vec<DeliveryFrame>> {
        for state in self.scopes.values_mut() {
            if let Some(subscriber) = state.subscribers.get_mut(id) {
                if subscriber.dropped {
                    state.subscribers.remove(id);
                    return None;
                }
                return Some(subscriber.drain());
            }
        }
        None
    }

    pub fn heartbeat(&self, scope: &SubscriptionScope) -> DeliveryFrame {
        DeliveryFrame::Heartbeat {
            cursor: self
                .scopes
                .get(scope)
                .map(|state| state.next_cursor)
                .unwrap_or_default(),
        }
    }

    pub fn unsubscribe(&mut self, id: &SubscriberId) -> bool {
        self.scopes
            .values_mut()
            .any(|state| state.subscribers.remove(id).is_some())
    }

    pub fn subscriber_count(&self, scope: &SubscriptionScope) -> usize {
        self.scopes
            .get(scope)
            .map(|state| state.subscribers.len())
            .unwrap_or_default()
    }

    pub fn heartbeat_ms(&self) -> u64 {
        self.config.heartbeat_ms
    }
}

#[derive(Debug, Clone, Default)]
struct ScopeState {
    next_cursor: u64,
    retained: VecDeque<StoredEvent>,
    source_cursors: BTreeMap<String, u64>,
    source_events: BTreeMap<String, ResourceEventInput>,
    subscribers: BTreeMap<SubscriberId, SubscriberState>,
}

#[derive(Debug, Clone)]
struct StoredEvent {
    cursor: u64,
    event: ResourceEventInput,
}

#[derive(Debug, Clone)]
struct SubscriberState {
    filters: BTreeSet<String>,
    queue: VecDeque<DeliveryFrame>,
    bytes: usize,
    dropped: bool,
}

impl SubscriberState {
    fn new(resource_uris: Vec<String>) -> Self {
        Self {
            filters: resource_uris.into_iter().collect(),
            queue: VecDeque::new(),
            bytes: 0,
            dropped: false,
        }
    }

    fn matches(&self, frame: &DeliveryFrame) -> bool {
        match frame {
            DeliveryFrame::Change { event, .. } => {
                self.filters.is_empty()
                    || event
                        .resource_uris
                        .iter()
                        .any(|uri| self.filters.contains(uri))
            }
            DeliveryFrame::Reset { .. } | DeliveryFrame::Heartbeat { .. } => true,
        }
    }

    fn push(&mut self, frame: DeliveryFrame, config: &SubscriptionStoreConfig) -> bool {
        let bytes = frame.to_sse().len();
        if self.queue.len() >= config.max_subscriber_events
            || self.bytes.saturating_add(bytes) > config.max_subscriber_bytes
        {
            self.queue.clear();
            self.bytes = 0;
            self.dropped = true;
            return false;
        }
        self.bytes += bytes;
        self.queue.push_back(frame);
        true
    }

    fn drain(&mut self) -> Vec<DeliveryFrame> {
        self.bytes = 0;
        self.queue.drain(..).collect()
    }
}

fn validate_event(event: &ResourceEventInput) -> Result<(), SubscriptionError> {
    if event.schema_version != 1 {
        return Err(SubscriptionError::UnsupportedSchemaVersion(
            event.schema_version,
        ));
    }
    if event.source_id.is_empty() {
        return Err(SubscriptionError::EmptySourceId);
    }
    if event.resource_uris.is_empty() {
        return Err(SubscriptionError::EmptyResourceUris);
    }
    if event.resource_uris.iter().any(|uri| uri.is_empty()) {
        return Err(SubscriptionError::EmptyResourceUri);
    }
    if event.revision.is_empty() {
        return Err(SubscriptionError::EmptyRevision);
    }
    if event.occurred_at.is_empty() {
        return Err(SubscriptionError::EmptyOccurredAt);
    }
    Ok(())
}

fn prune_retention(state: &mut ScopeState, config: &SubscriptionStoreConfig, now_ms: u64) {
    while state.retained.len() > config.max_retained_events {
        remove_oldest(state);
    }
    while state.retained.front().is_some_and(|stored| {
        stored
            .event
            .occurred_at_ms
            .saturating_add(config.retention_window_ms)
            < now_ms
    }) {
        remove_oldest(state);
    }
}

fn remove_oldest(state: &mut ScopeState) {
    state.retained.pop_front();
}

fn event_matches(event: &ResourceEventInput, resource_uris: &[String]) -> bool {
    resource_uris.is_empty()
        || event
            .resource_uris
            .iter()
            .any(|uri| resource_uris.iter().any(|filter| filter == uri))
}

fn format_cursor(cursor: u64) -> String {
    format!("cf:{cursor}")
}

fn event_json(event: &ResourceEventInput) -> serde_json::Value {
    json!({
        "schema_version": event.schema_version,
        "source_id": event.source_id,
        "resource_uris": event.resource_uris,
        "revision": event.revision,
        "kind": event.kind.as_str(),
        "occurred_at": event.occurred_at,
    })
}

#[derive(Debug, Clone)]
pub struct ScopedSubscriptionStore {
    scope: SubscriptionScope,
    inner: Arc<Mutex<SubscriptionStore>>,
}

impl ScopedSubscriptionStore {
    pub fn new(scope: SubscriptionScope, store: SubscriptionStore) -> Self {
        Self::from_shared(scope, Arc::new(Mutex::new(store)))
    }

    pub fn from_shared(scope: SubscriptionScope, inner: Arc<Mutex<SubscriptionStore>>) -> Self {
        Self { scope, inner }
    }
}

impl core_subscriptions::SubscriptionStore for ScopedSubscriptionStore {
    fn append<'a>(
        &'a self,
        envelope: core_subscriptions::ChangeEnvelope,
    ) -> CoreStoreFuture<'a, core_subscriptions::StoreAppendResult> {
        Box::pin(async move {
            envelope.validate()?;
            let event = event_from_envelope(&envelope)?;
            let mut store = self
                .inner
                .lock()
                .map_err(|_| core_subscriptions::SubscriptionError::StateUnavailable)?;
            let receipt = store
                .append(&self.scope, event)
                .map_err(map_core_store_error)?;
            Ok(core_subscriptions::StoreAppendResult {
                delivered: core_subscriptions::DeliveredChange {
                    cursor: core_cursor(receipt.cursor)?,
                    envelope,
                },
                inserted: !receipt.deduplicated,
            })
        })
    }

    fn replay<'a>(
        &'a self,
        request: &'a core_subscriptions::SubscriptionRequest,
    ) -> CoreStoreFuture<'a, core_subscriptions::SubscriptionReplay> {
        Box::pin(async move {
            let after = match request.after.as_ref() {
                Some(cursor) => match parse_core_cursor(cursor) {
                    Some(cursor) => Some(cursor),
                    None => return Ok(core_reset_replay(request)),
                },
                None => None,
            };
            let store = self
                .inner
                .lock()
                .map_err(|_| core_subscriptions::SubscriptionError::StateUnavailable)?;
            let frames = store.replay(&self.scope, &request.resource_uris, after);
            let mut replay = core_subscriptions::SubscriptionReplay::default();
            for frame in frames {
                match frame {
                    DeliveryFrame::Change { cursor, event } => {
                        replay.events.push(core_subscriptions::DeliveredChange {
                            cursor: core_cursor(cursor)?,
                            envelope: envelope_from_event(event)?,
                        });
                    }
                    DeliveryFrame::Reset { .. } => {
                        replay.reset = Some(core_subscriptions::SubscriptionReset {
                            reason: "requested cursor is outside retained subscription history"
                                .to_string(),
                            resource_uris: request.resource_uris.clone(),
                        });
                    }
                    DeliveryFrame::Heartbeat { .. } => {}
                }
            }
            Ok(replay)
        })
    }
}

fn event_from_envelope(
    envelope: &core_subscriptions::ChangeEnvelope,
) -> Result<ResourceEventInput, core_subscriptions::SubscriptionError> {
    envelope.validate()?;
    let schema_version = u16::try_from(envelope.schema_version).map_err(|_| {
        core_subscriptions::SubscriptionError::InvalidEnvelope(
            "schema_version does not fit Cloudflare adapter".to_string(),
        )
    })?;
    Ok(ResourceEventInput {
        schema_version,
        source_id: envelope.source_id.clone(),
        resource_uris: envelope.resource_uris.clone(),
        revision: envelope.revision.clone(),
        kind: match envelope.kind {
            core_subscriptions::ChangeKind::Updated => ResourceEventKind::Updated,
            core_subscriptions::ChangeKind::Deleted => ResourceEventKind::Deleted,
        },
        occurred_at: envelope.occurred_at.clone(),
        occurred_at_ms: 0,
    })
}

fn envelope_from_event(
    event: ResourceEventInput,
) -> Result<core_subscriptions::ChangeEnvelope, core_subscriptions::SubscriptionError> {
    let envelope = core_subscriptions::ChangeEnvelope {
        schema_version: u32::from(event.schema_version),
        source_id: event.source_id,
        resource_uris: event.resource_uris,
        revision: event.revision,
        kind: match event.kind {
            ResourceEventKind::Updated => core_subscriptions::ChangeKind::Updated,
            ResourceEventKind::Deleted => core_subscriptions::ChangeKind::Deleted,
        },
        occurred_at: event.occurred_at,
    };
    envelope.validate()?;
    Ok(envelope)
}

fn core_cursor(
    cursor: u64,
) -> Result<core_subscriptions::DeliveryCursor, core_subscriptions::SubscriptionError> {
    core_subscriptions::DeliveryCursor::new(format_cursor(cursor))
}

fn parse_core_cursor(cursor: &core_subscriptions::DeliveryCursor) -> Option<u64> {
    cursor.as_str().strip_prefix("cf:")?.parse().ok()
}

fn core_reset_replay(
    request: &core_subscriptions::SubscriptionRequest,
) -> core_subscriptions::SubscriptionReplay {
    core_subscriptions::SubscriptionReplay {
        reset: Some(core_subscriptions::SubscriptionReset {
            reason: "requested cursor is outside retained subscription history".to_string(),
            resource_uris: request.resource_uris.clone(),
        }),
        events: Vec::new(),
    }
}

fn map_core_store_error(error: SubscriptionError) -> core_subscriptions::SubscriptionError {
    core_subscriptions::SubscriptionError::Store(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        source_id: &str,
        uri: &str,
        revision: &str,
        occurred_at_ms: u64,
    ) -> ResourceEventInput {
        ResourceEventInput {
            schema_version: 1,
            source_id: source_id.to_string(),
            resource_uris: vec![uri.to_string()],
            revision: revision.to_string(),
            kind: ResourceEventKind::Updated,
            occurred_at: "2026-10-09T00:00:00Z".to_string(),
            occurred_at_ms,
        }
    }

    fn core_envelope(
        source_id: &str,
        uri: &str,
        revision: &str,
    ) -> core_subscriptions::ChangeEnvelope {
        core_subscriptions::ChangeEnvelope {
            schema_version: core_subscriptions::SUBSCRIPTION_SCHEMA_VERSION,
            source_id: source_id.to_string(),
            resource_uris: vec![uri.to_string()],
            revision: revision.to_string(),
            kind: core_subscriptions::ChangeKind::Updated,
            occurred_at: "2026-10-09T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn append_deduplicates_source_ids_and_pages_known_sources() {
        let mut store = SubscriptionStore::default();
        let scope = SubscriptionScope::new("tenant-a", "repo-a");
        let first = store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            )
            .unwrap();
        let duplicate = store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1001),
            )
            .unwrap();
        let second = store
            .append(
                &scope,
                event("src-2", "gitfoundry://tenant/repo/issues", "r2", 1002),
            )
            .unwrap();

        assert_eq!(first.cursor, 1);
        assert_eq!(duplicate.cursor, first.cursor);
        assert!(duplicate.deduplicated);
        assert_eq!(second.cursor, 2);

        let page = store.known_source_ids(
            &scope,
            SourceIdPageRequest {
                after: None,
                limit: 1,
            },
        );
        assert_eq!(page.source_ids, vec!["src-1"]);
        assert_eq!(page.next_after.as_deref(), Some("src-1"));
        let next = store.known_source_ids(
            &scope,
            SourceIdPageRequest {
                after: page.next_after,
                limit: 100,
            },
        );
        assert_eq!(next.source_ids, vec!["src-2"]);
        assert_eq!(next.next_after, None);
    }

    #[test]
    fn append_conflicts_when_source_id_reused_with_different_payload() {
        let mut store = SubscriptionStore::default();
        let scope = SubscriptionScope::new("tenant-a", "repo-a");
        store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            )
            .unwrap();
        let conflict = store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r2", 1000),
            )
            .unwrap_err();
        assert_eq!(conflict, SubscriptionError::ConflictingSourcePayload);
    }

    #[tokio::test]
    async fn scoped_store_implements_core_subscription_store_contract() {
        let store = ScopedSubscriptionStore::new(
            SubscriptionScope::new("tenant-a", "repo-a"),
            SubscriptionStore::default(),
        );
        let first_envelope = core_envelope("src-1", "gitfoundry://tenant/repo/head", "r1");
        let duplicate_envelope = first_envelope.clone();
        let second_envelope = core_envelope("src-2", "gitfoundry://tenant/repo/head", "r2");

        let first = <ScopedSubscriptionStore as core_subscriptions::SubscriptionStore>::append(
            &store,
            first_envelope,
        )
        .await
        .unwrap();
        let duplicate = <ScopedSubscriptionStore as core_subscriptions::SubscriptionStore>::append(
            &store,
            duplicate_envelope,
        )
        .await
        .unwrap();
        let second = <ScopedSubscriptionStore as core_subscriptions::SubscriptionStore>::append(
            &store,
            second_envelope.clone(),
        )
        .await
        .unwrap();

        assert_eq!(first.delivered.cursor.as_str(), "cf:1");
        assert!(first.inserted);
        assert_eq!(duplicate.delivered.cursor, first.delivered.cursor);
        assert!(!duplicate.inserted);
        assert_eq!(second.delivered.cursor.as_str(), "cf:2");
        assert!(second.inserted);

        let replay = <ScopedSubscriptionStore as core_subscriptions::SubscriptionStore>::replay(
            &store,
            &core_subscriptions::SubscriptionRequest {
                resource_uris: vec!["gitfoundry://tenant/repo/head".to_string()],
                after: Some(first.delivered.cursor.clone()),
                limits: core_subscriptions::SubscriptionLimits::default(),
            },
        )
        .await
        .unwrap();
        assert_eq!(replay.reset, None);
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.events[0].cursor.as_str(), "cf:2");
        assert_eq!(replay.events[0].envelope, second_envelope);
    }

    #[test]
    fn replay_returns_reset_when_cursor_falls_out_of_retention() {
        let mut store = SubscriptionStore::new(SubscriptionStoreConfig {
            retention_window_ms: 7 * 24 * 60 * 60 * 1000,
            max_retained_events: 2,
            max_subscriber_events: 256,
            max_subscriber_bytes: 1024 * 1024,
            heartbeat_ms: 15_000,
        });
        let scope = SubscriptionScope::new("tenant-a", "repo-a");
        store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            )
            .unwrap();
        store
            .append(
                &scope,
                event("src-2", "gitfoundry://tenant/repo/head", "r2", 1001),
            )
            .unwrap();
        store
            .append(
                &scope,
                event("src-3", "gitfoundry://tenant/repo/head", "r3", 1002),
            )
            .unwrap();

        let replay = store.replay(
            &scope,
            &["gitfoundry://tenant/repo/head".to_string()],
            Some(0),
        );
        assert_eq!(replay, vec![DeliveryFrame::Reset { cursor: 3 }]);
    }

    #[test]
    fn subscribers_receive_matching_events_and_drop_when_queue_bounds_are_hit() {
        let mut store = SubscriptionStore::new(SubscriptionStoreConfig {
            retention_window_ms: 7 * 24 * 60 * 60 * 1000,
            max_retained_events: 100_000,
            max_subscriber_events: 1,
            max_subscriber_bytes: 180,
            heartbeat_ms: 15_000,
        });
        let scope = SubscriptionScope::new("tenant-a", "repo-a");
        let subscriber = store.subscribe(
            &scope,
            vec!["gitfoundry://tenant/repo/head".to_string()],
            None,
        );
        store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            )
            .unwrap();
        store
            .append(
                &scope,
                event("src-2", "gitfoundry://tenant/repo/head", "r2", 1001),
            )
            .unwrap();

        assert!(
            store.take_pending(&subscriber).is_none(),
            "slow subscriber must be dropped once bounded queue overflows"
        );
        assert_eq!(store.subscriber_count(&scope), 0);
    }

    #[test]
    fn subscriber_with_empty_filter_receives_live_events() {
        let mut store = SubscriptionStore::default();
        let scope = SubscriptionScope::new("tenant-a", "repo-a");
        let subscriber = store.subscribe(&scope, Vec::new(), None);
        store
            .append(
                &scope,
                event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            )
            .unwrap();

        assert_eq!(
            store.take_pending(&subscriber),
            Some(vec![DeliveryFrame::Change {
                cursor: 1,
                event: event("src-1", "gitfoundry://tenant/repo/head", "r1", 1000),
            }])
        );
    }

    #[test]
    fn frames_encode_as_sse_change_reset_and_heartbeat_messages() {
        let frame = DeliveryFrame::Change {
            cursor: 7,
            event: event("src-7", "gitfoundry://tenant/repo/head", "r7", 1007),
        };
        let sse = frame.to_sse();
        assert!(sse.contains("event: change\n"));
        assert!(sse.contains("\"cursor\":\"cf:7\""));
        assert!(sse.contains("\"occurred_at\":\"2026-10-09T00:00:00Z\""));
        assert!(!sse.contains("occurred_at_ms"));
        assert!(
            DeliveryFrame::Reset { cursor: 8 }
                .to_sse()
                .contains("event: reset\n")
        );
        assert!(
            DeliveryFrame::Heartbeat { cursor: 8 }
                .to_sse()
                .contains("event: heartbeat\n")
        );
    }
}
