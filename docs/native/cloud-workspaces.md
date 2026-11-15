# Cloud workspaces overview

Cost control for cloud sessions. A failed or forgotten codespace or Docker sandbox keeps drawing
quota (Codespaces: the free dollar pool) or credit (Docker: per runtime second, billed to the card
after the credit). **Project menu > Cloud workspaces…** (the item right after "Cloud settings…")
lists them all and lets you stop or delete them; a chip in the footer shows what runs (see "The footer chip").

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
  listed under. It **asks first** when a session is working on the workspace (the turn would be cut
  off) or the workspace is not this app's (whoever uses it would be); any other row stops at once.
  Stopping keeps the state. If the stop says the sessions it interrupted, that warning stays on
  screen under the summary until the next action on that workspace. A Stop only ever stops: it
  never deletes. What it does and how its result is told is under "What a Stop does and how it is
  verified".
* **Delete** (irreversible). Pressing it first **asks the workspace what would be lost**, on a worker
  thread, and shows the answer before anything can be confirmed:
  * a codespace: what GitHub reports (`gh codespace view --json gitStatus`, which never starts it):
    uncommitted changes, unpushed commits, or "GitHub did not report"; if that cannot be read, Delete
    is not offered. The prompt says this is **as GitHub last reported it**: it can be a few minutes
    behind, so a clean report is not proof that nothing was changed there just now;
  * what an earlier archive or convert recorded as not copied to this computer;
  * a sandbox is **not started to read it** (that would bill it again): the prompt says so, and says
    that anything in it that was not pushed or copied to this computer is lost.

  Delete needs an explicit acknowledgement ("I understand what is lost…") whenever something is
  listed, the contents cannot be read (every sandbox), or the workspace is not this app's; for a
  workspace not created by this app the prompt names it and says Monoptah cannot see who uses it. A
  clean codespace of this app asks only for the plain confirmation. The acknowledgement is not only
  a checkbox: the delete call carries `acknowledged`, and the app API refuses a workspace this app
  did not create without it, whatever the caller confirmed.

  The confirmation sends back the lines it showed. Right before the delete (after a sandbox's wait of
  up to 4 minutes, when it is stopped and reported `stopped`), the app **checks again**. What it
  reads fresh: the sessions in the store, a sandbox that was woken again (it is running again after
  it was stopped), a session moved into it, and any line that was not in what the user confirmed.
  Each refuses the delete and nothing is removed. What it does **not** see is an archive or convert
  that began after the click: the window's "is something working on it" is a copy taken when Delete
  was confirmed, so during the wait the protection is the woke check above. A sandbox in a state
  Monoptah does not recognise is neither stopped nor deleted (it could be running).

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

## What a Stop does and how it is verified

`sbx --cloud stop` only says the request was **accepted**; the sandbox reaches `stopped` some time
later (about 36 s for a micro one, 7 to 9 minutes for a large one). A sandbox with **Auto-resume**
starts again when **traffic reaches it**, and a session's ssh tunnel is such a path: measured on a
micro sandbox, the tunnel (`ssh -N -L …` with its `sbx --cloud ssh-connect` child) **survived the
stop** for as long as it was watched (6 minutes, its keepalives still answered), and the first
request sent through the forwarded port resumed the sandbox within 20 seconds. The app does exactly
that when it polls a session's Host. Closing the tunnel ends the `ssh-connect` child within a second.
An ssh that is *started* against a stopped sandbox does not resume it (`ssh-connect` refuses with
"cannot issue an SSH certificate… the sandbox is not running"); `exec`, `attach` and `run` do. So
Monoptah does two things around the call
(`CloudSessions::stop_cloud`, the one path every Stop takes, the dialog, the app API, a Mono and the
auto-stop alike):

