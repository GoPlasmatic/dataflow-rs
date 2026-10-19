// Run with alternating package directories to compare definition loading:
// node --expose-gc wasm/scripts/bench-definition-loading.mjs [package-dir]
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const dir = resolve(process.argv[2] ?? 'ui/node_modules/@goplasmatic/dataflow-wasm');
const wasm = await import(pathToFileURL(resolve(dir, 'dataflow_wasm.js')));
await wasm.default({ module_or_path: readFileSync(resolve(dir, 'dataflow_wasm_bg.wasm')) });
const definitions = [{
  id: 'large-definition', continue_on_error: true,
  ext: { document: 'x'.repeat(1024 * 1024) },
  tasks: Array.from({ length: 64 }, (_, i) => ({
    id: String(i), function: { name: 'log', input: { message: 'loaded' } },
  })),
}];
function load() {
  const engine = new wasm.WasmEngine(definitions);
  assert.equal(engine.workflow_count(), 1);
  engine.free();
}
for (let i = 0; i < 5; i++) load();
global.gc?.();
const before = process.memoryUsage();
const start = performance.now();
for (let i = 0; i < 100; i++) load();
const elapsed = performance.now() - start;
global.gc?.();
const after = process.memoryUsage();
console.log(JSON.stringify({ loads: 100, definition_kib: 1024,
  elapsed_ms: elapsed, rss_delta_mib: (after.rss - before.rss) / 2 ** 20 }));
