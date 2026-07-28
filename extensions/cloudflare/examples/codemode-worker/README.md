# incurs Code Mode Worker

This example runs the generic `incurs-codemode::CodeMode` lifecycle with the
Cloudflare executor, clock, and Durable Object SQLite store adapters.

- `math.sum` is explicitly read-only and runs immediately.
- `math.save` is not marked read-only, so execution pauses for approval.
- `/mcp` exposes the five lifecycle tools over stateless Streamable HTTP MCP.
- `/execute`, `/approve`, `/reject`, `/rollback`, `/pending`, and `/expire`
  remain direct diagnostic lifecycle routes.

## Run locally

Build the Worker with `worker-build --release`, then run it with
Wrangler 4.114.0 or newer:

```sh
npx wrangler dev
```

Loopback requests are authless when `MCP_AUTH_TOKEN` is unset. The Worker
rejects non-loopback requests in that state.

Use a direct diagnostic route:

```sh
curl -X POST http://localhost:8787/execute \
  -H 'content-type: application/json' \
  -d '{"code":"math.sum({ left: 2, right: 3 })"}'
```

Connect an MCP client to `http://localhost:8787/mcp` to use
`codemode_search`, `codemode_execute`, `codemode_execution`,
`codemode_decide`, and `codemode_cancel`.

The MCP endpoint requires `Content-Type: application/json` and an `Accept`
header containing both `application/json` and `text/event-stream`. It rejects
origins other than its own origin unless `MCP_ALLOWED_ORIGINS` contains the
origin.

## Configure remote access

Do not deploy the example remotely without authentication. Set one bearer
secret for the deployment:

```sh
npx wrangler secret put MCP_AUTH_TOKEN
```

Send the secret as `Authorization: Bearer <token>` on every route. The Worker
hashes the secret into the Durable Object name, so changing the secret creates
a separate tenant and does not expose the token in the object identifier.

For browser clients on another origin, set `MCP_ALLOWED_ORIGINS` to a
comma-separated list of exact origins, such as
`https://inspector.example.com`. Requests without an `Origin` header remain
valid for non-browser MCP clients.
