# Monoptah Native 0.39.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.38.1

- Background model: a new setting in Settings > Providers > Background tasks chooses the model that writes session titles, worktree branch names, commit messages and pull request text, on the desktop and for a paired phone's Git actions. Automatic keeps using each session's provider and its quick model.
- Editor: Zig files get syntax highlighting and fold their struct, enum, union, error-set and switch blocks. ZON files are not supported.
- Paired phones can answer an agent's pending questions, page back through the history of long conversations, and reopen the last project or Mono on launch.
- Paired phones can browse Monos with search, filters and unread replies, see reply bubbles, reactions, suggested replies, document cards and the sessions a Mono launched, create Monos, and edit a Mono's name, mascot, color, model, projects, soul and memory. Saving a soul or memory file is refused if it changed on the desktop since it was opened.
- Paired phones can reset or delete a Mono. They do what the desktop does: a reset stops the reply, deletes the old conversation and starts a fresh one. A phone has no checkbox for it, so a reset also deletes the provider's session files on this computer whenever that provider supports it, as the desktop's dialog does by default. Those files can't be recovered. A delete also stops the Mono's workflows.
- Monoptah does not build or publish a phone app. These changes are what the desktop does for a phone that is paired with it.

### Privacy: phone notifications go through Expo

Paired phones get push notifications while the desktop is running and the phone app is closed. They are sent from your computer through Expo's push service (`exp.host`), a third party. Each notification carries the content needed to show it: the session title, the project name, and an excerpt of the reply, or for a Mono its name and an excerpt of its message.

Nothing is sent unless a phone has registered for notifications. The desktop has no setting that turns push notifications off on their own: the kinds of notification are chosen on the phone. Removing the phone on the desktop stops them, and so does turning off phone access.

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
