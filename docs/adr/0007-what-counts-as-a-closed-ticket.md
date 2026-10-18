# 7. What counts as a closed ticket

Date: 2026-10-16
Status: Accepted; amended by ADR 14 (delivered statuses, subtasks)

## Context

ClickUp groups statuses into types: open, custom, done and closed. Statuses meaning
"not delivered" (Cancelled, Duplicate, Won't fix) usually sit in the closed group too.
Tickets also get reopened and closed again.

## Decision

- A ticket counts as closed when its status belongs to the done or closed group.
- Each linked workspace has an exclusion list of statuses, picked from the statuses
  that actually exist on that board. Tickets in an excluded status earn nothing.
- A ticket that was reopened and closed again belongs to the Period of its last close,
  and counts once.
- Subtasks (tickets with a parent) earn nothing themselves; their titles are input to
  the parent's Assessment. (Builder's call: prevents a parent and its subtasks from
  scoring the same work twice.)

## Consequences

- A ticket counted in last month's leaderboard moves to this month if it is reopened
  and closed again, so a past month's numbers can change.
- Until a user fills the exclusion list, cancelled tickets in the closed group earn
  points.
