# Monoptah Native 0.35.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon, merged with the upstream native 0.16.0-preview.10 work. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.34.3

- Merged upstream native 0.16.0-preview.10. A new Overview tab in a Mono shows its current activity, a contribution calendar, merged pull requests, lines changed and upcoming habits. The session list is now called Sub-agents, with cards showing model, provider, checkout and state.
- Heartbeats: an idle Mono reviews recent work, sessions left unattended, notes and inbox items in its projects and suggests up to two follow-ups. On by default after 30 minutes of quiet; the Mono's settings offer an off switch and 15-minute, 30-minute, 1-hour and 2-hour intervals. Quiet checks leave no chat report, and a heartbeat that started sessions keeps its history.
- /visualize and native visual documents: visuals published in a Mono chat also appear in Documents. New HTML artifacts are no longer accepted; use a Markdown document or visual_show.
- Monos open every reply with a short sentence and hand longer work to background sessions. Launched sessions stay in the background unless split into the workspace explicitly.
- New opt-in setting in Settings > Providers: Identify as Monoptah to Claude (experimental).
- Hermes Agent receives the reasoning effort selected in the model picker, slash commands have distinct icons, and a transcript crash while messages merge during animation is fixed.

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
