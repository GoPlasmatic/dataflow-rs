#!/usr/bin/env bash
#
# Builds the documentation book and the embedded debugger UI from this
# checkout's WASM engine, verifying the WASM artifact first. Used both locally
# and by .github/workflows/docs.yml.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

echo "Building and overlaying local WASM into UI..."
(cd ui && npm run wasm:local)

echo "Copying WASM to docs playground..."
mkdir -p docs/src/wasm
cp wasm/pkg/dataflow_wasm.js docs/src/wasm/
cp wasm/pkg/dataflow_wasm_bg.wasm docs/src/wasm/

echo "Building Debugger UI..."
(cd ui && npm run build)

echo "Building mdBook..."
mdbook build docs

echo "Copying debugger to docs/book/debugger..."
mkdir -p docs/book/debugger
cp -r ui/dist/* docs/book/debugger/

echo ""
echo "Build complete!"
echo "  Docs:     docs/book/"
echo "  Debugger: docs/book/debugger/"
