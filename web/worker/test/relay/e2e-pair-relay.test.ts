// End to end through the real API and the real DesktopRoom: pairing, device registration, grants, the Noise IK join
// of a device, confirmation by the desktop, revoke from the web, unpair, and logout. Only HTTP routes and
// WebSockets are used; the single direct D1 write is the clearing of the pair/poll window that `poll` does.
import { describe, expect, it } from "vitest";
import {
  decodeCtrlBody,
  DesktopGrantsResponse,
  DesktopMeResponse,
  DesktopsResponse,
  FrameType,
  GrantResponse,
  GrantsResponse,
  PairApproveResponse,
  PairInfoResponse,
  PairPollResponse,
  RegisterDeviceResponse,
  StateMessageSchema,
} from "@monoptah/protocol";
import { MockDesktop, fingerprint, generateKeyPair } from "@monoptah/mock-desktop";
import { TestClient } from "@monoptah/mock-desktop/test-client";
import { toBase64Url } from "../../src/lib/crypto.ts";
import { authedCall, loginAs } from "../support/accounts.ts";
import { WorkerdFrameSocket } from "./support/relay-adapter.ts";
import { approve, json, newAccount, poll, rawStart, reportState, sha256Base64Url, tokenCall } from "./support/api-support.ts";
import { desktopHeaders, desktopPath, deviceHeaders, devicePath, upgrade } from "./support/relay-support.ts";

async function open(path: string, headers: Record<string, string>): Promise<WorkerdFrameSocket> {
  const res = await upgrade(path, headers);
  if (res.status !== 101 || !res.webSocket) throw new Error(`upgrade answered ${res.status}`);
  return new WorkerdFrameSocket(res.webSocket);
}

/** A socket the Worker refuses is accepted and closed at once with the refusal code. */
async function refusalCode(path: string, headers: Record<string, string>): Promise<number | null> {
  const socket = await open(path, headers);
  await expect.poll(() => socket.closeCode, { timeout: 1000 }).not.toBeNull();
  return socket.closeCode;
}

function connectClient(desktopId: string, deviceId: string, socket: WorkerdFrameSocket, desktop: MockDesktop, deviceKey: ReturnType<typeof generateKeyPair>) {
  return TestClient.connect({ socket, desktopId, deviceId, desktopPublicKey: desktop.publicKey, deviceKey, deviceName: "Phone" });
}

