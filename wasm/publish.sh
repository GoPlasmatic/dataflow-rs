#!/bin/bash

# Publish script for @goplasmatic/dataflow-wasm
set -e

echo "Building WASM package..."
wasm-pack build --target web --out-dir pkg

echo "Patching package.json for npm..."
cd pkg

# Update package.json with correct npm configuration
cat > package.json << 'EOF'
{
  "name": "@goplasmatic/dataflow-wasm",
  "type": "module",
  "author": "Plasmatic Engineering <shankar@goplasmatic.io>",
  "description": "WebAssembly bindings for dataflow-rs workflow engine",
  "version": "2.0.4",
  "license": "Apache-2.0",
  "homepage": "https://github.com/GoPlasmatic/dataflow-rs",
  "repository": {
    "type": "git",
    "url": "https://github.com/GoPlasmatic/dataflow-rs",
    "directory": "wasm"
  },
  "bugs": {
    "url": "https://github.com/GoPlasmatic/dataflow-rs/issues"
  },
  "files": [
    "dataflow_wasm_bg.wasm",
    "dataflow_wasm.js",
    "dataflow_wasm.d.ts",
    "README.md"
  ],
  "main": "dataflow_wasm.js",
  "types": "dataflow_wasm.d.ts",
  "sideEffects": [
    "./snippets/*"
  ],
  "keywords": [
    "workflow",
    "engine",
    "wasm",
    "webassembly",
    "dataflow",
    "rust"
  ]
}
EOF

# Copy README
cp ../README.md ./README.md

echo "Publishing to npm..."
npm publish --access public

echo "Done! @goplasmatic/dataflow-wasm published successfully."
