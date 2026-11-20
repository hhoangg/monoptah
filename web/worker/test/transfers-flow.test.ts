// The whole transfer, end to end over a real Worker and a real (Miniflare) R2 bucket: the mock desktop decides, hashes,
// seals and uploads through the desktop routes; the browser side (the app's own download, decrypt and cache code) fetches
// through the device route with a session cookie. Covers a file over 4 MB, a reopen served from the cache, an object the
// bucket lost, and the refusals of the policy.
import { describe, expect, it } from "vitest";
import { MemoryBackend, MockTransfers, type TransferBackend } from "@monoptah/mock-desktop";
import { ContentCache } from "../../app/src/features/filePreview/transfer/cache.ts";
import { fetchPrepared, TransferError } from "../../app/src/features/filePreview/transfer/client.ts";
import { authedCall } from "./support/accounts.ts";
import { env, fakeRoom, newAccount, pairDesktop, registerDevice, reportState, requestGrant, tokenCall, withEnv } from "./relay/support/api-support.ts";
import { LARGE_BINARY_BYTES, LARGE_BINARY_PATH, LARGE_TEXT_PATH, OUTSIDE_TEXT_PATH } from "../../mock-desktop/src/mentioned.ts";

async function setup() {
  const roomEnv = withEnv({ ROOM: fakeRoom().namespace });
  const mine = await newAccount();
  const desktop = await pairDesktop(mine, { testEnv: roomEnv });
  const device = await registerDevice(mine, "Laptop", roomEnv);
  await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
  await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv);
  // The desktop's side: its own HEAD and PUT calls to the Worker, with its bearer token.
  const backend: TransferBackend = {
    async head(object) {
      const res = await tokenCall(`/api/desktop/transfers/${object}`, desktop.token, { method: "HEAD" });
      if (res.status === 404) return null;
      if (res.status !== 200) throw new Error(`head answered ${res.status}`);
      return Number(res.headers.get("Content-Length"));
    },
    async put(object, sealed) {
      const res = await tokenCall(`/api/desktop/transfers/${object}`, desktop.token, { method: "PUT", body: sealed as BodyInit, headers: { "Content-Type": "application/octet-stream", "Content-Length": String(sealed.length) } });
      if (res.status !== 200 && res.status !== 201) throw new Error(`put answered ${res.status}`);
    },
  };
  const transfers = new MockTransfers();
  transfers.backend = backend;
  // The browser's side: fetch with the session cookie, counting downloads.
  const downloads: string[] = [];
  const browserFetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    downloads.push(String(input));
    return authedCall(mine.session, String(input), { ...(init?.signal ? { signal: init.signal } : {}) });
  }) as typeof fetch;
  return { mine, desktop, device, transfers, downloads, browserFetch, cache: new ContentCache(), roomEnv };
}

