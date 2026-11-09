# Monoptah Native 0.50.1

## Changes since 0.50.0

- Fixed new codespaces (and SSH machines without systemd) failing at "Connect over SSH" with "This host needs systemd user services and lingering". 0.50.0 downloaded an older Monoptah Host package that could not run without systemd; the Host now comes from the same source as before 0.50.0. Retry a failed start to reuse its codespace.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
