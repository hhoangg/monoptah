# 13. Scoring is sized by a coverage percentage the user picks

Date: 2026-10-17
Status: Accepted

## Context

A Period can hold hundreds or thousands of closed tickets, and every assessment costs
AI tokens. The user wants to know the total before spending, and to choose how much
of the Period to assess.

## Decision

- After a workspace and Period are picked, the view shows: N closed tickets in the
  Period, A already assessed (an override counts as assessed), N - A not assessed,
  the runner and model that will be used, and an estimate of input tokens.
- The user picks a coverage percentage p (0-100) of the Period's total N. The target
  is ceil(p * N / 100) assessed tickets; the run assesses max(0, target - A) more.
  The view shows that number and its token estimate before anything runs.
- If the target is already met, there is nothing to score and the button says so.
- Which unassessed tickets are picked: round-robin across people (each person's
  oldest-closed unassessed ticket first, one person at a time), so a partial run
  covers everyone evenly. Tickets without an assignee come last.
- The leaderboard shows each person's coverage (assessed / total) next to their
  score, so a thin score is visible as thin.
- The token estimate is approximate and labelled so: it counts the ticket text the
  AI will read (title, description, tags, subtask titles) at about 4 characters per
  token plus the fixed rubric prompt per group; comments are fetched at run time
  and not counted.

## Consequences

- Scores of partially covered people are not comparable to fully covered ones; the
  coverage column is the guard against misreading them.
- Repeated runs with a higher percentage only assess the difference.
