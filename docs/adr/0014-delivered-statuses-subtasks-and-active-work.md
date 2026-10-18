# 14. Delivered statuses, subtasks, and active work

Date: 2026-10-17
Status: Accepted. Amends ADR 7.

## Context

Measured on the Casium workspace for one member over 30 days: 105 assigned tickets
were updated, but only 16 counted. The rest were missed for two reasons:

- 26 shipped tickets were subtasks, and ADR 7 gave subtasks no points. This team
  splits real work into subtasks.
- 22 tickets sat in "ready for casium testing", a custom status. For a developer the
  work is delivered there, but ClickUp only sets a done date for done/closed
  statuses, so the fetch (filtered by done date) never returned them.

The workspace's status history API returns an empty history (the "Total time in
Status" ClickApp appears to be off), so the date a ticket entered a custom status is
not always available.

## Decision

- **Delivered statuses** are chosen per workspace. Every status whose type is done or
  closed is delivered (the old behaviour); the user adds extra statuses such as
  "ready for casium testing". Unticking a done/closed status in the checklist adds it
  to the excluded statuses, so a done/closed status seen for the first time later
  still counts. Changing the choice needs no refetch.
- **Delivered date** of a ticket, in order of preference:
  1. its done date, when its status is of type done or closed;
  2. the date it entered its current status, from ClickUp's status history, when
     available;
  3. its last update date (approximate; the view says when dates are approximate).
- **Fetching** asks ClickUp for every ticket updated since the window start, whatever
  its status, and stores them all with their candidate dates. Classification happens
  in the app, not in the fetch.
- **Subtasks count.** A parent ticket counts only when no stored ticket has it as
  parent, because its work is already in its subtasks.
- **Active work** (status type custom and not a delivered status) earns no points and
  is never assessed. The leaderboard shows it as a separate "Active" count per member,
  anchored by last update date.
- Excluded statuses (ADR 7) keep working and win over delivered statuses.

## Consequences

- Fetches return more tickets (every updated ticket, not only closed ones).
- With the ClickApp off, a ticket's delivered month can move if it is edited later.
  Turning on "Total time in Status" in ClickUp makes dates exact without code changes.
- A parent whose subtasks all fall outside the fetched windows may count until those
  subtasks are fetched.
