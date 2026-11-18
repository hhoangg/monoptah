// S-02 capacity replay: a fake desktop and N devices against the S-07 spike relay (or any compatible URL).
// The desktop streams synthetic watch frames to each device at --rate frames/s; devices send a few requests
// per minute; optional keepalive pings. Counts frames, bytes and messages per minute on each side and reads the
// room's own counters (/stats) so "messages that ran webSocketMessage" is measured, not assumed.
//
// Usage (Node 24):
//   node replay.ts [--url ws://host:port] [--rate 2] [--duration-s 1800] [--ping-s 30] [--devices 2]
//                  [--device-req-per-min 10] [--seed 1] [--trace trace.json] [--out result.json]
// --trace replays a RECORDED stream instead of the synthetic one: a JSON array of {"atMs": number, "size": number}
// (frame offset from the start in ms, outer frame bytes). See FINDINGS.md "Synthetic stream" for how to record one.
// Without --url it starts `wrangler dev --local` for the S-07 spike on a free port and stops it at the end.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildFrame } from "../s-07-hibernation/src/frame.ts";
import { startWranglerDev } from "../s-07-hibernation/scripts/wrangler-dev.ts";
import { MinuteCounters, buildSchedule, pingTimes } from "./src/schedule.ts";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = (name: string, dflt: string): string => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? (args[i + 1] as string) : dflt;
};

const rate = Number(opt("rate", "2"));
const durationS = Number(opt("duration-s", "1800"));
const pingS = Number(opt("ping-s", "30"));
const nDevices = Number(opt("devices", "2"));
const reqPerMin = Number(opt("device-req-per-min", "10"));
const seed = Number(opt("seed", "1"));
const outFile = opt("out", "");
const urlArg = opt("url", "");
const traceFile = opt("trace", "");

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function open(url: string): Promise<WebSocket> {
  return new Promise((ok, fail) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    ws.addEventListener("open", () => ok(ws), { once: true });
    ws.addEventListener("error", () => fail(new Error(`open failed: ${url}`)), { once: true });
  });
}

async function roomStats(base: string, room: string): Promise<Record<string, number>> {
  const res = await fetch(`${base.replace(/^ws/, "http")}/stats?room=${room}`);
  return (await res.json()) as Record<string, number>;
}

async function main(): Promise<void> {
  const dev = urlArg ? null : await startWranglerDev(resolve(here, "../s-07-hibernation"));
  const base = urlArg || (dev as NonNullable<typeof dev>).wsUrl;
  const room = `replay${Date.now()}`;
  try {
    const desktop = await open(`${base}/desktop?room=${room}`);
    const deviceIds = Array.from({ length: nDevices }, (_, i) => `dev${i + 1}`);
    const devices = await Promise.all(deviceIds.map((id) => open(`${base}/device?room=${room}&device=${id}`)));

    const t0 = Date.now();
    const minute = () => Math.floor((Date.now() - t0) / 60_000);
    const desktopSent = new MinuteCounters();
    const desktopRecv = new MinuteCounters();
    const deviceSent = deviceIds.map(() => new MinuteCounters());
    const deviceRecv = deviceIds.map(() => new MinuteCounters());
    let pingsSent = 0;
    let pongs = 0;
    const closes: number[] = [];

    desktop.addEventListener("message", (e) => {
      if (e.data instanceof ArrayBuffer) desktopRecv.frame(minute(), e.data.byteLength);
      else desktopRecv.message(minute());
    });
    desktop.addEventListener("close", (e) => {
      closes.push(e.code);
      console.error(`desktop closed code=${e.code} at ${Math.round((Date.now() - t0) / 1000)} s`);
    });
    devices.forEach((d, i) => {
      d.addEventListener("close", (e) => console.error(`device ${i} closed code=${e.code} at ${Math.round((Date.now() - t0) / 1000)} s`));
      d.addEventListener("message", (e) => {
        if (e.data instanceof ArrayBuffer) deviceRecv[i]!.frame(minute(), e.data.byteLength);
        else {
          deviceRecv[i]!.message(minute());
          if (e.data === "pong") pongs++;
        }
      });
      d.addEventListener("close", (e) => closes.push(e.code));
    });

    // Event list: watch frames (desktop to every device), device requests, pings (every socket).
    type Ev = { atMs: number; run: () => void };
    const events: Ev[] = [];
    const schedule = traceFile ? (JSON.parse(readFileSync(traceFile, "utf8")) as { atMs: number; size: number }[]) : buildSchedule(rate, durationS, seed);
    for (const f of schedule) {
      events.push({
        atMs: f.atMs,
        run: () => {
          // One frame per device per tick: each device has its own Noise channel, so the desktop encrypts and
          // sends once per device (frame COUNT at the DO is rate x devices).
          for (const id of deviceIds) {
            desktop.send(buildFrame(f.size, id));
            desktopSent.frame(minute(), f.size);
          }
        },
      });
    }
    if (reqPerMin > 0) {
      const gap = 60_000 / reqPerMin;
      for (let t = gap; t < durationS * 1000; t += gap) {
        events.push({
          atMs: t,
          run: () => {
            const i = Math.floor(t / gap) % nDevices;
            devices[i]!.send(buildFrame(100, deviceIds[i] as string));
            deviceSent[i]!.frame(minute(), 100);
          },
        });
      }
    }
    for (const t of pingTimes(pingS, durationS)) {
      events.push({
        atMs: t,
        run: () => {
          for (const s of [desktop, ...devices]) {
            s.send("ping");
            pingsSent++;
          }
          desktopSent.message(minute());
        },
      });
    }
    events.sort((a, b) => a.atMs - b.atMs);

    const before = await roomStats(base, room);
    for (const ev of events) {
      const wait = t0 + ev.atMs - Date.now();
      if (wait > 0) await sleep(wait);
      ev.run();
    }
    await sleep(Math.max(0, t0 + durationS * 1000 - Date.now()));
    await sleep(1500); // let in-flight frames arrive
    const after = await roomStats(base, room);

    const totals = (m: MinuteCounters[]) =>
      m.reduce((acc, c) => ({ frames: acc.frames + c.total().frames, bytes: acc.bytes + c.total().bytes, messages: acc.messages + c.total().messages }), { frames: 0, bytes: 0, messages: 0 });
    const incomingAtDo = desktopSent.total().frames + totals(deviceSent).frames;
    const result = {
      params: { rate, durationS, pingS, nDevices, reqPerMin, seed, trace: traceFile || null, synthetic: !traceFile },
      desktopSent: desktopSent.total(),
      desktopRecv: desktopRecv.total(),
      devicesSent: totals(deviceSent),
      devicesRecv: totals(deviceRecv),
      perMinute: {
        desktopSentFrames: desktopSent.frames,
        desktopSentBytes: desktopSent.bytes,
        desktopRecvFrames: desktopRecv.frames,
        deviceRecvFrames: deviceRecv.map((c) => c.frames),
      },
      pingsSent,
      pongs,
      room: { ctor: after["ctor"], webSocketMessageRuns: (after["msgs"] ?? 0) - (before["msgs"] ?? 0) },
      incomingBinaryFramesAtDo: incomingAtDo,
      closes,
    };
    const text = JSON.stringify(result, null, 2);
    if (outFile) writeFileSync(outFile, text + "\n");
    console.log(text);
    for (const s of [desktop, ...devices]) s.close();
  } finally {
    await dev?.stop();
  }
}

await main();
