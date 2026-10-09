import { rmSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { spawn } from "node:child_process";

const primary = instance(8977, ".wrangler/subscriptions-proof");
const secondary = instance(8978, ".wrangler/subscriptions-proof-second");
const cwd = new URL(".", import.meta.url).pathname;
const auth = { authorization: "Bearer owner" };
const scope = { tenant: "t1", repo: "r1" };
const resource = "gitfoundry://tenants/t1/repositories/r1/head";

rmSync(primary.persistTo, { recursive: true, force: true });
rmSync(secondary.persistTo, { recursive: true, force: true });

let server = await startWrangler(primary);
let secondServer;
try {
  await waitUntilReady(primary);
  await expectStatus(`${primary.base}/known-source-ids?tenant=t1&repo=r1`, 401);

  const first = await append(primary, "src-1", "r1", resource);
  assert(first.cursor === "cf:1" && first.deduplicated === false, "initial append assigns opaque cursor");

  const replay = await readOne(`${primary.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=cf%3A0`);
  assert(replay.event === "change" && replay.data.cursor === "cf:1", "replay after initial append");
  assert(replay.data.event.occurred_at === "2026-10-09T00:00:00Z", "replay emits core occurred_at field");

  const liveA = readOne(`${primary.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=cf%3A1`);
  const liveB = readOne(`${primary.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=cf%3A1`);
  await delay(100);
  await append(primary, "src-2", "r2", resource);
  const [frameA, frameB] = await Promise.all([liveA, liveB]);
  assert(frameA.data.cursor === "cf:2" && frameB.data.cursor === "cf:2", "two live listeners receive one append");

  const duplicateLive = readOne(`${primary.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=cf%3A2`, 750)
    .then(() => false, () => true);
  const duplicate = await append(primary, "src-2", "r2", resource);
  assert(duplicate.deduplicated === true && duplicate.cursor === "cf:2", "identical source id deduplicates");
  assert(await duplicateLive, "duplicate append does not fan out again");

  const conflict = await postEvent(primary, "src-2", "r2-conflict", resource, 409);
  assert(conflict.error.includes("different payload"), "same source id with different payload conflicts");

  const noGap = readOne(`${primary.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=cf%3A2`);
  await append(primary, "src-3", "r3", resource);
  assert((await noGap).data.cursor === "cf:3", "listener registration and replay do not miss concurrent append");

  const perf = await measureFanout(primary, "cf:3");
  assert(perf.connections === 100 && perf.events === 10, "performance proof uses 100 subscribers and 10 events");
  assert(perf.p95_ms < 1000, `fanout p95 ${perf.p95_ms}ms is under 1s`);

  const healthyBound = await proveHealthyReaderSurvivesPastEventBound(primary);
  assert(healthyBound.frames === 260, "healthy reader stays connected beyond 256 consumed messages");

  const countBound = await proveEventCountBound(primary);
  assert(countBound.done === true && countBound.frames === 256, `256 queued event bound closes stream: ${JSON.stringify(countBound)}`);

  const byteBound = await proveByteBound(primary);
  assert(byteBound.done === true && byteBound.frames === 0, "1MiB aggregate byte bound closes stream");

  const expiredDedup = await proveExpiredSourceDedup(primary);
  assert(expiredDedup.deduplicated === true && expiredDedup.noFanout === true, "expired replay row keeps source dedup checkpoint without fanout");

  await stopWrangler(server);
  server = await startWrangler(primary);
  await waitUntilReady(primary);
  const known = await getJson(`${primary.base}/known-source-ids?tenant=t1&repo=r1&after=src-0&limit=500`);
  assert(known.source_ids.includes("src-1") && known.source_ids.includes("src-2") && known.source_ids.includes("src-3"), "restart keeps durable source ids");

  const duplicateAfterRestart = await append(primary, "src-2", "r2", resource);
  assert(duplicateAfterRestart.deduplicated === true && duplicateAfterRestart.cursor === "cf:2", "source id deduplicates after restart");

  secondServer = await startWrangler(secondary);
  await waitUntilReady(secondary);
  await expectStatus(`${secondary.base}/known-source-ids?tenant=t2&repo=r2`, 401);
  const secondWorkerAppend = await append(secondary, "src-second-worker", "r1", "gitfoundry://tenants/t2/repositories/r2/head", { tenant: "t2", repo: "r2" });
  assert(secondWorkerAppend.cursor === "cf:1", "second Worker frontend imports the reusable adapter");

  console.log(JSON.stringify({
    ok: true,
    proof: "restart-reconnect-two-listeners-two-worker-instances-100-subscribers-server-queue-bounds",
    p95_ms: perf.p95_ms,
    p99_ms: perf.p99_ms,
    connections: perf.connections,
    source_ids: known.source_ids.length,
  }));
} finally {
  await stopWrangler(secondServer);
  await stopWrangler(server);
}

function instance(port, persistSuffix) {
  const root = new URL(".", import.meta.url).pathname;
  return {
    port,
    base: `http://127.0.0.1:${port}`,
    persistTo: `${root}${persistSuffix}`,
  };
}

async function startWrangler(target) {
  const child = spawn("wrangler", ["dev", "--local", "--port", String(target.port), "--inspector-port", "0", "--persist-to", target.persistTo], {
    cwd,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, WRANGLER_SEND_METRICS: "false", NO_COLOR: "1" },
  });
  child.stdout.on("data", (chunk) => process.stdout.write(chunk));
  child.stderr.on("data", (chunk) => process.stderr.write(chunk));
  return child;
}

