const AUTHORIZATION = "Bearer owner";
const PRIVATE_TOKEN = "incurs-subscriptions-v1";
const RETENTION_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_RETAINED_EVENTS = 100000;
const MAX_SUBSCRIBER_EVENTS = 256;
const MAX_SUBSCRIBER_BYTES = 1024 * 1024;
const HEARTBEAT_MS = 15000;

export default {
  async fetch(request, env) {
    if (!authorized(request)) {
      return json({ error: "unauthorized" }, 401);
    }
    const url = new URL(request.url);
    const scope = readScope(url);
    if (!scope) {
      return json({ error: "tenant and repo are required" }, 400);
    }
    const id = env.SUBSCRIPTIONS.idFromName(scope.key);
    const stub = env.SUBSCRIPTIONS.get(id);
    if (url.pathname === "/append" && request.method === "POST") {
      return stub.fetch(privateRequest("/append", scope, { method: "POST", body: await request.text() }));
    }
    if (url.pathname === "/known-source-ids" && request.method === "GET") {
      const path = `/known-source-ids?${url.searchParams.toString()}`;
      return stub.fetch(privateRequest(path, scope));
    }
    if (url.pathname === "/events" && request.method === "GET") {
      const path = `/listen?${url.searchParams.toString()}`;
      return stub.fetch(privateRequest(path, scope));
    }
    return json({ error: "not found" }, 404);
  },
};

export class SubscriptionDelivery {
  constructor(state) {
    this.state = state;
    this.sessions = new Map();
    this.nextSession = 0;
    this.ready = this.state.blockConcurrencyWhile(async () => this.migrate());
  }

  async migrate() {
    this.sql.exec("CREATE TABLE IF NOT EXISTS events (scope TEXT NOT NULL, cursor INTEGER NOT NULL, source_id TEXT NOT NULL, payload TEXT NOT NULL, resource_uris TEXT NOT NULL, occurred_at_ms INTEGER NOT NULL, PRIMARY KEY (scope, cursor))");
    this.sql.exec("CREATE TABLE IF NOT EXISTS sources (scope TEXT NOT NULL, source_id TEXT NOT NULL, cursor INTEGER NOT NULL, PRIMARY KEY (scope, source_id))");
  }

  get sql() {
    return this.state.storage.sql;
  }

  async fetch(request) {
    await this.ready;
    if (request.headers.get("x-incurs-private") !== PRIVATE_TOKEN) {
      return json({ error: "private service binding required" }, 403);
    }
    const url = new URL(request.url);
    const scope = request.headers.get("x-incurs-scope");
    if (!scope) {
      return json({ error: "scope is required" }, 400);
    }
    if (url.pathname === "/append" && request.method === "POST") {
      return this.append(scope, await request.json());
    }
    if (url.pathname === "/known-source-ids" && request.method === "GET") {
      return this.knownSourceIds(scope, url);
    }
    if (url.pathname === "/listen" && request.method === "GET") {
      return this.listen(scope, url);
    }
    if (url.pathname === "/reconcile" && request.method === "POST") {
      return json({ source_ids: this.allKnownSourceIds(scope) });
    }
    return json({ error: "not found" }, 404);
  }

  append(scope, event) {
    const error = validateEvent(event);
    if (error) {
      return json({ error }, 400);
    }
    const existing = first(this.sql.exec("SELECT cursor FROM sources WHERE scope = ? AND source_id = ?", scope, event.source_id));
    if (existing) {
      return json({ cursor: existing.cursor, source_id: event.source_id, deduplicated: true });
    }
    const max = first(this.sql.exec("SELECT COALESCE(MAX(cursor), 0) AS cursor FROM events WHERE scope = ?", scope));
    const cursor = Number(max?.cursor ?? 0) + 1;
    this.sql.exec(
      "INSERT INTO events (scope, cursor, source_id, payload, resource_uris, occurred_at_ms) VALUES (?, ?, ?, ?, ?, ?)",
      scope,
      cursor,
      event.source_id,
      JSON.stringify(event),
      JSON.stringify(event.resource_uris),
      event.occurred_at_ms,
    );
    this.sql.exec("INSERT INTO sources (scope, source_id, cursor) VALUES (?, ?, ?)", scope, event.source_id, cursor);
    this.prune(scope, event.occurred_at_ms);
    this.broadcast(scope, { event: "change", data: { cursor, event } });
    return json({ cursor, source_id: event.source_id, deduplicated: false });
  }

  knownSourceIds(scope, url) {
    const after = url.searchParams.get("after") ?? "";
    const limit = clamp(Number(url.searchParams.get("limit") ?? 500), 1, 500);
    const found = rows(this.sql.exec(
      "SELECT source_id FROM sources WHERE scope = ? AND source_id > ? ORDER BY source_id LIMIT ?",
      scope,
      after,
      limit + 1,
    )).map((row) => row.source_id);
    const page = found.slice(0, limit);
    return json({ source_ids: page, next_after: found.length > limit ? page.at(-1) : null });
  }

