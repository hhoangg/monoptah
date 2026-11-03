# Monoptah Native 0.41.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.40.3

Today now works like a small task board.

- **Workflow statuses.** Every task has a status: To do, In progress and Done by default. The new Statuses dialog (header button, or Edit statuses in any status menu) adds, renames, recolors and reorders statuses, and moves your own statuses between the Not started, Started and Finished stages. Built-in statuses can be renamed and recolored but not deleted. Deleting a status moves its tasks to the built-in status of the same stage. Existing Today tasks are migrated in place.
- **List and Board.** A List / Board switch in the header, remembered between launches. The list groups tasks by status. The board has one column per status: drag a card to another column to change its status, add a task to a column, or add a status at the end. Filters, the date strip and read-only past days work in both. Past days keep the status names and colors they had that day.
- **Detail panel.** A new layout with a breadcrumb, a status pill with a menu, priority and due, a Needs check callout with direct actions (Keep done, Move back to To do, Mark done, Dismiss), Details, a Context section with the source's summary, and one Activity timeline of status changes, sync findings, your actions and notes.
- **Start session** no longer sends a prompt. It opens a new chat in the task's project (asking for a project first when the task has none) with an `@today/<task>` mention in the composer. On send, the mention adds the task's context; delete the mention to leave it out.
- **Monos and syncing.** A sync maps a source's own status (open, in progress, in review, merged, done and similar) to a stage, and a Mono can state the stage and a short summary per item. A status you change by hand is never overwritten by a sync. Monos can read the statuses with `today.statuses`, and their instructions now explain how a habit should fill each field.

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
