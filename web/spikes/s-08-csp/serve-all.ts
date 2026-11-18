// Builds the S-08 stub (patched and raw) and the S-03 page, then serves them for the Playwright run.
// 4180 patched + exact CSP, 4181 patched without style-src-attr, 4182 raw (unpatched) + exact CSP,
// 4183 raw without upgrade-insecure-requests, 4184 patched without it, 4185 patched without it and without
// style-src-attr (4183-4185 are for WebKit, which upgrades http://localhost to https), 4187 patched with no CSP at all (control), 4186 patched + exact CSP over https (self-signed), 4192 S-03 page + exact CSP (not 4190: Firefox blocks that port), 4193 the same without upgrade-insecure-requests (WebKit).
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startServer } from "./server.ts";

const here = fileURLToPath(new URL(".", import.meta.url));
const vite = fileURLToPath(new URL("../node_modules/.bin/vite", import.meta.url));
const build = (cwd: string, env: Record<string, string> = {}) =>
  execFileSync(vite, ["build", "--logLevel", "warn"], { cwd, env: { ...process.env, ...env }, stdio: "inherit" });

build(`${here}app`);
build(`${here}app`, { SPIKE_RAW: "1" });
build(`${here}../s-03-crypto/page`);

// 4186: patched build, exact CSP, over https with a throwaway self-signed certificate (the production shape).
const certDir = mkdtempSync(join(tmpdir(), "s08-cert-"));
execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=localhost",
  "-addext", "subjectAltName=DNS:localhost", "-keyout", join(certDir, "k.pem"), "-out", join(certDir, "c.pem")], { stdio: "ignore" });
await startServer(`${here}app/dist/patched`, 4186, "exact", { key: readFileSync(join(certDir, "k.pem")), cert: readFileSync(join(certDir, "c.pem")) });
await startServer(`${here}../s-03-crypto/page/dist`, 4192, "exact");
await startServer(`${here}../s-03-crypto/page/dist`, 4193, "no-uir");
await startServer(`${here}app/dist/raw`, 4182, "exact");
await startServer(`${here}app/dist/raw`, 4183, "no-uir");
await startServer(`${here}app/dist/patched`, 4184, "no-uir");
await startServer(`${here}app/dist/patched`, 4185, "no-style-attr-no-uir");
await startServer(`${here}app/dist/patched`, 4181, "no-style-attr");
await startServer(`${here}app/dist/patched`, 4187, "none");
await startServer(`${here}app/dist/patched`, 4180, "exact");
