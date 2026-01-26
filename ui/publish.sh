#!/bin/bash

# Publish script for @goplasmatic/dataflow-ui
set -e

echo "Building @goplasmatic/dataflow-ui..."

# Temporarily update package.json to use npm dependencies instead of file: references
# This is needed because npm publish doesn't support file: dependencies
echo "Updating dependencies for npm publish..."

# Backup original package.json
cp package.json package.json.backup

# Use node to update the dependencies
node -e "
const fs = require('fs');
const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8'));
pkg.dependencies['@goplasmatic/dataflow-wasm'] = '^2.0.4';
pkg.dependencies['@goplasmatic/datalogic-ui'] = '^4.0.9';
fs.writeFileSync('package.json', JSON.stringify(pkg, null, 2) + '\n');
"

# Build the library
npm run build:lib

echo "Publishing to npm..."
npm publish --access public

# Restore original package.json for local development
echo "Restoring development package.json..."
mv package.json.backup package.json

echo "Done! @goplasmatic/dataflow-ui published successfully."
