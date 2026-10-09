export const PRIVATE_ROUTING_HEADER = "x-incurs-private";
export const PRIVATE_ROUTING_VALUE = "incurs-subscriptions-v1";
export const SCOPE_HEADER = "x-incurs-scope";
export const RETENTION_MS = 7 * 24 * 60 * 60 * 1000;
export const MAX_RETAINED_EVENTS = 100000;
export const MAX_SUBSCRIBER_EVENTS = 256;
export const MAX_SUBSCRIBER_BYTES = 1024 * 1024;
export const HEARTBEAT_MS = 15000;

export function createIncursSubscriptionWorker(options = {}) {
  const bindingName = options.bindingName ?? "SUBSCRIPTIONS";
  const authorize = options.authorize ?? (() => false);
  const resolveScope = options.resolveScope ?? defaultResolveScope;
  return {
    async fetch(request, env, context) {
      if (!(await authorize(request, env, context))) {
        return json({ error: "unauthorized" }, 401);
      }
      const url = new URL(request.url);
      const scope = await resolveScope(request, env, context);
      if (!scope?.key) {
        return json({ error: "tenant and repo are required" }, 400);
      }
      const namespace = env[bindingName];
      if (!namespace) {
        return json({ error: `Durable Object binding ${bindingName} is not configured` }, 500);
      }
      const stub = namespace.get(namespace.idFromName(scope.key));
      if (url.pathname === "/append" && request.method === "POST") {
        return stub.fetch(privateRequest("/append", scope, { method: "POST", body: await request.text() }));
      }
      if (url.pathname === "/known-source-ids" && request.method === "GET") {
        return stub.fetch(privateRequest(`/known-source-ids?${url.searchParams.toString()}`, scope));
      }
      if (url.pathname === "/events" && request.method === "GET") {
        return stub.fetch(privateRequest(`/listen?${url.searchParams.toString()}`, scope));
      }
      if (url.pathname === "/reconcile" && request.method === "POST") {
        return stub.fetch(privateRequest("/reconcile", scope, { method: "POST", body: await request.text() }));
      }
      return json({ error: "not found" }, 404);
    },
  };
}

export class IncursSubscriptionDelivery {
  constructor(state) {
    this.state = state;
    this.sessions = new Map();
    this.nextSession = 0;
    this.ready = this.state.blockConcurrencyWhile(async () => this.migrate());
  }

  async migrate() {
    this.sql.exec("CREATE TABLE IF NOT EXISTS events (scope TEXT NOT NULL, cursor INTEGER NOT NULL, source_id TEXT NOT NULL, payload TEXT NOT NULL, resource_uris TEXT NOT NULL, occurred_at_ms INTEGER NOT NULL, PRIMARY KEY (scope, cursor))");
    this.sql.exec("CREATE INDEX IF NOT EXISTS events_scope_occurred_cursor ON events (scope, occurred_at_ms, cursor)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS sources (scope TEXT NOT NULL, source_id TEXT NOT NULL, cursor INTEGER NOT NULL, payload TEXT, PRIMARY KEY (scope, source_id))");
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (scope TEXT PRIMARY KEY, next_cursor INTEGER NOT NULL)");
    try {
      this.sql.exec("ALTER TABLE sources ADD COLUMN payload TEXT");
    } catch (_) {}
  }

  get sql() {
    return this.state.storage.sql;
  }

  async fetch(request) {
    await this.ready;
    if (request.headers.get(PRIVATE_ROUTING_HEADER) !== PRIVATE_ROUTING_VALUE) {
      return json({ error: "private service binding required" }, 403);
    }
    const url = new URL(request.url);
    const scope = request.headers.get(SCOPE_HEADER);
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
    const payload = stableJson(event);
    const existing = first(this.sql.exec("SELECT cursor, payload FROM sources WHERE scope = ? AND source_id = ?", scope, event.source_id));
    if (existing) {
      if (existing.payload && existing.payload !== payload) {
        return json({ error: "source_id already exists with different payload" }, 409);
      }
      return json({ cursor: formatCursor(existing.cursor), source_id: event.source_id, deduplicated: true });
    }

    const cursor = this.nextCursor(scope);
    const retainedAtMs = Date.now();
    this.sql.exec(
      "INSERT INTO events (scope, cursor, source_id, payload, resource_uris, occurred_at_ms) VALUES (?, ?, ?, ?, ?, ?)",
      scope,
      cursor,
      event.source_id,
      payload,
      JSON.stringify(event.resource_uris),
      retainedAtMs,
    );
    this.sql.exec("INSERT INTO sources (scope, source_id, cursor, payload) VALUES (?, ?, ?, ?)", scope, event.source_id, cursor, payload);
    this.prune(scope, retainedAtMs);
    this.broadcast(scope, { event: "change", data: { cursor: formatCursor(cursor), event } });
    return json({ cursor: formatCursor(cursor), source_id: event.source_id, deduplicated: false });
  }

  nextCursor(scope) {
    const current = Number(first(this.sql.exec("SELECT next_cursor FROM meta WHERE scope = ?", scope))?.next_cursor ?? 0);
    const cursor = current + 1;
    this.sql.exec(
      "INSERT INTO meta (scope, next_cursor) VALUES (?, ?) ON CONFLICT(scope) DO UPDATE SET next_cursor = excluded.next_cursor",
      scope,
      cursor,
    );
    return cursor;
  }

