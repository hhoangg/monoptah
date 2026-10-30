# Monoptah Native 0.36.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.35.0

- Mono workflows have a new Switch node. It routes a run to one of several named cases by comparing a value such as `{{triage.kind}}` with each case, and takes the `default` branch when none match. A Switch can also let the Mono choose the case, with an optional question.
- On the workflow board a Switch card has one output port per case plus `default`. The inspector edits the value, the question, who decides, and the list of cases; renaming or removing a case keeps its connected edge in step.
- Workflow validation checks Switch nodes: cases must be non-blank and unique, `default` is reserved, and every out-edge needs a case or `default` handle, with at most one edge per handle. `workflows.guide` documents the node.
- Existing workflows with If nodes load and save unchanged.
- Contributors: Refresh and Sync on the GitHub tab now fetch the default branch from `origin` and read `refs/remotes/origin/<branch>`, so commits that landed on the remote show up without a manual `git pull`. With no branch selected the page follows the default branch of `origin`, and unpushed commits are not counted. Fetching never changes the working tree or local branches. If `origin` cannot be reached, the stored data stays and the page says how old it is. Repositories without `origin` keep reading the local branch.

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
