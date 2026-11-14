# Monoptah Native 0.52.0

## Changes since 0.51.1

### New

- Picture emoji for Monos. A Mono's Settings has a new Emoji page: import emoji packs from `.zip` files (button or drop), let the Mono's own model sort them into emotions (or import without sorting), and review the result before anything is saved. Emotions can be renamed, described, added and removed, and emoji dragged between them. With "Use emoji in replies" on (off by default) and a "How often" setting (Rarely, Sometimes, Often), the Mono can add a fitting picture to a reply; GIFs animate. The Mono is told never to use them in code, commit messages, PR text or reports, and copying, notes, quotes, Telegram and notifications leave them out. Zip imports are checked for unsafe paths, size and image format; SVG is not accepted.
- Cloud workspaces overview, opened with "Cloud workspaces…" in the project menu. It lists every codespace of the project's repository and every Monoptah Docker sandbox, with state, branch, size, age, the sessions that use it (or "No Monoptah session uses it") and an estimated cost, and lets you Stop or Delete them. Listing never wakes a stopped workspace. Stop asks first when the workspace is busy or not created by this app, a delete names the sessions it removes, and a sandbox in an unknown state is never deleted.
- Cost control in Settings > Cloud: stop (never delete) a cloud workspace that nothing needs. A Docker sandbox stops after 10 minutes idle by default (Off, 5, 10, 15 or 30 min) and when its last session is archived or deleted; for codespaces both are off by default, since GitHub idles them itself. A notice says what was stopped, and sending a message resumes it.
- A Docker Sandbox can start from a saved Docker cloud template or a public image instead of the default sandbox plus a dev container build, which is much faster. Choose it in the Environment row of the project's Cloud settings; the size then comes from the template, and building the dev container is an opt-in.
- The Codespace panel in the new-session composer shows this month's Codespaces usage (compute hours, free usage used, amount billed) with a refresh button and a link to GitHub billing. It needs the `user` scope on `gh`; without it the line shows the command that adds it.
- Docker Sandbox surfaces (Cloud settings and the sandbox session panel) have a "View usage & credits" link to the Docker usage page.

### Improvements

- A new codespace defaults to the smallest machine the repository allows on the session's branch, instead of always `basicLinux32gb`, so repositories with `hostRequirements` no longer fail on Send. The Machine type list shows only allowed machines. This also applies to the Mono app API.
- A new Docker sandbox gets a size `sbx` accepts and one that meets the dev container's `hostRequirements`. Sizes `sbx` would refuse can no longer reach Send, and an `sbx` failure later in a start now explains how to fix it (install, `sbx login` or ssh setup).
- The cloud session panel is simpler: the "default / changed for this session" column, the highlight and the "N changed" badge are gone, and target descriptions no longer end in "· default".

### Fixes

- Picking a Model, Effort or Permissions value in a Mono's Settings no longer closes the dropdown or the whole dialog without applying it, also while the Mono is replying. The change applies to the next turn.
- A menu or list opened from inside a dialog or another menu now draws above it, so clicking a row that hangs past the dialog edge picks it instead of closing the dialog.
- Two-line items in the cloud target menu (Codespace, Docker Sandbox, Existing instance) no longer overlap.
- Docker sandbox states read from `sbx ls` are recognised again instead of showing as unknown.

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
