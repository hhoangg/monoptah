// E-01 acceptance smoke: register, pair the headless desktop, confirm a device, read a catalog.
//
// Test 1 runs now: the headless desktop plays the desktop side of the pairing (`pair`, architecture.md 6.4) and an
// account registered with the virtual authenticator approves its user code through the API (the /pair/:code page is
// driven in tests/w14-flows.spec.ts; here the API keeps this smoke independent of the UI).
// Test 2 serves the paired desktop (`serve --confirm-pending`), confirms a browser device and reads the catalog.
import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { connectDevice, generateKeyPair } from "../lib/device.ts";
import { clearRateLimits, namedInvite, origin } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import type { Actor } from "../lib/actor.ts";
import { headlessBin, probeHeadless, probePair, probePairing } from "../lib/probes.ts";
import { findCredentialLeaks, base64urlEncode } from "../lib/pure.ts";
import { modeProblems } from "../lib/r12.ts";

interface Headless {
  child: ChildProcess;
  /** Every JSON line the process printed on stdout, in order. */
  events: Record<string, unknown>[];
  /** Resolves with the exit code. */
  exited: Promise<number | null>;
  stderr: () => string;
  /** Raw stdout text, for leak checks. */
  stdoutText: () => string;
}

/** Spawns the headless binary. Its stdout is JSON lines (never the token); stderr is kept for failure messages. */
function runHeadless(args: string[]): Headless {
  const child = spawn(headlessBin(), args, { stdio: ["ignore", "pipe", "pipe"] });
  const events: Record<string, unknown>[] = [];
  let outText = "";
  child.stdout!.on("data", (chunk: Buffer) => {
    outText += chunk.toString();
  });
  createInterface({ input: child.stdout! }).on("line", (line) => {
    try {
      events.push(JSON.parse(line) as Record<string, unknown>);
    } catch {
      // not a JSON line: ignored
    }
  });
  let errText = "";
  child.stderr!.on("data", (chunk: Buffer) => {
    errText = (errText + chunk.toString()).slice(-2000);
  });
  const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)));
  return { child, events, exited, stderr: () => errText, stdoutText: () => outText };
}

const eventOf = (headless: Headless, name: string) => headless.events.find((e) => e["event"] === name);