describe("pair, relay, grant, revoke and unpair through the real API", () => {
  it("runs the whole life of a desktop and a device", async () => {
    const account = await newAccount();
    const desktopKey = generateKeyPair();
    const deviceKey = generateKeyPair();

    // 1. Desktop: pair/start; the user checks the fingerprint, approves; the desktop polls once for its token.
    const verifierBytes = crypto.getRandomValues(new Uint8Array(32));
    const startRes = await rawStart({
      publicKey: toBase64Url(desktopKey.publicKey),
      machineName: "Work Mac",
      os: "macos",
      appVersion: "1.0.0",
      challenge: await sha256Base64Url(verifierBytes),
    });
    expect(startRes.status).toBe(201);
    const start = (await startRes.json()) as { userCode: string; deviceCode: string };
    const verifier = toBase64Url(verifierBytes);

    const infoRes = await authedCall(account.session, `/api/pair/${start.userCode}`);
    expect(infoRes.status).toBe(200);
    expect(PairInfoResponse.parse(await infoRes.json()).fingerprint).toBe(fingerprint(desktopKey.publicKey));

    const approveRes = await approve(account.session, start.userCode);
    expect(approveRes.status).toBe(200);
    const approved = PairApproveResponse.parse(await approveRes.json());

    const polled = PairPollResponse.parse(await (await poll(start.deviceCode, verifier)).json());
    if (polled.status !== "approved") throw new Error(`first poll answered ${polled.status}`);
    expect(polled.desktopId).toBe(approved.desktopId);
    const desktopId = polled.desktopId;
    const token = polled.desktopToken;
    const secondPoll = await poll(start.deviceCode, verifier);
    const secondText = await secondPoll.text();
    expect(secondText).not.toContain(token);
    expect(secondText).not.toContain("desktopToken");

    // 2. The mock desktop joins the relay with the token; the token also opens /api/desktop/me.
    const desktop = new MockDesktop({ desktopId, staticKey: desktopKey });
    desktop.attach(await open(desktopPath(desktopId), desktopHeaders(token)));
    const meRes = await tokenCall("/api/desktop/me", token);
    expect(meRes.status).toBe(200);
    expect(DesktopMeResponse.parse(await meRes.json()).desktopId).toBe(desktopId);

    // 3. Browser: register the device, request the grant; the desktop is told over CTRL and lists the pending grant.
    const deviceRes = await authedCall(account.session, "/api/devices", json({ name: "Phone", publicKey: toBase64Url(deviceKey.publicKey) }));
    expect(deviceRes.status).toBe(201);
    const { deviceId } = RegisterDeviceResponse.parse(await deviceRes.json());
    const grantRes = await authedCall(account.session, `/api/desktops/${desktopId}/grants`, json({ deviceId }));
    expect([200, 201]).toContain(grantRes.status);
    const requested = GrantResponse.parse(await grantRes.json()).grant;
    expect(requested).toMatchObject({ deviceId, deviceName: "Phone", state: "pending", fingerprint: fingerprint(deviceKey.publicKey) });

    await expect
      .poll(() => desktop.relayFrames.some((f) => f.type === FrameType.CTRL && decodeCtrlBody(f.body).t === "grants"), { timeout: 1000 })
      .toBe(true);
    const listRes = await tokenCall("/api/desktop/grants", token);
    expect(listRes.status).toBe(200);
    expect(DesktopGrantsResponse.parse(await listRes.json()).grants).toMatchObject([
      { deviceId, deviceName: "Phone", publicKey: toBase64Url(deviceKey.publicKey), fingerprint: fingerprint(deviceKey.publicKey), state: "pending", permission: null },
    ]);

    // The device joins with its cookie and Origin; Noise IK completes; the desktop does not know the device: pending.
    const pendingSocket = await open(devicePath(desktopId, deviceId), deviceHeaders(account.session));
    const pendingClient = await connectClient(desktopId, deviceId, pendingSocket, desktop, deviceKey);
    expect(pendingClient.state).toEqual({ type: "state", state: "pending" });

    // 4. The desktop confirms, locally and then to the Worker. A new connection is confirmed and a call works.
    pendingClient.bye();
    pendingSocket.close(1000);
    await expect.poll(() => desktop.sessions.length).toBe(0);
    desktop.confirmDevice(fingerprint(deviceKey.publicKey), "control");
    const stateRes = await reportState(token, deviceId, { state: "confirmed", permission: "control" });
    expect(stateRes.status).toBe(200);

    const socket = await open(devicePath(desktopId, deviceId), deviceHeaders(account.session));
    const client = await connectClient(desktopId, deviceId, socket, desktop, deviceKey);
    expect(StateMessageSchema.parse(client.state)).toMatchObject({ state: "confirmed", permission: "control" });
    expect((await client.call("workspace.list")).projects.length).toBeGreaterThan(0);

    const grantsRes = await authedCall(account.session, `/api/desktops/${desktopId}/grants`);
    expect(GrantsResponse.parse(await grantsRes.json()).grants).toMatchObject([{ deviceId, state: "confirmed", permission: "control" }]);
    const desktopsRes = await authedCall(account.session, "/api/desktops");
    const { desktops } = DesktopsResponse.parse(await desktopsRes.json());
    expect(desktops).toHaveLength(1);
    expect(desktops[0]).toMatchObject({ id: desktopId, online: true });

    // 5. Revoke from the web (fresh session): the device socket closes with 4003 within 1 s; a new join is refused 4003.
    const revokeStart = Date.now();
    const revokeRes = await authedCall(account.session, `/api/desktops/${desktopId}/grants/${deviceId}`, json({ state: "revoked" }, "PUT"));
    expect(revokeRes.status).toBe(200);
    await expect.poll(() => socket.closeCode, { timeout: 1000 }).toBe(4003);
    expect(Date.now() - revokeStart).toBeLessThan(1000);
    expect(await refusalCode(devicePath(desktopId, deviceId), deviceHeaders(account.session))).toBe(4003);

    // 6. Unpair from the desktop: its socket closes with 4003 within 1 s, the token is dead, a rejoin is refused.
    const unpairStart = Date.now();
    const unpairRes = await tokenCall("/api/desktop/me", token, { method: "DELETE" });
    expect(unpairRes.status).toBe(200);
    await expect.poll(() => desktop.sessions.length).toBe(0);
    // A revoked token is a known token that is refused: 403 (an unknown one would be 401).
    const afterUnpair = await tokenCall("/api/desktop/me", token);
    expect(afterUnpair.status).toBe(403);
    expect(Date.now() - unpairStart).toBeLessThan(1000);
    expect(await refusalCode(desktopPath(desktopId), desktopHeaders(token))).toBe(4003);
    desktop.close();
  });

  it("closes the desktop socket with 4003 within 1 s of unpair", async () => {
    const account = await newAccount();
    const desktopKey = generateKeyPair();
    const verifierBytes = crypto.getRandomValues(new Uint8Array(32));
    const start = (await (await rawStart({ publicKey: toBase64Url(desktopKey.publicKey), machineName: "Mac", os: "macos", appVersion: "1.0.0", challenge: await sha256Base64Url(verifierBytes) })).json()) as { userCode: string; deviceCode: string };
    expect((await approve(account.session, start.userCode)).status).toBe(200);
    const polled = PairPollResponse.parse(await (await poll(start.deviceCode, toBase64Url(verifierBytes))).json());
    if (polled.status !== "approved") throw new Error(`poll answered ${polled.status}`);

    const socket = await open(desktopPath(polled.desktopId), desktopHeaders(polled.desktopToken));
    const started = Date.now();
    expect((await tokenCall("/api/desktop/me", polled.desktopToken, { method: "DELETE" })).status).toBe(200);
    await expect.poll(() => socket.closeCode, { timeout: 1000 }).toBe(4003);
    expect(Date.now() - started).toBeLessThan(1000);
    expect(await refusalCode(desktopPath(polled.desktopId), desktopHeaders(polled.desktopToken))).toBe(4003);
  });

  it("closes the device socket of a session with 4001 within 1 s of that session's logout", async () => {
    const account = await newAccount();
    const second = await loginAs(account.user.id);
    const desktopKey = generateKeyPair();
    const deviceKey = generateKeyPair();
    const verifierBytes = crypto.getRandomValues(new Uint8Array(32));
    const start = (await (await rawStart({ publicKey: toBase64Url(desktopKey.publicKey), machineName: "Mac", os: "macos", appVersion: "1.0.0", challenge: await sha256Base64Url(verifierBytes) })).json()) as { userCode: string; deviceCode: string };
    expect((await approve(account.session, start.userCode)).status).toBe(200);
    const polled = PairPollResponse.parse(await (await poll(start.deviceCode, toBase64Url(verifierBytes))).json());
    if (polled.status !== "approved") throw new Error(`poll answered ${polled.status}`);
    const { desktopId, desktopToken } = polled;
    const desktop = new MockDesktop({ desktopId, staticKey: desktopKey });
    desktop.attach(await open(desktopPath(desktopId), desktopHeaders(desktopToken)));
    desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: "control" });

    const { deviceId } = RegisterDeviceResponse.parse(await (await authedCall(account.session, "/api/devices", json({ name: "Phone", publicKey: toBase64Url(deviceKey.publicKey) }))).json());
    expect([200, 201]).toContain((await authedCall(account.session, `/api/desktops/${desktopId}/grants`, json({ deviceId }))).status);
    expect((await reportState(desktopToken, deviceId, { state: "confirmed", permission: "control" })).status).toBe(200);

    // The device joins with the second session; the first session stays valid.
    const socket = await open(devicePath(desktopId, deviceId), deviceHeaders(second));
    const client = await connectClient(desktopId, deviceId, socket, desktop, deviceKey);
    expect(client.state).toMatchObject({ state: "confirmed" });

    const started = Date.now();
    expect((await authedCall(second, "/api/auth/logout", { method: "POST" })).status).toBe(200);
    await expect.poll(() => socket.closeCode, { timeout: 1000 }).toBe(4001);
    expect(Date.now() - started).toBeLessThan(1000);
    expect((await authedCall(account.session, "/api/me")).status).toBe(200);
    desktop.close();
  });
});
