// E-01 acceptance smoke: register, pair the headless desktop, confirm a device, read a catalog.
// Skipped until WS-R (pairing routes, device routes) and WS-D1 (`web_link_headless serve`) land; the probes turn the
// tests on by themselves. The register half already passes in accounts.spec.ts.
import { spawn, type ChildProcess } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { origin, pairingInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { headlessBin, probeDevices, probeHeadless, probePairing } from "../lib/probes.ts";
import { base64urlEncode } from "../lib/pure.ts";

test.describe.serial("pair the headless desktop (E-01)", () => {
  test("TODO(E-01:WS-R R-03, WS-D1 R-05) pair, bring the desktop online", async ({ newActor }) => {
    const pairing = await probePairing(origin());
    const headless = probeHeadless();
    const reason = [pairing, headless].filter((p) => !p.run).map((p) => p.reason).join("; ");
    if (reason) console.log(`skip reason: ${reason}`);
    test.skip(reason !== "", reason);

    // Own account from the second bootstrap invite (the first one is used by accounts.spec.ts).
    const owner = await newActor();
    const verify = await owner.register(pairingInvite(), "e2epair");
    expect(verify.status).toBe(200);

    // Desktop side (device authorization, architecture.md 6.4), played from Node: start, then poll after approval.
    const verifier = randomBytes(32);
    const challenge = base64urlEncode(createHash("sha256").update(verifier).digest());
    const start = await fetch(`${origin()}/api/pair/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "CF-Connecting-IP": "198.51.100.230" },
      body: JSON.stringify({ publicKey: base64urlEncode(randomBytes(32)), machineName: "e2e-headless", os: "linux", appVersion: "0.0.0-e2e", challenge }),
    });
    expect(start.status).toBe(200);
    const started = (await start.json()) as { userCode: string; deviceCode: string };

    // Owner approves with a fresh re-auth.
    expect((await owner.call("GET", `/api/pair/${started.userCode}`)).status).toBe(200);
    expect((await owner.reauth()).status).toBe(200);
    expect((await owner.call("POST", `/api/pair/${started.userCode}/approve`, { body: {} })).status).toBe(200);

    const poll = await fetch(`${origin()}/api/pair/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "CF-Connecting-IP": "198.51.100.231" },
      body: JSON.stringify({ deviceCode: started.deviceCode, verifier: base64urlEncode(verifier) }),
    });
    const polled = (await poll.json()) as { status: string; desktopToken?: string };
    expect(polled.status).toBe("approved");

    // Run the headless desktop with the token (file mode 0600 in a temp dir that is removed at the end).
    const dir = mkdtempSync(join(tmpdir(), "monoptah-e2e-headless-"));
    let child: ChildProcess | null = null;
    try {
      const tokenFile = join(dir, "token");
      writeFileSync(tokenFile, polled.desktopToken ?? "", { mode: 0o600 });
      const relay = `${origin().replace(/^http/, "ws")}/relay/desktop`;
      child = spawn(headlessBin(), ["serve", "--relay", relay, "--token-file", tokenFile, "--data-dir", join(dir, "data")], { stdio: "ignore" });
      await expect
        .poll(async () => ((await owner.call("GET", "/api/desktops")).json["desktops"] as { online: boolean }[] | undefined)?.some((d) => d.online) ?? false, { timeout: 30_000 })
        .toBe(true);
    } finally {
      child?.kill("SIGTERM");
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("TODO(E-01:WS-R R-04, WS-S W-02) confirm a device and read a catalog", async ({ newActor }) => {
    const devices = await probeDevices(origin());
    const headless = probeHeadless();
    const reason = [devices, headless].filter((p) => !p.run).map((p) => p.reason).join("; ");
    if (reason) console.log(`skip reason: ${reason}`);
    test.skip(reason !== "", reason);
    await newActor();
    // The device half needs the browser link client (device key, Noise handshake over /relay, workspace.list) of the
    // built app. Fill this in with the link client's entry point when WS-S ships it; until then fail loudly instead
    // of passing without reading a catalog.
    throw new Error("device confirmation and catalog read are not wired yet: add them when the link client is served");
  });
});
