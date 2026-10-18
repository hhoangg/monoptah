# 9. Assessments live in the app's local database

Date: 2026-10-16
Status: Accepted

## Context

Assessments could live in the repo (shared by the team through git) or in the app's
own SQLite database on the machine, next to the `contrib_*` tables that Contributors
sync already writes (`src-tauri/src/contributors/store.rs`).

Contributors is expected to be run by one person, not by every team member.

## Decision

- Assessments, including manual overrides, are stored in the app's SQLite database,
  keyed by ticket source and ticket id.
- They are not written to the repo.

## Consequences

- No ticket titles or assessment reasons end up in git.
- A second machine running Contributors starts with no Assessments and would assess
  again, possibly with different results. If more people start using Contributors,
  revisit this (for example with an export/import of Assessments).
- Assessments are lost if the app's data directory is wiped.
