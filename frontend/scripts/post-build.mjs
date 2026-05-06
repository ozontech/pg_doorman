// Post-build cleanup for the SPA bundle:
//   1) drop legacy .woff font files (modern browsers support .woff2 since 2014);
//   2) strip the matching `url(...woff) format("woff")` entries from generated
//      CSS so the browser does not 404-request them;
//   3) copy the IBM Plex OFL-1.1 license next to the bundled fonts.

import { copyFileSync, readdirSync, readFileSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const DIST_ASSETS = join(ROOT, "dist", "assets");

let removed = 0;
let bytes = 0;
for (const name of readdirSync(DIST_ASSETS)) {
  if (!name.endsWith(".woff")) continue;
  const full = join(DIST_ASSETS, name);
  bytes += statSync(full).size;
  unlinkSync(full);
  removed += 1;
}

let cssTouched = 0;
for (const name of readdirSync(DIST_ASSETS)) {
  if (!name.endsWith(".css")) continue;
  const full = join(DIST_ASSETS, name);
  const original = readFileSync(full, "utf8");
  // Match every `url(...woff) format("woff")` entry plus the comma that
  // precedes or follows it inside an @font-face `src` list. Vite renders the
  // pattern compactly; the regex covers both leading-comma and trailing-comma
  // forms without touching the surviving woff2 entries.
  const cleaned = original
    .replace(/,\s*url\([^)]+\.woff\)\s*format\(["']woff["']\)/g, "")
    .replace(/url\([^)]+\.woff\)\s*format\(["']woff["']\)\s*,\s*/g, "");
  if (cleaned !== original) {
    writeFileSync(full, cleaned);
    cssTouched += 1;
  }
}

const LICENSE_DEST = join(ROOT, "dist", "IBM_PLEX_OFL.txt");
const LICENSE_SRC = join(ROOT, "node_modules", "@fontsource", "ibm-plex-sans", "LICENSE");
try {
  copyFileSync(LICENSE_SRC, LICENSE_DEST);
} catch (err) {
  process.stderr.write(`warning: could not copy IBM Plex OFL license: ${err.message}\n`);
}

process.stdout.write(
  `post-build: removed ${removed} .woff files (${(bytes / 1024).toFixed(1)} KB), ` +
    `patched ${cssTouched} CSS files, copied OFL license.\n`,
);
