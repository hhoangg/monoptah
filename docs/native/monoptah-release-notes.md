# Monoptah Native 0.50.2

## Changes since 0.50.1

- Clicking a file path in a cloud session chat (Markdown, image, HTML and other files) now opens the preview even after the agent has finished. Before, once the workspace tunnel was gone the card only said "Reconnect to view"; a click now reconnects the Codespace or Docker Sandbox, shows the progress and then opens the file.
- Monos and the app API now treat Codespaces and Docker Sandbox sessions the same way. The `codespaces.*` actions are replaced by `cloud.list`, `cloud.status`, `cloud.create`, `cloud.stop`, `cloud.resume` and `cloud.delete`, and `sessions.start` takes `target {kind, instance}` (an existing instance can also be given as `{session: <id>}`). Sessions report `target` and `cloudState` for both backends.
- A Mono can resume a stopped Docker Sandbox like a Codespace, including by sending to a session on it, and `sessions.pullEvidence` works for sandbox sessions. Stopping or deleting an instance lists the sessions on it, and deleting is refused while any of them is busy.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
