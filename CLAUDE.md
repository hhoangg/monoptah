# Monoptah

This repository is a fork of [`hardbeat920/monocode`](https://github.com/hardbeat920/monocode),
renamed **Monoptah**. It tracks upstream and merges from it regularly, while carrying
fork-local changes that are never offered back upstream.

## Git conventions

Full rules and examples: [`docs/git-conventions.md`](docs/git-conventions.md).

- Types (branch, commit, PR): `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.
- Branch: `<type>/<issue#>-<short-slug>` or `<type>/<short-slug>`, lowercase, about 40 chars. Rename an app-made `mc/...` branch with `git branch -m` before the first push.
- Commit: `<type>(<scope>): <summary>` (imperative, at most 72 chars, no period), English. A body (why and what) is required for `feat` and `fix`; add `Refs: #N` or `Fixes: #N` when there is an issue. One buildable change per commit.
- PR titles become native release-note lines (`feat`, `fix`, `perf` are listed; label a PR `user-visible` to include `docs`, `chore`, `ci`, `test`, `build`, `refactor`), so write them for users.
- PR: title in the commit format; the body fills every section of `.github/pull_request_template.md`, keeping its headings. Merge with squash, except upstream-sync PRs, which use a merge commit.
- Never commit to `main`, never force-push a branch with an open PR, never commit secrets or realistic-looking fake tokens.

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

If upstream changed `host/`, bump `host/VERSION` and cut a Host release (see "Releasing the
Host"). Give any new upstream Host workflow job the
`if: github.repository == 'hardbeat920/monocode'` gate.

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

For the native app, run Native Dev from a clean worktree that matches origin, so what you
see is what was pushed:

```bash
npm run native:dev -- --detach
```

`--detach` keeps the app alive when the task that started it ends (a background task
would otherwise take it down) and writes its stdout and stderr to a log file whose path it
prints. Native Dev keeps its data in `~/Library/Application Support/Monoptah Development`,
separate from the installed app.

## Developing the native app

These rules are team-wide and come from measurement: one session spent about 468 minutes
on about 240 cargo test runs because it re-ran tests after every small edit. Each run
relinks the large `monocode-gpui` test binary (median 1.4 min, p90 4.1 min).

- **Test in one pass per task.** Make all the edits for a task first. Then run `cargo fmt`,
  clippy and only the related tests, filtered to the touched modules, once, in a single
  pass. Fix and rerun only what failed. Do not run tests after each small edit. Run the full
  `npm run native:check` once per batch (before a release or a merge to main), not per task:
  CI runs the GPUI and core suites on Linux for every PR (see "What CI checks").
- **Seed the build cache in a new worktree.** Before the first cargo command, clone an
  existing target dir from the main checkout (or another worktree that has one) so crates
  are not rebuilt from scratch. The native workspace (`monocode-gpui` and `monocode-core`)
  builds into `crates/monocode-gpui/target`; the root workspace (`src-tauri`,
  `crates/mobile-link`) builds into `target`. On macOS `cp -c -R` makes an APFS clone,
  which is instant and takes no extra space:

  ```bash
  cp -c -R <checkout>/crates/monocode-gpui/target crates/monocode-gpui/target
  ```

  Elsewhere use plain `cp -R` (a real copy).
- **Always build the native workspace with the `native-dev` profile.** Plain `cargo test`,
  `build`, `check` or `clippy` uses the `dev` profile and fills a second `target/debug` (several
  GB) that nothing else reuses. Cargo has no setting for a default profile and ignores an alias
  that shadows a built-in command, so use the aliases in `crates/monocode-gpui/.cargo/config.toml`
  from inside `crates/monocode-gpui`: `cargo ntest`, `cargo nbuild`, `cargo ncheck`,
  `cargo nclippy`, `cargo nrun`. From the repo root pass
  `--manifest-path crates/monocode-gpui/Cargo.toml --profile native-dev`, as the npm scripts do.
  In `native-dev` the `monocode-gpui` crate is `opt-level = 0` and dependencies are `opt-level = 1`.
  The `monocode-gpui` and `monocode-core` crates keep `debug = "line-tables-only"`, so panic
  backtraces show file and line. Do not add `split-debuginfo` to the profiles: `unpacked` is
  rejected on windows-msvc and `packed` runs dsymutil on every macOS link (about +9s).
  `npm run worktrees:clean -- --prune-target` removes a leftover `target/debug` and stale
  incremental data (see the `cleanup-worktree` skill).
- **Keep tests Linux-portable.** GPUI tests run on Ubuntu in CI. Use `secondary-...`
  keystrokes (cmd on macOS, ctrl elsewhere), never `cmd-...`. Do not assume macOS paths,
  Keychain, WKWebView, AVPlayer or Metal. A test that truly runs only on macOS gets
  `#[cfg_attr(not(target_os = "macos"), ignore = "<specific reason>")]` or
  `#[cfg(target_os = "macos")]`. Never weaken an assertion to make a test pass.
- **Fix at the root, in the shared component.** Prefer the shared pieces (the modal dialog in
  `crates/monocode-gpui/src/modal.rs`, the popover menus and choice selects in
  `crates/monocode-gpui/src/popover.rs`, the one `sbx` runner in
  `crates/monocode-core/src/sandboxes.rs`) so a fix reaches every caller. The first fix for the dialog
  input barrier went into individual dialogs and covered 2 of about 35.
- **Review once per request.** Agents do one review per user request, not one per step. The
  implementer hands over a checklist that maps each point of the request to its change
  (`file:line`) and its test. The reviewer gets the original request, marks each point done
  or missing with evidence, then reviews the diff. Reviewers read code; they build or run
  tests only to prove a specific finding. High-risk changes (tokens and secrets, deleting
  data, migrations, billable cloud lifecycle) get their own review.
- **No realistic secrets in tests.** GitHub push protection blocks realistic-looking tokens
  such as `dckr_pat_...` in test sources. Build fake tokens with `concat!` or use obviously
  fake values.

## Cleaning up worktrees

When a session created a PR and that PR is merged, the agent cleans up after itself: its
worktree, local and remote branch, and any temp worktrees, using the `cleanup-worktree` skill
(`npm run worktrees:clean`). It refuses anything unmerged, dirty, unpushed, running, or the
worktree Native Dev runs from, and never forces. It never removes `release-main` (the
build-cache source and release checkout), not even when named explicitly, and `--merged` keeps
detached-HEAD worktrees (name one explicitly to remove it). Do not delete worktrees by hand.

For the user: `npm run worktrees:clean -- --merged --dry-run` lists what would go and why the
rest stays, then run it without `--dry-run` to delete (it asks first; non-TTY needs `--yes`).

## Releasing

Releases are native only for now, see "Releasing the native app". The Tauri workflows
`.github/workflows/ci.yml` and `release.yml` are paused: they have only `workflow_dispatch`,
so pushes, pull requests and `v*` tags start nothing. Keep that when merging from upstream.

To cut a Tauri release anyway: `npm run set-version -- X.Y.Z`, add a `CHANGELOG.md` section,
merge that through a PR titled `chore(release): vX.Y.Z`, tag the merged commit `vX.Y.Z` and push
the tag, then run `gh workflow run release.yml --ref vX.Y.Z`.
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

### Releasing the Host

The fork publishes its own remote Host (`host/`), separately from both apps. Its version lives
in `host/VERSION` as `X.Y.Z+monoptah` (semver build metadata marks the fork flavor); the root
`package.json` version is not used. `monocode-host --version` prints it and
`environment.describe` reports `host: {flavor: "monoptah", version: "X.Y.Z"}`.

1. `npm run host:release -- X.Y.Z` (plain `X.Y.Z`): needs a clean tree, writes `host/VERSION`,
   commits "Release Monoptah Host X.Y.Z" and creates the tag `host-vX.Y.Z`. It never pushes; it
   prints the push commands for you to review and run. To keep the git conventions (no commits
   to main, squash merges), set `host/VERSION` to `X.Y.Z+monoptah` in a PR titled
   `chore(release): Monoptah Host X.Y.Z` instead, then run the script on the up-to-date main:
   with the version already recorded it only tags HEAD. Push only the tag, not HEAD.
2. Pushing the tag starts `.github/workflows/host-release-monoptah.yml`. It checks the tag
   against `host/VERSION`, refuses when the release already exists, builds every target of
   `npm run host:package -- --all` (Linux and macOS `.tar.gz`, Windows `.zip`, asset names keep
   `monocode-host`), and publishes release `host-vX.Y.Z` with `--latest=false` (never "latest":
   Tauri's updater reads `releases/latest`). It uploads each package with its `.sha256` and
   `host-release.json` (`{version, assets: {target: sha256}}`, also a workflow artifact with a
   build-provenance attestation).
3. Never re-upload a version: the app pins the sha256 of every asset. To redo a failed release,
   bump the version.
4. The app pins the Host version, URL base and hashes (H2a, `host_release.rs`, written by
   `scripts/pin-host-release.mjs` from the run's `host-release.json`). Release the Host before
   the app that pins it.

Note: the paused Tauri `release.yml` still packages the Host, but those packages now report
`X.Y.Z+monoptah` while the Tauri app's bootstrap expects its own version. A Tauri release cut
after H1 would fail to install the Host until the Tauri bootstrap is handled (H2a covers the
native app only).

### Releasing the native app

Monoptah Native (GPUI) ships for macOS arm64 (ad-hoc signed, not notarized), Linux x86_64 and
Windows x86_64 (unsigned installer). It releases separately from the Tauri app. No macOS job
runs on GitHub Actions (hosted macOS runners are not being acquired): the macOS app is checked,
built and packaged on the maintainer's Mac, everything else is on CI.

What CI checks (`native.yml`): `monocode-core` clippy and tests plus `native:test:dev` on Linux
and Windows (the Windows core tests run in 3 shards from one nextest archive), GPUI `fmt` and clippy `-D warnings` on Linux, GPUI tests on Linux (built once with
`cargo nextest archive`, run in 3 shards, one process per test), and GPUI `cargo check
--all-targets` on Windows. The workflow runs on every PR (a `changes` job skips the heavy jobs when
no native path changed) and the `CI passed` job is the single check to require for merging. During development the local remainder is `npm run native:check:mac`
(clippy of the macOS-only code that Linux and Windows never compile); when you change macOS-only
code or tests gated to macOS, also run the tests you touched on macOS. `npm run native:check`
(the full gate, including the GPUI tests on macOS) still runs locally in `native:release:mac`
before every release. How often to run which check while developing is in "Developing the
native app".

How to release (timeline, notes rules, recovery and stuck runs: [`docs/native/release-flow.md`](docs/native/release-flow.md)):

1. **No notes PR.** Release notes are generated from the titles of the PRs merged since the
   previous `native-v*` tag (see `docs/git-conventions.md`, "Releases"). Optional highlights:
   `--notes FILE`, or `docs/native/monoptah-release-notes.md` with first line
   `# Monoptah Native X.Y.Z` for this version. Pick X.Y.Z by semver: `feat` is minor, `fix` and
   `perf` are patch, a `BREAKING CHANGE` is major.
2. **One command**, from an up-to-date `main` on the Mac, right after the last PR merges:
   `npm run native:release:mac -- X.Y.Z` (plain `X.Y.Z`; options `--since REF`, `--notes FILE`,
   `--recheck`, `--ci-timeout MINUTES`). Preconditions and the notes are checked first.
3. **In parallel.** The script dispatches `native-release-monoptah.yml` for the commit (the
   early run), so Linux, Windows and rpm build while the Mac runs `npm run native:check` (about
   20 minutes, remembered per commit and `rustc -V` in
   `crates/monocode-gpui/target/.native-check-stamp.json`), builds and packages. It waits for a
   green `native.yml` on main for HEAD only right before tagging (default 60 minutes). Any
   failure before the tag leaves nothing tagged or published and cancels the early run: fix it
   and run the script again.
4. **Publishing.** The script tags, pushes `native-vX.Y.Z` and creates the release with the three
   macOS files and the notes as body. The early run (or a fresh one) then signs every updater
   package in one `publish` job on Ubuntu (the signing key exists on CI only, the local script
   never reads it), verifies, publishes, and bumps the cask.
5. **`--dry-run` is optional**: a real run is safe on its own because every check happens before
   the tag. Use it to test changes to the script.
6. **If something fails** the script prints the exact commands (`gh run cancel ID`, and
   `gh workflow run native-release-monoptah.yml --ref main -f tag=native-vX.Y.Z`). Never re-run
   `native:release:mac` for an existing tag: it refuses. A run queued with no job for 10 minutes
   is cancelled and replaced by the script.

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
failed jobs or dispatch the workflow again with the tag only. Published assets are never
replaced: a platform whose files are all on the release is not rebuilt (its files are
downloaded and signed again) and one with no files is built, but never macOS. A platform with
only some files published fails the `plan` job: delete that platform's published assets (files
and `.sig`) by hand, then re-run. The signed files and feed are also kept as a workflow
artifact. The Linux and Windows packages use upstream's `scripts/package-native.mjs`.

The `homebrew` job only bumps `Casks/monoptah-native.rb` in the tap (macOS only; Linux and
Windows have no package-manager entry), it does not create it: add that file (modelled on
`Casks/monoptah.rb`) before the first release, or the job fails. It runs after `publish` when
`HOMEBREW_TAP_TOKEN` is set. While it is unset, bump the cask by hand with
`scripts/update-homebrew-cask.sh` and the sha from the `MonoptahNative_<version>_aarch64.zip.sha256`
asset. The fork scripts are `scripts/native-release-macos-local.mjs`,
`scripts/native-release-notes.mjs`, `scripts/package-native-monoptah.mjs` (also the asset table
for every platform) and `scripts/native-release-monoptah.mjs`.

The release script removes the built `Monoptah Native.app` from `target/.../release` after packaging (also on failure and in `--dry-run`) and marks that directory `.metadata_never_index`. Agents must not leave built `.app` bundles behind: remove any you build outside the release flow when done (Native Dev's `~/Applications/Monoptah Native Dev.app` is the exception).

Things to keep true after merging from upstream or changing repo settings:

- Upstream's `native-release.yml` is gated with `if: github.repository == 'hardbeat920/monocode'`
  on each job. Any job upstream adds there runs ungated on this fork (and fails without Apple
  and R2 secrets), so give it the same `if:`.
- The rolling feed is replaced with `gh release upload --clobber`. Do not enable GitHub
  immutable releases for this repository, or the feed can never be updated and CI cannot upload
  to the release the local script created.
- `native.yml` and `native-release-monoptah.yml` must not use a macOS runner. The
  `MONOPTAH_MACOS_RUNNER` repository variable is unused now and can be deleted.
- Every job in `native.yml` and `native-release-monoptah.yml` keeps a `timeout-minutes`.
- `native-release-monoptah.yml` keeps its per-tag concurrency group (`native-monoptah-release-<tag>`)
  and the `native-monoptah-publish` group on the `publish` job. Do not turn on `cancel-in-progress`
  there, and do not go back to one global group: the early run waits up to 2 hours for the tag.
- The workflow runs its helper scripts from the `release-tools/` checkout of the dispatched ref, not from the built commit, so the tag-only recovery works for older tags.
- Only the `publish` job may see the signing key or have `contents: write`.
