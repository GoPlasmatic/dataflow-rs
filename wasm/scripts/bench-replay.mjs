// Compare eager snapshots with a replay cursor in separate Node processes:
// node --expose-gc wasm/scripts/bench-replay.mjs snapshots [package-dir]
// node --expose-gc wasm/scripts/bench-replay.mjs cursor [package-dir]
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const mode = process.argv[2] ?? 'snapshots';
assert.ok(['snapshots', 'cursor'].includes(mode));
const dir = resolve(process.argv[3] ?? 'ui/node_modules/@goplasmatic/dataflow-wasm');
const wasm = await import(pathToFileURL(resolve(dir, 'dataflow_wasm.js')));
await wasm.default({ module_or_path: readFileSync(resolve(dir, 'dataflow_wasm_bg.wasm')) });
const trace = {
  initial: { data: { large: 'x'.repeat(256 * 1024), counter: -1 }, metadata: {}, temp_data: {} },
  steps: Array.from({ length: 256 }, (_, i) => ({
    workflow: 0, workflow_id: 'w', task: i, task_id: String(i),
    result: 'executed', status: 200,
    changes: [{ path: 'data.counter', old_value: i - 1, new_value: i }],
    mapping_ends: [1],
  })),
  truncated: false,
};
global.gc?.();
const before = process.memoryUsage();
const start = performance.now();
let kept;
if (mode === 'snapshots') {
  kept = wasm.contexts(trace);
  kept.forEach((context, i) => assert.equal(context.data.counter, i));
} else {
  kept = new wasm.TraceReplay(trace);
  for (let i = 0; i < trace.steps.length; i++) {
    assert.equal(kept.context_at(i).data.counter, i);
  }
  assert.equal(kept.context_at(0).data.counter, 0);
  assert.equal(kept.context_at(255).data.counter, 255);
}
const elapsed = performance.now() - start;
global.gc?.();
const after = process.memoryUsage();
console.log(JSON.stringify({
  mode, steps: trace.steps.length, context_kib: 256,
  elapsed_ms: elapsed,
  rss_delta_mib: (after.rss - before.rss) / 2 ** 20,
  heap_delta_mib: (after.heapUsed - before.heapUsed) / 2 ** 20,
}));
if (mode === 'cursor') kept.free();
