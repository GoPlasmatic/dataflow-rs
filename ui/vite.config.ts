import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';
import path from 'path';

export default defineConfig(({ command }) => ({
  plugins: [react(), wasm(), topLevelAwait()],
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
