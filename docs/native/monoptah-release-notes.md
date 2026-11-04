# Monoptah Native 0.42.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.41.1

This release brings in upstream MonoCode native preview 13. Today, Contributors, Mono workflows and the other Monoptah features work as before.

- **Tabs and terminals.** Pin title tabs from their context menu to keep them at the left of the tab strip. Up to three sit in a row, and more stack into a deck that fans out on hover. Drag a terminal out of the dock onto a pane edge to split it beside your chat, or onto the title strip to give it its own tab. The shell keeps running. Settings can place terminals in the title bar, and its New tab button then offers a session or a terminal. Inputs, the file editor and terminals have right-click editing menus.
- **Changes pill.** A pill above the composer, beside Commit, shows the session's added and removed lines and opens its review card with Keep, Undo and file review. Unresolved changes carry forward across turns until you keep, undo or commit them.
- **Tool groups.** Settings > Chat > Collapse tool groups keeps groups closed while they run. Fold finished work chooses whether finished work folds under the "worked for" header. The composer button turns into a labelled Send, Queue, Steer, Save draft or Stop pill.
- **Settings and search.** Settings has a collapsible navigation sidebar with search at the top. Long dropdowns have a filter field. Settings > General > Search as command panel opens search as a floating panel over your work. Model picker rows have a favorite star.
- **New chats.** New chats pick their provider and model the same way everywhere: project default, app default, then your last choice. When the default provider opens as a terminal, you are still asked which provider to use first.
- **Monos.** A Mono's settings can choose a separate model and effort for the sessions it starts. New Mono chats start at low effort. Claude and Codex get a Mono's standing rules as system instructions, and a resumed Codex thread gets updated rules in the message. Monos can reach the app through an MCP app tool, with the app CLI as fallback.
- **Reliability.** Smoother mouse-wheel scrolling, the reading position kept when rows fold away or a transcript reopens, windows that recover after a stalled display link, and provider starts that retry a brief "Text file busy" error. Dismissing an update notice no longer opens What's new.
- **Star on GitHub.** A dismissible sidebar prompt offers to star this fork for GitHub CLI accounts that have not starred it yet. It stays hidden when GitHub CLI is missing, signed out or offline.

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
