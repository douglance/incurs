# subscriptions-worker

This isolated Worker proves the reusable Cloudflare subscription delivery adapter exported from `../../subscriptions/worker.mjs`. The same implementation is also exported as `@incurs/cloudflare/subscriptions-worker` for product Workers that wire their own authorization and scope projection.

The public Worker is the authorization gate. This example accepts `Authorization: Bearer owner`, resolves trusted `tenant` and `repo` query parameters into a scope, then calls the Durable Object through the configured `SUBSCRIPTIONS` binding. `x-incurs-private` is only an internal routing marker; it is not product auth. Product Workers must keep append, listen, known-source, and reconcile calls behind their own auth and per-delivery reauthorization.

The reusable production Durable Object class is `IncursSubscriptionDelivery`; this proof Worker binds `ProofSubscriptionDelivery`, an example-only subclass that adds private `/proof/*` routes for local queue measurement. It stores core `ChangeEnvelope` payloads in SQLite, partitions cursor and source deduplication by trusted scope, rejects the same `(scope, source_id)` with a different payload, retains seven days or 100000 events, emits opaque `cf:{n}` cursors in `change`, `reset`, and `heartbeat` SSE frames, and drops streams that exceed 256 queued events or 1 MiB of aggregate queued frames.

Run the local proof:

```sh
node test-local.mjs
```

The proof starts `wrangler dev --local`, verifies unauthenticated public calls fail, appends and replays a core envelope, opens live listeners without a replay/register gap, proves duplicate append does not broadcast, proves conflicting duplicate payloads fail, measures 100 live subscribers at 10 events/sec with p95 under 1s, proves 256-event and 1MiB queue overflow closes streams, restarts Wrangler with persistent local DO state, and starts a second local Worker frontend that imports the same adapter.