1. **It closes its own connection first.** A session of a sandbox holds an ssh tunnel
   (`sbx --cloud ssh-connect`, with keepalives) to it. Before the stop is sent, the tunnels of every
   machine of that sandbox are closed (the ssh process is killed, which ends the `ssh-connect`
   child). The close is immediate and never waits for a tunnel that is being started. On macOS
   and Linux that the `sbx ssh-connect` child exits within a second was **measured on macOS
   only**; on Windows `Child::kill` ends `ssh.exe`, and the child's exit there is not verified.
   Everything the stop needs (the sandbox's id and Docker account) is resolved first, so a stop that
   cannot even be sent leaves the connections as they were. The machine itself is kept: the session's **Reconnect** pairs it again, and nothing else
   does. Background work never reconnects a sandbox (`SANDBOX_NOT_CONNECTED`); only a send, a click
   on Reconnect or on a file link resumes it. If the stop itself is refused, the error says the
   sessions need Reconnect because their connection was already closed. If closing fails, the stop
   is still sent (a sandbox that keeps billing is worse), the Stop's own warning says "Stop sent,
   but the app could not close its connection to it", and the reason shown if it comes back
   says so. A codespace has no such tunnel to close and is left as it was.
   The same close runs when **two consecutive listings** show a sandbox stopping or stopped while a
   tunnel to it is still open (a stop by the TTL, by another computer or by hand), except while the
   user's own Reconnect is bringing it back or a start of it is pending: a single, possibly stale,
   listing never closes a connection somebody has just opened. A listing also never closes a
   connection that was opened **after the listing was read** (a Reconnect that finished while the
   listing was being processed): the listing's read time is compared with the tunnel's start time.
   If the connection is busy (a request holds its lock), the close is **deferred**: it closes
   when that request ends, and the Stop's warning says "the app's connection to it is busy… it may
   wake the sandbox again".
2. **It verifies the result from read-only listings** (`sbx ls`, never `exec`, `ssh`, `attach` or
   `cp`, which would wake it) instead of trusting the call. The row's state comes from the listing
   and a line under it tells what became of the Stop; while it is on its way the dialog reads the
   listing again by itself (every 15 s, every 60 s once it is slow) and stops reading when it has
   settled. Closing the dialog ends the polling; the footer chip goes on.

   | The line says | When |
   | --- | --- |
   | `Stopping…` | The stop was accepted and the listing says stopping (or has not caught up: a listing that still says running up to 60 s after the request is not a verdict). |
   | `Taking longer than expected: still stopping after 11m…` | Still stopping 10 minutes after the request. It keeps being read; it may still bill. |
   | `Stopped` | The listing says stopped. |
   | `Still running: <reason>` | The listing says running more than 60 s after the request, **or running again after it had been seen stopped** (another computer's tunnel, the TTL, Auto-resume traffic). The reason names Auto-resume and whether Monoptah's own connection was closed. It stays until the user stops it again, brings it back with Reconnect (a Reconnect that really goes ahead; a refused or duplicate one, such as a send into a stopping sandbox, leaves it), or it is seen stopped, or a start (or its Retry) binds to the sandbox, or the user presses **Dismiss** on the line (for a sandbox resumed on purpose by other means, such as `sbx run` or another computer). A workspace is never quietly "Running" again after a Stop. |
   | `Could not read the state of …: <problem>` | The Stop is followed but the sandbox listing failed, or no longer lists it. It is read again slowly; it is given up after three readable listings without the sandbox, or 30 minutes after the request. Only a load of a project that lists the sandbox counts: another project's load (the footer chip after a project switch) neither counts nor says anything about it. |
   | the error | The call itself failed (shown as the row's error as before). |

   Only a **sandbox** Stop of this app is followed. A codespace Stop is left as it was (no line, no
   polling). The state of a workspace stopped by someone else, or before this app started, is told
   by the listing alone (the row and the chip), and counted from the first listing that showed it
   stopping. A sandbox that is being **deleted** is neither stopping nor followed.

   The follow tick reads only the sandbox listing (`sbx ls`, once per Docker account), merged into
   the rows loaded last; Refresh and opening the dialog still read `gh` too.

   The memory of a Stop is **in this process only**. After an app restart the Stop line is gone
   and the chip's 15-minute timer restarts at the first listing that shows the sandbox stopping
   (the row's state is always read from the listing, so nothing is hidden); a second Stop of a
   sandbox already shown stopping keeps the first request time.

   The app API's `cloud.stop` and a Mono's answer keep their `stopped: true` field, which now
   means the stop was **requested and accepted**, not that the sandbox has stopped.

## Loading

Read-only: `gh auth status`, `gh codespace list` and `sbx ls`, in the background, only when the
dialog opens or Refresh is pressed; the result is reused for 30 seconds and forgotten by every stop
or delete, whichever part of the app made it. Nothing starts a stopped workspace. If `gh` or `sbx`
is missing or signed out, the dialog says so with the command that fixes it and the other backend
still lists.

## The footer chip

The cost is also visible without opening the menu: a **cloud chip** sits in the footer next to the
provider usage chip (cloud glyph and `2 running`, `1 stopping` or `2 running, 1 stopping`). It is one click or one Enter or Space (it is
focusable like the usage chip) from the same Cloud workspaces dialog the menu item opens. The menu
item stays.

* **Shown** while at least one workspace of the open project is **Running or Starting**, and also
  while one is **Stopping** (or hibernating, on its way to stopped): a stopping sandbox still bills
  until it has stopped, and a Stop that gets stuck must not make the chip vanish. It hides only when
  every workspace is stopped or gone; then it is not drawn and takes no room. A sandbox that is
  being deleted does not count as stopping. It reports on **the window's project** on every tab: a
  session, the MonoCrab (Mono) tab, an app view (Today, Inbox, Notes, Contributors…), a terminal or a
  file pane, whatever folder that tab shows. Only a cloud session uses its own project, the local one
  it belongs to (its folder is `remote://…`). A window on a remote project that is not a cloud
  session of a local one shows no chip. One place decides this (`Shell::cloud_chip_project`,
  `crates/monocode-gpui/src/shell.rs`) and runs whenever the tab, the app view or the project
  changes.
* **Warning colour** (the theme's warning amber) when any workspace has a concern:
  * **Failed**: the workspace is in a failed state;
  * **Orphan**: it is this app's (see "Whose workspace it is"), no live session uses it and it is
    up or starting. A stopping or stopped orphan is not flagged (a stopping one is neutral, see
    below); a workspace this app did not create is never flagged as an orphan;
  * **Stopping for more than 15 minutes** (`STOPPING_WARN`,
    `crates/monocode-core/src/cloud_sessions/stopping.rs`): a stopping workspace is neutral, and
    turns to the warning only when it has been stopping that long. The time counts from the Stop
    this app had accepted, else from the first listing that showed it stopping;
  * **Running again after a stop**: a Stop was accepted and the listing says it runs (the dialog
    says `Still running: …`);
  * **Running for hours with no session working**: up for at least `LONG_RUNNING` (6 hours,
    `crates/monocode-core/src/cloud_sessions/footer_status.rs`) and no session of it is busy. The age
    counts from creation, an upper bound, like the cost estimate (neither CLI reports uptime).

  A failed or orphan workspace shows the chip only together with something that runs or stops
  (nothing running or stopping: nothing drawn). The chip's accessible description and hover tooltip name each workspace
  with its backend, state and concern, the worrying ones first.
* **Reading** reuses the overview loader above, so it is read-only and never wakes or starts a
  workspace and spends no billable call. It asks at most about **once a minute**, **only while the
  window is active**, and **only for a project that uses cloud**: it has saved Cloud settings (either
  backend) or a cloud session. A plain GitHub project never starts `gh`. Coming back to the window,
  switching project, or a stop or delete from the dialog read again at once (the core service still
  reuses an overview younger than 30 seconds). A missing or signed-out `gh` or `sbx` is **silent**: that
  backend lists nothing, the chip stays hidden when nothing runs, and there is no toast or error
  text. The dialog is where such a problem is explained.

## Cost numbers are estimates

* Docker (USD per hour, from Docker's usage page): Micro 1 vCPU/2g $0.07, Small 2/4g $0.14, Medium
  4/8g $0.28, Large 8/16g $0.56, XL 16/32g $1.12.
* Codespaces: cores x $0.09 per core-hour (`basicLinux32gb` is 2 cores, `standardLinux32gb` 4,
  `premiumLinux` 8, `largePremiumLinux` 16, `xLargePremiumLinux` 32).
* Neither CLI reports uptime, so a running workspace is counted from its creation ("at most"). A
  stopped workspace shows no running cost. Free allowances, storage, prebuilds and Docker's credit are
  not modelled. A size that is not in these tables shows no price.

The constants live in `crates/monocode-core/src/cloud_sessions/workspaces.rs`.
