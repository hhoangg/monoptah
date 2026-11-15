# Monoptah Native 0.53.1

## Changes since 0.53.0

### New

- A Mono can open labeled terminals of its project and work with them. It places them as dock tabs, title tabs of their own or split panes (never splitting your chat tab), arranges them into one "Logs <project>" split tab in columns, rows or a grid, runs commands and presses keys in them, reads their output, stops them and closes them. A label replaces the automatic title and a small "Mono" mark shows on the tab and pane header. The Mono never takes the keyboard and, by default, does not switch tabs or projects. Input goes only to terminals the Mono opened whose processes stay inside the project or one of its git worktrees; commands run with your rights.
- Agent sessions can read the terminals of their project or worktree (list and read only), and a Mono can read every plain terminal of its project. Output is limited in size and secrets are masked. Agent sessions, terminals with secret environment variables, cloud terminals and any terminal that ever ran an agent CLI are never listed or read. On Windows only the Mono's own terminals are readable. Mono terminals are not saved and nothing is run again after a restart.
- Workflow session steps can run in a GitHub codespace or Docker sandbox through a `target`, with a new instance or the instance of an earlier step (`sameAs`). Loop rounds, resumed steps and For each items follow clear reuse rules, the board card, inspector and review show where a step runs, and the run report lists every instance used. Cloud steps use Claude, and their instances are never stopped or deleted automatically. Stopping a workflow, a timeout or a failed step drops the queued first prompt of a step that is still starting.

### Fixes

- Stopping a Docker sandbox in Cloud workspaces now sticks. Before, the app's own connection could wake the sandbox again a few minutes later and the row went back to "Running" with no message. A Stop now closes the app's connection first (sessions stay disconnected until you press Reconnect), then checks the real state and says Stopped, Still running with the reason, Taking longer than expected, or the error. The footer chip shows "N stopping" while a Stop is in progress and warns if it gets stuck or is undone. Codespaces are unchanged.

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
