/* global console, process */
// Creates a placeholder `web/app/dist/index.html` when the app has not been built, so that
// `wrangler dev` and the Workers tests can start (the assets binding needs its directory).
// `app/dist` is git-ignored (web/.gitignore) and owned by the app build, which overwrites it.
// Never replaces an existing build.
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "..", "..", "app", "dist");
const index = join(dist, "index.html");

export function ensureAssets() {
  if (existsSync(index)) return false;
  mkdirSync(dist, { recursive: true });
  writeFileSync(
    index,
    '<!doctype html>\n<html lang="en"><head><meta charset="utf-8"><title>Monoptah</title></head>\n<body>Monoptah web placeholder: build web/app to replace it.</body></html>\n',
  );
  return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(ensureAssets() ? `created placeholder ${index}` : "app/dist already has an index.html");
}
