// S-03: crypto and key storage on local desktop engines (not iPhones; those are PENDING-DEVICE in FINDINGS.md).
// Every test records its numbers under /tmp/evidence/ws-q/browser/s03 (never committed).
import { chromium, expect, firefox, test, webkit, type Page } from "@playwright/test";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import type * as S03Lib from "../page/src/crypto-lib";
import { join } from "node:path";

// WebKit upgrades http://localhost subresources because of upgrade-insecure-requests, so it gets the
// same CSP without that one directive (port 4193); Chromium and Firefox use the exact CSP (4192).
const urlFor = (browserName: string) => (browserName === "webkit" ? "http://localhost:4193/" : "http://localhost:4192/");
const OUT = "/tmp/evidence/ws-q/browser/s03";

const save = (project: string, name: string, data: unknown) => {
  mkdirSync(OUT, { recursive: true });
  writeFileSync(`${OUT}/${project}-${name}.json`, JSON.stringify(data, null, 2));
};

type S03 = typeof S03Lib;
const api = (page: Page) => ({
  bench: (n: number) => page.evaluate((i) => (window as unknown as { s03: S03 }).s03.bench(i), n),
  matrix: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.matrix()),
  createKey: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.createKey()),
  checkKey: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.checkKey()),
  clearKey: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.clearKey()),
  persist: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.persist()),
  wrapRoundTrip: () => page.evaluate(() => (window as unknown as { s03: S03 }).s03.wrapRoundTrip()),
  storedContains: (h: string) => page.evaluate((x) => (window as unknown as { s03: S03 }).s03.storedContains(x), h),
});

test("noble X25519 + ChaCha20-Poly1305 throughput on 64 KiB messages", async ({ page, browser, browserName }, info) => {
  await page.goto(urlFor(browserName));
  const r = await api(page).bench(300);
  save(info.project.name, "bench", { browser: `${browser.browserType().name()} ${browser.version()}`, ...r });
  expect(r.encryptMiBs).toBeGreaterThanOrEqual(1);
  expect(r.decryptMiBs).toBeGreaterThanOrEqual(1);
});

test("WebCrypto availability matrix", async ({ page, browser, browserName }, info) => {
  await page.goto(urlFor(browserName));
  const m = await api(page).matrix();
  save(info.project.name, "matrix", { browser: `${browser.browserType().name()} ${browser.version()}`, ...m });
  expect(m["secureContext"]).toBe("true");
  // Every row reported in FINDINGS.md: all four are available on all three desktop engines.
  for (const row of ["X25519 deriveBits", "Ed25519 sign/verify", "AES-GCM encrypt/decrypt", "ECDH P-256 deriveBits"]) expect(m[row], row).toBe("ok");
});

test("wrapped X25519 key: round trip, reload, never stored in clear, AES key not exportable", async ({ page, browserName }, info) => {
  await page.goto(urlFor(browserName));
  const a = api(page);
  await a.clearKey();
  expect(await a.wrapRoundTrip()).toBe(true);
  const created = await a.createKey();
  expect(created.extractable).toBe(false);
  const before = await a.checkKey();
  expect(before).toMatchObject({ found: true, keyUsable: true, extractable: false, exportBlocked: true, publicKeyMatches: true, publicKeyHex: created.publicKeyHex });
  const scan = await a.storedContains(created.privateKeyHex);
  expect(scan.contains).toBe(false);
  expect(scan.scannedValues).toBeGreaterThan(3);
  await page.reload();
  const after = await api(page).checkKey();
  save(info.project.name, "key-reload", { before, after, scan });
  expect(after).toMatchObject({ found: true, keyUsable: true, extractable: false, publicKeyMatches: true, publicKeyHex: created.publicKeyHex });
  await api(page).clearKey();
});

test("navigator.storage.persist() and persisted()", async ({ page, browser, browserName }, info) => {
  await page.goto(urlFor(browserName));
  const p = await api(page).persist();
  save(info.project.name, "persist", { browser: `${browser.browserType().name()} ${browser.version()}`, ...p });
  expect(p.persistResult).not.toBeUndefined();
});

function* walk(dir: string): Generator<string> {
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    let st;
    try { st = statSync(p); } catch { continue; }
    if (st.isDirectory()) yield* walk(p);
    else if (st.isFile() && st.size < 64 * 1024 * 1024) yield p;
  }
}

test("AES key survives a browser restart; private key bytes are not in the profile on disk", async ({ browserName, browser }, info) => {
  const type = { chromium, firefox, webkit }[browserName];
  const dir = mkdtempSync(join(tmpdir(), `s03-${browserName}-`));
  try {
    const ctx1 = await type.launchPersistentContext(dir);
    const p1 = await ctx1.newPage();
    await p1.goto(urlFor(browserName));
    const created = await api(p1).createKey();
    await ctx1.close();

    const ctx2 = await type.launchPersistentContext(dir);
    const p2 = await ctx2.newPage();
    await p2.goto(urlFor(browserName));
    const check = await api(p2).checkKey();
    await ctx2.close();

    const priv = Buffer.from(created.privateKeyHex, "hex");
    const privHex = Buffer.from(created.privateKeyHex);
    let files = 0;
    const hits: string[] = [];
    for (const f of walk(dir)) {
      files++;
      const b = readFileSync(f);
      if (b.includes(priv) || b.includes(privHex)) hits.push(f);
    }
    save(info.project.name, "restart", { browser: `${browserName} ${browser.version()}`, check, profileFilesScanned: files, plaintextPrivateKeyHits: hits });
    expect(check).toMatchObject({ found: true, keyUsable: true, extractable: false, publicKeyMatches: true, publicKeyHex: created.publicKeyHex });
    expect(files).toBeGreaterThan(0);
    expect(hits).toEqual([]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
