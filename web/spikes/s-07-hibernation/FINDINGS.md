# S-07 Durable Object hibernation: findings

Status per acceptance item of roadmap.md S-07. "Local" = local workerd/Miniflare (wrangler 4.163.0, workerd 1.20261116.1), NOT production. "Docs" = official page read 2026-11-17.

Prototype: `worker/index.ts` (Worker + `SpikeRoom`; not the product `DesktopRoom`), pure pieces in `src/bucket.ts` and `src/frame.ts`. It uses `ctx.acceptWebSocket` with tags (`desktop`, `device`, `device:<id>`), `serializeAttachment` (role, device id, joinedAt, token bucket state), `setWebSocketAutoResponse("ping","pong")`, a constructor counter in `ctx.storage.kv` (`ctor`), an in-memory token bucket rebuilt lazily from the attachment, 1 desktop + N devices routing with the device id overwritten from the attachment, and no timer, alarm or outbound socket. Spike-only extras: frame type `0x7f` (stats reply), `/stats`, `/rl` (rate limit binding check for S-01).

## How to run

From `web/spikes`:

- Node unit tests: `node_modules/.bin/vitest run` (files `s-07-hibernation/test/bucket.test.ts`, `frame.test.ts`).
- workerd tests: `node_modules/.bin/vitest run -c s-07-hibernation/vitest.workers.config.ts` (file `test/relay.workers.test.ts`, 5 tests). The config sets its own `root`, so it also works from the spike dir. The controller wires this into `package.json` `test`.
- Local driver (starts `wrangler dev --local` on a free port, stops it at the end): `node s-07-hibernation/scripts/local-driver.ts sockets|idle|pings [--n N] [--idle-s S] [--poll-s S] [--url ws://host:port]`.

## Items

| Item | Status | Evidence |
|---|---|---|
| Relay prototype with acceptWebSocket, attachments, auto-response, 1 desktop + 3 devices | PASS (local) | `test/relay.workers.test.ts` "routes 1 desktop + 3 devices"; spoofed device id overwritten |
| Frames up to 65,555 bytes | PASS (local) | test "relays a 65,555 byte frame intact and rejects 65,556": byte-for-byte equal; 65,556 closes `1009` by the prototype's own check (`checkFrameSize`) |
| Auto-response works without waking the object | PASS (local) | test "auto-response answers ping...": 5 pings leave the `webSocketMessage` counter and the constructor counter unchanged. Also `/tmp/evidence/ws-q/s-07/pings.txt`: 100 pings, 100 pongs, handler runs 0. S-02 dry runs: 499 handler runs with 0, 12 and 6 pings |
| Eviction, constructor re-run, attachment restore, bucket rebuilt | PASS (local, with test helper) | test "evicts the object": `evictDurableObject(stub)` from `cloudflare:test` (default `webSockets: "hibernate"`); afterwards the constructor ran once more (`ctor` +1), both sockets still attached and routing, the device bucket was rebuilt from the attachment (tokens below the full 100, not a fresh bucket), no close frame. A ping sent after eviction is answered `pong` and the object stays unconstructed (constructedAt is later than the ping by the observation delay) |
| Idle room is evicted while sockets stay open | PASS locally, production PENDING-MAC | see below |
| No timer, alarm or outbound socket needed | PASS (local) | the prototype has none and all of the above works; docs say these block hibernation |
| Token bucket rebuilt after hibernation | PASS | `test/bucket.test.ts` "rebuilt from a snapshot" (pure) and the workerd test above |
| Max concurrent sockets to the 17 of architecture.md 4.4 and beyond | PASS locally (17 and far beyond); production PENDING-MAC | see below |
| Deploy with open sockets, close 1001 handling | PENDING-MAC | production behaviour, see steps |
| iOS 18 and 26 installed PWA background drop time | PENDING-DEVICE | see steps |

### Does local workerd evict a hibernatable object, and how

Observed (local workerd, not production), `scripts/local-driver.ts idle`, one open device socket, an HTTP `/stats` call every N seconds (each call constructs the object if it was evicted, and resets the idle clock):

- poll every 4 s: constructor counter stayed 1 for 24 s (never evicted)
- poll every 8 s: stayed 1 for 48 s (never evicted)
- poll every 12 s: counter +1 at every poll (evicted each time, socket still open)
- poll every 20 s over 200 s: +1 at every poll (11 constructions), socket open the whole time

