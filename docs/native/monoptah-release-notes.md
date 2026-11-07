# Monoptah Native 0.48.1

## Changes since 0.48.0

- A Codespace session appears as soon as its codespace is requested. A new Codespace panel follows the startup with each phase, its duration and the live creation log, and the transcript reports "Codespace ready in ..." or the phase that failed. Retry, Rebuild container, Open in browser and Delete codespace are available from the startup.
- Messages sent while the codespace starts are queued and sent when it is ready. If the start fails, they are marked as not sent.
- Codespace starts in progress are picked up again when the app relaunches.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
