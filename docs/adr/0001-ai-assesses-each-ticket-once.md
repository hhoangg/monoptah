# 1. AI assesses each closed ticket once and the result is stored

Date: 2026-10-16
Status: Accepted

## Context

Contributors scores people from git and GitHub only, so work tracked on a ticket
board (ClickUp first) is invisible. Ticket boards carry no reliable difficulty signal:
story points and time estimates are filled in for some tickets and not others, and
due dates are missing on many tickets and set unevenly on the rest.

A board can hold thousands of closed tickets. Scoring runs monthly or yearly, not
continuously.

## Decision

- An AI judges the difficulty of each closed ticket against a written Rubric.
- The result (an Assessment) is stored by ticket id and reused. A ticket is never
  re-assessed automatically, so the same ticket cannot get two different scores
  across runs. Re-assessing is an explicit user action.
- Each Assessment records the Rubric version and the model that made it.
- The Assessment runner is the user's choice: Claude Code or a direct API key. Both
  use the same Rubric and the same output format, so their results are comparable.
- Due dates play no part in the score.
- Ticket sources are pluggable. ClickUp is built first; adding Jira only adds a new
  fetcher.

## Consequences

- The first run over an existing board has to assess thousands of tickets, so it must
  run in the background, show progress, and resume after interruption.
- Changing the Rubric leaves older Assessments under the old version until the user
  chooses to re-assess them.
- Score quality depends on the Rubric, which becomes a product artifact that needs
  care and calibration.
