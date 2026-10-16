import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import pkg from './package.json' with { type: 'json' };

// The tests run against the engine `npm run wasm:local` overlaid into
// node_modules (ci.yml's ui job does that first): src/test/setup.ts
// initializes the wasm module from its bytes, since there is no browser to
// fetch it. The define mirrors both vite configs, so the version handshake
// has something to compare against.
export default defineConfig({
  plugins: [react()],
  define: {
    __DATAFLOW_UI_VERSION__: JSON.stringify(pkg.version),
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
