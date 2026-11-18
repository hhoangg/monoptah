// Fails when web/pnpm-lock.yaml holds more than one resolved variant of vitest or vite. A new dependency with its own
// peer set (workbox-build pulled in terser once) can split them, and the worker and app suites then fail with
// "Vitest failed to find the runner". The fix is `pnpm dedupe`, run from web/ (W-12 follow-up, final review 2 M14).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const lockfile = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "pnpm-lock.yaml");
const lines = readFileSync(lockfile, "utf8").split("\n");

// The `snapshots:` section lists one entry per resolved variant, `  name@version(peer)(peer):`. The `packages:` section has
// one entry per version without peers, so only the snapshot entries are counted.
// The file may start with a small document for pnpm itself (config dependencies); the project lockfile is the last one.
const start = lines.lastIndexOf("snapshots:");
if (start === -1) {
  console.error("check-lockfile: no snapshots section in pnpm-lock.yaml (unexpected lockfile format)");
  process.exit(1);
}
const problems = [];
for (const name of ["vitest", "vite"]) {
  const pattern = new RegExp(`^  ${name}@[^(\\s]+(\\(.*\\))?:$`);
  const variants = lines.slice(start).filter((line) => pattern.test(line));
  if (variants.length > 1) problems.push(`${name} has ${variants.length} variants in pnpm-lock.yaml (run \`pnpm dedupe\` in web/)`);
  if (variants.length === 0) problems.push(`${name} is not in the lockfile snapshots (unexpected lockfile format)`);
}
if (problems.length > 0) {
  console.error(problems.map((problem) => `check-lockfile: ${problem}`).join("\n"));
  process.exit(1);
}
console.log("check-lockfile: one vitest and one vite variant in pnpm-lock.yaml");
