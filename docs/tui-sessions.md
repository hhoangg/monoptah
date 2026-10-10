# Terminal (TUI) sessions

By default a new session is an app chat. Per provider, you can open new
sessions in the provider's own interactive CLI instead: **Settings → Providers →
(provider) → Session surface → Terminal (TUI)**.

The choice is made when a session is created and stays with it. Switching the
setting back to Chat only affects _new_ sessions; open terminal sessions stay
terminal sessions. There is no way to convert an existing session.

## What runs

The tab runs the provider's CLI in a terminal, in the project folder, using the
binary path from the provider settings. If you picked a non-default Claude or
Codex account, the CLI starts under that account's profile.

| Provider      | Launch                                                                                                         |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| Claude Code   | `claude --model <model> --permission-mode <mode>`, plus a conversation id (below) and the first prompt, if any |
| Antigravity   | `agy` (the interactive CLI, not the ACP server the chat uses)                                                  |
| Everyone else | the bare CLI, fresh each time                                                                                  |

Every provider is offered, whether or not its CLI has an interactive mode. A
CLI without one exits right away. The bar under the terminal then reads
`Process exited (code N)` with a note that it stopped right after starting, and
a **Restart** button.

## Restoring

For Claude, the app saves a conversation id before the CLI starts. Quitting and
reopening the app, or pressing **Restart**, reopens the same conversation with
`claude --resume`. A tab you never typed in has no conversation yet, so Claude
answers `No conversation found`; the app then relaunches once with
`claude --session-id` under the same id instead of showing an error. If the
launch still fails, the bar offers **Start new conversation**. Other providers
start fresh on restart because the app does not know how to resume them.

A terminal session with a conversation id is saved with the project's sessions,
so it appears in the sidebar list, survives closing the tab (archive), and
reopens from there into the same conversation. Its row has no preview text; it
shows the tab title. A terminal session with no id (every provider except
Claude) has nothing to reopen, so closing its tab leaves no row.

Closing a tab, archiving or deleting the session, and quitting the app all end
the CLI process.

## Limits

- No chat features: no composer, queued messages, plans, second opinion or
  handoff. Anything the app opens with a prepared message (Add to chat, notes,
  handoffs, Inbox Ask, Monos, remote sessions) always opens as chat.
- An automation follows the provider's surface when its CLI can take a first
  prompt (Claude); otherwise it opens as chat. In a terminal the prompt is
  passed to the CLI as its first prompt, and every run opens a new terminal
  session, so the Conversation setting (continue the last run) is unavailable.
  A "new worktree" automation creates its worktree before the CLI starts. The
  run is recorded as succeeded once the CLI stays up for a few seconds, and as
  failed if it exits or cannot start in that time. Run success still means the
  CLI took the prompt, not that its turn finished.
- A Claude terminal session shows as busy while a turn runs. The app learns this
  from Claude Code hooks it adds to the launch with `--settings`: a submitted
  prompt marks it busy, and `Stop`, `StopFailure`, `SessionEnd` or the CLI
  exiting mark it idle. Permission and "needs input" notifications do not clear
  busy.
  - Claude does not fire `Stop` when you interrupt a turn with Esc. That case
    only clears when Claude's idle prompt notification arrives, which comes after
    Claude has been waiting for about a minute, so the session stays busy that
    long.
  - Your own hooks are not affected: Claude merges the injected ones with them.
    With **Claude Code hooks** turned off in Settings this app injects nothing,
    so a terminal session never shows as busy.
  - Known limitation: the **Claude Code hooks** setting behaves differently on
    the two surfaces. For a chat session it also turns off every hook, yours
    included. For a terminal session it only stops this app injecting its own
    hooks, and your own global hooks still run. Someone who turned the setting
    off because a hook is slow or destructive will still see that hook fire in
    every terminal session.
  - Busy state does not survive a restart: the workspace snapshot always
    restores `busy: false`, and any event left in the spool is pruned before the
    first read. Afterwards only a stale _busy_ event is ignored; a stale idle one
    still applies, because nothing else would ever clear the flag.
  - On Windows nothing is injected, because Claude runs a hook command through
    Git Bash or PowerShell and no one command works in both. A terminal session
    never shows as busy there.
  - Other providers have no hook integration and also never show as busy.
- Inbox **Start work** passes its prompt to Claude as the first prompt. For a
  provider whose CLI cannot take one, the session opens as chat instead.
- "New worktree" on first send is unavailable; the header shows it disabled.
- Remote projects are always chat.
- The tab title follows the title the CLI sets, unless you or the app already
  named the tab.
