#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

VERSION=$(grep '^version = ' ../Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# Generate reference docs (EN — same source as RU, translation lives in ru/src/reference)
echo "Generating EN reference docs..."
cargo run --manifest-path ../Cargo.toml --bin pg_doorman -- generate-docs -o en/src/reference

# Install plugin assets (CSS/JS files) for both books.
# The install commands modify book.toml, so we save and restore it.
for lang in en ru; do
    cp $lang/book.toml $lang/book.toml.orig
    (cd $lang && mdbook-admonish install . && mdbook-mermaid install .)
    mv $lang/book.toml.orig $lang/book.toml
done

# Version injection in landing pages (EN + RU)
sed -i.bak "s/{{VERSION}}/$VERSION/g" en/src/index.md
sed -i.bak "s/{{VERSION}}/$VERSION/g" ru/src/index.md 2>/dev/null || true

# Build EN first — it cleans the entire book/ directory.
(cd en && mdbook build)

# Build RU into book/ru/ — won't disturb EN content.
if [ -f ru/src/SUMMARY.md ]; then
    # RU reference is hand-translated for now — no autogen yet.
    # If ru/src/reference is missing, fall back to EN copy so the build does not break.
    if [ ! -f ru/src/reference/general.md ]; then
        rm -rf ru/src/reference
        cp -r en/src/reference ru/src/reference
    fi
    (cd ru && mdbook build)
else
    echo "Skipping RU build: ru/src/SUMMARY.md not found."
fi

# Restore index.md templates
mv en/src/index.md.bak en/src/index.md
mv ru/src/index.md.bak ru/src/index.md 2>/dev/null || true

echo "Build complete. Output in book/ (EN at root, RU at book/ru/)"
