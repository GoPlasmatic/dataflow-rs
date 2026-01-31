#!/bin/bash

# Publish script for @goplasmatic/dataflow-wasm
set -e

echo "Building WASM package..."
wasm-pack build --target web --out-dir pkg

echo "Extracting metadata from Cargo.toml..."
CARGO_TOML="Cargo.toml"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$CARGO_TOML")
DESCRIPTION=$(sed -n 's/^description = "\(.*\)"/\1/p' "$CARGO_TOML")
LICENSE=$(sed -n 's/^license = "\(.*\)"/\1/p' "$CARGO_TOML")
REPOSITORY=$(sed -n 's/^repository = "\(.*\)"/\1/p' "$CARGO_TOML")
AUTHOR=$(sed -n 's/^authors = \["\(.*\)"\]/\1/p' "$CARGO_TOML")
# Extract keywords from Cargo.toml and append npm-specific ones
CARGO_KEYWORDS=$(sed -n 's/^keywords = \[\(.*\)\]/\1/p' "$CARGO_TOML")
KEYWORDS="${CARGO_KEYWORDS}, \"dataflow\", \"rust\""

echo "  Version: $VERSION"
echo "  Description: $DESCRIPTION"

echo "Patching package.json for npm..."
cd pkg

# Generate package.json from Cargo.toml metadata
cat > package.json << EOF
{
  "name": "@goplasmatic/dataflow-wasm",
  "type": "module",
  "author": "$AUTHOR",
  "description": "$DESCRIPTION",
  "version": "$VERSION",
  "license": "$LICENSE",
  "homepage": "$REPOSITORY",
  "repository": {
    "type": "git",
    "url": "$REPOSITORY",
    "directory": "wasm"
  },
  "bugs": {
    "url": "${REPOSITORY}/issues"
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
  "keywords": [${KEYWORDS}]
}
EOF

# Copy README
cp ../README.md ./README.md

echo "Publishing to npm..."
npm publish --access public

echo "Done! @goplasmatic/dataflow-wasm published successfully."
