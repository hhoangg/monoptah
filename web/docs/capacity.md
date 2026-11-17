# Capacity on the Cloudflare free tier

Numbers as of 2026-11. "Verified 2026-11-17" = re-read on the official page on that date (URL in the Source column). "Unverified" = the official page does not state it; each unverified item is listed under spike ticket S-01 in roadmap.md and has a pessimistic assumption or a fallback here. Measurements (CPU ms, DO duration) are done in the spikes S-02, S-05 and S-06.

## 1. Limits that matter

### Workers

| Limit | Free | Paid ($5/month) | Status | Source |
|---|---|---|---|---|
| Requests | 100,000/day, reset 00:00 UTC | 10M/month included, then $0.30/M | verified 2026-11-17 | [pricing](https://developers.cloudflare.com/workers/platform/pricing/) |
| CPU per invocation | 10 ms | 30 s default (up to 5 min); 30M CPU ms/month, then $0.02/M ms | verified 2026-11-17 | [pricing](https://developers.cloudflare.com/workers/platform/pricing/), [limits](https://developers.cloudflare.com/workers/platform/limits/) |
| Subrequests per invocation | 50 | 10,000 (up to 10M) | verified 2026-11-17 | [limits](https://developers.cloudflare.com/workers/platform/limits/) |
| Static asset requests | free, unlimited, not billed | same | verified 2026-11-17 (the page also says Workers runs before the Cloudflare cache, so cached requests of a Worker route are still billed; assets served by the asset binding are free) | [pricing](https://developers.cloudflare.com/workers/platform/pricing/) |
| Static assets per version / per file | 20,000 / 25 MiB | 100,000 / 25 MiB | verified 2026-11-17 | [limits](https://developers.cloudflare.com/workers/platform/limits/) |

Notes: wall-clock time is not billed. Outgoing Web Push calls are subrequests (not billed, but capped at 50 per invocation). 10 ms CPU is enough for a Hono route with a D1 query, but passkey verification (ECDSA/RSA via WebCrypto) and Web Push encryption must be measured in spikes S-05 and S-06 (CPU time is a measurement, not a page fact).

### Durable Objects (SQLite-backed class, required on free)

| Limit | Free | Paid | Status | Source |
|---|---|---|---|---|
| Requests | 100,000/day | 1M/month, then $0.15/M | verified 2026-11-17 | [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) |
| Duration | 13,000 GB-s/day | 400,000 GB-s/month, then $12.50/M GB-s | verified 2026-11-17 | same |
| Duration basis | 128 MB per object, always, regardless of real use | same | verified 2026-11-17 | same |
| SQLite rows read / written | 5M/day / 100,000/day | 25B / 50M per month, then $0.001/M and $1.00/M | verified 2026-11-17 | same |
| SQLite storage | 5 GB total | 5 GB-month, then $0.20/GB-month | verified 2026-11-17 | same |
| WebSocket billing | connection creation = 1 request; incoming messages billed 20:1 (100 messages = 5 requests); outgoing messages free; the ratio does not change DO metrics, which show real counts | same | verified 2026-11-17 | same (footnote 2) |
| Hibernation | an idle object that qualifies for hibernation is not billed duration, even before it is evicted | same | verified 2026-11-17 | same |
| Auto-response (`setWebSocketAutoResponse`) | adds no wall-clock time, so no duration charge. Whether it counts as a request: unverified | same | duration verified 2026-11-17; request counting unverified (the page does not say) | same (footnote 3) |
| Incoming WS message size | 32 MiB | 32 MiB | verified 2026-11-17 | [limits](https://developers.cloudflare.com/durable-objects/platform/limits/) |
| Hibernation attachment | 16 KiB (16,384 bytes) serialized per socket | same | verified 2026-11-17 | [WebSockets best practices](https://developers.cloudflare.com/durable-objects/best-practices/websockets/) |
| Concurrent WS per object | not stated by Cloudflare (the page only says "thousands of clients per instance"); the relay caps a room at 16 devices + 1 desktop, so this does not bind | same | unverified | [WebSockets best practices](https://developers.cloudflare.com/durable-objects/best-practices/websockets/) |
| Soft throughput per object | about 1,000 requests/s (soft) | same | verified 2026-11-17 | [limits](https://developers.cloudflare.com/durable-objects/platform/limits/) |

What blocks hibernation (all billed as duration while present): standard `accept()` instead of the Hibernation API, `setTimeout`/`setInterval`, alarms, outbound WebSockets, in-flight requests or pending I/O (up to 15 min). The relay must use none of them. When a daily limit is hit, "further operations of that type will fail", so the relay stops for everyone until 00:00 UTC.

### D1

| Limit | Free | Paid | Status | Source |
|---|---|---|---|---|
| Rows read / written | 5M/day / 100,000/day | 25B / 50M per month, then $0.001/M and $1.00/M | verified 2026-11-17 | [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/) |
| Storage | 5 GB total, 500 MB per database | 5 GB then $0.75/GB-month, 10 GB per database (1 TB per account) | verified 2026-11-17 | [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) |
| Queries per invocation | 50 (a `batch()` counts as one) | 1,000 | verified 2026-11-17 (the `batch()` rule is from the D1 docs, not re-read) | same |
| Databases | 10 | 100,000 included, then $0.10 per extra database per month | verified 2026-11-17 | same |

Rows read counts rows scanned, not returned: every query must hit an index.

### KV (not planned)

| Limit | Free | Paid | Status | Source |
|---|---|---|---|---|
| Reads / writes / deletes / lists | 100,000 / 1,000 / 1,000 / 1,000 per day | 10M reads, 1M writes, deletes and lists per month, then $0.50/M and $5/M | verified 2026-11-17 | [KV pricing](https://developers.cloudflare.com/kv/platform/pricing/) |

Decision: do not use KV. 1,000 writes/day is too low for rate limit counters or sessions. Sessions and invites live in D1, per-room quotas live in the DO. The Workers Rate Limiting binding ([docs](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/)) may be used for login and invite endpoints if the free plan allows it (unverified: the page does not state plan availability; period must be 10 or 60 s, and counts are per Cloudflare location and approximate, verified 2026-11-17). Fallback: a D1 counter table or one small "limiter" DO per IP bucket.

## 2. Usage model

Unit: one daily active user (DAU) with one or more desktops. One room (one DO) per desktop; all devices of that desktop share it.

### Assumptions

| Item | Light | Typical | Heavy |
|---|---|---|---|
| Desktops per user (rooms) | 1 | 1 | 2 |
| Devices (browsers/phones) | 1 | 2 | 3 |
| Account caps (hard limits, architecture.md 4.4): desktops / devices per user | 5 / 10 | 5 / 10 | 5 / 10 |
| Minutes per day a device watches an agent that is actively producing output ("streaming minutes", summed over rooms) | 10 | 30 | 120 |
| Watch frames per streaming minute (desktop, after coalescing to 2 frames/s) | 120 | 120 | 120 |
| Device requests per streaming minute (send, read older, file view) | 10 | 10 | 10 |
| Other incoming messages per day (idle UI use) | 50 | 100 | 200 |
| Desktop reconnects per day (laptop sleep, network change) | 4 | 4 | 8 |
| Device reconnects per day per device (iOS background drop, visibility resync) | 6 | 10 | 40 |
| Keepalive pings (30 s), assuming the desktop is online 24 h and each device 8 h | desktop 2,880 + device 960 | desktop 2,880 + 2 x 960 | 2 x 2,880 + 3 x 960 |
| App heartbeat (25 s, about 2.4 messages per minute per open watch, sent by the desktop, so incoming at the DO), watches open only while streaming | 24 | 72 | 288 |
| `room.presence()` RPCs from the desktop list (list views per day x desktops; each wakes a hibernated DO and is billed as one request) | 10 | 20 | 80 |
| Push notifications sent | 3 | 10 | 30 |
| API calls to `/api/*` (auth, desktop list, push register, device approve, admin) | 40 | 100 | 300 |
| D1 queries per API call (indexed, about 5 rows read each) | 2 | 2 | 2 |
| D1 rows written per day (session touch, push, audit, device) | 3 | 5 | 15 |

Basis: the desktop sends watch frames today at up to 20/s with a 50 ms coalescing timer (`crates/monocode-gpui/src/shell/mobile_host.rs:28`); the web path coalesces on the desktop to 2/s before encryption (mitigation below). The app heartbeat is kept at 25 s on the web path (10 s on QUIC), which is about 2.4 messages per minute per open watch; socket liveness additionally comes from the WebSocket auto-response ping.

### Durable Object requests per day

Formula: `requests = connections + (incoming messages + pings + heartbeats) / 20 + presence RPCs`. Pings are counted as billed (pessimistic, since request counting of auto-response is unverified).

| | Light | Typical | Heavy |
|---|---|---|---|
| Connections (desktop + devices x reconnects) | 4 + 6 = 10 | 4 + 2 x 10 = 24 | 8 + 3 x 40 = 128 |
| Watch frames (min x 120) | 1,200 | 3,600 | 14,400 |
| Device requests (min x 10) | 100 | 300 | 1,200 |
| Other messages | 50 | 100 | 200 |
| Pings | 3,840 | 4,800 | 11,520 |
| Heartbeats (25 s) | 24 | 72 | 288 |
| Incoming total | 5,214 | 8,872 | 27,608 |
| Billed incoming (/20) | 261 | 444 | 1,381 |
| Presence RPCs | 10 | 20 | 80 |
| **DO requests/day** | **281** | **488** | **1,589** |
| Users per 100,000/day | 356 | 205 | 63 |

Without counting pings: light 89, typical 248, heavy 1,012 requests, so the real result is between the two; the pessimistic one is used.

### Durable Object duration per day

Formula: `GB-s = active seconds x 0.125 GB x 1.3`. Active seconds are the streaming minutes x 60 (the object is awake while frames flow). The 1.3 factor covers wake-ups, short tails before hibernation and connection handling. Idle connected desktops hibernate and cost nothing.

| | Light | Typical | Heavy |
|---|---|---|---|
| Active seconds | 600 | 1,800 | 7,200 |
| x 0.125 GB | 75 | 225 | 900 |
| x 1.3 | **98 GB-s** | **293 GB-s** | **1,170 GB-s** |
| Users per 13,000 GB-s/day | 133 | 44 | 11 |

### Workers, D1, push

| | Light | Typical | Heavy |
|---|---|---|---|
| Worker requests (API + relay upgrades + push-trigger calls) | 40 + 10 + 3 = 53 | 100 + 24 + 10 = 134 | 300 + 128 + 30 = 458 |
| Users per 100,000/day | 1,880 | 746 | 218 |
| D1 rows read (calls x 2 queries x 5 rows) | 400 | 1,000 | 3,000 |
| Users per 5M/day | 12,500 | 5,000 | 1,667 |
| D1 rows written | 3 | 5 | 15 |
| Users per 100,000/day | 33,000 | 20,000 | 6,600 |
| DO SQLite rows written (about 5 per connect, quotas kept in memory) | 50 | 120 | 640 |
| Users per 100,000/day | 2,000 | 833 | 156 |
| Push subrequests per invocation | 1 | 1 | 1 (limit 50) |

### Binding limit and capacity

The binding limit is Durable Object duration (GB-s/day), then DO requests. Workers requests, D1 and KV are not close.

| Profile | Duration limit | DO request limit | Binding | DAU that fit |
|---|---|---|---|---|
| Light | 133 | 356 | duration | about 130 |
| Typical | 44 | 205 | duration | about 44 |
| Heavy | 11 | 63 | duration | about 11 |
| Mix 70% light, 25% typical, 5% heavy (average 200 GB-s/day) | 65 | about 250 | duration | **about 60 DAU** |

Headline: on the free tier expect **about 40 to 60 daily active users (about 50 desktops)** before the relay stops mid-day. At a DAU/registered ratio of 30%, that is roughly 150 to 200 registered invite-only users. Plan for upgrading earlier because a hit limit takes the whole relay down until 00:00 UTC.

Sensitivity: the result scales with watched streaming minutes, not with the number of users. One user streaming 2 hours a day uses as much as 10 light users. The 1.3 factor and the 120 frames/minute are assumptions: if the object stays awake between frames even when the agent is quiet (for example a 10 s hibernation delay after each burst), duration can be 1.5 to 2 times higher. Measure in the spike (section 5).

## 3. Mitigations

| Mitigation | Effect | Where |
|---|---|---|
| Hibernation API only (no timers, alarms, outbound sockets); per-socket state in `serializeAttachment` | Idle rooms cost 0 GB-s; the largest saving (24 h connected desktops cost nothing) | relay DO |
| `setWebSocketAutoResponse("ping", "pong")` for keepalive; app heartbeat kept at 25 s on the web path (about 2.4 messages per minute per open watch, sent only while a watch is open) | Auto-response needs no wake-up and no duration; heartbeats are few | relay + protocol |
| Desktop sends watch frames only to open watches (its `WatchSink` already does; no relay watcher signal is needed) | No frames, no wake-ups when nobody watches | desktop |
| Coalesce and batch watch updates on the desktop before encryption: 2 frames/s (or 500 ms) instead of 20/s; pack several patches per frame | Fewer incoming messages (10x vs today) and fewer encrypt/decrypt operations | desktop |
| Pack several small messages into one WebSocket message up to a size budget (for example 16 KiB) | The 20:1 ratio rewards fewer, larger messages | desktop, client |
| Desktop reconnect backoff with jitter; devices reconnect on `visibilitychange` only, never in a hot loop | Bounds connection requests | clients |
| Desktop keepalive interval 30 s (a 60 s interval halves ping volume if auto-response counts as billed) | Ping requests 2880 to 1440/day per socket | desktop |
| Avoid DO for idle desktops: a desktop connects to its room only when Settings enables the web link, and the room closes on revoke or disable | No standing sockets for abandoned desktops | desktop |
| Per-room quotas in memory: bytes/min, messages/min, max 16 devices per room, 4 sockets per device id, max frame 65,555 bytes (as architecture.md; larger payloads are chunked) | A runaway or hostile client cannot burn the daily budget; close with a typed code | relay DO |
| Per-user and per-IP limits at the Worker edge (login, invite, pairing) with the Rate Limiting binding or a D1 counter | Protects Workers and D1 budgets | Worker |
| D1 query caching: cache desktop list and device list in memory per isolate for 5 to 15 s; avoid `last_seen` writes on every request (write device last seen on disconnect, as architecture.md; presence lives in the DO); cache `room.presence()` results per isolate for 5 to 15 s and call it only while the desktop list is open (each call wakes a hibernated DO and is billed as a request) | Fewer reads, writes and DO wake-ups | Worker |
| Indexed queries, `batch()` for multi-statement actions | Rows read stays near rows returned | Worker |
| Push dedupe and collapse: one push per state change (finished, needs input), a `tag`/collapse key per session, no push while a device is actively connected to the room | Fewer subrequests and fewer D1 reads | Worker + desktop |
| Static assets served by the asset binding with long-lived hashed filenames; no Workers Caching | Unbilled asset requests | Worker config |
| Single DO class, one room per desktop, shard nothing else; no global DO (a global counter DO would be woken by every request) | Keeps duration proportional to real use | architecture |

## 4. Paid tier

Workers Paid is $5/month per account and includes:

| Resource | Included per month | Overage | Typical user per month | 100 typical users |
|---|---|---|---|---|
| Workers requests | 10M | $0.30/M | 4,000 | 400,000 (inside) |
| Workers CPU | 30M ms | $0.02/M ms | small | inside |
| DO requests | 1M | $0.15/M | 14,600 | 1.46M, overage 0.46M = $0.07 |
| DO duration | 400,000 GB-s | $12.50/M GB-s | 8,790 | 879,000, overage 479,000 = $5.99 |
| D1 rows read / written | 25B / 50M | $0.001/M, $1/M | 30,000 / 150 | inside |
| Total for 100 typical users | | | | about $11/month ($5 base + about $6) |

Paid limits are monthly pools, so one bursty day no longer takes the relay down. Overage cost per extra typical user is about $0.11/month, per heavy user about $0.44/month.

Threshold: move to Workers Paid when the plan shows about 70% of the free duration limit on a normal day, which is about 30 typical DAU (30 x 293 = 8,790 of 13,000 GB-s), or earlier if heavy users are present. In practice: before inviting beyond about 40 active users, or on the first day the relay returns limit errors. Self-hosters run in their own account; a single-user self-host fits comfortably on the free tier.

## 5. Monitoring

| What | How | Alert |
|---|---|---|
| DO requests, duration (GB-s), rows read/written per day | Cloudflare dashboard, Durable Objects metrics; GraphQL Analytics API for scripted checks (real counts, not 20:1 billed counts, so apply the ratio yourself) | 70% and 90% of each free limit |
| Workers requests, CPU time, errors | Workers metrics and Workers Logs | error rate over 1%; any 429 or limit error |
| D1 rows read/written, query latency | D1 metrics | 70% of 100,000 writes/day |
| Billing/usage notifications | Cloudflare notification center (the [notification list](https://developers.cloudflare.com/notifications/notification-available/) shows no Workers or DO usage alert, and usage-based billing alerts are for pay-as-you-go accounts only, verified 2026-11-17; so no alert on the free plan, check the dashboard daily) | on |
| Per-room counters | Relay logs a daily summary per room (connections, billed-message estimate, bytes) into Workers Logs; the admin page shows top rooms | room above 5x the median |
| Synthetic check | A cron-triggered Worker (free plan allows cron triggers; counts as a request) pings `/api/health` and one relay probe | failure twice in a row |

Spike measurement plan: replay a recorded 30 minute transcript stream (2 frames/s) through the relay with hibernation on, then read DO duration and request metrics the next day, to replace the 1.3 factor and the auto-response counting assumption with measured values.
