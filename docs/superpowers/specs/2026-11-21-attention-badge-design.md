# Attention badge (blinking red dot) - design

Status: spec for later, not scheduled. Estimate: about 1 week.
Source: feasibility study in session 8776555782383442 (reconstructed here; the session was not readable through the app CLI at writing time).

## Goal

Show a blinking red dot wherever the user is needed: an agent waits for an approval, a question
answer or a plan approval. The dot must be correct, appear fast and clear within a second of the
user resolving the item on any surface.

## What counts

- Counts: tool approval, question (ask-user), plan approval.
- Does not count: `usage_limit`, errors, finished runs, unread output. `usage_limit` gets its own
  separate, non-blinking state so "needs you" stays meaningful.

## Source of truth

The desktop is authoritative. Nothing else computes attention.

- Local sessions: pending approvals and questions on the `Conversation`.
- Remote sessions: `remote_marks` from connected hosts.
- Cloud sessions: the backend `needs_input` flag.

The desktop folds these into one value per session and one aggregate bit per desktop.

## Data model

Catalog session entry gains:

```json
"attention": { "kind": "approval" | "question" | "plan", "ids": ["<pending item id>"], "since": "<rfc3339>" }
```

- `attention` is absent (not null-with-fields) when nothing is pending.
- `ids` lets clients dedupe and close the matching notifications.
- `since` is the oldest pending item; used for ordering and staleness.
- Desktop summary gains `attention: bool` (aggregate per desktop) and a separate `usage_limited: bool`.

## Transport

- Delta push: when attention changes, the desktop sends a small delta (session id + new
  `attention` or removal) immediately, not on the next catalog refresh. Target: clear in under 1 s.
- Full catalog snapshots stay the recovery path; a snapshot always wins over older deltas
  (compare by sequence number).

## Web display rule

The web shows the dot only when all hold:

1. the link to the desktop is ready,
2. the data is fresh (last snapshot or delta within the freshness window),
3. the desktop is online.

Otherwise show no dot (never a stale one). An offline desktop shows its offline state instead.

## Surfaces

- Session row: dot next to the title.
- Mono row: dot when any session of that Mono needs you.
- Rail avatar (desktop): dot from the aggregate bit.
- Document title: `(N) Monoptah` where N is the count of sessions needing you.
- Favicon: variant with a red dot.
- `navigator.setAppBadge(N)` where supported; `clearAppBadge()` at 0.
- Blinking respects `prefers-reduced-motion` (steady dot instead).

## Notifications

- The service worker closes notifications whose `ids` are resolved (on delta or snapshot),
  so a resolved prompt does not linger on other devices.
- Notification tag = pending item id, so repeats replace instead of stack.

## Current gaps

1. No single attention value per session on the desktop; the three sources are separate.
2. The catalog carries no `attention` field, so web cannot show the state.
3. No per-desktop aggregate bit.
4. `usage_limit` is mixed with needs-you signals.
5. Changes reach web only with catalog refreshes, so clearing is slow (no delta push).
6. The service worker never closes resolved notifications; no title, favicon or app badge.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Stale dot after desktop goes offline | Display rule (link ready + fresh + online); hide otherwise |
| Delta lost or reordered | Sequence numbers; snapshot overrides; periodic snapshot |
| Notification spam across devices | Tag by item id; SW closes on resolve |
| Remote host marks lag | Treat host `remote_marks` as authoritative for its sessions; show only when the host link is fresh |
| Cloud `needs_input` polled slowly | Use the existing push path where present; otherwise accept poll latency and document it |
| Badge API unsupported | Feature-detect; title and favicon still work |
| Blinking is distracting | Reduced-motion support; dot blinks only on new attention, then stays steady |

## Plan (about 1 week)

1. Desktop: fold the three sources into `attention` per session + aggregate bit; split `usage_limited` (1.5 d).
2. Protocol: catalog field, desktop summary bits, delta message with sequence (1 d).
3. Web: store, display rule, session/Mono/rail dots (1.5 d).
4. Web: title, favicon, app badge, SW notification close (1 d).
5. Tests: core unit tests, protocol tests, web e2e with mock desktop (1 d).
