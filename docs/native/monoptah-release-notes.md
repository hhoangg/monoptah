# Monoptah Native 0.50.3

## Changes since 0.50.2

- macOS no longer asks for your login password after every upgrade. The desktop mobile identity used to live in the Keychain, and because the app is ad-hoc signed each build looked like a new app to it. The identity is now kept in the app's encrypted store. On first launch the existing Keychain entry is moved over unchanged, so paired phones stay paired, and then removed.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
