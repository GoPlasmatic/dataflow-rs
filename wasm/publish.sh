#!/bin/bash

# Publish script for @goplasmatic/dataflow-wasm
set -e

echo "Building WASM package..."
wasm-pack build --target web --out-dir pkg

echo "Publishing to npm..."
cd pkg
npm publish --access public

echo "Done! @goplasmatic/dataflow-wasm published successfully."
