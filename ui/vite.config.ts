import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import wasm from 'vite-plugin-wasm';
import path from 'path';
import pkg from './package.json' with { type: 'json' };

export default defineConfig(({ command }) => ({
  plugins: [react(), wasm()],
  // Baked in so the engine version handshake has something to compare against.
  // Must stay in sync with vite.lib.config.ts.
  define: {
    __DATAFLOW_UI_VERSION__: JSON.stringify(pkg.version),
  },
  // Use /dataflow-rs/debugger/ base path for production build (GitHub Pages)
  base: command === 'build' ? '/dataflow-rs/debugger/' : '/',
  server: {
    port: 3000,
    fs: {
      // Allow serving files from parent directories (for local linked packages)
      allow: [
        path.resolve(__dirname, '..'),  // dataflow-rs root
        path.resolve(__dirname, '../../datalogic-rs'),  // datalogic-rs for @goplasmatic/datalogic-ui
      ],
    },
  },
  optimizeDeps: {
    // Exclude WASM packages from pre-bundling
    exclude: ['@goplasmatic/dataflow-wasm', '@goplasmatic/datalogic'],
  },
}));
