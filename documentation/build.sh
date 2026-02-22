#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

VERSION=$(grep '^version = ' ../Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# Generate reference docs
echo "Generating reference docs..."
cargo run --manifest-path ../Cargo.toml --bin pg_doorman -- generate-docs -o en/src/reference

# Install plugin assets (CSS/JS files).
# The install commands modify book.toml, so we save and restore it.
cp en/book.toml en/book.toml.orig
(cd en && mdbook-admonish install . && mdbook-mermaid install .)
mv en/book.toml.orig en/book.toml

# Version injection in EN index
sed -i.bak "s/{{VERSION}}/$VERSION/g" en/src/index.md

# Build
(cd en && mdbook build)

# Restore template
mv en/src/index.md.bak en/src/index.md

echo "Build complete. Output in book/"
