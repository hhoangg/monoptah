# Monoptah has moved to zptah

> [!IMPORTANT]
> **Monoptah is retired. This repository is archived and read-only: it gets no more fixes or releases.**
> The app now lives in its own repository as **zptah**: <https://github.com/hhoangg/zptah>

## Why it moved

- Monoptah had diverged far from its upstream, [`hardbeat920/monocode`](https://github.com/hardbeat920/monocode).
- zptah has an independent identity: its own name, bundle id (`app.zptah.desktop`) and data directory.
- The archived Tauri app and the unmaintained mobile app were removed.
- zptah is an independent repository with its own issues and releases, instead of living in a fork network, which makes it easier to contribute.

## Install zptah

macOS (Apple Silicon):

```sh
brew install --cask hhoangg/zptah/zptah
```

Linux (AppImage, deb, rpm) and Windows (exe): download from the
[latest zptah release](https://github.com/hhoangg/zptah/releases/latest).

## Migrate to zptah

1. Install zptah (see above).
2. Quit Monoptah.
3. Open zptah, then go to **Settings > Import from Monoptah**. It copies your database, chats, attachments, accounts, Monos and memory, and settings. Your Monoptah data is left untouched.
4. Re-pair the web app at <https://zptah.app> (see "Not migrated" below).
5. Grant Full Disk Access to zptah once (see "Not migrated" below).
6. Remote SSH machines: zptah installs its own `zptah-host` next to the existing `monocode-host`. Nothing to do.

### Not migrated

- **Web and phone pairing**: this is not imported by design. Pair the web app again at <https://zptah.app>.
- **Full Disk Access**: macOS permission grants belong to the app, so grant Full Disk Access to zptah once in System Settings > Privacy & Security.

## FAQ

**Does Monoptah keep working?**
Yes. The installed app keeps running, but it is no longer maintained and gets no updates.

**Is my Monoptah data safe?**
Yes. The import only reads it. Your Monoptah data stays exactly as it was.

**How do I remove Monoptah?**
Once you are happy with zptah, run `brew uninstall --cask monoptah-native`. Do not pass `--zap`, so your Monoptah data is kept.

**Where do I report bugs or contribute?**
In the [zptah repository](https://github.com/hhoangg/zptah/issues).

---

# Archived README

Everything below is the README from before the move and is kept for history only.

<h1 align="center">Monocode</h1>

<p align="center">
  <strong>A GUI for your coding agents.</strong>
</p>

<p align="center">
  <img width="1680" height="1050" alt="MonoCode with agent sessions open side by side" src="https://github.com/user-attachments/assets/89e04066-69de-473d-9aab-88b9ca3e3b7b" />
</p>

> [!NOTE]
> **Try the new [Monocode Native preview](https://github.com/hardbeat920/monocode/releases)**, our experimental Rust + GPUI app for macOS, Linux and Windows. Read the [preview details](#monocode-native-preview) before installing.

Works with your subscriptions on Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent, and Devin. If they’re installed and logged in, MonoCode can run them. Tabs are sessions. The composer is the input. MonoCode does not sell tokens.

## Install

Install and sign in to at least one supported agent first. See [provider setup](docs/providers.md) for instructions.

| Platform | Native | Tauri |
| --- | --- | --- |
| macOS, Apple Silicon | [MonocodeNative_aarch64.dmg](https://github.com/hardbeat920/monocode/releases/download/native-v0.16.0-preview.14/MonocodeNative_0.16.0-preview.14_aarch64.dmg) | [MonoCode.dmg](https://dl.usemono.dev/MonoCode.dmg) |
| macOS, Intel | [MonocodeNative_x64.dmg](https://github.com/hardbeat920/monocode/releases/download/native-v0.16.0-preview.14/MonocodeNative_0.16.0-preview.14_x64.dmg) | [MonoCode_x64.dmg](https://dl.usemono.dev/MonoCode_x64.dmg) |
| Linux, x86_64 | [.deb, .rpm, or AppImage](https://github.com/hardbeat920/monocode/releases/tag/native-v0.16.0-preview.14) | [.deb, .rpm, or AppImage](https://github.com/hardbeat920/monocode/releases/latest) |
| Windows, x86_64 | [Installer](https://github.com/hardbeat920/monocode/releases/download/native-v0.16.0-preview.14/MonocodeNative_0.16.0-preview.14_x64-setup.exe) | [Installer](https://github.com/hardbeat920/monocode/releases/latest) |

On macOS, open the DMG and drag MonoCode to Applications. On Windows, run the installer. See [Linux setup](docs/install.md) for dependencies and package instructions.

## Get started

Open a project folder, choose an agent, and send a message. Each tab is a session. Split panes to work with sessions side by side.

- Keep conversations organized by project.
- Use separate Git worktrees for parallel tasks.
- Edit files and review changes in the app.
- Start a message with `/operator` to let an agent open sessions and manage worktrees and notes.

Monos are experimental agents that work across your projects. They keep a memory and can run scheduled tasks you set up with them.

MonoCode is still early. If something breaks, [report a bug](https://github.com/hardbeat920/monocode/issues).

## Guides

- [Provider setup](docs/providers.md)
- [Linux installation](docs/install.md)
- [Agent access with /operator](docs/agent-access.md)
- [Remote sessions, experimental](docs/remote-access.md)
- [Answer questions from Telegram](docs/telegram.md)

## Build from source

You’ll need Node.js 20+ and a stable Rust toolchain. See [build setup](docs/building.md) for platform dependencies and packaging.

The mobile app is a separate Expo project in [`mobile/`](mobile/README.md). Run it with `cd mobile && npm install && npm start`.

```bash
npm install
npm run tauri dev
```

Small, focused contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Monocode Native preview

The Rust + GPUI app is an experimental preview. React/Tauri remains the
production app.

Download a packaged preview for macOS, Linux or Windows from the
[releases page](https://github.com/hardbeat920/monocode/releases). See
[known limitations](docs/native/parity.md) before testing.

You can install it alongside MonoCode, but **the preview uses your existing
MonoCode database and saves changes there**. Back up your data with the app
closed before testing, and avoid editing the same conversations in both apps
at once.

To try GPUI from this checkout with Node.js 22+, Rust and Xcode on macOS:

```sh
npm run native:dev     # launch with a separate development database
npm run native:build   # create an optimized app using installed MonoCode data
```

Type `r` then Enter in the development terminal to rebuild and restart; there is
no hot reload. The release app saves changes to your installed MonoCode data.
See the [native guide](docs/native/README.md) for prerequisites, build output,
checks and preview limitations.

## Contributors

Thanks to everyone who contributes to MonoCode!

[![MonoCode contributors](https://contrib.rocks/image?repo=hardbeat920/monocode)](https://github.com/hardbeat920/monocode/graphs/contributors)

Contributor image by [contrib.rocks](https://contrib.rocks).

## License

[MIT](LICENSE). Provider names and logos are trademarks of their owners. See [NOTICE](NOTICE).
