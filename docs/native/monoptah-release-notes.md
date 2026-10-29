# Monoptah Native 0.34.2

A release of the experimental Rust + GPUI app for macOS on Apple silicon, merged with the upstream native 0.16.0-preview.9 work. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.34.1

- Archiving a session now archives the sessions it started too, so they no longer break out into separate top-level rows. Restoring the parent brings back the children that were archived with it, and a session you archived on its own stays archived. Archiving a child alone still archives only that child. This applies to the context menu, multi-select, the keyboard shortcut and Monos. The app `sessions.archive` action follows the same rules; it is refused when the calling session was started by the session being archived.
- Faster startup and fewer full-window redraws during animations, with GPU memory released when the app goes quiet.
- Mermaid diagrams render natively in Markdown and resize with the panel.
- Cmd/Ctrl+Enter sends a mid-turn message the opposite way from your usual follow-up preference (steer or queue).
- Composer file mentions show file icons and insert short names. Terminal links open on click.
- The workspace picker includes All checkouts, while selecting a project folder keeps that folder as the session scope.
- Pasted images are stored per conversation and removed with that conversation; shared legacy attachments are kept.
- Plain-link prompts wrap inside their bubbles, and the Edit GitHub link dialog keeps its Update button inside the dialog.
- Disabling mobile access or shutting down cancels pending mobile startup.

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