  listen(scope, url) {
    const filters = url.searchParams.getAll("resource_uri");
    const cursor = parseCursor(url.searchParams.get("cursor"));
    const replay = this.replay(scope, filters, cursor);
    const id = ++this.nextSession;
    let timer;
    const stream = new ReadableStream({
      start: (controller) => {
        const session = { scope, filters, controller, queued: 0, bytes: 0 };
        this.sessions.set(id, session);
        for (const frame of replay) {
          enqueueFrame(session, frame);
        }
        timer = setInterval(() => {
          const current = Number(first(this.sql.exec("SELECT COALESCE(MAX(cursor), 0) AS cursor FROM events WHERE scope = ?", scope))?.cursor ?? 0);
          enqueueFrame(session, { event: "heartbeat", data: { cursor: current } });
        }, HEARTBEAT_MS);
      },
      cancel: () => {
        clearInterval(timer);
        this.sessions.delete(id);
      },
    });
    return new Response(stream, {
      headers: {
        "content-type": "text/event-stream",
        "cache-control": "no-store",
        "x-accel-buffering": "no",
      },
    });
  }

  replay(scope, filters, cursor) {
    const current = Number(first(this.sql.exec("SELECT COALESCE(MAX(cursor), 0) AS cursor FROM events WHERE scope = ?", scope))?.cursor ?? 0);
    const firstRow = first(this.sql.exec("SELECT cursor FROM events WHERE scope = ? ORDER BY cursor LIMIT 1", scope));
    if (cursor !== null && firstRow && cursor < Number(firstRow.cursor) - 1) {
      return [{ event: "reset", data: { cursor: current } }];
    }
    return rows(this.sql.exec(
      "SELECT cursor, payload, resource_uris FROM events WHERE scope = ? AND cursor > ? ORDER BY cursor",
      scope,
      cursor ?? 0,
    ))
      .map((row) => ({ cursor: Number(row.cursor), event: JSON.parse(row.payload), resource_uris: JSON.parse(row.resource_uris) }))
      .filter((row) => matches(row.resource_uris, filters))
      .map((row) => ({ event: "change", data: { cursor: row.cursor, event: row.event } }));
  }

  prune(scope, now) {
    const retained = rows(this.sql.exec("SELECT cursor, source_id, occurred_at_ms FROM events WHERE scope = ? ORDER BY cursor", scope));
    const expired = retained.filter((row, index) => retained.length - index > MAX_RETAINED_EVENTS || Number(row.occurred_at_ms) + RETENTION_MS < now);
    for (const row of expired) {
      this.sql.exec("DELETE FROM events WHERE scope = ? AND cursor = ?", scope, row.cursor);
      this.sql.exec("DELETE FROM sources WHERE scope = ? AND source_id = ? AND cursor = ?", scope, row.source_id, row.cursor);
    }
  }

  broadcast(scope, frame) {
    for (const [id, session] of this.sessions) {
      if (session.scope !== scope || !matches(frame.data.event.resource_uris, session.filters)) {
        continue;
      }
      if (!enqueueFrame(session, frame)) {
        this.sessions.delete(id);
      }
    }
  }

  allKnownSourceIds(scope) {
    return rows(this.sql.exec("SELECT source_id FROM sources WHERE scope = ? ORDER BY source_id", scope)).map((row) => row.source_id);
  }
}

function authorized(request) {
  return request.headers.get("authorization") === AUTHORIZATION;
}

function readScope(url) {
  const tenant = url.searchParams.get("tenant");
  const repo = url.searchParams.get("repo");
  if (!tenant || !repo) {
    return null;
  }
  return { key: `${tenant}/${repo}` };
}

function privateRequest(path, scope, init = {}) {
  const headers = new Headers(init.headers);
  headers.set("x-incurs-private", PRIVATE_TOKEN);
  headers.set("x-incurs-scope", scope.key);
  if (init.body && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  return new Request(`https://incurs-subscriptions.internal${path}`, { ...init, headers });
}

function validateEvent(event) {
  if (event?.schema_version !== 1) return "schema_version must be 1";
  if (!event.source_id) return "source_id is required";
  if (!Array.isArray(event.resource_uris) || event.resource_uris.length === 0) return "resource_uris must be nonempty";
  if (event.resource_uris.some((uri) => typeof uri !== "string" || uri.length === 0)) return "resource_uris must contain nonempty strings";
  if (!event.revision) return "revision is required";
  if (event.kind !== "updated" && event.kind !== "deleted") return "kind must be updated or deleted";
  if (!Number.isInteger(event.occurred_at_ms)) return "occurred_at_ms must be an integer";
  return null;
}

function rows(cursor) {
  return typeof cursor.toArray === "function" ? cursor.toArray() : Array.from(cursor);
}

function first(cursor) {
  return rows(cursor)[0];
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, Number.isFinite(value) ? value : max));
}

function parseCursor(value) {
  if (value === null || value === "") return null;
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= 0 ? parsed : null;
}

function matches(resourceUris, filters) {
  return filters.length === 0 || resourceUris.some((uri) => filters.includes(uri));
}

function enqueueFrame(session, frame) {
  const text = `event: ${frame.event}\ndata: ${JSON.stringify(frame.data)}\n\n`;
  const bytes = new TextEncoder().encode(text).byteLength;
  if (session.queued >= MAX_SUBSCRIBER_EVENTS || session.bytes + bytes > MAX_SUBSCRIBER_BYTES) {
    try {
      session.controller.close();
    } catch (_) {}
    return false;
  }
  session.queued += 1;
  session.bytes += bytes;
  session.controller.enqueue(text);
  return true;
}

function json(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
