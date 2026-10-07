# Terminal (TUI) sessions

By default a new session is an app chat. Per provider, you can open new
sessions in the provider's own interactive CLI instead: **Settings → Providers →
(provider) → Session surface → Terminal (TUI)**.

The choice is made when a session is created and stays with it. Switching the
setting back to Chat only affects *new* sessions; open terminal sessions stay
terminal sessions. There is no way to convert an existing session.

## What runs

The tab runs the provider's CLI in a terminal, in the project folder, using the
binary path from the provider settings. If you picked a non-default Claude or
Codex account, the CLI starts under that account's profile.

| Provider | Launch |
| --- | --- |
| Claude Code | `claude --model <model> --permission-mode <mode>`, plus a conversation id (below) and the first prompt, if any |
| Antigravity | `agy` (the interactive CLI, not the ACP server the chat uses) |
| Everyone else | the bare CLI, fresh each time |

Every provider is offered, whether or not its CLI has an interactive mode. A
CLI without one exits right away. The bar under the terminal then reads
`Process exited (code N)` with a note that it stopped right after starting, and
a **Restart** button.

## Restoring

For Claude, the app saves a conversation id before the CLI starts. Quitting and
reopening the app, or pressing **Restart**, reopens the same conversation with
`claude --resume`. If that id turns out to be unknown to Claude, the bar offers
**Start new conversation**. Other providers start fresh on restart because the
app does not know how to resume them.

Closing a tab, archiving or deleting the session, and quitting the app all end
the CLI process.

## Limits

- Terminal sessions are not saved to the project history list; they come back
  only through the workspace restore.
- No chat features: no composer, queued messages, plans, second opinion or
  handoff. Anything the app opens with a prepared message (Add to chat, notes,
  handoffs, Inbox Ask, Monos, automations, remote sessions) always opens as chat.
- Inbox **Start work** passes its prompt to Claude as the first prompt. For a
  provider whose CLI cannot take one, the session opens as chat instead.
- "New worktree" on first send is unavailable; the header shows it disabled.
- Remote projects are always chat.
- The tab title follows the title the CLI sets, unless you or the app already
  named the tab.
