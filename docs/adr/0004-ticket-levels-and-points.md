# 4. Assessments store a level; points come from a table anchored to PR delivery

Date: 2026-10-16
Status: Accepted

## Context

Tickets are a dimension of the same leaderboard as PR delivery (ADR 3), so ticket
points must sit on a comparable scale. A PR earns between `SCORING.PR_BASE` (10) and
`SCORING.PR_CAP` (72) delivery points.

## Decision

- An Assessment places a ticket in one of five levels: XS, S, M, L, XL. It stores the
  criterion scores the level is derived from (ADR 8), never points.
- A points table turns a level into points. Starting values: XS 3, S 8, M 16, L 32,
  XL 48.
- Anchor: a typical (median) ticket is worth about as much as a typical (median) PR.
  XL stays below the PR cap so a single ticket never outweighs the largest PR.
- After a run on real data, the app shows the median ticket points next to the median
  PR delivery points so the table can be tuned.
- Changing the table or the `tickets` weight never requires re-assessing tickets.

## Consequences

- The points table is configuration, alongside `config.weights`.
- Calibration is a property of each team's data; the starting values are a guess to
  be corrected after the first run.
