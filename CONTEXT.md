# Domain glossary

Terms used when talking about Contributors and ticket scoring. Code, docs and
discussions should use these words with these meanings.

## Contributors

- **Person**: one team member as Contributors sees them. A Person gathers several
  identities (git emails, GitHub logins).
- **Identity**: one account that can be attributed to a Person, e.g. a git author
  email or a GitHub login.
- **Period**: the date range a leaderboard covers (a month, or All time).
- **Dimension**: one weighted part of a Person's score: delivery, care, review,
  consistency, breadth.

## Ticket Contributors

- **Ticket Contributors**: the app-level view that ranks ClickUp users by the points
  of their closed tickets. Separate from repo Contributors; needs no repository.
- **Ticket member**: a ClickUp user as Ticket Contributors sees them, keyed by ClickUp
  user id.
- **Coverage**: the share of a Period's closed tickets that are assessed, overall or
  per Ticket member.
- **Coverage target**: the percentage of the Period's closed tickets the user wants
  assessed; a scoring run assesses only the difference to reach it.

## Ticket scoring

- **Ticket source**: a ticket board the app reads from. ClickUp first; Jira and others
  later. Each source only knows how to fetch tickets; everything after fetching is
  shared.
- **Workspace**: the ClickUp workspace Ticket Contributors reads, picked from those the
  configured token can access, optionally narrowed to some of its spaces.
- **Delivered status**: a status the user counts as "work delivered" for a workspace.
  Default: every done/closed status; custom ones such as "ready for testing" can be
  added.
- **Delivered ticket** (formerly closed ticket): a ticket in a delivered status and not
  in an excluded status, including subtasks; a parent with subtasks does not count. It
  belongs to the Period of its delivered date. Only delivered tickets are assessed and
  scored.
- **Delivered date**: done date for done/closed statuses, else the date it entered its
  current status (when ClickUp provides status history), else its last update.
- **Active ticket**: a ticket in a custom status that is not delivered. Counted per
  member as activity, never scored.
- **Excluded status**: a done/closed status that means "not delivered" (Cancelled,
  Duplicate). Configured per workspace.
- **Assessment**: the stored result of judging one closed ticket's difficulty. Made
  once per ticket and reused on every later run.
- **Assessment runner**: what performs an Assessment. The user chooses between Claude
  Code (their own subscription) and a direct API key.
- **Rubric**: the fixed rules an Assessment follows. Has a version; every Assessment
  records the Rubric version it was made under.
- **Criterion**: one of the Rubric's five questions (ambiguity, investigation, system
  scope, risk, coordination), scored 0-2 by the AI with a reason.
- **Level**: the difficulty bucket an Assessment assigns: XS, S, M, L or XL.
- **Points table**: configuration that turns a Level into points.
- **Difficulty**: how hard the problem behind a ticket was (ambiguity, investigation,
  risk, system knowledge needed). Not the size of the code change; that is delivery.
- **Override**: a level set by hand on one ticket, with an optional note. Never
  replaced by an AI Assessment.
- **Due date**: a ticket's planned deadline. Not used for scoring, because many
  tickets have none and the ones that exist are set unevenly.