  currentCursor(scope) {
    return Number(first(this.sql.exec("SELECT next_cursor FROM meta WHERE scope = ?", scope))?.next_cursor ?? 0);
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
    const parsed = parseCursor(url.searchParams.get("cursor"));
    const id = ++this.nextSession;
    const stream = new ReadableStream({
      start: (controller) => {
        const session = { scope, filters, controller, queued: 0, bytes: 0, timer: null, active: true };
        this.sessions.set(id, session);
        const replay = parsed.valid ? this.replay(scope, filters, parsed.cursor) : [this.resetFrame(scope, filters)];
        for (const frame of replay) {
          if (!this.enqueueToSession(id, session, frame)) return;
        }
        session.timer = setInterval(() => {
          if (!this.enqueueToSession(id, session, { event: "heartbeat", data: { cursor: formatCursor(this.currentCursor(scope)) } })) {
            this.dropSession(id, session);
          }
        }, HEARTBEAT_MS);
      },
      pull: () => {
        const session = this.sessions.get(id);
        if (session) {
          session.queued = 0;
          session.bytes = 0;
        }
      },
      cancel: () => this.dropSession(id),
    }, { highWaterMark: MAX_SUBSCRIBER_EVENTS, size: () => 1 });
    return new Response(stream, {
      headers: {
        "content-type": "text/event-stream",
        "cache-control": "no-store",
        "x-accel-buffering": "no",
      },
    });
  }

  replay(scope, filters, cursor) {
    const current = this.currentCursor(scope);
    const firstRow = first(this.sql.exec("SELECT cursor FROM events WHERE scope = ? ORDER BY cursor LIMIT 1", scope));
    if (cursor > current || (firstRow && cursor < Number(firstRow.cursor) - 1)) {
      return [this.resetFrame(scope, filters)];
    }
    return rows(this.sql.exec(
      "SELECT cursor, payload, resource_uris FROM events WHERE scope = ? AND cursor > ? ORDER BY cursor",
      scope,
      cursor,
    ))
      .map((row) => ({ cursor: Number(row.cursor), event: JSON.parse(row.payload), resource_uris: JSON.parse(row.resource_uris) }))
      .filter((row) => matches(row.resource_uris, filters))
      .map((row) => ({ event: "change", data: { cursor: formatCursor(row.cursor), event: row.event } }));
  }

  resetFrame(scope, resourceUris) {
    return { event: "reset", data: { cursor: formatCursor(this.currentCursor(scope)), resource_uris: resourceUris } };
  }

  prune(scope, now) {
    this.sql.exec("DELETE FROM events WHERE scope = ? AND occurred_at_ms < ?", scope, now - RETENTION_MS);
    const cutoff = first(this.sql.exec(
      "SELECT cursor FROM events WHERE scope = ? ORDER BY cursor DESC LIMIT 1 OFFSET ?",
      scope,
      MAX_RETAINED_EVENTS,
    ));
    if (cutoff) {
      this.sql.exec("DELETE FROM events WHERE scope = ? AND cursor <= ?", scope, cutoff.cursor);
    }
  }

  broadcast(scope, frame) {
    for (const [id, session] of this.sessions) {
      if (session.scope !== scope || !matches(frame.data.event.resource_uris, session.filters)) {
        continue;
      }
      if (!this.enqueueToSession(id, session, frame)) {
        this.dropSession(id, session);
      }
    }
  }

  enqueueToSession(id, session, frame) {
    if (!session.active) return false;
    const text = `event: ${frame.event}\ndata: ${JSON.stringify(frame.data)}\n\n`;
    const chunk = new TextEncoder().encode(text);
    const desiredSize = session.controller.desiredSize;
    if (session.queued >= MAX_SUBSCRIBER_EVENTS || (desiredSize !== null && desiredSize <= 0) || session.bytes + chunk.byteLength > MAX_SUBSCRIBER_BYTES) {
      this.dropSession(id, session);
      return false;
    }
    session.queued += 1;
    session.bytes += chunk.byteLength;
    try {
      session.controller.enqueue(chunk);
      if (session.queued >= MAX_SUBSCRIBER_EVENTS || (session.controller.desiredSize !== null && session.controller.desiredSize <= 0)) {
        this.dropSession(id, session);
      }
      return true;
    } catch (_) {
      this.dropSession(id, session);
      return false;
    }
  }

  dropSession(id, session = this.sessions.get(id)) {
    if (!session) return;
    session.active = false;
    if (session.timer) clearInterval(session.timer);
    this.sessions.delete(id);
    try {
      session.controller.close();
    } catch (_) {}
  }

  allKnownSourceIds(scope) {
    return rows(this.sql.exec("SELECT source_id FROM sources WHERE scope = ? ORDER BY source_id", scope)).map((row) => row.source_id);
  }
}

function defaultResolveScope(request) {
  const url = new URL(request.url);
  const tenant = url.searchParams.get("tenant");
  const repo = url.searchParams.get("repo");
  return tenant && repo ? { key: `${tenant}/${repo}` } : null;
}

function privateRequest(path, scope, init = {}) {
  const headers = new Headers(init.headers);
  headers.set(PRIVATE_ROUTING_HEADER, PRIVATE_ROUTING_VALUE);
  headers.set(SCOPE_HEADER, scope.key);
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
  if (typeof event.occurred_at !== "string" || Number.isNaN(Date.parse(event.occurred_at))) return "occurred_at must be an RFC3339 timestamp string";
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
  if (value === null || value === "") return { valid: true, cursor: 0 };
  const text = String(value);
  if (!text.startsWith("cf:")) return { valid: false, cursor: 0 };
  const parsed = Number(text.slice(3));
  return Number.isInteger(parsed) && parsed >= 0 ? { valid: true, cursor: parsed } : { valid: false, cursor: 0 };
}

function formatCursor(value) {
  return `cf:${Number(value)}`;
}

function matches(resourceUris, filters) {
  return filters.length === 0 || resourceUris.some((uri) => filters.includes(uri));
}

function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function json(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
