// S-01 item 2, local half: does the `ratelimits` binding work under `wrangler dev --local`?
// Uses the S-07 spike Worker (route /rl, binding LIMITER = 3 calls per 60 s). Production availability on the
// free plan is a different question (PENDING-MAC, see FINDINGS.md).
// Usage: node scripts/check-ratelimit.ts
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { startWranglerDev } from "../../s-07-hibernation/scripts/wrangler-dev.ts";

const dev = await startWranglerDev(resolve(dirname(fileURLToPath(import.meta.url)), "../../s-07-hibernation"));
try {
  const codes: number[] = [];
  for (let i = 0; i < 6; i++) {
    const res = await fetch(`${dev.url}/rl?key=check-fixed`);
    await res.text();
    codes.push(res.status);
  }
  console.log(JSON.stringify({ limit: 3, periodS: 60, statuses: codes }));
  const ok = codes.filter((c) => c === 200).length;
  const limited = codes.filter((c) => c === 429).length;
  console.log(ok === 3 && limited === 3 ? "PASS: 3 allowed then 429 (binding works locally)" : "UNEXPECTED");
  process.exitCode = ok === 3 && limited === 3 ? 0 : 1;
} finally {
  await dev.stop();
}
