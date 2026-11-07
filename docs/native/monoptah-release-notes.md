# Monoptah Native 0.49.0

## Changes since 0.48.1

- Named Claude accounts can now authenticate with a token from `claude setup-token` or with an Anthropic API key instead of browser sign-in. Open the chevron next to "Sign in and add" in Settings → Providers, choose "Use setup-token" or "Use API key", and paste the value. Browser sign-in stays the default.
- The token or key is stored encrypted in the app database, so upgrades do not ask for the macOS keychain password, and it is passed only to that account's Claude Code process. Editing an account lets you replace it, switch back to browser sign-in, or keep the current sign-in and only rename.
- Token and API key accounts show "Token configured" or "API key configured" instead of usage limits, which the provider does not report for them.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
