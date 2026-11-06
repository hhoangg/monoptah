# Native 0.48.0 release audit

Checked on 2026-11-06. Publication is blocked; this document is not a release confirmation.

- Latest published non-draft, non-prerelease native version: `native-v0.46.0`, commit `b7ea1ea05904dc7f42fc8a314877b24807c83674`.
- Fetched main: `d7ba8e1dfefcc1a6e3a903ec780a004d3fe5871b`.
- Existing `native-v0.47.0` tag: `f043303269738e0e64ff0ad8783b7bbb1f08c496`. No corresponding published release. Do not move this tag to include newer commits.
- Next proposed version: 0.48.0, covering the existing preview-14 changes plus merged Codespaces functionality (PR #43), numeric session ID copying (#44), session cleanup APIs (#45), and automatic Claude token capture (#46).
- Native version comes from the release tag and release-notes heading; no Tauri/package version bump is needed.
- Work prepared in an isolated worktree from fetched main. Existing workspaces and their untracked files were preserved. No open PR or running release was found in the remote checks.

## Verification

- `node --test scripts/package-native-monoptah.test.mjs scripts/native-release-monoptah.test.mjs`: 14 passed, 1 skipped, 0 failed.
- `npm run native:test:dev`: 53 passed, 1 skipped, 0 failed outside the sandbox (the first sandboxed attempt could not execute sccache).
- `git diff --check`: passed.
- Main CI: https://github.com/hhoangg/monoptah/actions/runs/42240265069
  - Linux core passed.
  - Windows core: 3722 passed, 16 failed, 2 ignored. The session_worktree fixtures fail at `git push origin main` with `hostname contains invalid characters`; the fixture uses a canonicalized local Windows path as its remote.
  - macOS core and desktop jobs were cancelled because GitHub could not acquire a hosted arm64 runner, even with the configured macos-15 fallback.
- Prior 0.47.0 attempt: https://github.com/hhoangg/monoptah/actions/runs/42061449243
  - All four packaging jobs passed; signing/publishing was cancelled due to the same runner-capacity issue. No release was published.

## Remaining release gate

Fix the Windows test fixture in a separately reviewed change and obtain passing native CI, including macOS. Refresh main and these notes if additional changes merge. Commit the notes to main, wait for native.yml to pass on that commit, recheck releases and active workflows, then push a new unused native tag. Verify versioned assets, signed rolling feed, and Homebrew results before reporting publication success. No tag or publish was triggered by this audit.
