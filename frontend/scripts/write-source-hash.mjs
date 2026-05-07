// Write the current source hash into `dist/.source-hash`. Invoked by
// `npm run build` after `vite build` succeeds.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { computeSourceHash } from "./source-hash.mjs";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const hash = computeSourceHash();
writeFileSync(join(ROOT, "dist", ".source-hash"), hash + "\n", "utf8");
process.stdout.write(`source-hash: ${hash}\n`);
