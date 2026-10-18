# 5. Ticket points are split equally between the assignees at close

Date: 2026-10-16
Status: Accepted

## Context

A closed ticket can have zero, one or several assignees, and other people touch it
too: the creator, commenters, whoever tests it.

## Decision

- A ticket's points are split equally between its assignees at the moment it closed.
- A ticket closed with no assignee is credited to nobody and counted as unattributed,
  the way the code leaderboard already counts commits it cannot attribute.
- Creators, commenters and testers earn no ticket points in the first version.
  Counting comments is easy to game, so it waits until there is real data to judge it.
- A ticket board user becomes an Identity of a Person. It is matched automatically
  when its email equals an email the Person already has, and otherwise mapped by hand
  in the Identities tab, the same way git identities are.

## Consequences

- People whose work is mostly testing, triage or review on tickets, without being an
  assignee, are under-represented in the tickets dimension.
- Reassigning a ticket just before closing moves all of its credit. This is accepted.
