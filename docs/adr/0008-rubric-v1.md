# 8. Rubric v1: five criteria scored 0-2, summed into a level

Date: 2026-10-16
Status: Accepted

## Context

ADR 4 has an Assessment store a level (XS to XL). Asking the AI for a level directly
gives an unexplained label. Asking it to score named criteria makes each Assessment
explainable and easier to check.

## Decision

The AI scores five criteria, each 0, 1 or 2, with one short reason per criterion:

| Criterion | 0 | 1 | 2 |
|---|---|---|---|
| Ambiguity | Requirement is clear | Some points need clarifying | Has to work out what to build |
| Investigation | Cause known at once | Needs reproducing and tracing | Cause hard to find, several hypotheses |
| System scope | One place | A few related parts | Many parts, deep architecture knowledge |
| Risk | Breaking it is harmless | Affects users | Data, security, payments, production |
| Coordination | Done alone | Talks with one or two people | Depends on many people or another team |

The app, not the AI, sums the criteria (0-10) and maps the sum to a level:
XS 0-1, S 2-3, M 4-5, L 6-7, XL 8-10.

An Assessment stores the five criterion scores, their reasons, the Rubric version
(`1`), the runner and the model. The level is not stored: the app derives it from the
criterion scores whenever it reads them, so the mapping has one home.

The AI reads: title, description, tags, subtask titles, comments (truncated), and how
long the ticket spent in an in-progress status. It does not read assignee names or
PR size (ADR 2).

## Consequences

- Changing the sum-to-level mapping does not need re-assessment; changing the
  criteria does, and bumps the Rubric version.
- Time in progress is the only signal not written by people, so it partly guards
  against a ticket that reads as harder than it was.
