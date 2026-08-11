// Overlays a locally built wasm package onto the installed one, so the
// debugger runs against this checkout's engine instead of the last npm
// release.
//
// The dependency stays pinned to a *published* version so `npm ci` can resolve
// the lockfile before a matching engine exists; a `file:` dependency would
// break that resolution. This script is what bridges the gap, and ci.yml's ui
// job runs it for the same reason a developer does. release.yml deliberately
// does the opposite — it installs the engine it has just published, so the
// release validates what consumers actually receive.
//
// Needed whenever the engine has unreleased changes — a workflow field the
// published wasm has never heard of is silently ignored by serde, so the
// debugger runs but quietly does the wrong thing.
//
// Usage: npm run wasm:local   (builds, verifies, then overlays)
//
// `predev` runs it with --if-available, which downgrades a missing wasm-pack
// from an error to a warning: a contributor touching only `ui/` should not
// need the Rust toolchain to start the dev server. The engine version
// handshake in src/engines/versionCheck.ts is the backstop for that case — it
// throws at execution time if the installed engine predates this build.
//
// It always rebuilds rather than reusing whatever is in wasm/pkg. A stale
// build usually carries the *same* version as the checkout, so the handshake
// cannot see it; only rebuilding keeps "the engine I am debugging" equal to
// "the engine I am editing". wasm-pack no-ops in ~2s when nothing changed.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';

const optional = process.argv.includes('--if-available');
const SRC = '../wasm/pkg';
const DEST = 'node_modules/@goplasmatic/dataflow-wasm';
const FILES = ['dataflow_wasm.js', 'dataflow_wasm_bg.wasm', 'dataflow_wasm.d.ts'];
const VITE_CACHE = 'node_modules/.vite';

const build = spawnSync(
  'wasm-pack',
  ['build', '../wasm', '--target', 'web', '--out-dir', 'pkg', '--release'],
  { stdio: 'inherit' },
);

if (build.error?.code === 'ENOENT') {
  const message =
    'wasm-pack is not installed, so this checkout\'s engine cannot be built.\n' +
    '  Install it from https://drager.github.io/wasm-pack/installer/';
  if (optional) {
    console.warn(
      `⚠ ${message}\n` +
        '  Continuing against the installed @goplasmatic/dataflow-wasm. If it\n' +
        '  predates the engine in this checkout, running a workflow fails with a\n' +
        '  version mismatch rather than silently misbehaving.',
    );
    process.exit(0);
  }
  console.error(`✗ ${message}`);
  process.exit(1);
}

if (build.status !== 0) {
  console.error('\n✗ wasm-pack build failed; leaving the installed package alone.');
  process.exit(1);
}

if (!existsSync(SRC)) {
  console.error(`✗ ${SRC} not found even though the build reported success.`);
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
