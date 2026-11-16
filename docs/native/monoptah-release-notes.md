# Monoptah Native 0.53.2

## Changes since 0.53.1

### New

- Cloud workspaces now opens in a drawer that slides in from the right, from the cloud chip at the far right of the footer. Esc, a click outside it or Close slides it out. The chip shows while the project has any workspace (running, stopping, stopped, failed or deleting) and hides when there is none. "Cloud workspaces..." is no longer in the project menu, and the Telegram menu item is hidden.
- Claude sign-in has a new "Sign in with code" choice (in the Claude sign-in options menu, the add-account editor in the footer and the footer sign-in panel). The app opens the sign-in page, which shows a code, and you paste that code into the app. Available on macOS and Linux for now.
- Workflow cloud steps: the text options of a `target` (branch, machine, memory, ttl, template, image) can use templates, checked again before the step starts, so a bad value fails the step and creates nothing. A step that creates a new instance can name its `account`, which must have a setup token or API key. A run that was interrupted keeps the list of cloud instances it created: the run page, the workflow page and removing a workflow name them, and they are kept out of history trimming. Nothing is stopped or deleted automatically.

### Fixes

- Claude browser sign-in now shows the sign-in link and accepts a pasted verification code when the browser shows a code instead of returning to the app. A rejected or expired code says so clearly. Coming back to the app to paste the code no longer resets the sign-in.
- Clicking a Mono or switching tabs within the same project no longer makes the project rail jump to the project's row.
- Faster native development and test builds; `worktrees:clean` never removes the release worktree (developers only).

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
