# Monoptah Native 0.51.0

## Changes since 0.50.4

### New

- Continue in cloud and Continue locally. A local session can be moved to a GitHub Codespace or Docker Sandbox from its menu, and brought back later, as one session with one conversation on both sides. Code travels by pushing the session's branch (never with force, never to the repository's default branch; if the branch has diverged the target is `<branch>-cloud`), and uncommitted tracked changes travel as a patch. Ignored files such as `.env.local` are never carried. A divider in the transcript marks the move. While a session is in the cloud, local runs and the local terminal are refused, and rewinding to before the move is blocked. If the transcripts on both sides diverged, the session forks to a new id instead of overwriting.
- Continue locally does not destroy local work. It saves a verified backup first, updates the working tree only if you have no conflicting changes, and offers Restore my backup and Discard backup afterwards. If the cloud workspace was lost, you get a summary and Reset to local. After a fully successful return the codespace or sandbox is deleted, unless other sessions still use it (they are named) or something could not be saved; a failed cleanup offers Retry or Keep in the session menu.
- Claude setup for cloud workspaces. Settings > Cloud holds one kit (skills, plugins, MCP servers, settings and your user `CLAUDE.md` with its `@` imports and `rules/*.md`) that is copied to every new cloud workspace in a "Claude setup" start step, with a per-start switch in the cloud picker. This step never fails the start. Secret values in MCP `headers` and `env` are stored on the workspace with mode 0600 and given only to Claude processes. MCP servers that sign in with OAuth are copied as configuration only and marked "Sign in in the cloud"; the app never reads the macOS Keychain or Claude Code's credentials for them.
- Docker Sandbox sessions now sign `gh` in with the saved Git credential (GitHub only), so Create PR works in the sandbox, and the agent is told that git and `gh` are set up and must not print the token.
- Cloud sessions are marked with a cloud badge wherever sessions are listed: sidebar rows (both layouts), title tabs, split pane headers, Mono launch cards, the Mono Sub-agents panel and app search results. Hovering shows the backend, workspace and state, for example `Codespace - octo-abc - Available`. The Sub-agents panel can be filtered to local or cloud sessions.
- Files and Changes now show the cloud workspace of a cloud session, with a Cloud / Local switch and a connection strip whose Connect action is the only thing that wakes a workspace. Saving a cloud file checks that the cloud copy has not changed; if it has, a conflict dialog lets you keep yours or take the cloud version. Remote text files over 1 MB open read-only. Push, pull, sync and PR on a cloud workspace are confirmed actions with timeouts. When the workspace is disconnected the panels show the last known files and Git state.
- Mono app API: `sessions.moveToCloud`, `sessions.moveToLocal`, `sessions.moveStatus` and `sessions.cancelMove`. `moveToCloud` takes the same cloud target as `sessions.start`. `moveToLocal` only opens the Continue locally dialog; you confirm it. All four are refused for unattended runs (habits, heartbeats, workflow questions).
- Mono app API: `accounts.list` lists the Claude, Codex and Devin accounts a session can use, and `sessions.start` takes an `account` field (a label, an id or `default`) for local sessions. In a Mono composer you can also write `@provider/label` to pick an account.
- Undo and redo in the composer and other plain text fields (Cmd+Z / Cmd+Shift+Z on macOS, Ctrl+Z / Ctrl+Y elsewhere; rebindable in Settings > Keybindings), and terminal-style prompt history: Up on the first line recalls earlier prompts of that chat, Down returns to your unsent draft. History comes from the saved transcript, so it survives restarts. Secret fields keep neither undo steps nor history.
- You can select and copy text in Documents (Mono documents, lightbox, plans, Markdown previews) and in the Notes preview, with drag, double-click (word), triple-click (paragraph), Cmd/Ctrl+C and Select All.

### Improvements

- Monoptah Host 1.2.0 is released with this version, and the app now installs the Host from Monoptah's own releases, pinned with checksums. The cloud features above need it (copying your `CLAUDE.md` needs 1.2.0; until then the rest of the kit is copied and the setup says your instructions follow after the update). Codespace and Docker Sandbox workspaces get it automatically: a new workspace installs it, and an existing one is upgraded when you start or reconnect it. A running turn is never interrupted; the upgrade then waits for the next connect. A Host on your own SSH machine is not upgraded automatically; use Update Host in Settings > Connections.
- Clicking a file in a cloud session chat now opens it directly instead of offering to copy it to this Mac. Large files download with progress and Cancel, and above 500 MB the app asks first. Files no viewer can show open in the default app only if they are a document, image, audio, video or data file; programs and installers are revealed instead.
- Starting several codespace sessions at once no longer fails with "Another SSH connection is being set up". The Host install now refuses Linux systems with musl or glibc older than 2.28 before downloading and reports the real error.
- Moving a session to a new Docker Sandbox waits for a slow code restore (up to 15 minutes) instead of failing with "The host request did not complete".
- The Continue in cloud, Continue locally, archive, Docker Sandbox setup and Git credential dialogs now block the window behind them: no hover, clicks, scroll or shortcuts reach it.
- Starting a session in a codespace without an SSH server now says to add the `ghcr.io/devcontainers/features/sshd:1` dev container feature, with a "How to fix" link, instead of showing the raw `gh` error.
- Transcript text selection can start from a margin or a gap between lines and paragraphs, and drags snap to the nearest line. A plain click still selects nothing.
- Setup-token quota: a manual refresh checks at most once every 60 seconds, a click during a running check is ignored, the idle chip reads "refresh to check usage", and a 404 names the model used for the check.
- Settings: the CLI account row is named "CLI login", and opening Settings loads the model list of every installed provider whose list is missing.

### Fixes

- Cloud sessions were missing from the title tab strip, also after a restart.
- After a Mono deleted a codespace or sandbox, its sessions stayed in the Sub-agents panel. They are now removed, and a session whose workspace failed to start shows "Failed to start".
- In automation runs, a continued session keeps its own account unless that account was removed.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
