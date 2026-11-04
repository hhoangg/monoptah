# Monoptah Native 0.41.1

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.41.0

Today tasks can now carry tags, and the filters can pick several values at once.

- **Tags.** One tag list for the whole app, managed from the new Tags button next to Statuses, or Edit tags in a task's Tags menu. Add, rename, recolor (a few preset swatches plus a custom color) and drag to reorder. Names are unique ignoring case. Deleting a tag first says how many tasks use it, then removes it from all of them. A rename or a new color shows everywhere at once.
- **Tags on tasks.** List rows and board cards show a task's tags as colored chips after the project. The detail panel has a Tags row with a searchable menu: check tags on or off, create a tag from what you typed, or open Edit tags. Tag changes appear in the Activity timeline. Past days are read only and show tags with their current names and colors.
- **Existing tags migrated.** Free text tags already on Today tasks become tag definitions with default colors, merged when they differ only in case, and every task keeps its tags.
- **Multi-select filters.** Status and Tag filters let you check several options. The Status menu is grouped by stage, and on the Board a status filter hides the columns you did not pick. The Tag filter matches any of the chosen tags by default, can switch to all of them, and has a No tag option. Filters combine, the row shows how many are active with Clear filters, and the filters are remembered between launches.
- **Monos.** `today.add` and `today.update` take tag names and refuse names that are not in your tag list, so agents cannot create tags. `today.tags` lists the tags. A sync adds matching tags and never removes tags you set.

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
