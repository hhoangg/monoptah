// R-12: the end-to-end scenario of roadmap.md on the E-01 harness. One serial scenario, no browser: a software
// passkey (lib/soft-actor.ts) drives the real Worker API, the headless desktop (`web_link_headless`) is the desktop,
// and the TS device client of web/mock-desktop is the browser's link client. Every security-relevant step asserts
// the failure mode (close code, error code or HTTP status); the expected values come from architecture.md 4.4, 4.5,
// 6.7 and from the Worker and desktop code, not from what a run happened to print.
//
// Not covered with the real headless token: token reuse and the live-token `replaces` re-pair. The headless binary
// seals its token and never reveals it, so those two are asserted with a desktop that the test pairs itself (it holds
// the token). Credential leaks of the headless processes are checked structurally (no reader of the sealed token).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CloseCodes, FrameType, MAX_FRAME_SIZE } from "@monoptah/protocol";
import { test, expect } from "@playwright/test";
import { ClientError, type TestClient } from "../../mock-desktop/src/client.ts";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { connectDevice, generateKeyPair, type KeyPair } from "../lib/device.ts";
import { clearRateLimits, namedInvite, origin, restartRelay } from "../lib/env.ts";
import { probeHeadless } from "../lib/probes.ts";
import { HeadlessDesktop, RecordingSocket, TsDesktopPairing, closeCodeOf, connectDesktop, leaksOf, modeProblems } from "../lib/r12.ts";
import { base64urlEncode, testIp } from "../lib/pure.ts";
import { SoftActor } from "../lib/soft-actor.ts";

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

interface Device {
  deviceId: string;
  key: KeyPair;
}

// A fresh pair/start window: the suite pairs more headless desktops than the per-IP limit allows (lib/env.ts).
test.beforeAll(clearRateLimits);

