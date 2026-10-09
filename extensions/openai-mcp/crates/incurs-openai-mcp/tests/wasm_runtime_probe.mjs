import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const wasmPath = resolve(
  here,
  "../../../target/wasm32-unknown-unknown/debug/examples/openai_mcp_wasm_runtime.wasm",
);
const outDir = mkdtempSync(resolve(tmpdir(), "incurs-openai-server-wasm-bindgen-"));
try {
  const wasmBindgen = process.env.WASM_BINDGEN ?? "wasm-bindgen";
  run(wasmBindgen, [wasmPath, "--target", "web", "--out-dir", outDir]);
  const jsPath = resolve(outDir, "openai_mcp_wasm_runtime.js");
  const bgPath = resolve(outDir, "openai_mcp_wasm_runtime_bg.wasm");
  const generated = await import(pathToFileURL(jsPath).href);
  const wasmBytes = await readFile(bgPath);
  const exports = generated.initSync({ module: wasmBytes });
  const smoke =
    exports.openai_mcp_server_wasm_smoke ??
    generated.openai_mcp_server_wasm_smoke;
  if (typeof smoke !== "function") {
    throw new Error("generated wasm-bindgen module did not export smoke function");
  }
  const result = smoke();
  if (result !== 0) {
    throw new Error(`openai_mcp_server_wasm_smoke failed with ${result}`);
  }
} finally {
  if (!process.env.INCURS_OPENAI_SERVER_WASM_KEEP_TEMP) {
    rmSync(outDir, { recursive: true, force: true });
  }
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
  }
}