So local workerd hibernates and evicts an idle object with open hibernatable sockets between 8 and 12 s after the last event, with no test helper, consistent with the documented 10 s. Raw output: `/tmp/evidence/ws-q/s-07/idle*.txt`. The test-only helper `evictDurableObject` (and `evictAllDurableObjects`, `abortAllDurableObjects`) in `@cloudflare/vitest-plugin` types (`types/cloudflare-test.d.ts`) forces it immediately; `webSockets: "close"` would close the sockets instead.

Documented only (not observed here):

- "After 10 seconds of no incoming request or event" a Durable Object that meets all conditions hibernates; "after 70-140 seconds of inactivity" an idle in-memory object that cannot hibernate is evicted. Source: https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/ (read 2026-11-17).
- Hibernation needs: no `setTimeout`/`setInterval` callbacks, no standard WebSocket API, no request or event in progress (same page).
- "When a Durable Object receives no events (such as alarms or messages) for a short period, it is evicted from memory"; attachment limit 16,384 bytes; "A forced code update disconnects WebSockets attached to the running instance". Source: https://developers.cloudflare.com/durable-objects/best-practices/websockets/ (read 2026-11-17). Neither page names a close code for the deploy case, so 1001 is NOT confirmed by docs: PENDING-MAC.
- Local limits: a local test does not show duration billing stopping. That is production metric work (S-02 step list).

### Max concurrent sockets (local workerd, not production)

`scripts/local-driver.ts sockets --n 2000`, one room (one DO), Node 24 global WebSocket, 1 desktop + devices with distinct ids, prototype enforces no cap. Output `/tmp/evidence/ws-q/s-07/sockets.txt`:

- 17, 50, 100, 250, 500, 1000 all opened and the room's own count (`ctx.getWebSockets().length`) matched exactly.
- The 1016th open failed (1015 open). The same stop happens with the sockets spread over 10 rooms (so it is NOT a per-object limit), and a plain HTTP request to the dev server also failed at that point. It is a process-wide limit of the local stack (consistent with a 1024 file descriptor limit in the local workerd process; the shell reports `ulimit -n` 4096, so I did not prove which process hits it). Meaning: the 17 of architecture.md 4.4 is far inside what local workerd accepts; the production per-object ceiling is not measurable here. PENDING-MAC step 6.

## PENDING-DEVICE: iOS 18 and 26 installed PWA background socket drop

Probe page: `device/index.html` + `spike.ts` (no inline script, no `innerHTML`, Trusted Types safe, same-origin, works under the CSP of `web/worker/src/csp.ts`; the shipped `_headers` has that CSP with `connect-src 'self' wss://*.workers.dev`). It logs `ws open`, `ws close code=`, `visibility visible/hidden`, `pagehide`, `pageshow`, `tick gap N s` (the page timer stopped, i.e. the page was frozen) and `lastPongAge` (seconds since the last pong at the moment of the visibility change), with time stamps, on screen and in `localStorage` (survives a killed page). It pings every 5 s while open.

Hosting: the PWA needs HTTPS. Use the deployed spike Worker of PENDING-MAC steps 1 to 3 (it serves the page as static assets on the same origin, so `wss://monoptah-spike-07.<subdomain>.workers.dev/device?room=ios&device=ios1` is the default URL the page fills in).

Steps, on each device (iPhone on iOS 18, iPhone on iOS 26), repeat for each background duration D in 10 s, 30 s, 60 s, 3 min, 10 min:

