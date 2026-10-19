// Check the built import graph: optional editors and diagrams must stay lazy.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { gzipSync } from 'node:zlib';

const manifest = JSON.parse(readFileSync('dist/.vite/manifest.json', 'utf8'));
const initial = new Set();
function visit(key) {
  if (initial.has(key)) return;
  initial.add(key);
  for (const dependency of manifest[key].imports ?? []) visit(dependency);
}
for (const [key, chunk] of Object.entries(manifest)) {
  if (chunk.isEntry) visit(key);
}
assert.ok(initial.size > 0, 'no application entry found');
for (const suffix of [
  '@goplasmatic/datalogic-ui/dist/index.js',
  '/flow-diagram/WorkflowFlowDiagram.tsx',
  '/flow-diagram/GroupFlowDiagram.tsx',
]) {
  const key = Object.keys(manifest).find((key) => key.endsWith(suffix));
  assert.ok(key && manifest[key].isDynamicEntry, `${suffix} must have a lazy entry`);
  assert.ok(!initial.has(key), `${suffix} is loaded at startup`);
}
let bytes = 0;
let gzip = 0;
for (const key of initial) {
  const data = readFileSync(resolve('dist', manifest[key].file));
  bytes += data.length;
  gzip += gzipSync(data).length;
}
console.log(`Initial JavaScript: ${(bytes / 1000).toFixed(1)} kB, ${(gzip / 1000).toFixed(1)} kB gzip; editor and diagrams deferred`);