async function stopWrangler(child) {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    delay(5000).then(() => child.kill("SIGKILL")),
  ]);
}

async function waitUntilReady(target) {
  for (let i = 0; i < 100; i += 1) {
    try {
      const response = await fetch(`${target.base}/known-source-ids?tenant=t1&repo=r1`, { headers: auth });
      if (response.ok) return;
    } catch (_) {}
    await delay(100);
  }
  throw new Error(`wrangler on ${target.port} did not become ready`);
}

async function expectStatus(url, status) {
  const response = await fetch(url);
  await response.text();
  assert(response.status === status, `${url} returns ${status}`);
}

async function append(target, sourceId, revision, uri, targetScope = scope, options = {}) {
  return postEvent(target, sourceId, revision, uri, 200, targetScope, options);
}

async function postEvent(target, sourceId, revision, uri, expectedStatus, targetScope = scope, options = {}) {
  const response = await fetch(`${target.base}/append?tenant=${targetScope.tenant}&repo=${targetScope.repo}`, {
    method: "POST",
    headers: { ...auth, "content-type": "application/json" },
    body: JSON.stringify({
      schema_version: 1,
      source_id: sourceId,
      resource_uris: [uri],
      revision,
      kind: "updated",
      occurred_at: options.occurredAt ?? "2026-10-09T00:00:00Z",
    }),
  });
  const text = await response.text();
  assert(response.status === expectedStatus, `${sourceId} append status ${expectedStatus}, got ${response.status}: ${text}`);
  return JSON.parse(text);
}