1. Open `https://monoptah-spike-07.<subdomain>.workers.dev/` in Safari. Tap Share, "Add to Home Screen", Add. Close Safari. Open the new "S-07" icon (installed PWA, `standalone=true` shown at the top of the page).
2. Tap "Clear log", then "Connect". The log must show `ws open`. Wait 10 s and see `ws open` still the last socket line.
3. Note the wall clock time T0, press the Home button/gesture (the PWA goes to background). Do not lock the screen for the "app switch" case; repeat later with a locked screen as a separate row.
4. After D seconds by a real clock, return to the PWA.
5. Read the log: `visibility hidden` time vs `visibility visible` time, `tick gap` (page frozen for how long), `ws close code=...` if any (code 1006 means dropped without a close frame), and `readyState` in the `visibility visible` line (1 = open, 3 = closed). The drop time is: the last `lastPongAge`-valid state before hidden plus the smallest D at which `ws close` or `readyState=3` appears. The brief's "about 10 s" holds if D = 10 s already closes the socket.
6. Cross check server side while the PWA is backgrounded: `curl https://monoptah-spike-07.<subdomain>.workers.dev/stats?room=ios` shows `sockets` (the room's open socket count; note this call constructs the object, which is fine). If it drops to 0 while the PWA is still in the background, the drop is real and its time is the poll time.
7. Record per row: iOS version, device model, D, screen locked yes/no, result (still open / closed with code / frozen gap), in the table below.

| iOS | Device | D | Locked | Result |
|---|---|---|---|---|
| 18.x | | | | PENDING-DEVICE |
| 26.x | | | | PENDING-DEVICE |

## PENDING-MAC: deploy behaviour, real eviction, duration metric stop

Needs the maintainer's own Cloudflare account (free plan). The spike Worker is named `monoptah-spike-07` and is deleted at the end. Run from `web/spikes/s-07-hibernation`.

1. Build the probe page: `../node_modules/.bin/vite build -c device/vite.config.ts` (writes `device/dist`, not committed).
2. Deploy: `../node_modules/.bin/wrangler deploy -c wrangler.deploy.jsonc` (log in first with `wrangler login`). Note the URL `https://monoptah-spike-07.<subdomain>.workers.dev`.
3. Smoke: `curl https://monoptah-spike-07.<subdomain>.workers.dev/stats?room=smoke` returns JSON with `ctor: 1`.
4. Real eviction and duration stop: from `web/spikes` run the S-02 driver against the URL, which opens the sockets and streams frames: `node s-02-capacity/replay.ts --url wss://monoptah-spike-07.<subdomain>.workers.dev --duration-s 1800 --ping-s 30 --out /tmp/s02-prod.json`. Then leave 2 sockets open idle for 1 hour (`node s-07-hibernation/scripts/local-driver.ts idle --url wss://... --idle-s 3600 --poll-s 120`; `ctor` should grow by 1 per poll if the object is evicted while the socket stays open). Read the DO duration (Cloudflare dashboard, Workers & Pages, Durable Objects, the `monoptah-spike-07` namespace, Metrics; GraphQL dataset `durableObjectsPeriodicGroups` and `durableObjectsInvocationsAdaptiveGroups`, https://developers.cloudflare.com/durable-objects/observability/graphql-analytics/): during the idle hour the duration must stay near 0 GB-s. Record in S-02 FINDINGS.
5. Deploy behaviour with open sockets: open 1 desktop + 3 devices with `node s-02-capacity/replay.ts --url wss://... --duration-s 600 --ping-s 30` (it prints every socket's close code and second). While it runs, change one comment in `worker/index.ts` and run `wrangler deploy -c wrangler.deploy.jsonc` again. Read the close codes in the replay output: confirm sockets close with 1001 (or whatever appears) and that a reconnect works. Record the exact code; architecture.md 4.5 assumes 1001.
6. Max concurrent sockets per object in production: `node s-07-hibernation/scripts/local-driver.ts sockets --url wss://... --n 2000` (raise the client file descriptor limit first, `ulimit -n 8192`). Record where it stops, if it does. Use a fresh room name per run (the driver does).
7. Delete everything: `../node_modules/.bin/wrangler delete --name monoptah-spike-07` (it also removes the DO data of the namespace). Verify it is gone in the dashboard.

## Proposed doc changes (for the doc owners)

- architecture.md 4.5: the docs name no close code for a deploy; mark 1001 "unconfirmed until S-07 PENDING-MAC step 5" and keep the client rule "any unexpected close: reconnect with a jittered backoff" as the real contract.
- architecture.md 4.4 "Hibernation": local workerd evicts an idle object with open sockets after about 10 s (observed 8 to 12 s), matching the lifecycle docs; frames to 65,555 bytes relay intact locally.
- capacity.md 1 (Concurrent WS per object): add "local workerd: no per-object limit seen up to 1000 sockets; the local process stops at about 1015 sockets overall; production not measured".
