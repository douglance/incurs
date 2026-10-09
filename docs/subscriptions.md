# Live subscriptions implementation contract

Approved scope: reusable Incurs subscription core and Cloudflare delivery, GitFoundry standalone UI, MCP clients, and a real ChatGPT widget. Preserve owner-only authorization and the unified command catalog. Background ChatGPT webhook tasks are out of scope.

## Shared event boundary
- Resource change input: schema_version (1), source_id (stable durable receipt identity), resource_uris (nonempty canonical URI list), revision (authoritative opaque revision), kind (updated or deleted), occurred_at (RFC3339). Tenant/repository scope and source provenance are trusted host context, not client authority.
- Delivery adds an opaque cursor. A cursor orders delivery, not business revisions. Retention gaps produce an explicit reset requiring authoritative reads.
- GitFoundry resource URIs use gitfoundry://tenants/{tenant}/repositories/{repository}/{resource}; resource is head, forks, code, issues, reviews, or checks. Workspace repository-list changes use gitfoundry://tenants/{tenant}/repositories. URI builders and operation resource mappings are shared/generated catalog metadata.
- Only durable committed changes enter the projection. Project Git result receipts idempotently; reconcile after crashes. Transactional metadata uses an atomic outbox. No dependency on a best-effort handler callback for correctness.
- Standalone endpoint: GET /api/v1/tenants/{tenant}/events, authenticated same-origin SSE, resource filters and last cursor. A single workspace connection updates the cache; no browser polling.
- Keep existing API behavior and Code Mode. Use Incurs core portable HTTP streaming for new protocol handling. Modern subscriptions/listen and supported legacy resources subscriptions are covered; do not advertise unimplemented capabilities.
- Incurs owns generic core types/traits and delivery mechanisms. GitFoundry owns source projection and live permission checks. Existing OpenAI app resource helpers are reused.

## Bounds and recovery
Seven days or 100000 retained events per tenant; 15-second heartbeat and authorization recheck; authorization before every event; per-subscriber 256 events or 1 MiB queue; slow consumers reconnect/replay. Initial connection subscribes before snapshot, buffering/reconciling updates. Stable source IDs deduplicate delivery ingestion. Dropped streams release resources. Expired credentials and revoked permissions stop delivery.

## ChatGPT
Versioned MCP Apps UI resource and metadata, existing OpenAI resource subscribe/listener bridge first. If the host lacks resource capability, an authenticated tool returns an app-only read stream grant (5-minute TTL) and the widget uses fetch SSE with Authorization; grants never enter URLs or model-visible structuredContent. Renew through the host bridge and declare CSP domains. Use maintained Cloudflare OAuth provider with PKCE S256, CIMD, protected-resource discovery and owner Access consent; token maps to persisted owner principal and live grants. Do not expose protected data through OAuth discovery. Real-host acceptance is required, a browser fixture alone is insufficient.

## Gates
Test-first each slice; native/WASM gates; independent browser host; mutation probes; CLI write updates existing browser and real ChatGPT widget. Cover crash between commit and enqueue, restart, disconnect, replay, gaps, duplicate/out-of-order events, revocation and tenant substitution, draft preservation, conflict no-resubmit, queue bounds. Measure p95 below 1 second with 100 subscribers and 10 events/sec and zero unrelated reloads. Release Incurs dependencies before pinning GitFoundry; staging Alchemy then authenticated acceptance then production, with feature flags and rollback restoring Refresh controls. Preserve unrelated dirty worktrees.