async function getJson(url, init = {}) {
  const response = await fetch(url, init.headers ? init : { ...init, headers: auth });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${response.status}: ${text}`);
  }
  return JSON.parse(text);
}

async function readOne(url, timeoutMs = 5000) {
  const stream = await openStream(url, timeoutMs);
  try {
    while (true) {
      const frame = await stream.nextFrame(timeoutMs);
      if (frame) return frame;
      throw new Error("SSE ended before a frame arrived");
    }
  } finally {
    stream.close();
  }
}

async function openStream(url, openTimeoutMs = 10000) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), openTimeoutMs);
  let response;
  try {
    response = await fetch(url, { headers: auth, signal: controller.signal });
  } finally {
    clearTimeout(timeout);
  }
  if (!response.ok || !response.body) {
    throw new Error(`SSE failed: ${response.status}`);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let ended = false;
  return {
    async nextFrame(readTimeoutMs = 0) {
      while (true) {
        const frameEnd = buffer.indexOf("\n\n");
        if (frameEnd >= 0) {
          const raw = buffer.slice(0, frameEnd);
          buffer = buffer.slice(frameEnd + 2);
          return parseFrame(raw);
        }
        const read = readTimeoutMs > 0
          ? await Promise.race([
              reader.read(),
              delay(readTimeoutMs).then(() => ({ timedOut: true })),
            ])
          : await reader.read();
        if (read.timedOut) {
          ended = true;
          controller.abort();
          throw new Error(`SSE frame timed out after ${readTimeoutMs}ms`);
        }
        const { value, done } = read;
        if (done) {
          ended = true;
          return null;
        }
        buffer += decoder.decode(value, { stream: true });
      }
    },
    get ended() {
      return ended;
    },
    close() {
      controller.abort();
    },
  };
}

async function measureFanout(target, afterCursor) {
  const perfResource = "gitfoundry://tenants/t1/repositories/r1/perf";
  const startTimes = new Map();
  const streamUrl = `${target.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(perfResource)}&cursor=${encodeURIComponent(afterCursor)}`;
  const streams = await Promise.all(Array.from({ length: 100 }, () => openStream(streamUrl, 60000)));
  try {
    const collectors = streams.map(async (stream) => {
      const latencies = [];
      while (latencies.length < 10) {
        const frame = await stream.nextFrame(20000);
        if (!frame) break;
        const sourceId = frame.data?.event?.source_id;
        if (sourceId && startTimes.has(sourceId)) {
          latencies.push(performance.now() - startTimes.get(sourceId));
        }
      }
      return latencies;
    });
    for (let i = 0; i < 10; i += 1) {
      const sourceId = `perf-${i}`;
      startTimes.set(sourceId, performance.now());
      await append(target, sourceId, `perf-${i}`, perfResource);
      await delay(100);
    }
    const perConnection = await Promise.race([
      Promise.all(collectors),
      delay(20000).then(() => { throw new Error("100-subscriber fanout timed out"); }),
    ]);
    const latencies = perConnection.flat().sort((a, b) => a - b);
    assert(latencies.length === 1000, `captured ${latencies.length} fanout deliveries`);
    return {
      connections: perConnection.length,
      events: 10,
      p95_ms: percentile(latencies, 0.95),
      p99_ms: percentile(latencies, 0.99),
    };
  } finally {
    for (const stream of streams) stream.close();
  }
}

async function proveHealthyReaderSurvivesPastEventBound(target) {
  const boundResource = "gitfoundry://tenants/t1/repositories/r1/healthy-count-bound";
  const stream = await openStream(`${target.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(boundResource)}&cursor=cf%3A0`, 30000);
  let frames = 0;
  try {
    for (let i = 0; i < 260; i += 1) {
      await append(target, `healthy-count-${i}`, `healthy-count-${i}`, boundResource);
      const frame = await stream.nextFrame(10000);
      assert(frame?.data?.event?.source_id === `healthy-count-${i}`, `healthy reader receives event ${i}`);
      frames += 1;
    }
    return { frames };
  } finally {
    stream.close();
  }
}

async function proveEventCountBound(target) {
  const boundResource = "gitfoundry://tenants/t1/repositories/r1/count-bound";
  const hold = await proofHold(target, boundResource);
  for (let i = 0; i < 257; i += 1) {
    await append(target, `bound-count-${i}`, `bound-count-${i}`, boundResource);
  }
  return proofDrain(target, hold.token, 300, 5000);
}

async function proveExpiredSourceDedup(target) {
  const expiredResource = "gitfoundry://tenants/t1/repositories/r1/expired-retention";
  await append(target, "expired-old", "expired-old", expiredResource, scope, { occurredAt: "2026-09-30T00:00:00Z" });
  await proofPrune(target);
  const noFanout = readOne(`${target.base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(expiredResource)}&cursor=cf%3A0`, 750)
    .then(() => false, () => true);
  const duplicate = await append(target, "expired-old", "expired-old", expiredResource, scope, { occurredAt: "2026-09-30T00:00:00Z" });
  return { deduplicated: duplicate.deduplicated, noFanout: await noFanout };
}

async function proveByteBound(target) {
  const boundResource = "gitfoundry://tenants/t1/repositories/r1/byte-bound";
  const hold = await proofHold(target, boundResource);
  await append(target, "bound-byte", "x".repeat(1024 * 1024), boundResource);
  return proofDrain(target, hold.token, 1, 5000);
}

async function proofHold(target, resourceUri, cursor = "cf:0") {
  return getJson(`${target.base}/proof/hold?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resourceUri)}&cursor=${encodeURIComponent(cursor)}`);
}

async function proofPrune(target) {
  return getJson(`${target.base}/proof/prune?tenant=t1&repo=r1`);
}

async function proofDrain(target, token, maxFrames, timeoutMs) {
  return getJson(`${target.base}/proof/drain?tenant=t1&repo=r1&token=${encodeURIComponent(token)}&max_frames=${maxFrames}&timeout_ms=${timeoutMs}`);
}

function parseFrame(raw) {
  const lines = raw.split("\n");
  const event = lines.find((line) => line.startsWith("event: "))?.slice(7);
  const data = lines.find((line) => line.startsWith("data: "))?.slice(6);
  return { event, data: JSON.parse(data) };
}

function percentile(values, quantile) {
  const index = Math.min(values.length - 1, Math.ceil(values.length * quantile) - 1);
  return Math.round(values[index]);
}

function assert(condition, label) {
  if (!condition) throw new Error(`assertion failed: ${label}`);
}
