# Monoptah Native 0.51.1

## Changes since 0.51.0

### New

- Docker accounts in Settings > Cloud. Add an account with its Docker ID and a personal access token created with "Allow Cloud Sandboxes access" (the token page opens from a link). The token is checked when you save, with a clear error for a wrong Docker ID or token and for a token without Cloud Sandboxes access, and nothing is saved then. Each account can replace its token, be checked again or be removed; removing is refused while a project or a sandbox uses it. Tokens are stored encrypted in the app database, never in the Keychain, and never appear in commands, logs or files.
- A Docker account per project. Every `sbx` command and every sandbox ssh connection (tunnels, scripts, terminal, Host upgrade) runs as the account that created the sandbox, or as the project's account before a sandbox exists. A revoked or expired token tells you to replace it in Settings > Cloud.
- Settings > Cloud has a Backends checklist: `gh` signed in with the codespace scope, `sbx` installed and its ssh set up. Check again runs the checks again, and Fix runs the needed command in a terminal.
- One Cloud settings dialog per project, opened with "Cloud settings…" in the project menu or from the composer. It replaces the Codespaces setup and Docker Sandbox setup dialogs. A Codespaces | Docker Sandbox switch shows the same rows for both backends: the default backend, a Ready checklist with Fix, the workspace (repository, base branch, dev container, machine or size, stop when idle or runtime limit) and the credentials (Docker account, Claude account, Git). Git credentials are managed in a side panel inside the dialog instead of a second dialog.
- Codespaces can use a Claude account from the app instead of the Codespaces secret. Pick it in the project's Cloud settings; it reaches the codespace on every start, reconnect, wake and Host upgrade. A change never interrupts a running turn: it is applied once the codespace is idle, and the next send says "Applying the Claude account to the codespace" while it happens.
- Mono app API: `sessions.start` now takes `account` for a new cloud instance too.

### Improvements

- Existing setups keep working as before: without a Docker account, sandboxes keep using the Mac's `sbx login`, and codespaces keep using the Codespaces secret until you choose an app account. Saved sandbox settings are read as they are.
- The composer's cloud panel shows the same rows for both backends (Machine or Size, Stop when idle or Runtime limit, Base branch, Branch, Docker account, Claude account, Claude setup), each marked as the project default or "changed for this session". Only changed rows override the project settings. A codespace can start on a new working branch. The Workspace menu gains "Existing instance…" and "Cloud settings for this project…".
- A Docker or Claude account changed for one session stays with its codespace or sandbox across stop, reconnect and delete, also after the project default changes.
- The selects in Cloud settings open as a floating menu under the field instead of pushing the form down, pick up accounts, credentials and machines that load while they are open, and keep the checked value highlighted.
- The Git credential form no longer shows a read-only Host row; the platform (GitHub or GitLab) already sets it.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- No Host update comes with this version; Monoptah Host stays at 1.2.0.
- "Existing instance…" for Docker Sandbox lists only sandboxes that have a ready session of the project. Docker token expiry shows "Not known", because Docker Hub does not report it.

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
