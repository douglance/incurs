import {
  IncursSubscriptionDelivery,
  PRIVATE_ROUTING_HEADER,
  PRIVATE_ROUTING_VALUE,
  SCOPE_HEADER,
  createIncursSubscriptionWorker,
} from "../../../subscriptions/worker.mjs";

const authorize = (request) => request.headers.get("authorization") === "Bearer owner";
const application = createIncursSubscriptionWorker({ authorize });

export class ProofSubscriptionDelivery extends IncursSubscriptionDelivery {
  constructor(state) {
    super(state);
    this.proofReaders = new Map();
  }

  async fetch(request) {
    await this.ready;
    const url = new URL(request.url);
    if (!url.pathname.startsWith("/proof/")) {
      return super.fetch(request);
    }
    if (request.headers.get(PRIVATE_ROUTING_HEADER) !== PRIVATE_ROUTING_VALUE) {
      return json({ error: "private service binding required" }, 403);
    }
    const scope = request.headers.get(SCOPE_HEADER);
    if (!scope) {
      return json({ error: "scope is required" }, 400);
    }
    if (url.pathname === "/proof/hold" && request.method === "POST") {
      return this.proofHold(scope, url);
    }
    if (url.pathname === "/proof/drain" && request.method === "POST") {
      return this.proofDrain(url);
    }
    if (url.pathname === "/proof/prune" && request.method === "POST") {
      this.prune(scope, Date.now() + (8 * 24 * 60 * 60 * 1000));
      return json({ pruned: true });
    }
    return json({ error: "not found" }, 404);
  }

  proofHold(scope, url) {
    url.pathname = "/listen";
    const response = this.listen(scope, url);
    const token = crypto.randomUUID();
    this.proofReaders.set(token, {
      reader: response.body.getReader(),
      decoder: new TextDecoder(),
      buffer: "",
    });
    return json({ token });
  }

  async proofDrain(url) {
    const token = url.searchParams.get("token");
    const held = this.proofReaders.get(token);
    if (!held) {
      return json({ error: "held proof reader not found" }, 404);
    }
    const maxFrames = clamp(Number(url.searchParams.get("max_frames") ?? 300), 1, 1000);
    const timeoutMs = clamp(Number(url.searchParams.get("timeout_ms") ?? 5000), 1, 60000);
    let frames = 0;
    let bytes = 0;
    while (frames < maxFrames) {
      const read = await readWithTimeout(held.reader, timeoutMs);
      if (read.timedOut) {
        return json({ done: false, frames, bytes, timed_out: true });
      }
      if (read.done) {
        this.proofReaders.delete(token);
        return json({ done: true, frames, bytes });
      }
      bytes += read.value.byteLength;
      held.buffer += held.decoder.decode(read.value, { stream: true });
      while (held.buffer.includes("\n\n")) {
        const frameEnd = held.buffer.indexOf("\n\n");
        const raw = held.buffer.slice(0, frameEnd);
        held.buffer = held.buffer.slice(frameEnd + 2);
        if (raw.trim()) frames += 1;
      }
    }
    return json({ done: false, frames, bytes, truncated: true });
  }
}

export default {
  async fetch(request, env, context) {
    const url = new URL(request.url);
    if (url.pathname.startsWith("/proof/")) {
      if (!authorize(request)) {
        return json({ error: "unauthorized" }, 401);
      }
      const scope = resolveScope(url);
      if (!scope) {
        return json({ error: "tenant and repo are required" }, 400);
      }
      const namespace = env.SUBSCRIPTIONS;
      const stub = namespace.get(namespace.idFromName(scope.key));
      return stub.fetch(privateRequest(url, scope, { method: "POST" }));
    }
    return application.fetch(request, env, context);
  },
};

function resolveScope(url) {
  const tenant = url.searchParams.get("tenant");
  const repo = url.searchParams.get("repo");
  return tenant && repo ? { key: `${tenant}/${repo}` } : null;
}

function privateRequest(url, scope, init = {}) {
  const headers = new Headers(init.headers);
  headers.set(PRIVATE_ROUTING_HEADER, PRIVATE_ROUTING_VALUE);
  headers.set(SCOPE_HEADER, scope.key);
  return new Request(`https://incurs-subscriptions.internal${url.pathname}?${url.searchParams.toString()}`, { ...init, headers });
}

async function readWithTimeout(reader, timeoutMs) {
  return Promise.race([
    reader.read(),
    new Promise((resolve) => setTimeout(() => resolve({ timedOut: true }), timeoutMs)),
  ]);
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, Number.isFinite(value) ? value : max));
}

function json(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
