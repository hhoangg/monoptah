# Cloud workspaces overview

Cost control for cloud sessions. A failed or forgotten codespace or Docker sandbox keeps drawing
quota (Codespaces: the free dollar pool) or credit (Docker: per runtime second, billed to the card
after the credit). **Project menu > Cloud workspaces…** (the item right after "Cloud settings…")
lists them all and lets you stop or delete them.

## What a row shows

Backend and name, state (Running, Starting, Stopping, Hibernating, Stopped, Failed), branch, machine
or size (and, for a sandbox, the Docker account it lives in), age and last use, what GitHub last
reported about a codespace's git state, the Monoptah sessions that use it, and an estimated cost
line.

* **Codespaces**: every codespace of the project's repository (`gh codespace list --repo`).
* **Docker sandboxes**: sandbox names carry no repository, so a sandbox named `monoptah-...` is
  listed when a session of this project uses it or when no session at all does. One that only
  another project's sessions use is left out. Sandboxes are listed **once per Docker account** the
  project can reach, each as that account (`sbx ls` with that account's token): the account of every
  sandbox the project's sessions are bound to (a sandbox lives in the account that created it) and
  the project's own account from its Cloud settings, or the machine's `sbx login` when it names none.
  The machine login is not asked, and not required, for a project that uses an account. An account
  that cannot be listed is named in a notice (with its own fix, not `sbx login`) and the others still
  list.

### Whose workspace it is

Other Monoptah apps (Native Dev, the installed app, the Tauri app) and other computers keep separate
stores but share the same `gh` and `sbx` accounts, so a workspace in the list may belong to one of
them. Monoptah only calls a workspace **its own** when this app's store knows it: a live or archived
session, a start record (a failed start has one), a move, a local session moved into it or converted
from it, a saved list of what a delete would lose, or a Docker account pin.

* Known to this store and used by no live session: an **orphan** (highlighted). Nothing in Monoptah
  will ever stop it.
* Not known to this store: **Not created by this app**. It is never called an orphan, because another
  app, another computer or a person may be using it right now.

## Actions

* **Stop** (workspace is up): the app API's `cloud.stop`, run as the Docker account the row was
  listed under.
* **Delete** (irreversible). Pressing it first **asks the workspace what would be lost**, on a worker
  thread, and shows the answer before anything can be confirmed:
  * a codespace: what GitHub reports (`gh codespace view --json gitStatus`, which never starts it):
    uncommitted changes, unpushed commits, or "GitHub did not report"; if that cannot be read, Delete
    is not offered;
  * what an earlier archive or convert recorded as not copied to this computer;
  * a sandbox is **not started to read it** (that would bill it again): the prompt says so, and says
    that anything in it that was not pushed or copied to this computer is lost.

  Delete needs an explicit acknowledgement ("I understand what is lost…") whenever something is
  listed, the contents cannot be read (every sandbox), or the workspace is not this app's; for a
  workspace not created by this app the prompt names it and says Monoptah cannot see who uses it. A
  clean codespace of this app asks only for the plain confirmation.

  The confirmation sends back the lines it showed. Right before the delete (after a sandbox's wait of
  up to 4 minutes, when it is stopped and reported `stopped`), the app **checks again**: a session
  working on it, an archive or convert in progress, a sandbox that was woken again, a session moved
  into it, and any line that was not in what the user confirmed all refuse the delete. Nothing is
  removed then.

  A **local session moved into the workspace** (its location is Cloud) blocks the delete from this dialog
  outright, like the automatic cleanup after a return (the app API's `cloud.delete` is unchanged): bring the conversation back to this computer first
  (Reset to local), or the turns that were not synced back are lost.

  After the checks, the usual cleanup runs: the workspace's sessions are removed from the window and
  the Sub-agents panel and the workspace is marked gone. A sandbox is stopped first and deleted once
  it reports `stopped`; a sandbox that is stopping or hibernating is waited for and a refused delete
  is retried.
* **Open in browser** (codespaces): the address is read with `gh api`, which never starts it.

The dialog **stays open while a stop or delete runs** (Escape and Close are ignored), so the result
always reaches the window and it forgets the sessions of a workspace that is gone.

## Loading

Read-only: `gh auth status`, `gh codespace list` and `sbx ls`, in the background, only when the
dialog opens or Refresh is pressed; the result is reused for 30 seconds and forgotten by every stop
or delete, whichever part of the app made it. Nothing starts a stopped workspace. If `gh` or `sbx`
is missing or signed out, the dialog says so with the command that fixes it and the other backend
still lists.

## Cost numbers are estimates

* Docker (USD per hour, from Docker's usage page): Micro 1 vCPU/2g $0.07, Small 2/4g $0.14, Medium
  4/8g $0.28, Large 8/16g $0.56, XL 16/32g $1.12.
* Codespaces: cores x $0.09 per core-hour (`basicLinux32gb` is 2 cores, `standardLinux32gb` 4,
  `premiumLinux` 8, `largePremiumLinux` 16, `xLargePremiumLinux` 32).
* Neither CLI reports uptime, so a running workspace is counted from its creation ("at most"). A
  stopped workspace shows no running cost. Free allowances, storage, prebuilds and Docker's credit are
  not modelled. A size that is not in these tables shows no price.

The constants live in `crates/monocode-core/src/cloud_sessions/workspaces.rs`.
