// Fails the build when the emitted declaration bundle is missing, empty, or
// export-free — vite-plugin-dts has silently produced an empty lib.d.ts
// before (dataflow-ui v2.1.3). Runs via the `postbuild:lib` hook so every
// producer of the artifact (CI, release.yml, manual publishes) is covered.
import { readFileSync } from 'node:fs'

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
