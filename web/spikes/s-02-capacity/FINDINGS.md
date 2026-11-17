# S-02 capacity replay: findings

Local = local workerd (wrangler 4.163.0), not production. Production numbers are PENDING-MAC.

## What was built

- `replay.ts` (Node 24, `node s-02-capacity/replay.ts [--url ws://host:port] [--rate 2] [--duration-s 1800] [--ping-s 30|60|0] [--devices 2] [--device-req-per-min 10] [--seed 1] [--out file.json]`). Without `--url` it starts `wrangler dev --local` for the S-07 spike on a free port (own state dir) and stops it. Fake desktop + N devices against the S-07 relay. Counts frames, bytes and messages per minute per side, pings and pongs, closes (with time), and the room's own `webSocketMessage` run counter via `/stats`.
- `src/schedule.ts`: seeded PRNG, frame size model, schedule, ping schedule, per minute counters. Tests: `test/schedule.test.ts` (Node, 6 tests).
- Frame size model (ASSUMPTION: no recorded transcript stream exists in the repo): 70% uniform 60..400 bytes (token deltas), 25% uniform 400..4000 (tool output), 5% uniform 4000..16000 (large snapshots, up to the 16 KiB packing budget); mean about 1,200 bytes. Frame COUNT and message COUNT, which drive requests, do not depend on it; bytes and CPU do. Replace by a real capture before trusting byte totals.
- Fan-out: each device has its own Noise channel (architecture.md 4.4), so the desktop sends one frame per device per tick. At 2 frames/s and 2 devices the room receives 4 frames/s from the desktop (240 per minute), not 120. Device requests: 10 per minute total (100 byte frames) as capacity.md.

## Local dry run (2 minutes, 2 devices, 2 frames/s) with and without pings

Three runs in parallel (own wrangler dev each), output in `/tmp/evidence/ws-q/s-02/dryrun-120s-ping{0,30,60}.json`:

| Run | Desktop frames sent | Per minute (desktop sent) | Bytes sent | Device frames received (both) | Device frames sent | Pings sent (all 3 sockets) | Pongs seen (devices) | `webSocketMessage` runs at the room | Closes |
|---|---|---|---|---|---|---|---|---|---|
| no ping | 480 | 240, 240 | 446,464 | 480 | 19 | 0 | 0 | 499 | none |
| ping 30 s | 480 | 240, 240 | 446,464 | 480 | 19 | 12 | 8 | 499 | none |
| ping 60 s | 480 | 240, 240 | 446,464 | 480 | 19 | 6 | 4 | 499 | none |

Reading: all frames arrived (480 of 480), 499 = 480 + 19 incoming binary frames in every run, so pings added 0 handler runs: auto-response does not wake the handler. 30 s pings over 2 minutes: 4 per socket (3 sockets, 12). Billing of those pings is the S-01 item 1 question.

Local numbers so far for the decisions (not production):

- Coalescing 2/s: the relay handles 240 incoming frames/min for 2 devices without loss and without a rate limit breach (limit 40 msg/s per device, bucket 100). It does not tell if 2/s is the right rate for UX or billing; the 20:1 billing ratio makes frame count cheap (240 frames/min = 12 billed requests/min, 720 per streaming hour). Decision stays with production duration data (below).
- Ping 30 s vs 60 s: locally identical (0 handler runs). 60 s halves ping volume (2,880 to 1,440 per day per socket). Decision rule: if S-01 item 1 shows auto-responses are NOT counted, keep 30 s (better dead-socket detection, iOS drop detection, presence within the 90 s window of architecture.md 4.4, which needs a ping interval well under 90 s: at 60 s one lost ping already breaks the window, so 30 s is the safer value); if they ARE counted, 60 s saves about 1,440 x sockets requests/day, but then the 90 s presence window must become 150 s or the ping 45 s. Do not decide before S-01 item 1.

## PENDING-MAC: production run and next day numbers

