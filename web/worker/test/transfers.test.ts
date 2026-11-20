// Encrypted file transfers (modules/transfers): authorization of R2 put, head and get, immutability, object naming,
// ranges, and the rule that no body, key or object name is ever logged.
import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_SEALED_BYTES, OBJECT_NAME, objectKey, parseRange, sealedSize } from "../src/modules/transfers/index.ts";
import { authedCall } from "./support/accounts.ts";
import { env, fakeRoom, newAccount, pairDesktop, registerDevice, reportState, requestGrant, tokenCall, withEnv } from "./relay/support/api-support.ts";
import { call } from "./helpers.ts";

const HASH = "a".repeat(64);
const OBJECT = `${HASH}.${"b".repeat(16)}`;

async function setup() {
  const room = fakeRoom();
  const roomEnv = withEnv({ ROOM: room.namespace });
  const mine = await newAccount();
  const desktop = await pairDesktop(mine, { testEnv: roomEnv });
  const device = await registerDevice(mine, "Laptop", roomEnv);
  await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
  await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
  return { roomEnv, mine, desktop, device, room };
}

function bytes(length: number, seed = 1): Uint8Array {
  return Uint8Array.from({ length }, (_, i) => (i * 7 + seed) % 251);
}

function put(token: string | null, object: string, body: Uint8Array, headers: Record<string, string> = {}) {
  return tokenCall(`/api/desktop/transfers/${object}`, token, { method: "PUT", body, headers: { "Content-Type": "application/octet-stream", "Content-Length": String(body.length), ...headers } });
}

function download(session: { cookie: string }, desktopId: string, deviceId: string, object: string, headers: Record<string, string> = {}, testEnv?: Parameters<typeof authedCall>[3]) {
  return authedCall(session, `/api/transfers/${desktopId}/${object}?device=${deviceId}`, { headers }, testEnv);
}

afterEach(() => vi.restoreAllMocks());

describe("object names and keys", () => {
  it("accepts a content hash and a key id and nothing else", () => {
    expect(OBJECT_NAME.test(OBJECT)).toBe(true);
    for (const bad of ["", "../x", `${HASH}`, `${HASH}.${"b".repeat(15)}`, `${HASH.toUpperCase()}.${"b".repeat(16)}`, `${OBJECT}/x`, `${OBJECT}%2e`]) {
      expect(OBJECT_NAME.test(bad), bad).toBe(false);
    }
  });

  it("keys an object by the transfers prefix, the desktop and the name, never a path", () => {
    expect(objectKey("d1", OBJECT)).toBe(`transfers/d1/${OBJECT}`);
  });

  it("computes the sealed size as the desktop does", () => {
    expect(sealedSize(0)).toBe(16);
    expect(sealedSize(1)).toBe(17);
    expect(sealedSize(1024 * 1024)).toBe(1024 * 1024 + 16);
    expect(sealedSize(1024 * 1024 + 1)).toBe(1024 * 1024 + 1 + 32);
    expect(MAX_SEALED_BYTES).toBe(sealedSize(64 * 1024 * 1024));
  });

  it("parses the simple ranges and refuses the rest", () => {
    expect(parseRange("bytes=0-99", 1000)).toEqual({ offset: 0, length: 100 });
    expect(parseRange("bytes=900-", 1000)).toEqual({ offset: 900, length: 100 });
    expect(parseRange("bytes=900-5000", 1000)).toEqual({ offset: 900, length: 100 });
    expect(parseRange("bytes=1000-", 1000)).toBe("unsatisfiable");
    expect(parseRange("bytes=5-2", 1000)).toBe("unsatisfiable");
    expect(parseRange("bytes=-5", 1000)).toBeNull();
    expect(parseRange("bytes=0-1,5-6", 1000)).toBeNull();
    expect(parseRange(undefined, 1000)).toBeNull();
  });
});

