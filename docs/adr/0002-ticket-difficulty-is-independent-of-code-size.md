# 2. Ticket difficulty is independent of code size

Date: 2026-10-16
Status: Accepted

## Context

Contributors already rewards code volume through the delivery dimension (PR size,
capped). A ticket linked to a PR could look like it is being counted twice if its
difficulty also earned points.

But a small diff does not mean an easy task. A three-line fix can be the end of days
of investigating a race condition; a large diff can be mechanical.

## Decision

- Difficulty measures the problem: ambiguity, investigation effort, risk, and how much
  of the system must be understood. It does not measure the size of the change.
- The Rubric must not use lines changed, file count or PR size as a difficulty signal.
- A ticket linked to a PR earns its difficulty score in full, alongside the PR's
  delivery score. The two measure different things, so this is not double counting.
- No automatic flag compares difficulty against PR size.

## Consequences

- Without code size as an anchor, the Rubric must rely on the ticket's own content
  (description, subtasks, comment discussion, status history) and on calibration
  examples to keep Assessments consistent.
- A ticket with a long, dramatic description could look harder than it is. The Rubric
  needs to judge the problem, not the prose.
