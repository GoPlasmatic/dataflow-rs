// Fails the build when the emitted declaration bundle is missing, empty, or
// export-free — vite-plugin-dts has silently produced an empty lib.d.ts
// before (dataflow-ui v2.1.3). Runs via the `postbuild:lib` hook so every
// producer of the artifact (CI, release.yml, manual publishes) is covered.
import { readFileSync, readdirSync } from 'node:fs'
import { createRequire, registerHooks } from 'node:module'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const path = 'dist/lib.d.ts'
let content
try {
  content = readFileSync(path, 'utf8')
} catch {
  console.error(`${path} is missing`)
  process.exit(1)
}

const exportLines = content.split('\n').filter((line) => line.includes('export')).length
if (exportLines === 0) {
  console.error(`${path} contains no export declarations:\n${content}`)
  process.exit(1)
}
console.log(`${path} OK (${content.length} bytes, ${exportLines} export lines)`)

// A split library needs compatible chunk extensions in both formats. Load
// the actual artifacts so an ESM chunk accidentally required by CJS fails here.
// A browser bundler handles the external stylesheets; Node skips only CSS.
registerHooks({
  load(url, context, nextLoad) {
    if (url.endsWith('.css')) return { format: 'module', source: '', shortCircuit: true }
    return nextLoad(url, context)
  },
})
const require = createRequire(import.meta.url)
const esm = await import(pathToFileURL(resolve('dist/index.js')))
const cjs = require(resolve('dist/index.cjs'))
for (const [format, entry] of [['ESM', esm], ['CJS', cjs]]) {
  for (const name of ['WorkflowVisualizer', 'defaultEngineFactory', 'useDebugger']) {
    if (typeof entry[name] !== 'function') throw new Error(`${format}: missing ${name}`)
  }
}
for (const file of readdirSync('dist/chunks')) {
  const path = resolve('dist/chunks', file)
  if (file.endsWith('.cjs')) require(path)
  else if (file.endsWith('.js')) await import(pathToFileURL(path))
}
console.log('ESM and CommonJS entries and chunks load successfully')
