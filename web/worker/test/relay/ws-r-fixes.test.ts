// Fixes of the second WS-R review: token rate limits, pairing mint guards, no-op grant reports, presence cache,
// pending socket cap and the pure presence decision.
import { PRESENCE_TIMEOUT_MS } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { sha256Hex } from "../../src/lib/crypto.ts";
import { TOKEN_API_LIMIT, TOKEN_RELAY_LIMIT } from "../../src/modules/desktops/token.ts";
import { isDesktopOnline } from "../../src/relay/presence.ts";
import { invalidatePresence, presenceOf, revokeInRooms } from "../../src/relay/rooms.ts";
import { authedCall, freshIp } from "../support/accounts.ts";
import { callWithEnv } from "../helpers.ts";
import {
  addDevice,
  addToken,
  addGrant,
  closeAll,
  connect,
  connectDesktop,
  connectDevice,
  devicePath,
  deviceHeaders,
  desktopPath,
  hs1,
  roomStub,
  seedRoom,
  upgrade,
  Sock,
} from "./support/relay-support.ts";
import { approve, auditActions, env, fakeRoom, grantRow, json, newAccount, pairDesktop, poll, registerDevice, reportState, requestGrant, startPairing, tokenCall, withEnv } from "./support/api-support.ts";

async function pairingStatus(deviceCode: string): Promise<string> {
  const row = await env.DB.prepare("SELECT status FROM pairing_requests WHERE device_code_hash = ?").bind(await sha256Hex(deviceCode)).first<{ status: string }>();
  return row?.status ?? "";
}

describe("desktop token rate limits (in memory, per token)", () => {
  it("answers 429 rate_limited with Retry-After after 60 API calls, before any D1 lookup", async () => {
    // An unknown token: every call before the limit is 401, so the 61st being 429 shows the limit comes first.
    const token = `unknown-${crypto.randomUUID()}`;
    for (let i = 0; i < TOKEN_API_LIMIT; i++) expect((await tokenCall("/api/desktop/me", token)).status).toBe(401);
    const limited = await tokenCall("/api/desktop/grants", token);
    expect(limited.status).toBe(429);
    expect(await limited.json()).toMatchObject({ code: "rate_limited" });
    expect(Number(limited.headers.get("Retry-After"))).toBeGreaterThanOrEqual(1);
  });

  it("does not limit one token because another is busy", async () => {
    const busy = `unknown-${crypto.randomUUID()}`;
    for (let i = 0; i <= TOKEN_API_LIMIT; i++) await tokenCall("/api/desktop/me", busy);
    const other = await newAccount();
    const paired = await pairDesktop(other);
    expect((await tokenCall("/api/desktop/me", paired.token)).status).toBe(200);
  });

  it("accepts and closes 4008 the 13th relay upgrade of one token in a minute", async () => {
    const f = await seedRoom();
    const socks: Sock[] = [];
    for (let i = 0; i < TOKEN_RELAY_LIMIT; i++) socks.push(await connectDesktop(f));
    const res = await upgrade(desktopPath(f.desktopId), { Authorization: `Bearer ${f.token}` });
    expect(res.status).toBe(101);
    const ws = res.webSocket as WebSocket;
    const closed = new Promise<number>((resolve) => ws.addEventListener("close", (e) => resolve(e.code)));
    ws.accept();
    expect(await closed).toBe(4008);
    await closeAll(...socks);
  });
});

describe("desktop grant state report", () => {
  it("is a no-op (200, no UPDATE, no audit row) when state and permission are unchanged", async () => {
    const mine = await newAccount();
    const desktop = await pairDesktop(mine);
    const device = await registerDevice(mine);
    await requestGrant(mine, desktop.desktopId, device.deviceId);
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" })).status).toBe(200);
    const before = await grantRow(desktop.desktopId, device.deviceId);
    const auditBefore = (await auditActions(mine.user.id)).length;
    // Same report again, and a pending-on-pending report on another device.
    await env.DB.prepare("UPDATE device_grants SET updated_at = 5 WHERE device_id = ?").bind(device.deviceId).run();
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" })).status).toBe(200);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.updated_at).toBe(5);
    expect((await auditActions(mine.user.id)).length).toBe(auditBefore);
    // A different permission still writes.
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" })).status).toBe(200);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.permission).toBe("control");
    expect(before?.permission).toBe("read");
  });

  it("calls the room again for a revoked report on a grant the server already revoked, writing nothing", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const desktop = await pairDesktop(mine, { testEnv: roomEnv });
    const device = await registerDevice(mine, "Phone", roomEnv);
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv);
    room.calls.revokes.length = 0;
    const audits = (await auditActions(mine.user.id)).length;
    expect((await reportState(desktop.token, device.deviceId, { state: "revoked" }, roomEnv)).status).toBe(200);
    expect(room.calls.revokes).toEqual([{ desktopId: desktop.desktopId, target: { deviceId: device.deviceId }, code: 4003 }]);
    expect((await auditActions(mine.user.id)).length).toBe(audits);
  });
});

