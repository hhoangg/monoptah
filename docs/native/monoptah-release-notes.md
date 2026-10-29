# Monoptah Native 0.34.3

A release of the experimental Rust + GPUI app for macOS on Apple silicon, merged with the upstream native 0.16.0-preview.9 work. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.34.2

- Settings > Worktrees can delete several worktrees at once: tick the rows (or select all), then choose Delete selected. The confirmation lists which worktrees have uncommitted changes or an unknown status. They are deleted one by one, a failure does not stop the rest, and the ones that failed stay selected with the reason shown. Settings stays open while a delete is running.
- A folder's color now tints its icon, its chevron and the line joining it to its sessions, so the color is visible while the folder is open.
- Escape now does one thing: it closes the topmost dialog or overlay, and only when nothing is open does it stop the turn of the conversation you are looking at. It no longer stops the sidebar-selected session from a Mono chat, from Settings or from a dialog, and it targets the focused pane in a split.
- Terminal: New in Home (Cmd+Alt+J, Ctrl+Alt+J elsewhere) opens a terminal at your home directory. It is also in the app menu, the command palette and the Keybindings page. Cmd+` still opens one in the project folder.
- Cmd+` and Cmd+J now work in a Mono chat. The terminal dock is shown there and opens in the project the Mono works on, instead of silently creating hidden terminals in the last session's folder. Hidden terminals saved by earlier versions may appear once in that project's dock; close them there.

## What it is

Monoptah Native is a native rewrite of the Monoptah desktop app. It works with the same agent CLIs as Monoptah and covers the main workflows:

- Chat with the supported providers (Claude Code, Codex, Cursor, Grok Build, OpenCode, Pi, omp, fx, Hermes Agent, Antigravity and Devin), with streaming, resume, steering and cancellation.
- Sessions, projects, split panes, terminals, a file editor and Git actions in one window.
- Markdown, Mermaid diagrams and inline charts in conversations.
- Inbox, notes, Monos, automations, remote connections, and macOS notifications and Dock badges.
- In-app updates from Settings > General > About.

Feature and platform parity with Monoptah is still being tested. See `docs/native/parity.md` at this release's tag for the known limitations.

## Installing next to Monoptah

Install with Homebrew:

```sh
brew install --cask hhoangg/monoptah/monoptah-native
```

Monoptah Native has its own name and bundle identifier (`com.monoptah.native`), so it installs beside the Monoptah (Tauri) app instead of replacing it.

**Ad-hoc signed.** This fork has no Apple Developer account, so the app is signed ad-hoc and not notarized. The first launch is blocked by macOS: open System Settings > Privacy & Security, scroll to Security, and choose Open Anyway next to Monoptah Native. Later updates installed from inside the app launch normally.

Because builds are ad-hoc signed, macOS may ask again for Keychain access and privacy permissions (for example Files and Folders) after an update.

## Your data

- Monoptah Native uses your existing Monoptah database (`com.monoptah.desktop/monocode.db`) and saves changes there. Back up your Monoptah data with both apps closed before trying it.
- Settings you change in Monoptah Native are stored separately from the Tauri app's settings. They are not copied back.
- Run only one of the two apps at a time while Telegram forwarding, Contributors sync or scoring, or scheduled Mono workflows are on. With both open they would work on the same bots, tickets and workflows and undo or duplicate each other's work. Avoid editing the same conversation in both apps at once. See `docs/native/parity.md` for details.

## Updates

Monoptah Native updates itself from this fork's native releases and does not use the Tauri app's update feed. The Homebrew cask is marked as self-updating, so `brew upgrade` is not needed after the first install.

## Reporting problems

Please include the version, your macOS version, the provider and the steps to reproduce. Remove credentials and private conversation content from logs and screenshots.