test.describe.serial("pair the headless desktop (E-01)", () => {
  let dataDir = "";
  let owner: Actor;
  let desktopId = "";
  let serving: Headless | null = null;

  // A fresh pair/start window: the suite pairs more headless desktops than the per-IP limit allows (lib/env.ts).
  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    serving?.child.kill("SIGTERM");
    if (dataDir) rmSync(dataDir, { recursive: true, force: true });
  });

  test("register, then approve the headless desktop's pairing code", async ({ newSharedActor }) => {
    const pairing = await probePairing(origin());
    const headless = probePair();
    const reason = [pairing, headless].filter((p) => !p.run).map((p) => p.reason).join("; ");
    if (reason) console.log(`skip reason: ${reason}`);
    test.skip(reason !== "", reason);

    // Own account from the second bootstrap invite (the first one is used by accounts.spec.ts).
    owner = await newSharedActor();
    expect((await owner.register(namedInvite("pairing"), "e2epair")).status).toBe(200);
    dataDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-headless-"));

    // Desktop side: `pair` starts the device authorization, prints the user code and polls until it is approved.
    const pair = runHeadless(["pair", "--server", origin(), "--data-dir", join(dataDir, "data"), "--machine-name", "e2e-headless"]);
    try {
      await expect
        .poll(() => {
          if (eventOf(pair, "pairing") === undefined && pair.child.exitCode !== null) {
            throw new Error(`pair exited with ${pair.child.exitCode} before printing a user code: ${pair.stderr()}`);
          }
          return eventOf(pair, "pairing") !== undefined;
        }, { timeout: 15_000 })
        .toBe(true);
      const started = eventOf(pair, "pairing") as { userCode: string; confirmationCode: string; fingerprint: string };
      expect(started.userCode).toMatch(/^[A-Z0-9]{4}-[A-Z0-9]{4}$/);

      // Browser side: what the /pair/:code page shows (machine name, fingerprint, confirmation code), then approve
      // with a fresh passkey re-auth.
      const info = await owner.call("GET", `/api/pair/${started.userCode}`);
      expect(info.status).toBe(200);
      expect(info.json).toMatchObject({ machineName: "e2e-headless", fingerprint: started.fingerprint, confirmationCode: started.confirmationCode });
      expect((await owner.reauth()).status).toBe(200);
      const approved = await owner.call("POST", `/api/pair/${started.userCode}/approve`, { body: {} });
      expect(approved.status).toBe(200);

      expect(await pair.exited, pair.stderr()).toBe(0);
      const paired = eventOf(pair, "paired") as { desktopId: string; account: string; fingerprint: string };
      expect(paired).toMatchObject({ desktopId: approved.json["desktopId"], account: "e2epair", fingerprint: started.fingerprint });
      desktopId = paired.desktopId;
      // The token is minted by the poll, sealed in the data dir's store and never printed: no stream or event carries
      // a credential, and no plain token file is left behind.
      // There is no test-only reader of the sealed token, so the scan is structural: no credential field name and no
      // 32+ character base64url or hex run (ids and fingerprints set aside) on stdout or stderr, and the sealed store
      // and its key are private files (0600, not on Windows).
      expect(findCredentialLeaks(`${pair.stdoutText()}\n${pair.stderr()}`)).toEqual([]);
      expect(JSON.stringify(pair.events)).not.toMatch(/desktopToken|deviceCode|verifier/);
      expect(existsSync(join(dataDir, "data", "web-pairing.json"))).toBe(false);
      expect(modeProblems(join(dataDir, "data"))).toEqual([]);
    } finally {
      pair.child.kill("SIGTERM");
    }

    // The account sees the desktop, with the key the desktop displayed. It is not online: nothing serves it yet.
    const list = await owner.call("GET", "/api/desktops");
    expect(list.status).toBe(200);
    const row = (list.json["desktops"] as { id: string; name: string; online: boolean }[]).find((d) => d.id === desktopId);
    expect(row).toMatchObject({ name: "e2e-headless", online: false });
    const directory = await owner.call("GET", `/api/desktops/${desktopId}`);
    expect(directory.json["fingerprint"]).toBe((eventOf(pair, "paired") as { fingerprint: string }).fingerprint);
  });

  test("serve, confirm a device, read the catalog", async () => {
    const headless = probeHeadless();
    if (!headless.run) console.log(`skip reason: ${headless.reason}`);
    test.skip(!headless.run, headless.reason);
    test.skip(desktopId === "", "the pairing test did not run");

    // The desktop: loads its pairing, dials the relay and confirms pending grants by itself (E2E-only flag).
    serving = runHeadless(["serve", "--data-dir", join(dataDir, "data"), "--confirm-pending", "read-only"]);
    await expect
      .poll(async () => {
        if (serving!.child.exitCode !== null) throw new Error(`serve exited with ${serving!.child.exitCode}: ${serving!.stderr()}`);
        return ((await owner.call("GET", "/api/desktops")).json["desktops"] as { id: string; online: boolean }[]).some((d) => d.id === desktopId && d.online);
      }, { timeout: 30_000 })
      .toBe(true);

    // The browser device: register its key and ask for access to the desktop.
    const deviceKey = generateKeyPair();
    const registered = await owner.call("POST", "/api/devices", { body: { name: "e2e-browser", publicKey: base64urlEncode(deviceKey.publicKey) } });
    expect(registered.status).toBe(201);
    const deviceId = String(registered.json["deviceId"]);
    const requested = await owner.call("POST", `/api/desktops/${desktopId}/grants`, { body: { deviceId } });
    expect([200, 201]).toContain(requested.status);

    // The encrypted link: Noise IK against the key the directory pinned, then the desktop confirms the device.
    const directory = await owner.call("GET", `/api/desktops/${desktopId}`);
    const cookie = await owner.sessionCookie();
    expect(cookie).toBeDefined();
    const client = await connectDevice({
      origin: origin(),
      desktopId,
      deviceId,
      deviceKey,
      desktopPublicKey: String(directory.json["publicKey"]),
      cookie: `${cookie?.name}=${cookie?.value}`,
      deviceName: "e2e-browser",
    });
    try {
      await expect.poll(() => client.state?.state, { timeout: 30_000 }).toBe("confirmed");
      expect(client.state?.permission).toBe("read");
      const catalog = await client.call("workspace.list");
      expect(catalog.projects.map((p) => p.name)).toContain(E2E_CATALOG.projectName);
      expect(catalog.sessions.map((s) => s.title)).toContain(E2E_CATALOG.sessionTitle);
    } finally {
      client.bye();
    }
    expect(findCredentialLeaks(`${serving.stdoutText()}\n${serving.stderr()}`)).toEqual([]);
    expect(modeProblems(join(dataDir, "data"))).toEqual([]);
  });
});
