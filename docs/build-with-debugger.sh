#!/bin/bash
# Build docs with embedded debugger UI

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Building UI..."
cd "$PROJECT_ROOT/ui"
npm run build

echo "Building mdbook..."
cd "$PROJECT_ROOT"
mdbook build docs

echo "Copying debugger to docs..."
mkdir -p docs/book/debugger
cp -r ui/dist/* docs/book/debugger/

echo ""
echo "Build complete!"
echo "  Docs: docs/book/"
echo "  Debugger: docs/book/debugger/"
