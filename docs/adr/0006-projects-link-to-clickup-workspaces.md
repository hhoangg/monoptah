# 6. A project links to one or more ClickUp workspaces

Date: 2026-10-16
Status: Superseded by ADR 12

## Context

Contributors is per project (config in `.monoptah/contributors.json`), while a ClickUp
workspace usually holds tickets for many things. The app must know which tickets
belong to which project.

## Decision

- A project links to one or more ClickUp workspaces. The picker lists the workspaces
  the configured ClickUp token can access (the app already fetches this list for the
  inbox).
- The links are stored in `.monoptah/contributors.json`. Only workspace ids go there;
  the token stays in app settings.
- Within a linked workspace, the user may narrow to some spaces. By default every
  space counts. (Builder's call: covers a workspace shared by several projects
  without forcing anyone to configure it.)
- Assessments are stored per ticket id, independent of projects, so a ticket reachable
  from two projects is assessed once.

## Consequences

- A project that links a shared workspace without narrowing will count tickets that
  belong to other projects.
- Jira later needs its own equivalent link (site plus projects), stored next to the
  ClickUp links.
