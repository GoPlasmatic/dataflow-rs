#!/bin/bash

# Publish script for @goplasmatic/dataflow-ui
set -e

echo "Building @goplasmatic/dataflow-ui..."

# Build the library
npm run build:lib

echo "Publishing to npm..."
npm publish --access public

echo "Done! @goplasmatic/dataflow-ui published successfully."
