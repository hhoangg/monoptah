# Monoptah Native 0.38.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.37.0

- Monos: mention a workflow in a Mono chat with `@workflow/<slug>`. The turn gets its state, inputs, requirements and last run, so the Mono can discuss, edit or run it. A mention never starts a run by itself.
- Workflows: Run opens a small form prefilled with the current values when a workflow has inputs. Values set there apply to that run only and are not saved. The `workflows.run` action accepts the same `values`.
- Workflows can declare what they need with `requires`: a command on the PATH, an environment variable, or a configured MCP server, each with an optional hint. Add them on the board, or in YAML. Only names are checked, nothing from the file is run, and a variable's value is never read into any output.
- Workflows: a run does not start while a requirement is missing. Run shows a checklist with Re-check, the Mono gets a list of what is missing, and a scheduled run ends as failed with that list in the Mono's chat, then the schedule moves on. The workflow list and page show what is missing, and the review of an imported workflow shows what it needs.
- Workflows: `workflows.validate` warns about commands in command steps that `requires` does not declare.

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
