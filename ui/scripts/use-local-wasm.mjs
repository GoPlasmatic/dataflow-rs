// Overlays a locally built wasm package onto the installed one, so the
// debugger runs against this checkout's engine instead of the last npm
// release.
//
// This mirrors what release.yml's publish-ui job already does: the dependency
// stays pinned to a *published* version so `npm ci` can resolve the lockfile,
// and the fresh build is copied over node_modules. Using a `file:` dependency
// instead would break that resolution.
//
// Needed whenever the engine has unreleased changes — a workflow field the
// published wasm has never heard of is silently ignored by serde, so the
// debugger runs but quietly does the wrong thing.
//
// Usage: npm run wasm:local   (builds, verifies, then overlays)
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';

const SRC = '../wasm/pkg';
const DEST = 'node_modules/@goplasmatic/dataflow-wasm';
const FILES = ['dataflow_wasm.js', 'dataflow_wasm_bg.wasm', 'dataflow_wasm.d.ts'];
const VITE_CACHE = 'node_modules/.vite';

if (!existsSync(SRC)) {
  console.error(
    `✗ ${SRC} not found.\n` +
      `  Build it first:  wasm-pack build ../wasm --target web --out-dir pkg --release`,
  );
  process.exit(1);
}

if (!existsSync(DEST)) {
  console.error(`✗ ${DEST} not found. Run \`npm ci\` first.`);
  process.exit(1);
}

// Refuse to overlay a broken build — the same guard the release pipeline runs.
const verify = spawnSync('node', ['../wasm/scripts/verify-wasm.mjs', SRC], { stdio: 'inherit' });
if (verify.status !== 0) {
  console.error('\n✗ Local wasm build failed verification; not overlaying it.');
  process.exit(1);
}

for (const file of FILES) {
  const from = join(SRC, file);
  if (!existsSync(from)) {
    console.error(`✗ ${from} missing from the build output`);
    process.exit(1);
  }
  copyFileSync(from, join(DEST, file));
  console.log(`  copied ${file} (${(statSync(from).size / 1024).toFixed(0)} KiB)`);
}

// Vite keys its dependency cache off the lockfile, which does not change when
// files are overwritten in place. Without this the dev server keeps serving
// the previous glue against the new binary — which fails in a far more
// confusing way than the original error, because init succeeds and only a
// later mangled-name lookup throws.
if (existsSync(VITE_CACHE)) {
  rmSync(VITE_CACHE, { recursive: true, force: true });
  console.log('  cleared node_modules/.vite');
}

console.log(
  `\n✓ ${DEST} now holds this checkout's engine.\n` +
    '  Restart `npm run dev` and hard-reload the browser (Cmd/Ctrl+Shift+R).\n' +
    '  Re-run this after any engine change, and after every `npm ci`/`npm install`.',
);
