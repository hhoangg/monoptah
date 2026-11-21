# Plan: retire the archived Tauri app

Status: plan only. Nothing is deleted, renamed or reconfigured by this document.

## Facts found (2026-11-21)

- Tauri CI and release are paused: `.github/workflows/ci.yml` and `release.yml` are `workflow_dispatch` only.
- The repo's GitHub "Latest" release is `v0.33.0` (Tauri). Every native release (`native-v*`),
  the `native-latest` feed and `host-v*` are created with `--latest=false`
  (`scripts/native-release-macos-local.mjs:300`, `native-release-monoptah.yml:813`,
  `host-release-monoptah.yml:173`), because the Tauri updater reads
  `https://github.com/hhoangg/monoptah/releases/latest/download/latest.json` (`src-tauri/tauri.conf.json`).
- Native feed: `releases/download/native-latest/latest.json`, compiled in via
  `MONOCODE_NATIVE_UPDATER_ENDPOINT` (`crates/monocode-gpui/build.rs`). Same updater key as Tauri.
- Native identity: name "Monoptah Native", bundle id `com.monoptah.native` (`crates/monocode-core/src/brand.rs`,
  `scripts/bundle-native-macos.sh:23-30`, `scripts/package-native.mjs`). Linux package `monoptah-native`.
- The installed native app reads the Tauri data dir: `monocode_core::store::monocode_database()`
  -> `Application Support/com.monoptah.desktop/monocode.db` (`crates/monocode-gpui/src/main.rs:290`,
  `store/monocode_import.rs:32`, `brand.rs:9 DESKTOP_APP_ID`), and Tauri WebView prefs
  (`desktop_preferences/webview.rs`). So the data dir already is shared and must stay.
- No keychain item is tied to `com.monoptah.*`; only `com.monocode.mobile` (legacy identity migration).
- Homebrew tap `hhoangg/homebrew-monoptah`: `Casks/monoptah.rb` (Tauri), `Casks/monoptah-native.rb` (native).

## Inventory

### Tauri-only (can go)

- `src-tauri/` except the assets native uses (below).
- `src/`, `index.html`, `mono-chat.html`, `quick-composer.html`, `vite.config.ts`, `vitest.config.ts`,
  `playwright.config.ts`, `tsconfig.json`, `tsconfig.node.json`, `public/`, `tests/browser/`.
- Scripts: `bump-version.mjs`, `make-update-manifest.mjs`, `repack-appimage.sh`,
  `assert-appimage-host-libs.sh`, `install-linux-deps-{debian,fedora}.sh`,
  `generate-hugeicons.mjs`, `generate-material-icons.mjs` (verify), `release-channel.cjs` (verify users).
- Workflows: `ci.yml`, `release.yml`.
- `package.json` scripts: `dev`, `dev:stable`, `build`, `preview`, `test`, `test:watch`, `test:browser`,
  `check`, `check:web`, `check:rust`, `set-version`, `tauri`, `tauri:stable`, `tauri:dev`,
  `setup:linux:*`, `build:linux`, `build:fedora`, `build:windows`.
- Deps: `@tauri-apps/*` (but see `native.yml:137`: signing tests use `@tauri-apps/cli`), React,
  CodeMirror, TipTap, xterm, vite, tailwind and other frontend-only packages.

### Shared (keep)

- `crates/mobile-link` (used by `monocode-core`, web protocol, mobile). Root `Cargo.toml` workspace
  then has only this member.
- `host/`, `scripts/update-homebrew-cask.sh` (+ test), `scripts/check-fork-branding.sh`,
  `scripts/check-web-isolation.mjs`, `scripts/worktrees-clean.mjs`, all `native:*`, `host:*` scripts.
- Assets native reads from `src-tauri/`: `icons/icon.ico` (`crates/monocode-gpui/build.rs:26,30`,
  `scripts/package-native.mjs:353`), `icons/icon.icns`, `macos/Assets.car`, `Entitlements.plist`
  (`scripts/bundle-native-macos.sh:65,67,111`), updater `pubkey` in `tauri.conf.json`
  (`scripts/native-release-macos-local.mjs:111-114,666`).
- Host deps: `esbuild`, `typescript`, `@types/node`, `vitest` (verify against `host/`).
- Core code that reads Tauri data (`monocode_import.rs`, `webview.rs`, `credential_guard.rs`) stays:
  it is the migration path for users and the live data dir.

## Option 1: delete everything Tauri-only

1. Move shared assets out of `src-tauri/`: `assets/native/` (icons, Assets.car, Entitlements.plist)
   and `assets/native/updater.pub` (pubkey). Repoint `build.rs`, `package-native.mjs`,
   `bundle-native-macos.sh`, `native-release-macos-local.mjs`, the comment in
   `native-release-monoptah.yml:156`. One PR, native CI must be green.
2. Make `@tauri-apps/cli` unnecessary for signing tests (`native.yml:137`), or keep it as the only Tauri dep.
3. Delete the Tauri-only paths above; root `Cargo.toml` members = `["crates/mobile-link"]`; regenerate
   `Cargo.lock` and `package-lock.json`.
4. CI: delete `ci.yml`, `release.yml`; drop `src-tauri/**` and `src/...` entries from the
   `native.yml` `changes` filter (lines 70-73); `web-guard.yml` isolated build keeps working (it
   deletes paths that no longer exist; simplify later).
5. Branding checker: drop `src/`, `src-tauri/src/` from the scan and the 6 Tauri ALLOW entries and
   the `fs.rs` fixture filter. The checker must still pass with no new ALLOW entries.
6. Web isolation: `web/docs/tauri-reuse.md` rules stay (copies keep provenance by commit hash;
   the commit still exists in history). Update text to say the source is archived.