describe("desktop upload (PUT and HEAD)", () => {
  it("stores ciphertext under the desktop's own prefix and reports its size", async () => {
    const { desktop } = await setup();
    const sealed = bytes(sealedSize(100));
    expect((await tokenCall(`/api/desktop/transfers/${OBJECT}`, desktop.token, { method: "HEAD" })).status).toBe(404);
    const res = await put(desktop.token, OBJECT, sealed);
    expect(res.status).toBe(201);
    expect(await res.json()).toEqual({ ok: true, stored: true });
    const head = await tokenCall(`/api/desktop/transfers/${OBJECT}`, desktop.token, { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(head.headers.get("Content-Length")).toBe(String(sealed.length));
    const listed = await env.TRANSFERS.list({ prefix: "transfers/" });
    expect(listed.objects.map((o) => o.key)).toContain(`transfers/${desktop.desktopId}/${OBJECT}`);
  });

  it("never replaces an object: a second upload under the same name keeps the first", async () => {
    const { desktop, mine, device } = await setup();
    const first = bytes(48, 1);
    const second = bytes(48, 9);
    expect((await put(desktop.token, OBJECT, first)).status).toBe(201);
    const again = await put(desktop.token, OBJECT, second);
    expect(again.status).toBe(200);
    expect(await again.json()).toEqual({ ok: true, stored: false });
    const got = await download(mine.session, desktop.desktopId, device.deviceId, OBJECT);
    expect(new Uint8Array(await got.arrayBuffer())).toEqual(first);
  });

  it("answers a plain GET on the HEAD route with 405, not with content", async () => {
    const { desktop } = await setup();
    await put(desktop.token, OBJECT, bytes(32));
    expect((await tokenCall(`/api/desktop/transfers/${OBJECT}`, desktop.token, { method: "GET" })).status).toBe(405);
  });

  it("needs the desktop token: no token, a bad token and a session cookie are refused", async () => {
    const { desktop, mine } = await setup();
    expect((await put(null, OBJECT, bytes(32))).status).toBe(401);
    expect((await put("not-a-token", OBJECT, bytes(32))).status).toBe(401);
    const viaCookie = await authedCall(mine.session, `/api/desktop/transfers/${OBJECT}`, { method: "PUT", body: bytes(32), headers: { "Content-Length": "32" } });
    expect(viaCookie.status).toBe(401);
    expect((await tokenCall(`/api/desktop/transfers/${OBJECT}`, null, { method: "HEAD" })).status).toBe(401);
    expect((await env.TRANSFERS.list({ prefix: `transfers/${desktop.desktopId}/` })).objects).toHaveLength(0);
  });

  it("keeps every desktop in its own prefix", async () => {
    const roomEnv = withEnv({ ROOM: fakeRoom().namespace });
    const mine = await newAccount();
    const a = await pairDesktop(mine, { testEnv: roomEnv });
    const other = await newAccount();
    const b = await pairDesktop(other, { testEnv: roomEnv });
    expect((await put(a.token, OBJECT, bytes(40))).status).toBe(201);
    // The same name under another desktop is another object.
    expect((await tokenCall(`/api/desktop/transfers/${OBJECT}`, b.token, { method: "HEAD" })).status).toBe(404);
    expect((await tokenCall(`/api/desktop/transfers/${OBJECT}`, a.token, { method: "HEAD" })).status).toBe(200);
  });

  it("refuses a bad object name, a missing or huge Content-Length and a body that is not what it declares", async () => {
    const { desktop } = await setup();
    for (const name of ["..", "abc", `${HASH}.zz`, "x".repeat(300)]) {
      expect((await put(desktop.token, name, bytes(32))).status, name).toBeGreaterThanOrEqual(400);
    }
    expect((await put(desktop.token, OBJECT, bytes(32), { "Content-Length": String(MAX_SEALED_BYTES + 1) })).status).toBe(413);
    expect((await put(desktop.token, OBJECT, bytes(8))).status).toBe(400);
    expect((await put(desktop.token, OBJECT, bytes(40), { "Content-Length": "abc" })).status).toBe(411);
    expect((await env.TRANSFERS.list({ prefix: "transfers/" })).objects.filter((o) => o.key.includes(desktop.desktopId))).toHaveLength(0);
  });
});

describe("device download (GET)", () => {
  it("serves the bytes to a confirmed device of the owner, with no-store headers", async () => {
    const { desktop, mine, device } = await setup();
    const sealed = bytes(sealedSize(5000));
    await put(desktop.token, OBJECT, sealed);
    const res = await download(mine.session, desktop.desktopId, device.deviceId, OBJECT);
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toBe("application/octet-stream");
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    expect(res.headers.get("Accept-Ranges")).toBe("bytes");
    expect(res.headers.get("X-Content-Type-Options")).toBe("nosniff");
    // Never a page: attachment, and a sandbox CSP for anything that still navigates to it.
    expect(res.headers.get("Content-Disposition")).toMatch(/^attachment;/);
    expect(res.headers.get("Content-Security-Policy")).toBe("sandbox");
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(sealed);
  });

  it("serves a range as 206 with Content-Range, and 416 past the end", async () => {
    const { desktop, mine, device } = await setup();
    const sealed = bytes(sealedSize(1000));
    await put(desktop.token, OBJECT, sealed);
    const part = await download(mine.session, desktop.desktopId, device.deviceId, OBJECT, { Range: "bytes=10-19" });
    expect(part.status).toBe(206);
    expect(part.headers.get("Content-Range")).toBe(`bytes 10-19/${sealed.length}`);
    expect(new Uint8Array(await part.arrayBuffer())).toEqual(sealed.slice(10, 20));
    const past = await download(mine.session, desktop.desktopId, device.deviceId, OBJECT, { Range: `bytes=${sealed.length}-` });
    expect(past.status).toBe(416);
  });

  it("needs a session, and a confirmed grant of this device for this desktop", async () => {
    const { desktop, mine, device, roomEnv } = await setup();
    await put(desktop.token, OBJECT, bytes(64));
    expect((await call(`/api/transfers/${desktop.desktopId}/${OBJECT}?device=${device.deviceId}`)).status).toBe(401);
    // Another user, with a device of their own.
    const stranger = await newAccount();
    const theirs = await registerDevice(stranger, "Theirs", roomEnv);
    expect((await download(stranger.session, desktop.desktopId, theirs.deviceId, OBJECT)).status).toBe(404);
    expect((await download(stranger.session, desktop.desktopId, device.deviceId, OBJECT)).status).toBe(404);
    // A device that was never confirmed.
    const pending = await registerDevice(mine, "Pending", roomEnv);
    await requestGrant(mine, desktop.desktopId, pending.deviceId, roomEnv);
    expect((await download(mine.session, desktop.desktopId, pending.deviceId, OBJECT)).status).toBe(404);
    // A device with no grant on this desktop.
    const free = await registerDevice(mine, "Free", roomEnv);
    expect((await download(mine.session, desktop.desktopId, free.deviceId, OBJECT)).status).toBe(404);
    // A grant that was revoked.
    await reportState(desktop.token, device.deviceId, { state: "revoked" }, roomEnv);
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT)).status).toBe(404);
  });

  it("binds the device id to the session that holds its link", async () => {
    const { desktop, mine, device, room, roomEnv } = await setup();
    await put(desktop.token, OBJECT, bytes(64));
    // This session holds the link of the device: allowed. Another session of the same user borrowing the id: refused.
    room.deviceSessionHashes.set(device.deviceId, [mine.session.idHash]);
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT, {}, roomEnv)).status).toBe(200);
    room.deviceSessionHashes.set(device.deviceId, ["f".repeat(64)]);
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT, {}, roomEnv)).status).toBe(404);
    // No live link: the confirmed grant decides, as before.
    room.deviceSessionHashes.delete(device.deviceId);
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT, {}, roomEnv)).status).toBe(200);
  });

  it("cannot reach an object of another desktop, even of the same user", async () => {
    const { desktop, mine, device, roomEnv } = await setup();
    const second = await pairDesktop(mine, { testEnv: roomEnv, machineName: "Second" });
    await put(second.token, OBJECT, bytes(64));
    // The device is confirmed on the first desktop only; the object is under the second one's prefix.
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT)).status).toBe(404);
    expect((await download(mine.session, second.desktopId, device.deviceId, OBJECT)).status).toBe(404);
  });

  it("answers a missing object and bad parameters without telling them apart from a refusal", async () => {
    const { desktop, mine, device } = await setup();
    expect((await download(mine.session, desktop.desktopId, device.deviceId, OBJECT)).status).toBe(404);
    expect((await download(mine.session, desktop.desktopId, "not-a-uuid", OBJECT)).status).toBe(400);
    expect((await download(mine.session, desktop.desktopId, device.deviceId, "zz")).status).toBe(400);
    expect((await authedCall(mine.session, `/api/transfers/${desktop.desktopId}/${OBJECT}`)).status).toBe(400);
  });
});

describe("logging", () => {
  it("never logs a body, a key or an object name, on success or on failure", async () => {
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((level) => vi.spyOn(console, level).mockImplementation(() => undefined));
    const { desktop, mine, device } = await setup();
    const marker = new TextEncoder().encode("SECRET-CONTENT-MARKER-0123456789");
    await put(desktop.token, OBJECT, marker);
    await put(desktop.token, OBJECT, marker);
    await put(desktop.token, "../bad", marker);
    await put("bad-token", OBJECT, marker);
    await tokenCall(`/api/desktop/transfers/${OBJECT}`, desktop.token, { method: "HEAD" });
    await download(mine.session, desktop.desktopId, device.deviceId, OBJECT);
    await download(mine.session, desktop.desktopId, device.deviceId, `${HASH}.${"c".repeat(16)}`);
    const logged = JSON.stringify(spies.flatMap((spy) => spy.mock.calls));
    expect(logged).not.toContain("SECRET-CONTENT-MARKER");
    expect(logged).not.toContain(HASH);
    expect(logged).not.toContain(OBJECT);
    expect(logged).not.toContain(desktop.token);
  });
});
