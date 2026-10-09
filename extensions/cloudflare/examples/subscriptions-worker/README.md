# subscriptions-worker

This isolated Worker proves the Cloudflare delivery shape for Incurs subscriptions without adding it to the extension workspace or changing shared lockfiles.

The public Worker is the authorization gate. It accepts `Authorization: Bearer owner`, resolves trusted `tenant` and `repo` query parameters into a scope, then calls the Durable Object with private headers. The Durable Object rejects every request missing `x-incurs-private: incurs-subscriptions-v1`, so append, known-source, replay, and listen operations are not public DO routes.

The Durable Object stores committed events in SQLite, deduplicates by `(scope, source_id)`, retains seven days or 100000 events, emits `change`, `reset`, and `heartbeat` SSE frames, and drops streams that exceed 256 events or 1 MiB of queued frames.

Run the local proof:

```sh
node test-local.mjs
```

The proof starts `wrangler dev --local`, appends an event, reconnects with replay, opens two live listeners, broadcasts one append to both listeners, restarts Wrangler with persistent local DO state, and verifies source deduplication after restart.
