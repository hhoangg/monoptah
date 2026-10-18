# 3. Tickets are a sixth dimension of the existing leaderboard

Date: 2026-10-16
Status: Superseded by ADR 12

## Context

Contributors has one leaderboard built from git and GitHub, with five dimensions
(delivery, care, review, consistency, breadth), each multiplied by a configurable
weight and summed into one score.

Two shapes were considered for ticket scores:

- A separate ticket leaderboard, with a toggle to merge it with the code leaderboard.
  Merging would need a rule to put two scores with different scales on one axis
  (for example each person's share of the team total), and shares are only comparable
  within one period.
- A sixth dimension of the existing leaderboard.

## Decision

- Tickets become a sixth dimension, `tickets`, next to the existing five.
- It has its own weight in `config.weights`, like the others, and appears in the
  breakdown bar and the formula sidebar.
- There is one leaderboard and one score per Person.

## Consequences

- Ticket points must be on a scale that sits sensibly next to PR delivery points
  (a PR earns up to 72). This calibration is part of the Rubric design.
- A Person with only ticket board activity appears on the leaderboard through the
  tickets dimension alone.
- Existing scores are unchanged when no ticket source is configured, because the
  tickets dimension is then zero for everyone.
