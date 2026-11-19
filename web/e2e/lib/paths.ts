import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const e2eDir = resolve(here, "..");
export const webDir = resolve(e2eDir, "..");
export const repoDir = resolve(webDir, "..");
export const wranglerBin = join(e2eDir, "node_modules", ".bin", "wrangler");
export const wranglerConfig = join(webDir, "wrangler.jsonc");
export const vectorsPath = join(webDir, "protocol", "vectors.json");

/** Where every output of a run goes (Playwright artifacts, report, wrangler log). Never inside the repo by default. */
export function outputDir(env: NodeJS.ProcessEnv = process.env): string {
  const dir = env["E2E_OUTPUT_DIR"];
  return dir && dir.length > 0 ? resolve(dir) : "/tmp/evidence/ws-q/e2e";
}