test("R-12: pair to revoke, key change, relay restart, token reuse, replay, oversized frame", async () => {
  const headless = probeHeadless();
  if (!headless.run) {
    // In CI a missing or incapable binary is a failed job, not a pass by skipping.
    if (process.env["CI"] === "true") throw new Error(`R-12 cannot run in CI: ${headless.reason}`);
    console.log(`skip reason: ${headless.reason}`);
  }
  test.skip(!headless.run, headless.reason);
  test.setTimeout(900_000);

  const base = origin();
  const owner = new SoftActor({ origin: base, rpId: "localhost", ip: testIp(200) });
  const root = mkdtempSync(join(tmpdir(), "monoptah-e2e-r12-"));
  const desktops: HeadlessDesktop[] = [];
  const clients: TestClient[] = [];
  const spawnDesktop = (args: string[]): HeadlessDesktop => {
    const desktop = new HeadlessDesktop(args);
    desktops.push(desktop);
    return desktop;
  };
  const cookie = (): string => {
    const session = owner.sessionCookie();
    if (!session) throw new Error("the owner has no session");
    return `${session.name}=${session.value}`;
  };
  const deviceUrl = (desktopId: string, deviceId: string): string => `${base.replace(/^http/, "ws")}/relay/device/${desktopId}?device=${deviceId}`;
  const deviceHeaders = (): Record<string, string> => ({ Cookie: cookie(), Origin: base });

  /** Pairs a headless desktop in `dir`: starts `pair`, approves its code through the API, waits for `paired`. */
  const pairHeadless = async (dir: string, name: string, replaces?: string): Promise<{ desktopId: string; fingerprint: string; approved: number }> => {
    const pair = spawnDesktop(["pair", "--server", base, "--data-dir", dir, "--machine-name", name]);
    const started = await pair.waitFor("pairing", {}, 20_000);
    const userCode = String(started["userCode"]);
    expect((await owner.call("GET", `/api/pair/${userCode}`)).json).toMatchObject({ machineName: name, fingerprint: started["fingerprint"] });
    expect((await owner.reauth()).status).toBe(200);
    const approval = await owner.call("POST", `/api/pair/${userCode}/approve`, { body: replaces ? { replaces } : {} });
    expect(approval.status).toBe(200);
    expect(await pair.exited, pair.stderr()).toBe(0);
    const paired = pair.matching("paired")[0] as { desktopId: string; fingerprint: string; account: string };
    expect(paired.account).toBe("r12owner");
    // No credential on stdout or stderr: the token is sealed in the data dir's store (mode 0600, key beside it).
    expect(leaksOf(pair)).toEqual([]);
    expect(modeProblems(dir)).toEqual([]);
    return { desktopId: paired.desktopId, fingerprint: paired.fingerprint, approved: approval.status };
  };

  const serve = async (dir: string, confirm: "read-only" | "control" | null): Promise<HeadlessDesktop> => {
    const desktop = spawnDesktop(["serve", "--data-dir", dir, ...(confirm ? ["--confirm-pending", confirm] : [])]);
    await desktop.waitFor("online", {}, 60_000);
    return desktop;
  };

  const online = async (desktopId: string): Promise<boolean> => {
    const list = await owner.call("GET", "/api/desktops");
    return (list.json["desktops"] as { id: string; online: boolean }[]).some((d) => d.id === desktopId && d.online);
  };

  /** GET /api/desktops caches presence for 10 s per isolate, so the answer is polled. */
  const expectOnline = async (desktopId: string, expected: boolean): Promise<void> => {
    await expect.poll(() => online(desktopId), { timeout: 40_000, intervals: [1_000] }).toBe(expected);
  };

  /** Registers a browser device and asks for access to the desktop. */
  const newDevice = async (desktopId: string, name: string): Promise<Device> => {
    const key = generateKeyPair();
    const registered = await owner.call("POST", "/api/devices", { body: { name, publicKey: base64urlEncode(key.publicKey) } });
    expect(registered.status).toBe(201);
    const deviceId = String(registered.json["deviceId"]);
    const requested = await owner.call("POST", `/api/desktops/${desktopId}/grants`, { body: { deviceId } });
    expect([200, 201]).toContain(requested.status);
    return { deviceId, key };
  };

  const directoryKey = async (desktopId: string): Promise<{ publicKey: string; fingerprint: string }> => {
    const directory = await owner.call("GET", `/api/desktops/${desktopId}`);
    expect(directory.status).toBe(200);
    return { publicKey: String(directory.json["publicKey"]), fingerprint: String(directory.json["fingerprint"]) };
  };

  const link = async (desktopId: string, device: Device, name: string, extra: { channel?: number; pinned?: string; record?: boolean } = {}): Promise<{ client: TestClient; recorder: RecordingSocket | null }> => {
    let recorder: RecordingSocket | null = null;
    const client = await connectDevice({
      origin: base,
      desktopId,
      deviceId: device.deviceId,
      deviceKey: device.key,
      desktopPublicKey: extra.pinned ?? (await directoryKey(desktopId)).publicKey,
      cookie: cookie(),
      deviceName: name,
      ...(extra.channel === undefined ? {} : { channel: extra.channel }),
      ...(extra.record ? { wrapSocket: (socket) => (recorder = new RecordingSocket(socket)) } : {}),
    });
    clients.push(client);
    return { client, recorder };
  };

  const confirmed = async (client: TestClient): Promise<void> => {
    await expect.poll(() => client.state?.state, { timeout: 30_000 }).toBe("confirmed");
  };

  const grantOf = async (desktopId: string, deviceId: string): Promise<{ state: string; permission: string | null } | undefined> => {
    const grants = (await owner.call("GET", `/api/desktops/${desktopId}/grants`)).json["grants"] as { deviceId: string; state: string; permission: string | null }[];
    return grants.find((g) => g.deviceId === deviceId);
  };

  /** Confirms like the user does on the desktop: the device shows pending first, then the `approve` command. */
  const approveOnDesktop = async (desktop: HeadlessDesktop, client: TestClient, deviceId: string, permission: "read-only" | "control"): Promise<void> => {
    await expect.poll(() => client.state?.state, { timeout: 30_000 }).toBe("pending");
    desktop.command({ cmd: "approve", deviceId, permission });
    await desktop.waitFor("confirmed", { deviceId });
    await confirmed(client);
  };

  const userTexts = (turns: { items: { type: string; text?: unknown }[] }[]): string[] =>
    turns.flatMap((turn) => turn.items.filter((item) => item.type === "user").map((item) => String(item.text)));

  try {
    // ---- 1. register, pair, approve -------------------------------------------------------------------------
    const dirA = join(root, "a");
    let desktopA = "";
    let fingerprintA = "";
    await test.step("register an account and pair the headless desktop (pair, approve)", async () => {
      expect((await owner.register(namedInvite("r12"), "r12owner")).status).toBe(200);
      const paired = await pairHeadless(dirA, "r12-desktop");
      desktopA = paired.desktopId;
      fingerprintA = paired.fingerprint;
      const directory = await directoryKey(desktopA);
      expect(directory.fingerprint).toBe(fingerprintA);
    });

    // ---- 2. serve, register a device, confirm on the desktop ------------------------------------------------
    // No `--confirm-pending`: a device stays pending until the `approve` command, as it does until the user clicks
    // Confirm in Settings > Connections > Web app (architecture.md 6.3).
    let serveA = await serve(dirA, null);
    await expectOnline(desktopA, true);
    const device1 = await newDevice(desktopA, "r12-browser-1");
    let client1: TestClient;
    await test.step("register a device: it is pending and no method is served until the desktop confirms", async () => {
      ({ client: client1 } = await link(desktopA, device1, "r12-browser-1"));
      // The Noise session completes and shows the pending state: no permission, no capabilities of a confirmed device.
      await expect.poll(() => client1.state?.state, { timeout: 30_000 }).toBe("pending");
      expect(client1.state?.permission).toBeUndefined();
      // The server directory shows the request as pending too (the desktop is the authority and has not confirmed).
      expect((await grantOf(desktopA, device1.deviceId))?.state).toBe("pending");
      // security.md 6 (:151): until confirmed "no method is served". The pending session drops what it decrypts (no
      // error, no close), so a request is never answered and never reaches the app.
      const answer = await Promise.race([
        client1.request("workspace.list").then(() => "answered", () => "rejected"),
        sleep(3_000).then(() => "unanswered"),
      ]);
      expect(answer).toBe("unanswered");
      expect(serveA.matching("request")).toHaveLength(0);
      expect(client1.closeCode).toBeNull();
      expect(client1.byeReceived).toBe(false);
    });
    await test.step("confirm it on the desktop (approve command): confirmed, control, server grant confirmed", async () => {
      serveA.command({ cmd: "approve", deviceId: device1.deviceId, permission: "control" });
      const event = await serveA.waitFor("confirmed", { deviceId: device1.deviceId });
      expect(event["permission"]).toBe("control");
      await confirmed(client1);
      expect(client1.state?.permission).toBe("control");
      await expect.poll(async () => (await grantOf(desktopA, device1.deviceId))?.state, { timeout: 15_000 }).toBe("confirmed");
      // The request sent while pending was dropped for good; a new one is served.
      expect(serveA.matching("request")).toHaveLength(0);
      expect((await client1.call("workspace.list")).projects.map((p) => p.name)).toContain(E2E_CATALOG.projectName);
    });

    // ---- 3. read, send, watch -------------------------------------------------------------------------------
    await test.step("read the catalog and the conversation", async () => {
      const catalog = await client1!.call("workspace.list");
      expect(catalog.projects.map((p) => p.name)).toContain(E2E_CATALOG.projectName);
      expect(catalog.sessions.map((s) => s.title)).toContain(E2E_CATALOG.sessionTitle);
      const read = await client1!.call("conversation.read", { conversationId: E2E_CATALOG.sessionId });
      expect(read.turns).toHaveLength(2);
      expect(serveA.matching("request", { method: "conversation.read" })).toHaveLength(1);
    });

    await test.step("watch, send, and stop streaming", async () => {
      const watch = await client1!.openWatch({ conversationId: E2E_CATALOG.sessionId, tail: 5 });
      await expect.poll(() => watch.frames.length, { timeout: 10_000 }).toBeGreaterThan(0);
      const sent = await client1!.call("conversation.send", { conversationId: E2E_CATALOG.sessionId, text: "hello r12" });
      expect(sent.sessionId).toBe(E2E_CATALOG.sessionId);
      // The headless reply comes 1500 ms after the send, three pacer intervals (500 ms): the frame with the user turn
      // is busy and is delivered long before the reply, and they cannot merge. Busy is seen first, then not busy.
      const hasUser = (f: { turns: unknown }): boolean => JSON.stringify(f.turns).includes("hello r12");
      await expect.poll(() => watch.frames.some((f) => f.busy && hasUser(f)), { timeout: 10_000 }).toBe(true);
      await expect
        .poll(() => watch.frames.some((f) => !f.busy && JSON.stringify(f.turns).includes("Echo: hello r12")), { timeout: 15_000 })
        .toBe(true);
      const firstBusy = watch.frames.findIndex((f) => f.busy && hasUser(f));
      const reply = watch.frames.findIndex((f) => !f.busy && JSON.stringify(f.turns).includes("Echo: hello r12"));
      expect(firstBusy).toBeGreaterThanOrEqual(0);
      expect(reply).toBeGreaterThan(firstBusy);
      // The stop method ends a held stream.
      await client1!.call("conversation.send", { conversationId: E2E_CATALOG.sessionId, text: "keep going [hold]" });
      await expect.poll(() => watch.frames.at(-1)?.busy, { timeout: 10_000 }).toBe(true);
      await client1!.call("session.cancel", { conversationId: E2E_CATALOG.sessionId });
      await expect.poll(() => watch.frames.at(-1)?.busy, { timeout: 10_000 }).toBe(false);
      expect(JSON.stringify(watch.frames.at(-1)?.turns)).not.toContain("Echo: keep going");
      watch.cancel();
    });

    // ---- 4. revoke a device: from the server, and from the desktop ------------------------------------------
    await test.step("revoke a device from the server: 4003, and the join is refused with 4003", async () => {
      expect((await owner.reauth()).status).toBe(200);
      const revoked = await owner.call("PUT", `/api/desktops/${desktopA}/grants/${device1.deviceId}`, { body: { state: "revoked" } });
      expect(revoked.status).toBe(200);
      await expect.poll(() => client1!.closeCode, { timeout: 10_000 }).toBe(CloseCodes.REVOKED);
      expect(await closeCodeOf(deviceUrl(desktopA, device1.deviceId), deviceHeaders())).toBe(CloseCodes.REVOKED);
      // The desktop drops the row after the grants sync.
      await serveA.waitFor("revoked", { scope: "device", deviceId: device1.deviceId }, 30_000);
    });

    let device2Id = "";
    await test.step("revoke a device from the desktop: state revoked and BYE, server mirror revoked, join 4003", async () => {
      const device2 = await newDevice(desktopA, "r12-browser-2");
      device2Id = device2.deviceId;
      const { client: client2 } = await link(desktopA, device2, "r12-browser-2");
      await approveOnDesktop(serveA, client2, device2.deviceId, "read-only");
      serveA.command({ cmd: "revoke", deviceId: device2.deviceId });
      await serveA.waitFor("revoked", { scope: "device", deviceId: device2.deviceId });
      await expect.poll(() => client2.byeReceived, { timeout: 10_000 }).toBe(true);
      // The desktop sends an encrypted `state: revoked` before its BYE (architecture.md 6.1 step 4, relay.rs revoke).
      expect(client2.states.map((s) => s.state)).toEqual(["pending", "confirmed", "revoked"]);
      await expect.poll(async () => (await grantOf(desktopA, device2.deviceId))?.state, { timeout: 15_000 }).toBe("revoked");
      // The desktop's report reaches the server, whose room closes the live socket of that device with 4003
      // (architecture.md 4.4, security.md 6 (:153)).
      await expect.poll(() => client2.closeCode, { timeout: 15_000 }).toBe(CloseCodes.REVOKED);
      expect(await closeCodeOf(deviceUrl(desktopA, device2.deviceId), deviceHeaders())).toBe(CloseCodes.REVOKED);
    });

    // ---- 5. revoke a desktop ---------------------------------------------------------------------------------
    // The desktop maps the relay's 4001, 4003 and 4011 all to "revoked" (relay.rs), so the 4003 itself is observed on
    // the live device socket of that room here, and on the desktop socket in step 8 (a desktop played by the test).
    await test.step("revoke a desktop: live sockets close 4003, the desktop reports revoked for good and stays off", async () => {
      const dirB = join(root, "b");
      const paired = await pairHeadless(dirB, "r12-desktop-b");
      const serveB = await serve(dirB, null);
      await expectOnline(paired.desktopId, true);
      const deviceB = await newDevice(paired.desktopId, "r12-browser-b");
      const { client: liveB } = await link(paired.desktopId, deviceB, "r12-browser-b");
      await expect.poll(() => liveB.state?.state, { timeout: 30_000 }).toBe("pending");
      serveB.command({ cmd: "status" });
      expect((await serveB.waitFor("status", {}, 10_000))["state"]).toBe("connected");
      expect((await owner.reauth()).status).toBe(200);
      expect((await owner.call("DELETE", `/api/desktops/${paired.desktopId}`)).status).toBe(200);
      await expect.poll(() => liveB.closeCode, { timeout: 15_000 }).toBe(CloseCodes.REVOKED);
      await serveB.waitFor("revoked", { scope: "desktop" }, 30_000);
      // Revoked is terminal in the desktop link (relay.rs): its status is `revoked`, not offline or connecting, which
      // is what a reconnecting client would report. It is asked now and again once the server shows it offline.
      const statusAfter = async (from: number): Promise<unknown> => {
        serveB.command({ cmd: "status" });
        return (await serveB.waitFor("status", {}, 10_000, from))["state"];
      };
      expect(await statusAfter(serveB.events.length)).toBe("revoked");
      await expectOnline(paired.desktopId, false);
      expect(await statusAfter(serveB.events.length)).toBe("revoked");
      expect(serveB.matching("online")).toHaveLength(1);
      expect(await closeCodeOf(`${base.replace(/^http/, "ws")}/relay/desktop/${paired.desktopId}`, { Authorization: "Bearer not-a-real-token" })).toBe(CloseCodes.UNAUTHORIZED);
      expect(leaksOf(serveB)).toEqual([]);
      serveB.kill();
    });

    // ---- 6. desktop key change -------------------------------------------------------------------------------
    const device3 = await newDevice(desktopA, "r12-browser-3");
    const oldKey = await directoryKey(desktopA);
    await test.step("desktop key change: unpair, re-pair with a new key, the old pin fails", async () => {
      const { client: live } = await link(desktopA, device3, "r12-browser-3");
      await approveOnDesktop(serveA, live, device3.deviceId, "control");
      // Unpair revokes the desktop's token and closes the room: the live device socket gets 4003.
      serveA.command({ cmd: "unpair" });
      await serveA.waitFor("unpaired", {}, 30_000);
      expect(await serveA.exited).toBe(0);
      await expect.poll(() => live.closeCode, { timeout: 15_000 }).toBe(CloseCodes.REVOKED);
      await expectOnline(desktopA, false);

      // Pair again onto the same desktop: new key, same id.
      const repaired = await pairHeadless(dirA, "r12-desktop", desktopA);
      expect(repaired.desktopId).toBe(desktopA);
      expect(repaired.fingerprint).not.toBe(fingerprintA);
      const newKey = await directoryKey(desktopA);
      expect(newKey.fingerprint).toBe(repaired.fingerprint);
      expect(newKey.publicKey).not.toBe(oldKey.publicKey);
      // Every non-revoked grant starts over as pending; the revoked ones (device 1 and 2) stay revoked.
      expect((await grantOf(desktopA, device3.deviceId))?.state).toBe("pending");
      expect((await grantOf(desktopA, device1.deviceId))?.state).toBe("revoked");
      expect((await grantOf(desktopA, device2Id))?.state).toBe("revoked");

      serveA =await serve(dirA, "control");
      // A device that pinned the old key: the IK handshake cannot complete, the desktop answers BYE (no state).
      const stale = await link(desktopA, device3, "r12-browser-3", { pinned: oldKey.publicKey, channel: 7 }).then(
        () => null,
        (error: unknown) => error,
      );
      expect(stale).toBeInstanceOf(ClientError);
      expect((stale as ClientError).code).toBe("bye");
      // With the new key the same device is confirmed again and reads.
      const { client } = await link(desktopA, device3, "r12-browser-3");
      await confirmed(client);
      expect((await client.call("workspace.list")).projects.map((p) => p.name)).toContain(E2E_CATALOG.projectName);
      client.bye();
    });

    // ---- 7. relay restart ------------------------------------------------------------------------------------
    let main: { client: TestClient; recorder: RecordingSocket | null };
    await test.step("relay restart: both sides reconnect and a read works again", async () => {
      const before = await link(desktopA, device3, "r12-browser-3", { channel: 2 });
      await confirmed(before.client);
      const onlineBefore = serveA.matching("online").length;
      await restartRelay();
      // The device socket ended with the relay (no close frame: 1006 or a going-away code).
      await expect.poll(() => before.client.closeCode, { timeout: 15_000 }).not.toBeNull();
      // The desktop reconnects by itself (backoff up to 60 s).
      await expect.poll(() => serveA.matching("online").length, { timeout: 120_000, intervals: [1_000] }).toBeGreaterThan(onlineBefore);
      expect(serveA.matching("offline").length).toBeGreaterThan(0);
      // D1 survived: the owner's session still works and the directory shows the desktop online.
      await expect.poll(() => online(desktopA), { timeout: 30_000 }).toBe(true);
      // The device reconnects (new socket, new handshake) and reads.
      main = await link(desktopA, device3, "r12-browser-3", { channel: 3, record: true });
      await confirmed(main.client);
      const read = await main.client.call("conversation.read", { conversationId: E2E_CATALOG.sessionId });
      expect(read.turns.length).toBeGreaterThanOrEqual(2);
    });

    // ---- 8. token reuse --------------------------------------------------------------------------------------
    await test.step("token reuse: a consumed device code, and a revoked desktop token, are refused", async () => {
      const pairing = new TsDesktopPairing(base, testIp(201));
      const userCode = await pairing.start("r12-token-desktop");
      expect((await owner.reauth()).status).toBe(200);
      const approved = await owner.call("POST", `/api/pair/${userCode}/approve`, { body: {} });
      expect(approved.status).toBe(200);
      const desktopId = String(approved.json["desktopId"]);
      // A wrong verifier on the real device code is 403 `forbidden`, with no state change and no token (handlers.ts,
      // poll). The poll limit is one per 3 s window, so the next poll waits for a new window.
      const wrong = await fetch(`${base}/api/pair/poll`, { method: "POST", headers: { "Content-Type": "application/json", "CF-Connecting-IP": testIp(201) }, body: JSON.stringify({ deviceCode: pairing.deviceCode, verifier: base64urlEncode(new Uint8Array(32)) }) });
      expect(wrong.status).toBe(403);
      const wrongBody = (await wrong.json()) as Record<string, unknown>;
      expect(wrongBody["code"]).toBe("forbidden");
      expect(wrongBody["desktopToken"]).toBeUndefined();
      await sleep(3_500);
      const first = await pairing.poll();
      expect(first.status).toBe(200);
      expect(first.json["status"]).toBe("approved");
      const token = String(first.json["desktopToken"]);
      expect(token.length).toBeGreaterThan(8);
      // The code is single use: a second poll (after the 3 s window) gets "expired" and never the token again.
      await sleep(3_500);
      const second = await pairing.poll();
      expect(second.status).toBe(200);
      expect(second.json["status"]).toBe("expired");
      expect(second.json["desktopToken"]).toBeUndefined();
      // An unknown device code is 404.
      const unknown = await fetch(`${base}/api/pair/poll`, { method: "POST", headers: { "Content-Type": "application/json", "CF-Connecting-IP": testIp(201) }, body: JSON.stringify({ deviceCode: "not-a-device-code", verifier: base64urlEncode(new Uint8Array(32)) }) });
      expect(unknown.status).toBe(404);

      // The token works, then the owner revokes the desktop: the live socket closes 4003 and the token is dead.
      const desktop = await connectDesktop(base, desktopId, token);
      const me = await fetch(`${base}/api/desktop/me`, { headers: { Authorization: `Bearer ${token}` } });
      expect(me.status).toBe(200);
      // A live token on the room of another desktop is unauthorized (4001), not a revocation.
      expect(await closeCodeOf(`${base.replace(/^http/, "ws")}/relay/desktop/${desktopA}`, { Authorization: `Bearer ${token}` })).toBe(CloseCodes.UNAUTHORIZED);
      expect((await owner.reauth()).status).toBe(200);
      expect((await owner.call("DELETE", `/api/desktops/${desktopId}`)).status).toBe(200);
      await expect.poll(() => desktop.closeCode(), { timeout: 30_000 }).toBe(CloseCodes.REVOKED);
      const reuse = `${base.replace(/^http/, "ws")}/relay/desktop/${desktopId}`;
      expect(await closeCodeOf(reuse, { Authorization: `Bearer ${token}` })).toBe(CloseCodes.REVOKED);
      const reused = await fetch(`${base}/api/desktop/me`, { headers: { Authorization: `Bearer ${token}` } });
      expect(reused.status).toBe(403);
      expect(((await reused.json()) as { code: string }).code).toBe("forbidden");
    });

    // ---- 8b. re-pair with `replaces` against a live token ----------------------------------------------------
    await test.step("re-pair with replaces while the old token is live: old socket 4003, old token dead, grants reset", async () => {
      // Architecture.md 6.7 (:543) and security.md 6 (:154): same id, new key, old tokens revoked, every non-revoked
      // grant back to `pending` (permission cleared), revoked grants stay revoked, and the room closes every old socket 4003.
      const first = new TsDesktopPairing(base, testIp(202));
      const firstCode = await first.start("r12-replace-desktop");
      expect((await owner.reauth()).status).toBe(200);
      const firstApproval = await owner.call("POST", `/api/pair/${firstCode}/approve`, { body: {} });
      expect(firstApproval.status).toBe(200);
      const target = String(firstApproval.json["desktopId"]);
      const oldToken = String((await first.poll()).json["desktopToken"]);
      const confirmedDevice = await newDevice(target, "r12-replace-confirmed");
      const revokedDevice = await newDevice(target, "r12-replace-revoked");
      const pendingDevice = await newDevice(target, "r12-replace-pending");
      // The desktop (played by the test, with its token) confirms one device; the owner revokes another.
      const confirm = await fetch(`${base}/api/desktop/grants/${confirmedDevice.deviceId}/state`, { method: "POST", headers: { "Content-Type": "application/json", Authorization: `Bearer ${oldToken}` }, body: JSON.stringify({ state: "confirmed", permission: "read" }) });
      expect(confirm.status).toBe(200);
      expect((await owner.reauth()).status).toBe(200);
      expect((await owner.call("PUT", `/api/desktops/${target}/grants/${revokedDevice.deviceId}`, { body: { state: "revoked" } })).status).toBe(200);
      expect((await grantOf(target, confirmedDevice.deviceId))?.state).toBe("confirmed");

      const oldSocket = await connectDesktop(base, target, oldToken);
      expect(oldSocket.closeCode()).toBeNull();
      expect((await fetch(`${base}/api/desktop/me`, { headers: { Authorization: `Bearer ${oldToken}` } })).status).toBe(200);

      const second = new TsDesktopPairing(base, testIp(203));
      const secondCode = await second.start("r12-replace-desktop");
      expect((await owner.reauth()).status).toBe(200);
      const replaced = await owner.call("POST", `/api/pair/${secondCode}/approve`, { body: { replaces: target } });
      expect(replaced.status).toBe(200);
      expect(replaced.json["desktopId"]).toBe(target);

      await expect.poll(() => oldSocket.closeCode(), { timeout: 30_000 }).toBe(CloseCodes.REVOKED);
      expect(await closeCodeOf(`${base.replace(/^http/, "ws")}/relay/desktop/${target}`, { Authorization: `Bearer ${oldToken}` })).toBe(CloseCodes.REVOKED);
      const reused = await fetch(`${base}/api/desktop/me`, { headers: { Authorization: `Bearer ${oldToken}` } });
      expect(reused.status).toBe(403);
      expect(((await reused.json()) as { code: string }).code).toBe("forbidden");

      // The new token comes from the next poll, and works.
      const issued = await second.poll();
      expect(issued.json["status"]).toBe("approved");
      const newToken = String(issued.json["desktopToken"]);
      expect(newToken).not.toBe(oldToken);
      expect((await fetch(`${base}/api/desktop/me`, { headers: { Authorization: `Bearer ${newToken}` } })).status).toBe(200);

      const afterConfirmed = await grantOf(target, confirmedDevice.deviceId);
      expect(afterConfirmed?.state).toBe("pending");
      expect(afterConfirmed?.permission ?? null).toBeNull();
      expect((await grantOf(target, pendingDevice.deviceId))?.state).toBe("pending");
      expect((await grantOf(target, revokedDevice.deviceId))?.state).toBe("revoked");
      (await connectDesktop(base, target, newToken)).socket.close();
    });

    // ---- 9. replay of a frame --------------------------------------------------------------------------------
    await test.step("replay of a frame: the desktop answers BYE and the request has no second effect", async () => {
      const text = "replay-once";
      const recorder = main!.recorder as RecordingSocket;
      await main!.client.call("conversation.send", { conversationId: E2E_CATALOG.sessionId, text });
      const captured = recorder.lastTransport();
      const sendsBefore = serveA.matching("request", { method: "conversation.send" }).length;
      // The relay forwards it (it cannot read it); the desktop's Noise nonce is past it, so decryption fails.
      recorder.raw().send(captured);
      await expect.poll(() => main!.client.byeReceived, { timeout: 10_000 }).toBe(true);
      expect(main!.client.closeCode).toBeNull(); // the relay socket stays open: the rejection is the desktop's
      await sleep(500);
      expect(serveA.matching("request", { method: "conversation.send" })).toHaveLength(sendsBefore);
      const { client: reader } = await link(desktopA, device3, "r12-browser-3", { channel: 4 });
      await confirmed(reader);
      const read = await reader.call("conversation.read", { conversationId: E2E_CATALOG.sessionId });
      expect(userTexts(read.turns).filter((t) => t === text)).toHaveLength(1);
      reader.bye();
    });

    // ---- 10. oversized frame ---------------------------------------------------------------------------------
    await test.step("oversized frame: the relay closes the socket with 1009", async () => {
      const big = new Uint8Array(MAX_FRAME_SIZE + 1);
      big[0] = 1;
      big[1] = FrameType.TRANSPORT;
      const code = await closeCodeOf(deviceUrl(desktopA, device3.deviceId), deviceHeaders(), big);
      expect(code).toBe(CloseCodes.MESSAGE_TOO_BIG);
      // Only that socket was closed: another session of the device still works.
      const { client } = await link(desktopA, device3, "r12-browser-3", { channel: 5 });
      await confirmed(client);
      expect((await client.call("workspace.list")).projects).toHaveLength(1);
      client.bye();
    });

    // ---- 11. no credential leaked -----------------------------------------------------------------------------
    await test.step("no headless process printed a credential, and the sealed store and key are private", async () => {
      expect(desktops.length).toBeGreaterThan(4);
      for (const desktop of desktops) expect(leaksOf(desktop)).toEqual([]);
      expect(modeProblems(dirA)).toEqual([]);
      expect(modeProblems(join(root, "b"))).toEqual([]);
    });
  } finally {
    for (const desktop of desktops) desktop.kill();
    rmSync(root, { recursive: true, force: true });
  }
});
