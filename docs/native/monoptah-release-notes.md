# Monoptah Native 0.40.0

A release of the experimental Rust + GPUI app for macOS on Apple silicon. The React/Tauri app, Monoptah, remains the production app.

## Changes since 0.39.0

- Today: a new page above Inbox that keeps a daily ledger of your work across all projects. It shows what is new, what carried over and what needs a check, keeps past days as they were, and rolls over to a new work day at 04:00 (Settings > General).
- Ask a Mono to set up a source, for example your GitHub review requests or your ClickUp tasks. It fetches once and shows you the list, then saves a habit that keeps Today up to date. Monoptah has no fetching of its own for this: the Mono uses gh, MCP or anything else, and rules live in the habit.
- A failed sync changes nothing and shows a warning, so it never looks like an empty list. A task leaves only when its source confirms it or you act on it.
- Tasks have a detail panel with Open link, Start session, Snooze and Dismiss, and a timeline of notes that stay on this machine.
- Habits do not catch up when the app was closed more than 2 hours past their time. Today then shows Not synced today, and Sync now runs them.
- The Homebrew cask is back up to date with this release.