describe("grant request notifies the desktop only for a new grant", () => {
  it("calls notifyGrants once, not for the existing grant", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const desktop = await pairDesktop(mine, { testEnv: roomEnv });
    const device = await registerDevice(mine, "Phone", roomEnv);
    room.calls.notifies.length = 0;
    expect((await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv)).status).toBe(201);
    expect((await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv)).status).toBe(200);
    expect(room.calls.notifies).toEqual([desktop.desktopId]);
  });
});

describe("pairing token mint guards", () => {
  /** Approves a replace of `desktopId` and returns the pairing, not yet polled. */
  async function approveReplace(mine: Awaited<ReturnType<typeof newAccount>>, desktopId: string) {
    const pairing = await startPairing();
    expect((await approve(mine.session, pairing.userCode, { replaces: desktopId })).status).toBe(200);
    return pairing;
  }

  it("approve A replaces D, approve B replaces D, B polls, then A gets no token", async () => {
    const mine = await newAccount();
    const d = await pairDesktop(mine);
    const a = await approveReplace(mine, d.desktopId);
    const b = await approveReplace(mine, d.desktopId);
    expect(await pairingStatus(a.deviceCode)).toBe("denied");
    const polledB = (await (await poll(b.deviceCode, b.verifier)).json()) as { status: string; desktopToken?: string };
    expect(polledB.status).toBe("approved");
    const polledA = (await (await poll(a.deviceCode, a.verifier)).json()) as { status: string; desktopToken?: string };
    expect(polledA.status).toBe("denied");
    expect(polledA.desktopToken).toBeUndefined();
    const { results } = await env.DB.prepare("SELECT token_hash FROM desktop_tokens WHERE desktop_id = ? AND revoked_at IS NULL").bind(d.desktopId).all();
    expect(results).toHaveLength(1);
  });

  it("mints no token when the desktop key was replaced after the approval", async () => {
    const mine = await newAccount();
    const pairing = await startPairing();
    expect((await approve(mine.session, pairing.userCode)).status).toBe(200);
    await env.DB.prepare("UPDATE desktops SET public_key = 'replaced-key' WHERE id = (SELECT desktop_id FROM pairing_requests WHERE device_code_hash = ?)").bind(await sha256Hex(pairing.deviceCode)).run();
    const polled = (await (await poll(pairing.deviceCode, pairing.verifier)).json()) as { status: string; desktopToken?: string };
    expect(polled.status).toBe("expired");
    expect(polled.desktopToken).toBeUndefined();
    const count = await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = (SELECT desktop_id FROM pairing_requests WHERE device_code_hash = ?)").bind(await sha256Hex(pairing.deviceCode)).first<{ n: number }>();
    expect(count?.n).toBe(0);
  });

  it("collects the token in the 60 s grace after expiry, once, and not after the grace", async () => {
    const mine = await newAccount();
    const late = await startPairing();
    expect((await approve(mine.session, late.userCode)).status).toBe(200);
    // Approved at expires_at - 1, polled at expires_at + 2.
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(Math.floor(Date.now() / 1000) - 2, await sha256Hex(late.deviceCode)).run();
    const first = (await (await poll(late.deviceCode, late.verifier)).json()) as { status: string; desktopToken?: string };
    expect(first.status).toBe("approved");
    expect(first.desktopToken).toBeTruthy();
    expect(((await (await poll(late.deviceCode, late.verifier)).json()) as { status: string }).status).toBe("expired");
    const tooLate = await startPairing();
    expect((await approve(mine.session, tooLate.userCode)).status).toBe(200);
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = ? WHERE device_code_hash = ?").bind(Math.floor(Date.now() / 1000) - 61, await sha256Hex(tooLate.deviceCode)).run();
    const none = (await (await poll(tooLate.deviceCode, tooLate.verifier)).json()) as { status: string; desktopToken?: string };
    expect(none.status).toBe("expired");
    expect(none.desktopToken).toBeUndefined();
  });

  it("mints no token for an approved request that expired before the poll", async () => {
    const mine = await newAccount();
    const pairing = await startPairing();
    expect((await approve(mine.session, pairing.userCode)).status).toBe(200);
    await env.DB.prepare("UPDATE pairing_requests SET expires_at = 1 WHERE device_code_hash = ?").bind(await sha256Hex(pairing.deviceCode)).run();
    const polled = (await (await poll(pairing.deviceCode, pairing.verifier)).json()) as { status: string; desktopToken?: string };
    expect(polled.status).toBe("expired");
    expect(polled.desktopToken).toBeUndefined();
  });

  it("clears a stale requested_permission when the desktop re-reports the same confirmed permission", async () => {
    const mine = await newAccount();
    const desktop = await pairDesktop(mine);
    const device = await registerDevice(mine);
    await requestGrant(mine, desktop.desktopId, device.deviceId);
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" })).status).toBe(200);
    await env.DB.prepare("UPDATE device_grants SET requested_permission = 'read' WHERE device_id = ?").bind(device.deviceId).run();
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" })).status).toBe(200);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.requested_permission).toBeNull();
  });

  it("denies other approved requests of the desktop on unpair (DELETE /api/desktop/me)", async () => {
    const mine = await newAccount();
    const d = await pairDesktop(mine);
    const other = await approveReplace(mine, d.desktopId);
    // The replace revoked the old tokens; a new token (as a poll would mint) lets the desktop unpair.
    const token = await addToken(d.desktopId);
    expect(await pairingStatus(other.deviceCode)).toBe("approved");
    expect((await tokenCall("/api/desktop/me", token, { method: "DELETE" })).status).toBe(200);
    expect(await pairingStatus(other.deviceCode)).toBe("denied");
  });

  it("denies approved requests of a deleted desktop (DELETE /api/desktops/:id)", async () => {
    const mine = await newAccount();
    const d = await pairDesktop(mine);
    const a = await approveReplace(mine, d.desktopId);
    expect((await authedCall(mine.session, `/api/desktops/${d.desktopId}`, { method: "DELETE" })).status).toBe(200);
    expect(await pairingStatus(a.deviceCode)).toBe("denied");
  });

  it("does not deny the approved requests of another user's desktop when a delete is refused", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const d = await pairDesktop(theirs);
    const a = await approveReplace(theirs, d.desktopId);
    expect((await authedCall(mine.session, `/api/desktops/${d.desktopId}`, { method: "DELETE" })).status).toBe(404);
    expect(await pairingStatus(a.deviceCode)).toBe("approved");
  });
});

