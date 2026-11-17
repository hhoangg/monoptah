// Run after `vite build` (see the `build` script). Fails when the Worker assets directory
// (app/dist/client, the `assets.directory` of web/wrangler.jsonc) lacks `_headers` or the default
// CSP, or holds a Worker bundle, a sourcemap or the dev mock API marker that would be served to the public.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const client = join(here, "..", "dist", "client");
const problems = [];

const headers = join(client, "_headers");
if (!existsSync(headers)) {
  problems.push("dist/client/_headers is missing");
} else {
  const text = readFileSync(headers, "utf8");
  if (!process.env.MONOPTAH_RP_ORIGIN && !text.includes("connect-src 'self' wss://zptah.app;")) {
    problems.push("dist/client/_headers lacks the default CSP connect-src 'self' wss://zptah.app");
  }
  if (!text.includes("Content-Security-Policy:")) problems.push("dist/client/_headers has no Content-Security-Policy");
}

// The dev mock API must never ship: its unique marker (features/accounts/mock/index.ts) may not
// appear in any served file.
const MOCK_MARKER = "__MONOPTAH_MOCK_API__";

for (const entry of readdirSync(client, { recursive: true, withFileTypes: true })) {
  if (!entry.isFile()) continue;
  const name = entry.name;
  const dir = entry.parentPath.slice(client.length);
  if (name.endsWith(".map")) problems.push(`sourcemap in the assets directory: ${join(dir, name)}`);
  const file = join(entry.parentPath, name);
  if (/\.(js|mjs|css|html|json|map)$/.test(name) && readFileSync(file, "utf8").includes(MOCK_MARKER)) {
    problems.push(`mock API marker ${MOCK_MARKER} found in ${join(dir, name)}`);
  }
  if (name === "wrangler.json" || name === "index.js") problems.push(`Worker bundle file in the assets directory: ${join(dir, name)}`);
}

if (problems.length > 0) {
  console.error(problems.map((problem) => `check-dist: ${problem}`).join("\n"));
  process.exit(1);
}
console.log("check-dist: dist/client has _headers and no Worker bundle, sourcemap or mock API");
