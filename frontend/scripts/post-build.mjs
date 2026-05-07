// Post-build cleanup for the SPA bundle:
//   1) drop legacy .woff font files (modern browsers support .woff2 since 2014);
//   2) strip the matching `url(...woff) format("woff")` entries from generated
//      CSS so the browser does not 404-request them;
//   3) copy the JetBrains Mono OFL-1.1 license next to the bundled fonts;
//   4) replace every compressible asset (.js / .css / .html / .svg) with its
//      gzipped counterpart so include_dir!() embeds only the pre-compressed
//      form. Browsers that advertise gzip get the bytes verbatim; the rare
//      non-gzip client gets a flate2 decompression on the server side.

import { copyFileSync, readdirSync, readFileSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

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

const LICENSE_DEST = join(ROOT, "dist", "JETBRAINS_MONO_OFL.txt");
const LICENSE_SRC = join(ROOT, "node_modules", "@fontsource", "jetbrains-mono", "LICENSE");
try {
  copyFileSync(LICENSE_SRC, LICENSE_DEST);
} catch (err) {
  process.stderr.write(`warning: could not copy JetBrains Mono OFL license: ${err.message}\n`);
}

// Pre-gzip every compressible asset and delete the original. The web
// listener serves the bytes verbatim with `Content-Encoding: gzip` to
// browsers (which all advertise gzip) and decompresses with flate2 for
// the rare client that does not. The .source-hash file stays excluded
// — it is a CI artifact and gzipping it adds noise.
const COMPRESSIBLE_EXT = [".js", ".css", ".html", ".svg"];
const GZIP_EXCLUDE = new Set([".source-hash"]);
const DIST_ROOT = join(ROOT, "dist");

let gzipFiles = 0;
let gzipFromBytes = 0;
let gzipToBytes = 0;
function gzipDir(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      gzipDir(full);
      continue;
    }
    if (GZIP_EXCLUDE.has(entry.name)) continue;
    if (!COMPRESSIBLE_EXT.some((ext) => entry.name.endsWith(ext))) continue;
    const raw = readFileSync(full);
    const gz = gzipSync(raw, { level: 9 });
    writeFileSync(full + ".gz", gz);
    unlinkSync(full);
    gzipFiles += 1;
    gzipFromBytes += raw.length;
    gzipToBytes += gz.length;
  }
}
gzipDir(DIST_ROOT);

process.stdout.write(
  `post-build: removed ${removed} .woff files (${(bytes / 1024).toFixed(1)} KB), ` +
    `patched ${cssTouched} CSS files, copied OFL license, ` +
    `pre-gzipped ${gzipFiles} files (${(gzipFromBytes / 1024).toFixed(1)} KB → ${(gzipToBytes / 1024).toFixed(1)} KB).\n`,
);