describe("pair/poll rate limit as seen by a real client", () => {
  it("answers the second poll in a row with 429 rate_limited and Retry-After (the middleware wins over slow_down)", async () => {
    const pairing = await startPairing();
    const send = () =>
      callWithEnv(env, "/api/pair/poll", {
        method: "POST",
        headers: { "CF-Connecting-IP": freshIp(), "Content-Type": "application/json" },
        body: JSON.stringify({ deviceCode: pairing.deviceCode, verifier: pairing.verifier }),
      });
    const first = await send();
    expect(first.status).toBe(200);
    expect(await first.json()).toEqual({ status: "pending" });
    const second = await send();
    expect(second.status).toBe(429);
    expect(await second.json()).toMatchObject({ code: "rate_limited" });
    expect(Number(second.headers.get("Retry-After"))).toBeGreaterThanOrEqual(1);
  });
});

describe("presence cache of GET /api/desktops", () => {
  it("asks each room once within 10 s and again after a revoke in that room", async () => {
    let calls = 0;
    const ns = {
      idFromName: (name: string) => ({ name }),
      get: () => ({
        presence: () => {
          calls += 1;
          return Promise.resolve({ online: true });
        },
        revoke: () => Promise.resolve(),
      }),
    } as unknown as typeof env.ROOM;
    const id = crypto.randomUUID();
    invalidatePresence();
    expect((await presenceOf({ ROOM: ns }, [id])).get(id)).toBe(true);
    expect((await presenceOf({ ROOM: ns }, [id])).get(id)).toBe(true);
    expect(calls).toBe(1);
    await revokeInRooms({ ROOM: ns }, [id], { all: true }, 4003);
    await presenceOf({ ROOM: ns }, [id]);
    expect(calls).toBe(2);
  });

  it("stays bounded", async () => {
    const ns = { idFromName: (name: string) => ({ name }), get: () => ({ presence: () => Promise.resolve({ online: false }) }) } as unknown as typeof env.ROOM;
    invalidatePresence();
    const ids = Array.from({ length: 1100 }, () => crypto.randomUUID());
    await presenceOf({ ROOM: ns }, ids);
    let calls = 0;
    const counting = { idFromName: (name: string) => ({ name }), get: () => ({ presence: () => { calls += 1; return Promise.resolve({ online: false }); } }) } as unknown as typeof env.ROOM;
    await presenceOf({ ROOM: counting }, ids);
    // The oldest entries were dropped: they are asked again, the newest are served from the cache.
    expect(calls).toBeGreaterThan(0);
    expect(calls).toBeLessThan(ids.length);
  });
});

