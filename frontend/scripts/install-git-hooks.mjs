// Point the parent repository's `core.hooksPath` at the project-level
// `.githooks` directory so the dist-sync pre-commit hook runs locally.
// Silent no-op when run outside a git checkout (e.g. CI cache restore,
// Docker build) — CI re-checks via `npm run check-dist`.

import { execFileSync } from "node:child_process";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));

try {
  execFileSync("git", ["rev-parse", "--git-dir"], {
    cwd: REPO_ROOT,
    stdio: "ignore",
  });
} catch {
  process.exit(0);
}

try {
  execFileSync("git", ["config", "--local", "core.hooksPath", ".githooks"], {
    cwd: REPO_ROOT,
    stdio: "ignore",
  });
  process.stdout.write("git core.hooksPath -> .githooks\n");
} catch (err) {
  process.stderr.write(`warning: failed to configure git hooks: ${err.message}\n`);
  process.exit(0);
}