describe("a file opened from the web, through R2", () => {
  it("uploads a file over 4 MB once, downloads and opens it, and serves a reopen from the cache", async () => {
    const { desktop, device, transfers, downloads, browserFetch, cache } = await setup();
    const first = await transfers.prepare({ path: LARGE_BINARY_PATH }, true);
    expect(first.size).toBe(LARGE_BINARY_BYTES);
    expect(first.size).toBeGreaterThan(4 * 1024 * 1024);
    expect(first.reused).toBe(false);
    const stored = await env.TRANSFERS.head(`transfers/${desktop.desktopId}/${first.object}`);
    expect(stored?.size).toBe(first.size + 6 * 16);

    const progress: number[] = [];
    const bytes = await fetchPrepared(first, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: browserFetch, cache, onProgress: (received) => progress.push(received) });
    expect(bytes.length).toBe(LARGE_BINARY_BYTES);
    expect(bytes[1000]).toBe((1000 * 131 + (1000 >>> 8)) & 0xff);
    expect(progress.at(-1)).toBe(stored?.size);
    expect(downloads).toHaveLength(1);

    // Reopen: the desktop is asked again, finds the object, resends the same key; the browser has the hash cached.
    const again = await transfers.prepare({ path: LARGE_BINARY_PATH }, true);
    expect(again.reused).toBe(true);
    expect(again.key).toBe(first.key);
    expect(transfers.uploads).toHaveLength(1);
    const reopened = await fetchPrepared(again, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: browserFetch, cache });
    expect(reopened).toBe(bytes);
    expect(downloads).toHaveLength(1);
  });

  it("opens a large text file and keeps no plaintext in the bucket", async () => {
    const { desktop, device, transfers, browserFetch, cache } = await setup();
    const file = await transfers.prepare({ path: LARGE_TEXT_PATH }, true);
    expect(file.size).toBeGreaterThan(4_000_000);
    const bytes = await fetchPrepared(file, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: browserFetch, cache });
    expect(new TextDecoder().decode(bytes.slice(0, 20))).toBe("Example line of a la");
    const held = await (await env.TRANSFERS.get(`transfers/${desktop.desktopId}/${file.object}`))!.arrayBuffer();
    const marker = new TextEncoder().encode("Example line of a large fixture file");
    const text = new Uint8Array(held);
    let found = false;
    for (let i = 0; i + marker.length < Math.min(text.length, 200_000) && !found; i++) found = marker.every((byte, k) => text[i + k] === byte);
    expect(found).toBe(false);
  });

  it("uploads again when R2 lost the object, under the same name and key", async () => {
    const { desktop, device, transfers, browserFetch, cache } = await setup();
    const first = await transfers.prepare({ path: "report.html" }, false);
    await env.TRANSFERS.delete(`transfers/${desktop.desktopId}/${first.object}`);
    const again = await transfers.prepare({ path: "report.html" }, false);
    expect(again.reused).toBe(false);
    expect(again.object).toBe(first.object);
    expect(again.key).toBe(first.key);
    expect(transfers.uploads).toHaveLength(2);
    const bytes = await fetchPrepared(again, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: browserFetch, cache });
    expect(new TextDecoder().decode(bytes)).toContain("Example report");
  });

  it("tells a download of an object that vanished apart from other failures", async () => {
    const { desktop, device, transfers, browserFetch, cache } = await setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    await env.TRANSFERS.delete(`transfers/${desktop.desktopId}/${file.object}`);
    await expect(fetchPrepared(file, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: browserFetch, cache })).rejects.toMatchObject({ problem: "gone" });
  });

  it("refuses the download to a device that is not confirmed, and to a signed-out browser", async () => {
    const { mine, desktop, device, transfers, cache, roomEnv } = await setup();
    const file = await transfers.prepare({ path: "report.html" }, false);
    const stranger = await newAccount();
    const theirs = await registerDevice(stranger, "Theirs", roomEnv);
    const asStranger = (async (input: RequestInfo | URL) => authedCall(stranger.session, String(input))) as typeof fetch;
    await expect(fetchPrepared(file, { desktopId: desktop.desktopId, deviceId: theirs.deviceId, fetch: asStranger, cache })).rejects.toMatchObject({ problem: "gone" });
    const signedOut = (async (input: RequestInfo | URL) => authedCall({ cookie: "" }, String(input))) as typeof fetch;
    await expect(fetchPrepared(file, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: signedOut, cache })).rejects.toBeInstanceOf(TransferError);
    await expect(fetchPrepared(file, { desktopId: desktop.desktopId, deviceId: device.deviceId, fetch: signedOut, cache: new ContentCache() })).rejects.toMatchObject({ problem: "signed_out" });
    expect(mine.session.cookie).not.toBe("");
  });

  it("applies the desktop's policy before anything is uploaded", async () => {
    const { desktop, transfers } = await setup();
    await expect(transfers.prepare({ path: OUTSIDE_TEXT_PATH }, false)).rejects.toMatchObject({ code: "forbidden" });
    await expect(transfers.prepare({ path: "/Users/example/.ssh/id_rsa" }, true)).rejects.toMatchObject({ code: "path_denied" });
    expect((await env.TRANSFERS.list({ prefix: `transfers/${desktop.desktopId}/` })).objects).toHaveLength(0);
    expect(transfers.uploads).toHaveLength(0);
  });

  it("works with the in-memory server too (the default of the mock)", async () => {
    const transfers = new MockTransfers();
    expect(transfers.backend).toBeInstanceOf(MemoryBackend);
  });
});
