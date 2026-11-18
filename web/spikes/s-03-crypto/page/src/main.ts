// Test page for S-03. Exposes `window.s03` for Playwright and buttons for a human on a phone.
import { bench, checkKey, clearKey, createKey, matrix, persist, storedContains, wrapRoundTrip } from "./crypto-lib";

declare global {
  interface Window { s03: typeof api }
}

const api = { bench, matrix, createKey, checkKey, storedContains, clearKey, persist, wrapRoundTrip };
window.s03 = api;

const out = document.getElementById("out")!;
const show = (v: unknown) => { out.textContent = JSON.stringify(v, null, 2); };
document.getElementById("ua")!.textContent = navigator.userAgent;

const guard = (fn: () => Promise<unknown>) => () => { out.textContent = "running..."; fn().then(show, (e: unknown) => { out.textContent = `error: ${String(e)}`; }); };

document.getElementById("bench")!.addEventListener("click", guard(async () => ({ ua: navigator.userAgent, bench: bench(200) })));
document.getElementById("check-key")!.addEventListener("click", guard(async () => ({ at: new Date().toISOString(), check: await checkKey() })));
document.getElementById("clear")!.addEventListener("click", guard(async () => { await clearKey(); return "cleared"; }));
document.getElementById("run-all")!.addEventListener("click", guard(async () => {
  const created = await createKey();
  return {
    ua: navigator.userAgent,
    bench: bench(200),
    matrix: await matrix(),
    persist: await persist(),
    wrapRoundTrip: await wrapRoundTrip(),
    keyCreated: { publicKeyHex: created.publicKeyHex, extractable: created.extractable },
    check: await checkKey(),
  };
}));
