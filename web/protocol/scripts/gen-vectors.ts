// Writes vectors.json and methods.json next to package.json. Run with
// `pnpm --filter @monoptah/protocol vectors:gen`. The test suite fails when the
// committed files differ from a fresh run.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { buildMethodsJson, buildVectors, serialize } from "./vectors-lib.ts";

const root = new URL("../", import.meta.url);

writeFileSync(fileURLToPath(new URL("vectors.json", root)), serialize(buildVectors()));
writeFileSync(fileURLToPath(new URL("methods.json", root)), serialize(buildMethodsJson()));
console.log("wrote vectors.json and methods.json");
