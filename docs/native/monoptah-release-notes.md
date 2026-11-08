# Monoptah Native 0.50.0

## Changes since 0.49.0

- Cloud sessions can now run in Docker Sandboxes (`sbx --cloud`) as well as GitHub Codespaces. Pick the cloud and its settings (machine, branch, resources) in the composer; the project's saved cloud settings are only the defaults. Requires `sbx login` and `sbx --cloud setup ssh` done once by you; the repository's devcontainer is used when there is one, otherwise the session runs directly in the sandbox.
- Sending the first prompt creates the codespace or sandbox. The prompt waits in the message queue and is sent once the workspace is ready, while a panel above the composer shows the current step and its latest log line.
- A stopped codespace or sandbox can be resumed from the session: the Send button turns into "Resume", and sending a prompt resumes the workspace and then sends it. Only your own send or click resumes a sandbox; background work never does.
- Sandbox sessions keep a Git credential (GitHub or GitLab.com token, stored encrypted in the app database) and a Claude setup-token or API key out of command lines and logs.
- Connection notices for cloud sessions now sit above the composer, and a stopped workspace no longer reports "The host request did not complete" from a stale connection.
- Files from a cloud session can be opened in the app from their path in the chat; large files are downloaded to a temporary local folder first.
- Mono can archive a cloud session and then keep, stop or delete its codespace or sandbox (`sessions.archive` with `cloud`). The local copy is always saved first, and deleting is only done when you ask for it.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
