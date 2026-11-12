# Cloud workspaces overview

Cost control for cloud sessions. A failed or forgotten codespace or Docker sandbox keeps drawing
quota (Codespaces: the free dollar pool) or credit (Docker: per runtime second, billed to the card
after the credit). **Project menu > Cloud setup > Cloud workspaces** lists them all and lets you
stop or delete them.

## What a row shows

Backend and name, state (Running, Starting, Stopping, Hibernating, Stopped, Failed), branch, machine
or size, age and last use, the Monoptah sessions that use it, and an estimated cost line. A workspace
no live session uses is highlighted as an orphan: nothing in Monoptah will ever stop it.

* **Codespaces**: every codespace of the project's repository (`gh codespace list --repo`).
* **Docker sandboxes**: `sbx ls` shows the whole account and sandbox names carry no repository, so a
  sandbox named `monoptah-...` is listed when a session of this project uses it or when no session
  at all does. One that only another project's sessions use is left out.

## Actions

* **Stop** (workspace is up): the app API's `cloud.stop`.
* **Delete** (asks first, irreversible): the app API's `cloud.delete` checks (a working session, an
  archive in progress), then the usual cleanup: the workspace's sessions are removed from the window
  and the Sub-agents panel and the workspace is marked gone. A sandbox is stopped first and deleted
  once it reports `stopped`; a sandbox that is stopping or hibernating is waited for (up to 4 minutes)
  and a refused delete is retried.
* **Open in browser** (codespaces): the address is read with `gh api`, which never starts it.

## Loading

Read-only: `gh auth status`, `gh codespace list` and `sbx ls`, in the background, only when the
dialog opens or Refresh is pressed; the result is reused for 30 seconds. Nothing starts a stopped
workspace. If `gh` or `sbx` is missing or signed out, the dialog says so with the command that fixes
it and the other backend still lists.

## Cost numbers are estimates

* Docker (USD per hour, from Docker's usage page): Micro 1 vCPU/2g $0.07, Small 2/4g $0.14, Medium
  4/8g $0.28, Large 8/16g $0.56, XL 16/32g $1.12.
* Codespaces: cores x $0.09 per core-hour (`basicLinux32gb` is 2 cores, `standardLinux32gb` 4,
  `premiumLinux` 8, `largePremiumLinux` 16, `xLargePremiumLinux` 32).
* Neither CLI reports uptime, so a running workspace is counted from its creation ("at most"). A
  stopped workspace shows no running cost. Free allowances, storage, prebuilds and Docker's credit are
  not modelled. A size that is not in these tables shows no price.

The constants live in `crates/monocode-core/src/cloud_sessions/workspaces.rs`.
