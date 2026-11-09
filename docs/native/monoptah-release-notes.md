# Monoptah Native 0.50.4

## Changes since 0.50.3

- You can now choose the default Claude or Codex account for new sessions. In Settings the DEFAULT badge follows the chosen account and every other account has a button to make it the default. A project with its own account choice keeps it; removing the default account falls back to the CLI login. New Mono chats also start on the default account instead of always on the CLI login.
- Settings now lists each Agent CLI's models for the account new sessions use, so Claude no longer shows "0 models" with "Use by default" disabled when no session is open.
- Choosing the CLI login for a project now survives a desktop reload, and removing an account a project had chosen no longer pins that project to the CLI login after a restart.
- Claude accounts added with `claude setup-token` can now show their 5-hour and 7-day quota. Their tokens cannot read the usage API, so clicking refresh on the usage chip sends one tiny request and reads the quota from its response; this only happens on that click, never in the background. The result is shown as "Checked Xm ago" and is forgotten when the token is changed or removed. API key accounts still show no quota, now with a message that says why.
- In the Classic layout, the usage quota row (with refresh and terminal controls) now stays under the composer while a Mono chat is open.

## Downloads and updates

- macOS Apple silicon: app archive and ZIP; Homebrew cask `hhoangg/monoptah/monoptah-native`.
- Linux x86_64: AppImage, DEB and RPM (built on AlmaLinux 10).
- Windows x86_64: NSIS installer.
- Every updater package and the shared native update feed are signed. Native releases use their own feed and do not replace the Tauri update feed.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
