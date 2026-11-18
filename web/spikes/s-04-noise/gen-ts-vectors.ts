// Writes vectors/monoptah-ik.ts.json. Run: node web/spikes/s-04-noise/gen-ts-vectors.ts
import { writeFileSync } from "node:fs";
import { buildVectors } from "./ts-vectors.ts";

const target = new URL("./vectors/monoptah-ik.ts.json", import.meta.url);
writeFileSync(target, `${JSON.stringify(buildVectors(), null, 1)}\n`);
console.log(`wrote ${target.pathname}`);
