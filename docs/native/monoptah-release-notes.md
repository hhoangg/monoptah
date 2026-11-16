# Monoptah Native 0.53.4

## Changes since 0.53.3

### Fixes

- Provider sign-in no longer leaves a login process (such as `claude auth login`) running after Monoptah Native ends. The app now stops it when you quit, including a quit from the Dock or at logout, and also when the app is force-quit or crashes. On macOS and Linux, a login process left over from an earlier crash is also stopped at the next start. On Windows, a CLI started through a `.cmd` shim can still leave its `node.exe` behind after a crash. (#135)

### Docs

- The repository now documents its git conventions and has pull request and issue templates. (#136, #137)

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- No Host update comes with this version; Monoptah Host stays at 1.2.0.
- "Existing instance…" for Docker Sandbox lists only sandboxes that have a ready session of the project. Docker token expiry shows "Not known", because Docker Hub does not report it.
- Cost control does not stop workspaces that no Monoptah session uses; stop those from the Cloud workspaces overview.

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