1. Deploy the S-07 spike as in `s-07-hibernation/FINDINGS.md` steps 1 to 3. Do not use that namespace for anything else that day (the metrics are per namespace and per UTC day).
2. Baseline: leave the spike with no traffic for one full UTC day (or note the idle desktop: a single idle socket, `local-driver.ts idle --url wss://... --idle-s 3600 --poll-s 120`) and read DO duration (GB-s) and requests for that day = B_gbs, B_req (should be near 0).
3. Replay run A (no ping): `node s-02-capacity/replay.ts --url wss://monoptah-spike-07.<subdomain>.workers.dev --duration-s 1800 --ping-s 0 --out /tmp/s02-A.json`. Use a different UTC day from run B.
4. Replay run B (ping 30 s) on another UTC day, and run C (ping 60 s) on a third: same command with `--ping-s 30` and `--ping-s 60`. One run per UTC day is required because the metrics are daily.
5. The next day read, for each run day: DO duration GB-s (dashboard Durable Objects metrics, or GraphQL `durableObjectsPeriodicGroups`) and requests (`durableObjectsInvocationsAdaptiveGroups { sum { requests } }`); https://developers.cloudflare.com/durable-objects/observability/graphql-analytics/. The metrics show real counts, not the 20:1 billed counts. Copy the `incomingBinaryFramesAtDo`, `webSocketMessageRuns` and `ctor` values from the output JSON next to it.
6. Fill the table below.
7. Delete the spike when done (`wrangler delete --name monoptah-spike-07`).

| Run | Date | Streaming minutes M | DO requests (dashboard) | DO duration GB-s (dashboard) | `incomingBinaryFramesAtDo` | pings sent |
|---|---|---|---|---|---|---|
| Baseline | | 0 | PENDING-MAC | PENDING-MAC | 0 | 0 |
| A no ping | | 30 | PENDING-MAC | PENDING-MAC | | 0 |
| B ping 30 s | | 30 | PENDING-MAC | PENDING-MAC | | |
| C ping 60 s | | 30 | PENDING-MAC | PENDING-MAC | | |

## Formula: dashboard numbers to the capacity.md section 2 table

Per run day (use run A first):

- `G` = measured GB-s per streaming minute = `(gbs_A - B_gbs) / M`, M = 30. Active time per streaming minute is 60 s at 0.125 GB, so the old factor was 7.5 GB-s per streaming minute x 1.3 = 9.75. The new "factor" = `G / 7.5` (replaces 1.3; if G is 9.75 the 1.3 was right). Duration per day for a profile = `streaming minutes x G`.
- `F` = measured incoming frames per streaming minute = `incomingBinaryFramesAtDo / M` from the replay output (240 locally at 2 devices; it scales with devices, so per device = F / devices) replaces 120. Cross-check with the dashboard: `requests_A - B_req - connection requests (3)` is about `incoming messages` (the dashboard shows real counts); it should match `webSocketMessageRuns` and `incomingBinaryFramesAtDo` within a few percent.
- Ping effect: `P = requests_B - requests_A` (pings counted in the metric, if the metric counts them) and `requests_C - requests_A`. Per ping request cost = `P / pingsSent`. 0 means auto-responses are not counted; about 1 means counted 1:1 in the metric (billing then applies 20:1).
- Requests per day for a profile (replaces section 2 "Durable Object requests per day"): `requests = connections + (F x devices_factor x streaming_min + device_requests + other + pings x c + heartbeats) / 20 + presence RPCs`, with `F` per device from above, `devices_factor` = devices, and `c` = 0 or 1 from `P`. Note the change from capacity.md: the table counts 120 frames per streaming minute in total, but the desktop sends one encrypted frame per device, so the real count is `F_per_device x devices`: typical x2, heavy x3. Light 1,200 stays, typical 3,600 becomes 7,200, heavy 14,400 becomes 43,200 (with 120 per device-minute; still /20 billed, so typical +180 requests/day).
- Headline: recompute "Users per 13,000 GB-s/day" with the measured `G`: `users = 13000 / (streaming_min_per_user x G)` for light (10 min), typical (30), heavy (120), and the mix with the same 70/25/5 weights.

## Proposed doc changes (for the doc owners, after the production numbers)

- capacity.md section 2: replace "x 1.3" and the "120 frames/minute" by the measured `G`/7.5 and `F`; add the per-device fan-out multiplier to the "Watch frames" row.
- capacity.md section 3 and 5: record the ping decision (30 s vs 60 s) with the S-01 item 1 result.
- architecture.md 4.4: if ping goes to 60 s, widen the 90 s presence window.
