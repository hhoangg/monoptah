import * as cfWorkers from "cloudflare:workers";
import { evictDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_FRAME, TYPE_STATS, buildFrame, readDeviceId } from "../src/frame.ts";

interface Stats {
  ctor: number;
  msgs: number;
  constructedAt: number;
  builtBuckets: number;
  lastBuiltTokens: number;
  sockets: number;
}

// The shared tsconfig.workers.json declares no Env/Exports for this spike, so type what is used here.
const { env, exports } = cfWorkers as unknown as {
  env: { ROOM: DurableObjectNamespace };
  exports: { default: { fetch(input: string, init?: RequestInit): Promise<Response> } };
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function connect(path: string, room: string): Promise<WebSocket> {
  const res = await exports.default.fetch(`http://spike.test${path}${path.includes("?") ? "&" : "?"}room=${room}`, {
    headers: { Upgrade: "websocket" },
  });
  const ws = res.webSocket;
  if (!ws) throw new Error(`no websocket, status ${res.status}`);
  ws.accept();
  ws.binaryType = "arraybuffer";
  return ws;
}

/** Collects messages in arrival order; `next` waits for one. */
function inbox(ws: WebSocket) {
  const queue: (string | ArrayBuffer)[] = [];
  const waiters: ((v: string | ArrayBuffer) => void)[] = [];
  let closed: { code: number } | null = null;
  const closeWaiters: ((c: { code: number }) => void)[] = [];
  ws.addEventListener("message", (e) => {
    const w = waiters.shift();
    if (w) w(e.data as string | ArrayBuffer);
    else queue.push(e.data as string | ArrayBuffer);
  });
  ws.addEventListener("close", (e) => {
    closed = { code: e.code };
    for (const w of closeWaiters.splice(0)) w(closed);
  });
  return {
    next: (ms = 3000) =>
      new Promise<string | ArrayBuffer>((resolve, reject) => {
        const q = queue.shift();
        if (q !== undefined) return resolve(q);
        const t = setTimeout(() => reject(new Error("timeout waiting for message")), ms);
        waiters.push((v) => {
          clearTimeout(t);
          resolve(v);
        });
      }),
    pending: () => queue.length,
    closed: () => closed,
    /** Resolves with the close event's code; generous timeout, no timing assumption about the server. */
    whenClosed: (ms = 15_000) =>
      new Promise<{ code: number }>((resolve, reject) => {
        if (closed) return resolve(closed);
        const t = setTimeout(() => reject(new Error("timeout waiting for close")), ms);
        closeWaiters.push((c) => {
          clearTimeout(t);
          resolve(c);
        });
      }),
  };
}

async function stats(ws: WebSocket, box: ReturnType<typeof inbox>): Promise<Stats> {
  ws.send(buildFrame(20, "x", TYPE_STATS));
  const m = (await box.next()) as ArrayBuffer;
  return JSON.parse(new TextDecoder().decode(new Uint8Array(m).subarray(20))) as Stats;
}

async function httpStats(room: string): Promise<Stats> {
  const res = await exports.default.fetch(`http://spike.test/stats?room=${room}`);
  return (await res.json()) as Stats;
}

describe("S-07 relay prototype in workerd", () => {
  it("auto-response answers ping with pong and does not run webSocketMessage", async () => {
    const dev = await connect("/device?device=d1", "ar");
    const box = inbox(dev);
    const before = await stats(dev, box);
    for (let i = 0; i < 5; i++) {
      dev.send("ping");
      expect(await box.next()).toBe("pong");
    }
    const after = await stats(dev, box);
    // `stats` itself is one message each: the 5 pings added nothing.
    expect(after.msgs - before.msgs).toBe(1);
    expect(after.ctor).toBe(before.ctor);
    dev.close();
  });

  it("routes 1 desktop + 3 devices and rewrites the device id", async () => {
    const desk = await connect("/desktop", "rt");
    const deskBox = inbox(desk);
    const ids = ["d1", "d2", "d3"];
    const devs = await Promise.all(ids.map((id) => connect(`/device?device=${id}`, "rt")));
    const boxes = devs.map(inbox);
    // desktop to one device
    desk.send(buildFrame(100, "d2"));
    const got = new Uint8Array((await boxes[1]!.next()) as ArrayBuffer);
    expect(got.length).toBe(100);
    expect(boxes[0]!.pending()).toBe(0);
    expect(boxes[2]!.pending()).toBe(0);
    // device to desktop: a spoofed id is overwritten by the attached one
    devs[2]!.send(buildFrame(80, "d1"));
    const up = new Uint8Array((await deskBox.next()) as ArrayBuffer);
    expect(readDeviceId(up)).toBe("d3");
    for (const w of [desk, ...devs]) w.close();
  });

  it("relays a 65,555 byte frame intact and rejects 65,556 with close 1009", async () => {
    const desk = await connect("/desktop", "big");
    const dev = await connect("/device?device=d1", "big");
    const box = inbox(dev);
    const frame = buildFrame(MAX_FRAME, "d1");
    desk.send(frame);
    const got = new Uint8Array((await box.next()) as ArrayBuffer);
    expect(got.length).toBe(MAX_FRAME);
    expect(Buffer.compare(Buffer.from(got), Buffer.from(frame))).toBe(0);

    const dbox = inbox(desk);
    desk.send(buildFrame(MAX_FRAME + 1, "d1"));
    expect((await dbox.whenClosed()).code).toBe(1009);
    dev.close();
  });

  it("closes other text with 4012 and floods with 4008", async () => {
    const dev = await connect("/device?device=d1", "q");
    const box = inbox(dev);
    dev.send("hello");
    expect((await box.whenClosed()).code).toBe(4012);

    const flood = await connect("/device?device=d9", "q2");
    const fbox = inbox(flood);
    for (let i = 0; i < 150; i++) flood.send(buildFrame(20, "d9"));
    expect((await fbox.whenClosed()).code).toBe(4008);
  });

  it("evicts the object: constructor re-runs, attachment survives, bucket is rebuilt, ping needs no wake", async () => {
    const room = "ev";
    const desk = await connect("/desktop", room);
    // refill=0: the bucket never refills, so the token counts below do not depend on the wall clock.
    const dev = await connect("/device?device=d1&refill=0", room);
    const box = inbox(dev);
    // Consume 30 tokens of the device bucket so the persisted bucket is not full.
    for (let i = 0; i < 30; i++) dev.send(buildFrame(20, "d1"));
    const before = await stats(dev, box);
    expect(before.builtBuckets).toBe(1);
    expect(before.lastBuiltTokens).toBe(100); // fresh bucket on the first message

    const stub = env.ROOM.get(env.ROOM.idFromName(room));
    await evictDurableObject(stub); // default: hibernate the sockets

    // A ping must be answered without constructing the object again.
    const pingAt = Date.now();
    dev.send("ping");
    expect(await box.next()).toBe("pong");
    await sleep(400);
    const afterStats = await httpStats(room); // this request constructs the object (once)
    expect(afterStats.ctor).toBe(before.ctor + 1);
    // constructedAt is after the observation delay: the ping did not wake the object.
    expect(afterStats.constructedAt).toBeGreaterThan(pingAt + 300);

    // First routed message after eviction: sockets still attached, bucket rebuilt from the attachment.
    desk.send(buildFrame(40, "d1"));
    const routed = new Uint8Array((await box.next()) as ArrayBuffer);
    expect(routed.length).toBe(40);
    const s2 = await stats(dev, box);
    expect(s2.ctor).toBe(before.ctor + 1);
    expect(s2.sockets).toBe(2);
    // Device socket rebuilt its bucket only when the device sends (stats does not touch it).
    dev.send(buildFrame(20, "d1")); // the stats frame below is processed after it (same socket, in order)
    const s3 = await stats(dev, box);
    // New instance: the device bucket was built again from the attachment (100 - 30 = 70 tokens, no refill
    // because refill=0), not as a fresh full bucket of 100.
    expect(s3.builtBuckets).toBe(2); // desktop (routed above) and device
    expect(s3.lastBuiltTokens).toBe(70);
    expect(box.closed()).toBeNull();
    desk.close();
    dev.close();
  });
});
