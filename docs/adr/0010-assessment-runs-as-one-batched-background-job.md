# 10. Assessment runs on demand, as one batched background job

Date: 2026-10-16
Status: Accepted; fetching with Sync superseded by ADR 12

## Context

Assessment happens monthly or yearly, but the first run covers thousands of tickets.
Assessing one ticket per click, or one ticket per AI call, is far too slow.

## Decision

- Fetching tickets is automatic and runs with the existing Contributors Sync. It only
  calls the ClickUp API.
- Assessing is never automatic. One "Assess tickets" action assesses every
  unassessed closed ticket in scope in a single run. Before starting it shows how many
  tickets it will assess and which Assessment runner it will use.
- The run is a background job, like Sync jobs (`src-tauri/src/contributors/jobs.rs`):
  progress events, cancel, and resume. Each finished group of tickets is written to
  the database at once, so a cancelled or crashed run loses at most the group in
  flight, and the next run continues with what is left.
- Each AI call assesses a group of tickets (around 20-30), not one.
- Several groups run at the same time, with a small concurrency limit.
- With an API key, a large run can use the provider's batch API (about half the
  per-token price of normal API calls, slower to return); small runs use normal
  calls. With Claude Code there is no per-token cost, but a large run draws on the
  subscription's usage limits and may have to wait for them.
- An unassessed ticket counts 0 points, and the leaderboard warns how many tickets in
  the Period are still unassessed. No placeholder level is ever guessed.

## Consequences

- Group size, concurrency and the batch threshold are tuning values to settle during
  the build, against real rate limits.
- One malformed AI reply affects only its group; that group is retried, then its
  tickets stay unassessed and are reported.
