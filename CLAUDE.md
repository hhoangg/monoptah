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

`npm run set-version -- X.Y.Z`, add a `CHANGELOG.md` section, commit as `Release vX.Y.Z`,
then push the tag. Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds
macOS, Linux and Windows, signs the updater bundles with the `TAURI_SIGNING_PRIVATE_KEY`
secret, and publishes a GitHub Release with `latest.json`.

The same tag also bumps the Homebrew cask in `hhoangg/homebrew-monoptah` (the `homebrew`
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

Monoptah Native (GPUI, macOS arm64 only, ad-hoc signed, not notarized) releases separately
from the Tauri app. Edit `docs/native/monoptah-release-notes.md` (its first line must be
`# Monoptah Native X.Y.Z`), commit it, then push the tag `native-vX.Y.Z` (plain `X.Y.Z`, no
pre-release form). `.github/workflows/native-release-monoptah.yml` builds, signs with the same
`TAURI_SIGNING_PRIVATE_KEY`, and publishes:

- Release `native-vX.Y.Z` (never "latest": Tauri's updater reads `releases/latest`) with
  `MonoptahNative_X.Y.Z_aarch64.app.tar.gz` and `.sig` for the in-app updater, plus the
  `.zip` and `.zip.sha256` the cask pins.
- `latest.json` on the rolling pre-release `native-latest`, the feed compiled into the app.
  It is never replaced by an older version.

A re-run of the same tag after a partial failure reuses the published assets and never
replaces them; the signed files and feed are also kept as a workflow artifact.

The `homebrew` job only bumps `Casks/monoptah-native.rb` in the tap, it does not create it:
add that file (modelled on `Casks/monoptah.rb`) before the first release, or the job fails.
It runs when `HOMEBREW_TAP_TOKEN` is set. While it is unset, bump the cask by hand with
`scripts/update-homebrew-cask.sh` and the sha from the `MonoptahNative_<version>_aarch64.zip.sha256`
asset. The fork scripts are `scripts/package-native-monoptah.mjs` and
`scripts/native-release-monoptah.mjs`.

Two things to keep true after merging from upstream or changing repo settings:

- Upstream's `native-release.yml` is gated with `if: github.repository == 'hardbeat920/monocode'`
  on each job. Any job upstream adds there runs ungated on this fork (and fails without Apple
  and R2 secrets), so give it the same `if:`.
- The rolling feed is replaced with `gh release upload --clobber`. Do not enable GitHub
  immutable releases for this repository, or the feed can never be updated.
