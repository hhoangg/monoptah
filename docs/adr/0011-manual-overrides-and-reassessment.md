# 11. Manual overrides and re-assessment

Date: 2026-10-16
Status: Accepted

## Context

An AI Assessment will sometimes be wrong. The user needs to see why a ticket got its
level and to correct it, without the correction being lost on a later run.

## Decision

- Opening a Person on the leaderboard lists their scored tickets with level, the five
  criterion scores and their reasons.
- The user can override a ticket's level (XS to XL) with an optional note. Overrides
  are at level granularity, not per criterion.
- An overridden ticket is marked as such and is never replaced by an AI Assessment,
  including during re-assessment.
- The user can select several tickets and re-assess them in one run (for example after
  a Rubric change, or when a group of tickets looks mis-assessed). Re-assessment uses
  the same batched job as ADR 10.

## Consequences

- An override stores the level and note next to the original AI Assessment, so the
  AI's reasoning stays visible.
- The points of an overridden ticket follow the points table like any other level.
