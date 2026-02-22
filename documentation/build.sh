#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

VERSION=$(grep '^version = ' ../Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# Generate reference docs (EN + RU)
echo "Generating reference docs..."
cargo run --manifest-path ../Cargo.toml --bin pg_doorman -- generate-docs --all-languages -o en/src/reference

# Install plugin assets (CSS/JS files).
# The install commands modify book.toml, so we save and restore it.
install_plugins() {
    local dir=$1
    local toml="$dir/book.toml"
    cp "$toml" "$toml.orig"
    (cd "$dir" && mdbook-admonish install .)
    mv "$toml.orig" "$toml"
}

install_plugins en
install_plugins ru

# Mermaid only for EN book
cp en/book.toml en/book.toml.orig
(cd en && mdbook-mermaid install .)
mv en/book.toml.orig en/book.toml

# Version injection in EN index
sed -i.bak "s/{{VERSION}}/$VERSION/g" en/src/index.md

# Build both books
(cd en && mdbook build)
(cd ru && mdbook build)

# Restore template
mv en/src/index.md.bak en/src/index.md

echo "Build complete. Output in book/en/ and book/ru/"
