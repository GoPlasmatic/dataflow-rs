// Fails the build when the wasm-pack output is internally inconsistent — the
// binary and its JS glue disagree, so the package cannot initialize in a
// browser. Every published @goplasmatic/dataflow-wasm from 2.1.3 through 3.2.0
// shipped broken this way and nothing caught it, because `wasm-pack build`
// succeeds and `npm publish` has no idea the artifact is dead on arrival.
//
// Runs from release.yml before publishing, and from ci.yml on every build.
// Usage: node scripts/verify-wasm.mjs [pkg-dir]
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const pkgDir = process.argv[2] ?? 'pkg';

function fail(msg) {
  console.error(`\n✗ ${msg}`);
  process.exitCode = 1;
}

const files = readdirSync(pkgDir);
const wasmName = files.find((f) => f.endsWith('_bg.wasm'));
const glueName = files.find((f) => f.endsWith('.js') && !f.endsWith('_bg.js'));

if (!wasmName || !glueName) {
  console.error(`✗ ${pkgDir}: expected a *_bg.wasm and a glue .js, found: ${files.join(', ')}`);
  process.exit(1);
}

const wasmPath = join(pkgDir, wasmName);
const gluePath = join(pkgDir, glueName);
const bytes = readFileSync(wasmPath);
const glue = readFileSync(gluePath, 'utf8');
const module = new WebAssembly.Module(bytes);

console.log(`verifying ${pkgDir}/ (${wasmName}, ${(bytes.length / 1024).toFixed(0)} KiB)`);

// Stub every declared import so the module can be instantiated in Node.
const imports = {};
for (const imp of WebAssembly.Module.imports(module)) {
  imports[imp.module] ??= {};
  if (imp.kind === 'function') imports[imp.module][imp.name] = () => {};
  else if (imp.kind === 'memory') imports[imp.module][imp.name] = new WebAssembly.Memory({ initial: 1 });
  else if (imp.kind === 'global') imports[imp.module][imp.name] = new WebAssembly.Global({ value: 'i32', mutable: true }, 0);
  else if (imp.kind === 'table') imports[imp.module][imp.name] = new WebAssembly.Table({ initial: 1, element: 'anyfunc' });
}
const instance = new WebAssembly.Instance(module, imports);

// --- Check 1: the externref table must be growable ---------------------------
// `__wbindgen_init_externref_table` in the glue unconditionally calls
// `table.grow(4)` to install undefined/null/true/false. A binary whose table
// declares maximum == initial makes that throw RangeError on the very first
// init, so the package can never start. This is what an older binaryen does to
// the table when it processes the module without reference-types enabled.
const GROW_BY = 4;
const tableEntry = Object.entries(instance.exports).find(([, v]) => v instanceof WebAssembly.Table);

if (!tableEntry) {
  fail('no exported table found — cannot verify externref growability');
} else {
  const [tableName, table] = tableEntry;
  try {
    table.grow(GROW_BY);
    console.log(`  ✓ ${tableName} grew by ${GROW_BY} (length was ${table.length - GROW_BY})`);
  } catch (err) {
    fail(
      `${tableName} cannot grow by ${GROW_BY} (length=${table.length}, maximum appears pinned).\n` +
        `  ${glueName} calls table.grow(${GROW_BY}) during init, so this package would throw\n` +
        `  "${err.constructor.name}: ${err.message}"\n` +
        `  in every browser. Check the wasm-opt/binaryen version and that\n` +
        `  --enable-reference-types is in effect.`,
    );
  }
}

// --- Check 2: the glue and the binary must agree on exports ------------------
// Catches a mismatched pair — e.g. glue from one wasm-bindgen build against a
// binary from another, where the mangled closure-shim names no longer line up.
const exported = new Set(WebAssembly.Module.exports(module).map((e) => e.name));
const referenced = new Set(
  [...glue.matchAll(/(?<![A-Za-z0-9_$])wasm\.([A-Za-z_$][A-Za-z0-9_$]*)/g)].map((m) => m[1]),
);
const missing = [...referenced].filter((name) => !exported.has(name));

if (missing.length > 0) {
  fail(
    `${glueName} calls ${missing.length} export(s) the binary does not provide:\n` +
      missing.map((n) => `    ${n.length > 80 ? n.slice(0, 80) + '…' : n}`).join('\n') +
      `\n  The glue and the binary are from different builds.`,
  );
} else {
  console.log(`  ✓ all ${referenced.size} glue-referenced exports exist in the binary`);
}

if (process.exitCode) {
  console.error('\nRefusing to publish a broken wasm artifact.');
} else {
  console.log('\n✓ wasm artifact is internally consistent');
}
