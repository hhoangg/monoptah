# Monoptah Native 0.44.0

## Changes since 0.42.0

- Attach images and files to Today notes with the picker, clipboard, or drag and drop. Attachment-only notes are supported. Up to 10 files of 25 MB each are copied into app storage; task mentions include stored file paths and eligible note images in chat context.
- Link sessions to Today tasks, see their activity in task details, and continue work in an existing session with an unsent task mention that preserves its draft. Sessions previously started from task mentions are linked automatically.
- Keep Mono chats usable after failed completion reports or a missing provider account. Long report errors stay within a bounded area so the composer remains visible, and workflow suggestions are restored in the updated composer.
- Make bare URLs in agent replies and every web URL in user messages clickable, while preserving GitHub issue and pull request chips.
- Find saved conversation text faster with an incremental transcript search index.
- Share Mono habits as YAML files, and use selectable, linked text in Today notes with the updated note composer.
- Include upstream changes through 11769e9e: Claude authentication and queued-turn fixes, streaming performance improvements, and composer, settings, and sidebar refinements.
- Add Linux x86_64 AppImage and Debian packages and a Windows x64 installer alongside the signed macOS updater and Homebrew cask.

## Known limits

- The separate window repaint stall remains unresolved. This release adds bounded diagnostics to capture its cause; it does not establish a rendering recovery fix.
- macOS packages are ad-hoc signed and are not Apple-notarized. Linux and Windows desktop interaction still need broader runtime testing.
