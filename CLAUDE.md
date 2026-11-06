# Monoptah

This repository is a fork of [`hardbeat920/monocode`](https://github.com/hardbeat920/monocode),
renamed **Monoptah**. It tracks upstream and merges from it regularly, while carrying
fork-local changes that are never offered back upstream.

## After every merge from upstream

Upstream keeps adding links, menu items, dialog copy and docs that say "MonoCode" and
point at `hardbeat920/monocode`. Every merge can therefore quietly reintroduce the
upstream identity into a build called Monoptah. **Re-check after each merge** — do not
rely on remembering which files were fixed last time, because the next merge will touch
different ones.

Run this and resolve every hit before the merge is considered done:

```bash
./scripts/check-fork-branding.sh
```

It exits non-zero while anything remains, so it can gate a merge. It looks only at
string literals and links, so an identifier like `MonoCodeMark` does not trip it, and
it carries an ALLOW list of deliberate keeps with the reason for each. When you decide
a new hit should stay, add it to that list **with its reason** — never loosen the
pattern, because a checker that passes by looking away is worse than no checker.

For each hit, decide:

- **A link to upstream** → point it at this fork (`hhoangg/monoptah`), except the
  Website menu item, which goes to `https://zptah.app`.
- **Text a user can read** (dialog titles, tray tooltip, window title, button labels,
  "What's new in …", error copy) → say Monoptah.
- **A test asserting old copy** → update the assertion. A test that still passes only
  because it asserts "MonoCode" is a failure, not coverage.

Also confirm `src-tauri/tauri.conf.json` still has `productName: "Monoptah"`,
`identifier: "com.monoptah.desktop"`, and the updater `pubkey` and fork `endpoints` —
an upstream change to that file is the one most likely to hand the app back to MonoCode.

## Never rename these

They are identifiers, not branding. Changing them breaks working installs:

| Identifier | Why it stays |
|---|---|
| `monocode.*` localStorage key prefix | Renaming loses every saved preference on existing installs |
| `monocode-host` CLI name, its data dir, `com.monocode.host` launchd label | The remote host protocol depends on them |
| npm package `monocode-desktop`, crate `monocode`, lib `monocode_lib` | `scripts/bump-version.mjs` matches on them, and release artifacts are named from them |
| `INTERRUPT_MESSAGE` in `src/features/sessions/model/inFlight.ts` | Compared with `===` against text already stored in saved sessions; changing it makes old interrupt notices unrecognised and duplicated |

## Verifying

`npm run check` runs the full suite: vitest, `tsc --noEmit`, `cargo fmt --check`,
`cargo clippy -D warnings`, `cargo test`.

Read its exit status directly — never through a pipe. `npm run check | tail` reports
`tail`'s status, which is always 0, so a failing suite looks green. Use
`npm run check > log 2>&1; echo $?` or `${PIPESTATUS[0]}`.

## Seeing a change live

After changing code, run the dev build from source:

```bash
npm run tauri:dev
```

It uses `src-tauri/tauri.dev.conf.json` to give the app the identifier
`com.monoptah.desktop.dev` and the name "Monoptah Dev", so it keeps its own data
(`~/Library/Application Support/com.monoptah.desktop.dev` and
`~/Library/WebKit/com.monoptah.desktop.dev`). The installed Monoptah, with its
sessions, settings and ClickUp token, is not touched. Never run plain
`npm run tauri dev`: it uses the production identifier and shares that data.

On macOS the dev bundle reads the same overlay through `TAURI_CONFIG` in
`src-tauri/src/macos.rs`. Without that, the bundle would keep the production
identifier and share WebKit storage (localStorage) with the installed app.

## Releasing

Releases are native only for now, see "Releasing the native app". The Tauri workflows
`.github/workflows/ci.yml` and `release.yml` are paused: they have only `workflow_dispatch`,
so pushes, pull requests and `v*` tags start nothing. Keep that when merging from upstream.

To cut a Tauri release anyway: `npm run set-version -- X.Y.Z`, add a `CHANGELOG.md` section,
commit as `Release vX.Y.Z`, push the tag, then run `gh workflow run release.yml --ref vX.Y.Z`.
That workflow builds macOS, Linux and Windows, signs the updater bundles with the
`TAURI_SIGNING_PRIVATE_KEY` secret, and publishes a GitHub Release with `latest.json`.

The same run also bumps the Homebrew cask in `hhoangg/homebrew-monoptah` (the `homebrew`
job) when the `HOMEBREW_TAP_TOKEN` secret is set. Pre-release tags (containing `-`) are
skipped, and the script never lowers the cask version, so re-running an old tag is safe.
If the secret is missing the job only prints a notice, so bump `Casks/monoptah.rb` by hand
with `scripts/update-homebrew-cask.sh <cask-file> <version> <sha256>`, taking the sha from
the release's `Monoptah_<version>_aarch64.zip.sha256` asset. The token is a fine-grained PAT
and expires: if the job fails at the tap checkout, the likely cause is an expired
`HOMEBREW_TAP_TOKEN`, so create a new one and update the secret.

The updater private key lives at `~/.config/monoptah/updater.key` and is **not** in this
repo. Losing it means no further updates can be published to installed apps — they only
trust that one key, so a replacement key would strand them.

### Releasing the native app

Monoptah Native (GPUI) ships for macOS arm64 (ad-hoc signed, not notarized), Linux x86_64 and
Windows x86_64 (unsigned installer). It releases separately from the Tauri app. No macOS job
runs on GitHub Actions (hosted macOS runners are not being acquired): the macOS app is checked,
built and packaged on the maintainer's Mac, everything else is on CI.

What CI checks (`native.yml`): `monocode-core` clippy and tests on Linux and Windows, and GPUI
with `fmt` and clippy only (Linux) or clippy only (Windows). The GPUI tests and everything
macOS run locally in `npm run native:check`, which `native:release:mac` runs.

1. Edit `docs/native/monoptah-release-notes.md` (its first line must be
   `# Monoptah Native X.Y.Z`) and merge it into main through a PR.
2. Wait for `native.yml` on main to pass (Linux and Windows only now). Its `release-cache` job
   seeds the Linux and Windows build caches that the release reads (a miss is only slower).
3. From a clean checkout of main on the Mac, run `npm run native:release:mac -- X.Y.Z`
   (plain `X.Y.Z`, no pre-release form; add `--dry-run` first to do everything except tag,
   push and publish). `scripts/native-release-macos-local.mjs` checks that you are on an
   up-to-date main with no tracked changes, that the notes heading matches, that the tag and
   release do not exist and that `native.yml` passed for HEAD; checks for macOS SDK 26 and the
   Metal compiler; runs `npm run native:check`; builds and packages macOS; pushes the tag
   `native-vX.Y.Z`; creates the release with the three macOS files; and dispatches
   `.github/workflows/native-release-monoptah.yml`. A passing `native:check` is remembered per
   commit and `rustc -V` in `crates/monocode-gpui/target/.native-check-stamp.json`, so a real
   run after a dry run of the same commit skips it; add `--recheck` to run it again. If a step
   after the tag push fails it prints what was left behind and how to continue; if you are
   interrupted during publishing, check the tag and release state with `gh release view`.
4. The workflow checks the CI gate again, builds Linux, Windows and the rpm (AlmaLinux 10
   container, cache seeded from main by `release-cache-rpm`), signs every updater package in one `publish` job on Ubuntu with
   `TAURI_SIGNING_PRIVATE_KEY`, verifies them with the app's own verifier, publishes, and bumps
   the cask. The signing key exists on CI only; the local script never reads it, and it never
   needs the `.sig` of the macOS package. The workflow has no tag trigger (the tag is pushed
   before the macOS files are uploaded) and refuses a release that lacks the macOS files.

The release page and the macOS zip are public as soon as the script creates the release, and
they stay macOS-only if the workflow fails. The in-app feed and the cask change only at the
end, when `publish` and `homebrew` succeed. The workflow publishes:

- Release `native-vX.Y.Z` (never "latest": Tauri's updater reads `releases/latest`) with, for
  each updater package, the file and its `.sig`:
  - macOS arm64: `MonoptahNative_X.Y.Z_aarch64.app.tar.gz`, plus the `.zip` and `.zip.sha256`
    the cask pins.
  - Linux x86_64: `MonoptahNative_X.Y.Z_amd64.AppImage`, `monoptah-native_X.Y.Z_amd64.deb`
    and `monoptah-native-X.Y.Z-0.preview.0.el10.x86_64.rpm` (the release string comes from
    upstream's spec, which is written for previews).
  - Windows x86_64: `MonoptahNative_X.Y.Z_x64-setup.exe` (NSIS).
- `latest.json` on the rolling pre-release `native-latest`, the feed compiled into the app.
  It lists `darwin-aarch64-app`, `linux-x86_64-appimage`, `linux-x86_64-deb`,
  `linux-x86_64-rpm` and `windows-x86_64-nsis`, and it is never replaced by an older version.

Every platform is required: if a CI package job fails, nothing more is published. Re-run the
workflow (`gh workflow run native-release-monoptah.yml --ref main -f tag=native-vX.Y.Z`, or
re-run the failed jobs). Published assets are never replaced: a platform whose files are all
on the release is not rebuilt (its files are downloaded and signed again) and one with no
files is built, but never macOS. A platform with only some files published fails the `plan`
job: delete that platform's published assets (files and `.sig`) by hand, then re-run. Do not
re-run `native:release:mac` for an existing tag: it refuses. The signed files and feed are
also kept as a workflow artifact. The Linux and Windows packages use upstream's
`scripts/package-native.mjs`.

The `homebrew` job only bumps `Casks/monoptah-native.rb` in the tap (macOS only; Linux and
Windows have no package-manager entry), it does not create it: add that file (modelled on
`Casks/monoptah.rb`) before the first release, or the job fails. It runs after `publish` when
`HOMEBREW_TAP_TOKEN` is set. While it is unset, bump the cask by hand with
`scripts/update-homebrew-cask.sh` and the sha from the `MonoptahNative_<version>_aarch64.zip.sha256`
asset. The fork scripts are `scripts/native-release-macos-local.mjs`,
`scripts/package-native-monoptah.mjs` (also the asset table for every platform) and
`scripts/native-release-monoptah.mjs`.

Things to keep true after merging from upstream or changing repo settings:

- Upstream's `native-release.yml` is gated with `if: github.repository == 'hardbeat920/monocode'`
  on each job. Any job upstream adds there runs ungated on this fork (and fails without Apple
  and R2 secrets), so give it the same `if:`.
- The rolling feed is replaced with `gh release upload --clobber`. Do not enable GitHub
  immutable releases for this repository, or the feed can never be updated and CI cannot upload
  to the release the local script created.
- `native.yml` and `native-release-monoptah.yml` must not use a macOS runner. The
  `MONOPTAH_MACOS_RUNNER` repository variable is unused now and can be deleted.