7. CLAUDE.md "Never rename": `monocode-desktop`, `monocode` crate, `bump-version.mjs` rows go;
   `INTERRUPT_MESSAGE` row moves to the native/core constant if one exists, else is removed.
   Remove "Seeing a change live" Tauri part and "Releasing" (Tauri) section; update README,
   CONTRIBUTING, `docs/building.md`, `docs/install.md`, `docs/native/migration.md`.
8. Verify: `npm run native:check:mac`, native CI green, `npm run host:package -- --all`,
   `web` CI + web-guard + web-e2e green, `./scripts/check-fork-branding.sh` exit 0,
   `npm run native:release:mac -- X.Y.Z --dry-run`.

Pros: smaller repo, no dead CI, clearer docs. Cons: upstream merges touching `src/` or `src-tauri/`
now conflict as modify/delete on every merge (resolve by keeping the deletion; document in CLAUDE.md).

## Option 2: mark unsupported, keep code frozen

- Keep `src/`, `src-tauri/` as-is, paused workflows stay paused. Add a README banner and a top-level
  note in `src-tauri/` "archived, not built, not supported".
- Upstream merges keep landing in those paths without conflicts; no build risk.
- Cons: dead code keeps confusing agents and searches; branding checker keeps scanning it.

### Tauri GitHub releases: keep or delete

| | Keep | Delete |
|---|---|---|
| Existing Tauri installs | Keep working, no updates | Updater gets 404 on `releases/latest/...`, silent failure |
| Download links in old docs | Work | Break |
| Clarity | Old app looks current | Clear |

Recommended: keep the old `v*` releases, and first ship one **final Tauri update** (`v0.34.0`):
display name "Monoptah Deprecated", a startup dialog "This app is no longer supported. Install
Monoptah (native)" with a download button, and a final `latest.json`. Only after that final update
has been out a few weeks, switch Latest to native (below). After the switch, Tauri installs that did
not update stop seeing updates (the feed 404s, or serves a native manifest they reject by platform
key), which is acceptable once the final notice has shipped.

Caveat: cutting a Tauri release needs the paused `release.yml` (run by `workflow_dispatch`) and the
Host bootstrap issue in CLAUDE.md ("Releasing the Host" note). The final release can skip Host
packaging. Simpler alternative: publish only macOS for the final update.

## Naming

- Native becomes "Monoptah" (display name, menu, window title, `CFBundleName`, desktop entry name,
  Windows installer name, release titles, artifact names `Monoptah_...` for new releases).
- Keep the bundle id `com.monoptah.native`, the Linux package id, `StartupWMClass`, the log dir and
  the `native-latest` feed. Reason: changing the bundle id changes the macOS app identity:
  TCC grants (Accessibility, Files), notification permission, Launch Services, Gatekeeper first-run,
  any future keychain ACLs and the updater's install target. Data is already in
  `com.monoptah.desktop` (shared), so no data move is needed.
- If a new id is ever wanted: ship a release that copies/links data and prefs, keeps reading the old
  id, and re-requests permissions; never in the same release as the rename.
- The app bundle filename changes `Monoptah Native.app` -> `Monoptah.app`. The updater must replace
  the old bundle in place (or keep the old filename for one release). Check the native updater's
  install path logic before renaming; the cask needs `app "Monoptah.app"` and a `rename`/`zap` review.
  `Casks/monoptah.rb` (Tauri) and `monoptah-native.rb`: make `monoptah` point to native later and
  deprecate `monoptah-native` with `deprecate!`, after the Tauri cask has shown a deprecation notice.
- Tauri: `productName` "Monoptah Deprecated" in the final update only (display name). Identifier
  `com.monoptah.desktop` stays (data dir shared with native).

## Releases: native as Latest

- Today native and host releases pass `--latest=false`, so `v0.33.0` stays Latest.
- After the final Tauri update and the waiting period: change `native-release-macos-local.mjs:300`
  to `--latest` for `native-vX.Y.Z` (keep `native-latest` and `host-v*` at `--latest=false`), and
  update the comments in `native-release-monoptah.yml:24,752` and CLAUDE.md. Then
  `gh release edit native-vX.Y.Z --latest` once for the current version.
- `docs/install.md` links to `releases/latest` then reach native.

## Risks

| Risk | Mitigation |
|---|---|
| Deleting `src-tauri/` breaks native build (icons, entitlements, pubkey) | Move assets first, separate PR, green CI |
| Latest switch breaks Tauri updater | Final Tauri update first, then wait |
| Bundle rename loses permissions or leaves two apps | Keep bundle id; test updater on a copy; cask rename review |
| Upstream merges conflict on deleted paths | Documented resolution rule; or choose Option 2 |
| Losing `INTERRUPT_MESSAGE`/data compatibility | Core keeps reading `com.monoptah.desktop`; no data changes |
| Signing tests need `@tauri-apps/cli` | Decide in step 2 of Option 1 |
| Updater key confusion | Key stays the same; only the pubkey file moves |

## Order

1. Final Tauri update "Monoptah Deprecated" with switch notice (+ Tauri cask deprecation).
2. Native rename to "Monoptah" (display + artifacts), bundle id unchanged.
3. Wait 2-4 weeks; then mark native Latest and change the release script.
4. Move shared assets out of `src-tauri/`.
5. Option 1 deletion (or Option 2 freeze) + CI + docs + CLAUDE.md.
6. Cask: `monoptah` -> native.

## Estimate

- Final Tauri update: 1-1.5 d (paused pipeline, Host packaging).
- Native rename: 1 d (+ updater path test).
- Latest switch: 0.5 d.
- Asset move: 0.5 d. Deletion + CI + docs: 1-1.5 d.
- Total about 4-5 working days, spread over 3-5 weeks due to the waiting period.
