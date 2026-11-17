# S-01 free tier facts: findings

All doc facts read 2026-11-17. Local = local workerd (wrangler 4.163.0), not production. capacity.md is not edited here: each item ends with "capacity.md change proposed".

## Item 1: does a WebSocket auto-response count as a DO request

Docs (https://developers.cloudflare.com/durable-objects/platform/pricing/, footnote 3): "Application level auto-response messages handled by state.setWebSocketAutoResponse() will not incur additional wall-clock time, and so they will not be charged." The sentence is about wall-clock duration; it does not say whether the message counts as a request. Footnote 2: "For compute requests billing-only, a 20:1 ratio is applied to incoming WebSocket messages to factor in smaller messages for real-time communication." and "The 20:1 ratio does not affect Durable Object metrics and analytics, which reflect actual usage." So the docs do not state it.

Status: PENDING-MAC (test with the S-07 spike Worker).

Local fact already established: an auto-response never runs `webSocketMessage` (S-07: 100 pings, handler runs 0; `/tmp/evidence/ws-q/s-07/pings.txt`). That says nothing about billing.

PENDING-MAC steps (about 20 minutes plus the next day):

1. Deploy the S-07 spike as in `s-07-hibernation/FINDINGS.md` steps 1 to 3. Use a fresh room name `ar1`.
2. Open one device socket and send N = 10,000 text `ping` messages at about 20 per second with nothing else (a Node 24 script using the global WebSocket and `s-07-hibernation/scripts/local-driver.ts pings --url wss://... --n 10000` is enough; no other traffic to that namespace that day).
3. Next day read the namespace's DO requests for that UTC day (dashboard Durable Objects, `monoptah-spike-07`, Metrics, "Requests"; or GraphQL `durableObjectsInvocationsAdaptiveGroups { sum { requests } }` filtered by `date`).
4. Decision: if requests is about 10,000 / 20 = 500 or about 10,000, auto-responses count (at the 20:1 ratio, or one each; the docs say the metric shows actual counts, so about 10,000 means counted 1:1 in the metric and 20:1 in billing); if it is only a handful (the upgrade, the `/stats` call), they do not count.
5. Record N, the number read and the date here.

capacity.md change proposed: none yet. If auto-responses do NOT count, the "Pings" row of the section 2 requests table becomes 0 and the per-ping 30 s vs 60 s decision no longer matters for requests (only the lower bound "without counting pings" remains). If they count, the table stays as is (it already counts them pessimistically).

## Item 2: Rate Limiting binding on the free plan

Docs (https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/): the page does not mention a free plan or plan availability. Quotes: "Rate limits are approximate and apply separately in each Cloudflare location."; period "Must be either `10` or `60`."; "`simple` is the only supported type."; Wrangler 4.36.0 or later.

Local check: PASS. `node s-01-free-tier/scripts/check-ratelimit.ts` starts `wrangler dev --local` for the S-07 spike (`ratelimits` binding `LIMITER`, limit 3 per 60 s, route `/rl`) and fires 6 calls: statuses `200,200,200,429,429,429` (`/tmp/evidence/ws-q/s-01/ratelimit-local.txt`). The binding works locally with wrangler 4.163.0.

Production availability on the free plan: docs do not state, PENDING-MAC:

1. Deploy the spike with `wrangler deploy -c wrangler.deploy.jsonc` (S-07 steps 1 to 2; the config already declares `LIMITER`).
2. `for i in 1 2 3 4 5 6; do curl -s -o /dev/null -w "%{http_code}\n" https://monoptah-spike-07.<subdomain>.workers.dev/rl?key=a; done`
3. A deploy error mentioning the plan, or all 200 (no limiting), means unavailable or not enforced; 200 x3 then 429 means it works. Counts are per location and approximate, so a single location from one client is a fair test. Record the result here.
4. If unavailable: the fallback of capacity.md (D1 counter table, or a "limiter" DO per IP bucket) is the design.

capacity.md change proposed: in section 1 KV note, replace "(unverified: ...)" by the measured result (verified on the free plan or "not available, use the D1 counter"), and add "works in `wrangler dev --local` since wrangler 4.36" if kept.

## Item 3: maximum concurrent WebSockets per DO

Docs (https://developers.cloudflare.com/durable-objects/best-practices/websockets/): "Durable Objects can act as WebSocket servers that connect thousands of clients per instance." No hard number is stated.

Local (S-07, local workerd, not production): 17, 50, 100, 250, 500 and 1000 sockets in one room all opened and were counted by the room; the local stack stopped at about 1015 sockets in total, a process-wide limit, also across 10 rooms (details in `s-07-hibernation/FINDINGS.md`). The 17 of architecture.md 4.4 is far inside it.

Production: PENDING-MAC (S-07 step 6, `local-driver.ts sockets --url wss://... --n 2000`).

capacity.md change proposed: in section 1 "Concurrent WS per object" keep "not stated"; add "local workerd handles 1000 in one object; the 17 per room of architecture.md 4.4 does not bind" and the production result after step 6.

## Item 4: usage alerts on the free plan

Docs (https://developers.cloudflare.com/notifications/notification-available/): the list has no Workers or Durable Objects usage notification. "Usage Based Billing" is "Included with Professional plans or higher" and "Usage-based billing notifications are available to Pay-as-you-go accounts only." The page does not say it is available on the Free plan; the only notification found that mentions a free quota is Image Transformation (Cloudflare Images, all plans), which is not Workers or DO. Result: no usage alert found for the free plan. Fallback stays as in capacity.md section 5: a daily dashboard check and the synthetic check (cron Worker pinging `/api/health` and a relay probe). To close it for good: PENDING-MAC step: Dashboard, Notifications, Add, and check whether any Workers, Durable Objects or billing usage type is offered on the account; record the list of available types.

capacity.md change proposed: none (section 5 row already says "no alert on the free plan, check the dashboard daily"); add the date and "re-checked in the dashboard notification picker" once the step is done.

## Hono + D1 CPU

Not measured locally: local workerd does not report real CPU time of an isolate, and `wrangler dev` timings include dev overhead, so a local number would be a guess. PENDING-MAC:

1. Deploy a throwaway Worker `monoptah-spike-01` (own `wrangler.jsonc`, a Hono route `GET /d1` running one indexed `SELECT` through `env.DB.prepare(...).first()`, a D1 database `monoptah-spike-01` created with `wrangler d1 create`) on the free plan. (Needs `hono`, which is not a dependency of `web/spikes`; use the dependency the product Worker already has, `web/worker`, or request it from the controller.)
2. Send 200 requests: `for i in $(seq 200); do curl -s -o /dev/null https://monoptah-spike-01.<subdomain>.workers.dev/d1; done`.
3. Read CPU time: Workers & Pages, the Worker, Metrics, "CPU Time" chart (quantiles P50, P99, https://developers.cloudflare.com/workers/observability/metrics-and-analytics/ : the chart shows "historical CPU time data broken down into relevant quantiles", estimated by reservoir sampling). For per-invocation values enable `observability` in `wrangler.jsonc` and open Workers Logs, Observability, Events, expand one `cf-worker-event` invocation log and read the CPU time field; the Workers Logs page https://developers.cloudflare.com/workers/observability/logs/workers-logs/ does not name the field, so record the exact field name you see (docs do not state it). Alternatively `npx wrangler tail --format json` (the invocation event carries CPU time in recent wrangler versions; check the output).
4. Record P50, P99 and the first (cold) request value. Budget: 10 ms (capacity.md section 1). Delete: `wrangler delete --name monoptah-spike-01` and `wrangler d1 delete monoptah-spike-01`.

capacity.md change proposed: replace "10 ms CPU is enough for a Hono route with a D1 query" by the measured P99.

## Headline number

The capacity.md headline (about 40 to 60 DAU) is bound by DO duration, not by requests, so item 1 does not change it today whichever way it resolves (requests would only move from the "with pings" to the "without pings" column, 205 to 400+ users per 100,000/day, still above the duration limit). S-02 may change the duration factor and that would change the headline.
