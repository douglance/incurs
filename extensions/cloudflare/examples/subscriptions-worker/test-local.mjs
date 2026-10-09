import { rmSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { spawn } from "node:child_process";

const port = 8977;
const base = `http://127.0.0.1:${port}`;
const cwd = new URL(".", import.meta.url).pathname;
const persistTo = `${cwd}.wrangler/subscriptions-proof`;
const auth = { authorization: "Bearer owner" };
const resource = "gitfoundry://tenants/t1/repositories/r1/head";

rmSync(persistTo, { recursive: true, force: true });

let server = await startWrangler();
try {
  await waitUntilReady();
  await append("src-1", "r1");
  const replay = await readOne(`${base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=0`);
  assert(replay.event === "change" && replay.data.cursor === 1, "replay after initial append");

  const liveA = readOne(`${base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=1`);
  const liveB = readOne(`${base}/events?tenant=t1&repo=r1&resource_uri=${encodeURIComponent(resource)}&cursor=1`);
  await delay(100);
  await append("src-2", "r2");
  const [frameA, frameB] = await Promise.all([liveA, liveB]);
  assert(frameA.data.cursor === 2 && frameB.data.cursor === 2, "two live listeners receive one append");

  await stopWrangler(server);
  server = await startWrangler();
  await waitUntilReady();
  const known = await getJson(`${base}/known-source-ids?tenant=t1&repo=r1&limit=10`);
  assert(known.source_ids.includes("src-1") && known.source_ids.includes("src-2"), "restart keeps durable source ids");

  const duplicate = await append("src-2", "r2-duplicate");
  assert(duplicate.deduplicated === true && duplicate.cursor === 2, "source id deduplicates after restart");

  console.log(JSON.stringify({ ok: true, proof: "restart-reconnect-two-listeners", source_ids: known.source_ids }));
} finally {
  await stopWrangler(server);
}

async function startWrangler() {
  const child = spawn("wrangler", ["dev", "--local", "--port", String(port), "--inspector-port", "0", "--persist-to", persistTo], {
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

async function waitUntilReady() {
  for (let i = 0; i < 100; i += 1) {
    try {
      const response = await fetch(`${base}/known-source-ids?tenant=t1&repo=r1`, { headers: auth });
      if (response.ok) return;
    } catch (_) {}
    await delay(100);
  }
  throw new Error("wrangler did not become ready");
}

async function append(sourceId, revision) {
  return getJson(`${base}/append?tenant=t1&repo=r1`, {
    method: "POST",
    headers: { ...auth, "content-type": "application/json" },
    body: JSON.stringify({
      schema_version: 1,
      source_id: sourceId,
      resource_uris: [resource],
      revision,
      kind: "updated",
      occurred_at_ms: Date.now(),
    }),
  });
}

async function getJson(url, init = {}) {
  const response = await fetch(url, init.headers ? init : { ...init, headers: auth });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${response.status}: ${text}`);
  }
  return JSON.parse(text);
}

async function readOne(url) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
  try {
    const response = await fetch(url, { headers: auth, signal: controller.signal });
    if (!response.ok || !response.body) {
      throw new Error(`SSE failed: ${response.status}`);
    }
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { value, done } = await reader.read();
      if (done) throw new Error("SSE ended before frame");
      buffer += decoder.decode(value, { stream: true });
      const frameEnd = buffer.indexOf("\n\n");
      if (frameEnd >= 0) {
        controller.abort();
        return parseFrame(buffer.slice(0, frameEnd));
      }
    }
  } finally {
    clearTimeout(timeout);
  }
}

function parseFrame(raw) {
  const lines = raw.split("\n");
  const event = lines.find((line) => line.startsWith("event: "))?.slice(7);
  const data = lines.find((line) => line.startsWith("data: "))?.slice(6);
  return { event, data: JSON.parse(data) };
}

function assert(condition, label) {
  if (!condition) throw new Error(`assertion failed: ${label}`);
}