describe("pending grant sockets", () => {
  it("caps device sockets of pending grants at 4 per room (4010), still admits a confirmed device, and ignores a client x-grant-state", async () => {
    const f = await seedRoom();
    const pendingIds: string[] = [];
    for (let i = 0; i < 3; i++) {
      const id = await addDevice(f.userId);
      await addGrant(id, f.desktopId, "pending");
      pendingIds.push(id);
    }
    const socks: Sock[] = [];
    // Two sockets each for the first two pending devices (a spoofed header must not help).
    for (const id of pendingIds.slice(0, 2)) {
      for (let i = 0; i < 2; i++) socks.push(await connect(devicePath(f.desktopId, id), { ...deviceHeaders(f.session), "x-grant-state": "confirmed" }));
    }
    const fifth = await connectDevice(f, pendingIds[2] as string);
    expect(await fifth.closed()).toBe(4010);
    expect(socks.every((s) => s.closeCode === null)).toBe(true);
    // The confirmed device of the fixture is not affected.
    const confirmed = await connectDevice(f, f.deviceId);
    await confirmed.until(() => confirmed.ctrl().length === 1);
    expect(confirmed.closeCode).toBeNull();
    await closeAll(...socks, confirmed);
  });
});

describe("stale pending sockets", () => {
  it("lets a 5th pending device join after the desktop confirmed a grant with 4 pending sockets", async () => {
    const f = await seedRoom();
    const first = await addDevice(f.userId);
    const second = await addDevice(f.userId);
    const third = await addDevice(f.userId);
    for (const id of [first, second, third]) await addGrant(id, f.desktopId, "pending");
    const socks: Sock[] = [];
    for (const id of [first, first, second, second]) socks.push(await connectDevice(f, id));
    const blocked = await connectDevice(f, third);
    expect(await blocked.closed()).toBe(4010);
    const confirm = await tokenCall(`/api/desktop/grants/${first}/state`, f.token, json({ state: "confirmed", permission: "read" }));
    expect(confirm.status).toBe(200);
    const joined = await connectDevice(f, third);
    await joined.until(() => joined.ctrl().length === 1);
    expect(joined.closeCode).toBeNull();
    await closeAll(...socks, joined);
  });

  it("treats a device socket without a valid x-grant-state header as pending (fail closed)", async () => {
    const f = await seedRoom();
    const stub = roomStub(f.desktopId);
    const socks: Sock[] = [];
    const join = async (deviceId: string, extra: Record<string, string>): Promise<Sock> => {
      const res = await stub.fetch(`https://room/join`, {
        headers: { Upgrade: "websocket", "x-role": "device", "x-desktop-id": f.desktopId, "x-user-id": f.userId, "x-device-id": deviceId, "x-session-id-hash": "h", ...extra },
      });
      return new Sock(res.webSocket as WebSocket);
    };
    const variants: Record<string, string>[] = [{}, { "x-grant-state": "bogus" }, {}, { "x-grant-state": "bogus" }];
    for (const [i, extra] of variants.entries()) socks.push(await join(i < 2 ? f.deviceId : await addDevice(f.userId), extra));
    const fifth = await join(await addDevice(f.userId), {});
    expect(await fifth.closed()).toBe(4010);
    await closeAll(...socks);
  });
});

describe("rate breaches", () => {
  it("closes the desktop 4008 after 3 breaches of its 200 msg/s outbound rate", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    for (let i = 0; i < 320; i++) desktop.send(hs1(f.deviceId, 1, 4));
    expect(await desktop.closed()).toBe(4008);
  });

  it("closes a device 4008 when it breaches the 1 MiB/s byte rate with few messages", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    // 40 frames of about 64 KB: under the 100 message burst, over 1 MiB.
    for (let i = 0; i < 40; i++) device.send(hs1(f.deviceId, 1, 64_000));
    expect(await device.closed()).toBe(4008);
  });
});

describe("isDesktopOnline", () => {
  it("is online until 90 s after the last ping, or after the join when there was none", () => {
    const joined = 1_000_000;
    expect(isDesktopOnline(joined + PRESENCE_TIMEOUT_MS, joined, null)).toBe(true);
    expect(isDesktopOnline(joined + PRESENCE_TIMEOUT_MS + 1, joined, null)).toBe(false);
    const ping = joined + 60_000;
    expect(isDesktopOnline(ping + PRESENCE_TIMEOUT_MS, joined, ping)).toBe(true);
    expect(isDesktopOnline(ping + PRESENCE_TIMEOUT_MS + 1, joined, ping)).toBe(false);
    // A ping keeps a socket online that the join time alone would have timed out.
    expect(isDesktopOnline(joined + PRESENCE_TIMEOUT_MS + 5_000, joined, ping)).toBe(true);
  });
});
