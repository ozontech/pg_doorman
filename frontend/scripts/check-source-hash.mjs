// Compare the current source hash against `dist/.source-hash`. Exits with
// status 1 and a remediation message when the two disagree, so the
// pre-commit hook and CI both fail on stale `dist/`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { computeSourceHash } from "./source-hash.mjs";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const computed = computeSourceHash();
let committed;
try {
  committed = readFileSync(join(ROOT, "dist", ".source-hash"), "utf8").trim();
} catch {
  process.stderr.write("frontend/dist/.source-hash is missing.\n");
  process.stderr.write("Rebuild and commit dist:\n");
  process.stderr.write("  cd frontend && npm run build && git add dist\n");
  process.exit(1);
}
if (computed !== committed) {
  process.stderr.write("frontend/dist/ is out of sync with frontend/src/.\n");
  process.stderr.write(`  expected source-hash: ${computed}\n`);
  process.stderr.write(`  found in dist:        ${committed}\n\n`);
  process.stderr.write("Rebuild and commit dist:\n");
  process.stderr.write("  cd frontend && npm run build && git add dist\n");
  process.exit(1);
}
process.stdout.write(`source-hash OK: ${computed}\n`);
