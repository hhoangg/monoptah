# Monoptah Native 0.48.0

## Changes since 0.46.0

- Run native sessions in GitHub Codespaces: configure access from the project menu, create and reconnect cloud sessions, and use remote terminals and the app API. Background requests do not wake sleeping codespaces.
- Pull evidence into the local session folder and preserve a local archived copy before stopping or deleting a codespace. Conversion checks saved commits, preserves restore references, and asks about unsaved files before deletion.
- Capture the Claude token from `setup-token` automatically, normalize pasted tokens, and validate them before saving.
- Copy Session ID now copies the numeric native session ID. Mono habits can inspect archive and worktree facts and use guarded session cleanup APIs.
- Closing a terminal pane or title tab now asks for confirmation when a foreground process is running. Closing several tabs lists their running terminals together; cancelling keeps them open. Idle shells still close immediately.
- Close All Tabs and Close Other Tabs keyboard shortcuts preserve pinned tabs, matching the tab menu.
- The collapsed sidebar peeks open in the boxy Focused layout, and glass blur eases with the slide animation.
- On mobile, the message composer stays editable while the desktop reconnects.
- Synced upstream through native preview 14 (`a25b5b89`). Monoptah's terminal session surfaces and other fork features are preserved.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
