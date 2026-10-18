# 12. Ticket Contributors is its own view, separate from repo Contributors

Date: 2026-10-17
Status: Accepted. Supersedes ADR 3 and ADR 6; replaces the "fetch with Sync" part of ADR 10.

## Context

The first build put ticket scores inside a repository's Contributors page: a Tickets
tab, a sixth `tickets` dimension, workspace links in `.monoptah/contributors.json`,
and ticket fetching tied to the git Sync. In manual testing this was hard to use:
getting a ticket score took opening a repo, linking a workspace, syncing, fetching
and assessing, and an already-synced period offered no way to fetch at all. It also
missed the goal, which is to see contribution that happens outside any repository.

## Decision

- A new top-level view, **Ticket Contributors**, opens from the app menu like Inbox
  or Mail. It needs no repository.
- The user picks a ClickUp workspace (from the configured ClickUp token) and a
  Period: month, quarter, year or all time.
- Picking a workspace and Period fetches that window's closed tickets from ClickUp.
  Fetching costs no AI tokens.
- People are ClickUp users, keyed by ClickUp user id. No merging with git identities.
- Workspace choice, space narrowing, excluded statuses and the points table are app
  settings, stored with the app's other preferences, never in a repository.
- Repo Contributors goes back to exactly what it was before: no Tickets tab, no
  `tickets` dimension, no ticket config in `.monoptah/contributors.json`.
- A combined code-and-ticket view may come later, once this view is in use.

## Consequences

- ADRs 1, 2, 4, 5 (assignee split), 7, 8, 9, 11 still apply. The Rust fetch, store,
  rubric, runners and job manager are reused unchanged in behaviour.
- The ticket points table is no longer anchored to PR delivery points (ADR 4's anchor
  only mattered inside the code leaderboard); its default values stay.
