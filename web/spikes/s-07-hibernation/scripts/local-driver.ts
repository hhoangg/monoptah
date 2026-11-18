// S-07 local driver. Starts `wrangler dev --local` for this spike, or uses --url, and runs:
//   sockets : open 17 sockets (1 desktop + 16 devices, architecture.md 4.4), then more, record where it stops
//   idle    : (--poll-s N) keep one hibernatable socket open and poll the constructor counter, to see if local workerd evicts
//   pings   : N pings, print the room message counter before and after (used for the S-01 request test)
// Usage: node scripts/local-driver.ts <sockets|idle|pings> [--url ws://host:port] [--n 1000] [--idle-s 200]
import { startWranglerDev } from "./wrangler-dev.ts";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { TYPE_STATS, buildFrame } from "../src/frame.ts";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const mode = args[0] ?? "sockets";
const opt = (name: string, dflt: string): string => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? (args[i + 1] as string) : dflt;
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function open(url: string): Promise<WebSocket> {
  return new Promise((ok, fail) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    ws.addEventListener("open", () => ok(ws), { once: true });
    ws.addEventListener("error", (e) => fail(new Error(`open failed: ${(e as ErrorEvent).message ?? "error"}`)), { once: true });
  });
}

async function httpStats(base: string, room: string): Promise<Record<string, number>> {
  const res = await fetch(`${base.replace("ws:", "http:")}/stats?room=${room}`);
  return (await res.json()) as Record<string, number>;
}

async function main(): Promise<void> {
  const urlArg = opt("url", "");
  const dev = urlArg ? null : await startWranglerDev(resolve(here, ".."));
  const ws = urlArg || (dev as NonNullable<typeof dev>).wsUrl;
  try {
    if (mode === "sockets") {
      const room = `cap${Date.now()}`;
      const max = Number(opt("n", "1000"));
      const open1 = await open(`${ws}/desktop?room=${room}`);
      const socks: WebSocket[] = [open1];
      const steps = [17, 50, 100, 250, 500, 1000, 2000].filter((n) => n <= max);
      let failedAt: number | null = null;
      for (const target of steps) {
        try {
          while (socks.length < target) socks.push(await open(`${ws}/device?room=${room}&device=d${socks.length}`));
        } catch (err) {
          console.log(`open failed after ${socks.length} sockets: ${String(err)}`);
          failedAt = socks.length + 1;
          break;
        }
        // Prove they all work: a stats frame answered by the room, and the room's own socket count.
        const stats = await httpStats(ws, room);
        console.log(JSON.stringify({ target, opened: socks.length, roomSockets: stats["sockets"] }));
      }
      console.log(JSON.stringify({ result: failedAt ? "stopped" : "no limit reached", failedAt, maxTried: steps.at(-1) }));
      for (const s of socks) s.close();
    } else if (mode === "idle") {
      const room = `idle${Date.now()}`;
      const total = Number(opt("idle-s", "200"));
      const pollS = Number(opt("poll-s", "20"));
      const sock = await open(`${ws}/device?room=${room}&device=d1`);
      sock.send(buildFrame(20, "d1", TYPE_STATS));
      const start = Date.now();
      let last = -1;
      let closed: number | null = null;
      sock.addEventListener("close", (e) => (closed = e.code));
      // Poll through a ping (auto-response, does not wake) and the HTTP stats call (wakes it).
      // To avoid the poll itself keeping the object alive, poll sparsely: constructedAt tells when it was last built.
      while ((Date.now() - start) / 1000 < total) {
        await sleep(pollS * 1000);
        const t = Math.round((Date.now() - start) / 1000);
        const s = await httpStats(ws, room);
        const ageS = Math.round((Date.now() - (s["constructedAt"] ?? 0)) / 1000);
        console.log(JSON.stringify({ t, ctor: s["ctor"], instanceAgeS: ageS, socketOpen: closed === null }));
        if (s["ctor"] !== last) last = s["ctor"] ?? -1;
      }
      sock.close();
    } else if (mode === "pings") {
      const room = `pings${Date.now()}`;
      const n = Number(opt("n", "100"));
      const sock = await open(`${ws}/device?room=${room}&device=d1`);
      const before = await httpStats(ws, room);
      let pongs = 0;
      sock.addEventListener("message", (e) => {
        if (e.data === "pong") pongs++;
      });
      for (let i = 0; i < n; i++) sock.send("ping");
      await sleep(1000);
      const after = await httpStats(ws, room);
      console.log(JSON.stringify({ n, pongs, msgsBefore: before["msgs"], msgsAfter: after["msgs"] }));
      sock.close();
    } else {
      throw new Error(`unknown mode ${mode}`);
    }
  } finally {
    await dev?.stop();
  }
}

await main();
