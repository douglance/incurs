import http from "node:http";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = resolve(fileURLToPath(new URL(".", import.meta.url)));
const mimeTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  const relativePath = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
  const filePath = resolve(join(root, relativePath));
  if (!filePath.startsWith(root)) {
    response.writeHead(403);
    response.end("forbidden");
    return;
  }
  try {
    const body = await readFile(filePath);
    response.writeHead(200, { "content-type": mimeTypes.get(extname(filePath)) ?? "application/octet-stream" });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end("not found");
  }
});

await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
const address = server.address();
const url = `http://127.0.0.1:${address.port}/`;
const chromeCandidates = [
  process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
].filter(Boolean);
const executablePath = chromeCandidates.find((candidate) => existsSync(candidate));
const browser = await chromium.launch({ headless: true, executablePath });

try {
  const page = await browser.newPage();
  await page.goto(url);
  await page.waitForFunction(() => globalThis.__mcpAppsSmoke !== undefined, null, { timeout: 15000 });
  const smoke = await page.evaluate(() => globalThis.__mcpAppsSmoke);
  if (!smoke.ok) {
    throw new Error(smoke.error);
  }
  console.log(JSON.stringify(smoke.result));
} finally {
  await browser.close();
  await new Promise((resolveClose) => server.close(resolveClose));
}
