# 15. Members can be excluded from the Ticket Contributors board

Date: 2026-10-18
Status: Accepted

## Context

Some ClickUp users should not be ranked (managers, QA leads, bots, people who left),
but their tickets still appear in the workspace.

## Decision

- Each workspace scope has `excludedMembers`: ClickUp user ids, managed from a per-row
  "Exclude" action on the board and an "Excluded members" list in Scope to restore them.
- An excluded member is treated as if they were not an assignee. A ticket whose every
  assignee is excluded drops out entirely: not counted in N, not in coverage, never
  scored. A ticket shared with included members splits its points between the included
  members only.
- Excluded members do not appear in the leaderboard, the Active counts or the exported
  report. The report's method section lists the excluded members by name.
- Changing the list needs no refetch and no re-assessment.

## Consequences

- Excluding a member can raise other members' points on shared tickets.
- Assessments of dropped tickets stay stored; re-including the member brings them back.
