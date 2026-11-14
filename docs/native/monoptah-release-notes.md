# Monoptah Native 0.53.0

## Changes since 0.52.0

### New

- A Mono can use its own account. Its Settings has an Account row for the sign-in it thinks and replies with, and the defaults for sessions it starts have an Account row too, next to the model. Sessions a Mono starts take the harness, model, effort and account from those defaults unless the Mono names them, and a start that would use a removed account is refused with a clear message; a harness of another provider uses that provider's project or default account. The report after a session stops at a usage limit names the account.
- A cloud status chip in the footer, next to the usage chip, shows "N running" while a cloud workspace of the window's project runs or starts, and takes no room when nothing does. It turns to the warning colour for a workspace no session uses, a failed one, or one up for 6 hours with no busy session. Click it to open Cloud workspaces. It only reads state (nothing is woken or started), at most about once a minute while the window is active, and it follows the window's project on every tab and app view.
- Archiving or deleting a cloud session now acts on its cloud instance. One confirm dialog offers Stop instance (default) or Delete instance; Delete is refused while another live session shares the instance. Archive has a "Back up this session to local" option (on by default), and the instance is touched only after the backup succeeds. An instance that no longer exists counts as stopped or deleted.

### Fixes

- The first prompt of a cloud session started from the new-session composer is sent once the workspace is ready. Before, it was stored but never shown or sent, and the session stayed empty as "New session".
- Deleting a session is no longer blocked by a file pane you cannot see. Only a visible file tab or a pane with unsaved edits blocks it; the message names the file, and a delete started in the app brings that pane into view.

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
