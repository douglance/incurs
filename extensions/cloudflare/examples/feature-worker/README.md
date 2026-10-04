# incurs feature Worker

One Cloudflare Worker with every incurs feature on. Each route exercises one
feature, and `smoke.sh` proves each works under `wrangler dev` rather than
only compiling for wasm32.

| Route | Exercises |
| --- | --- |
| `POST /run` `{"argv": [...]}` | the CLI runtime: help, `--llms`, `--schema`, every output format, token counting, completions, OpenAPI commands, and remote MCP commands under `self` |
| `/api/...` | HTTP command routes, the OpenAPI document, skill discovery |
| `/api/mcp` | MCP over Streamable HTTP, all five standards |
| `GET /plugin` | an Agent Plugin package loaded from memory, with its HTTP MCP server connected |

The Worker's own `/api/mcp` is both the MCP server and the remote server its
`self` commands and plugin connect to, so one local run proves both directions.

## Authentication

The Worker fails closed. With the `MCP_AUTH_TOKEN` secret set, every route
requires `Authorization: Bearer <token>`, and the Worker sends the token on its
own MCP calls. Without it, only requests addressed to this machine are
admitted, so a deployment without a token refuses everything.

```sh
npx wrangler secret put MCP_AUTH_TOKEN
```

In production, a Worker usually cannot fetch its own public URL; point
`SELF_URL` at another deployment or replace the self-calls with a service
binding.

## Run the smoke test

Requires `worker-build`, `python3`, `jq`, and npm (Wrangler 4.114 or newer).

```sh
./smoke.sh
```

It starts a local API that serves the OpenAPI fixture, runs the Worker without
a token and then with one, and fails on any failed check or on a panic in the
Worker log.
