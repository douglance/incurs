# incurs Code Mode Worker

This example runs the generic `incurs-codemode::CodeMode` lifecycle with the
Cloudflare executor, clock, and Durable Object SQLite store adapters.

- `math.sum` is explicitly read-only and runs immediately.
- `math.save` is not marked read-only, so execution pauses for approval.
- `/execute`, `/approve`, `/reject`, `/rollback`, `/pending`, and `/expire`
  exercise the durable lifecycle.

Build the Worker with `worker-build --release`, then run it with
Wrangler 4.114.0 or newer:

Example:

```sh
npx wrangler dev

curl -X POST http://localhost:8787/execute \
  -H 'content-type: application/json' \
  -d '{"code":"math.sum({ left: 2, right: 3 })"}'
```
