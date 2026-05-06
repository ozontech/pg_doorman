// Compute a deterministic SHA-256 over all source files that influence the
// vite build output. The hash is committed alongside `dist/` as
// `dist/.source-hash` so CI and pre-commit hook can detect drift between
// source changes and the shipped bundle without relying on byte-stable
// builds (esbuild native binaries differ across machines).

import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

const TRACKED_DIRS = ["src", "public"];
const TRACKED_FILES = [
  "index.html",
  "package.json",
  "package-lock.json",
  "vite.config.ts",
  "tsconfig.json",
];

function* walk(dir) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (entry.name === "node_modules") continue;
    if (entry.name.startsWith(".")) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walk(full);
    } else if (entry.isFile()) {
      yield full;
    }
  }
}

export function computeSourceHash() {
  const files = [];
  for (const dir of TRACKED_DIRS) {
    for (const file of walk(join(ROOT, dir))) files.push(file);
  }
  for (const file of TRACKED_FILES) {
    const abs = join(ROOT, file);
    try {
      if (statSync(abs).isFile()) files.push(abs);
    } catch {
      // Skip optional files.
    }
  }
  files.sort();

  const outer = createHash("sha256");
  for (const file of files) {
    const inner = createHash("sha256");
    inner.update(readFileSync(file));
    const rel = relative(ROOT, file).split(sep).join("/");
    outer.update(rel);
    outer.update("\0");
    outer.update(inner.digest("hex"));
    outer.update("\n");
  }
  return outer.digest("hex");
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.stdout.write(computeSourceHash() + "\n");
}
