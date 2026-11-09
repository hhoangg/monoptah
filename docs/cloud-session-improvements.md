# Cloud session improvements (native app) - design

Status: **Phase 1, design only. Nothing here is implemented.** User decisions of
2026-11-09 are recorded in §6 and folded into the sections.
Branch: `feat/cloud-session-improvements` (from `origin/main` at `d08c9838`).
Scope: the GPUI app (`crates/monocode-core`, `crates/monocode-gpui`) and the remote Host
(`host/`), for both cloud backends: GitHub Codespaces and Docker Sandbox (`sbx`).

Four features:

1. Host `files.readBytes`: ship it and remove the copy-file workaround.
2. The Explorer, Changes panel and editor work on the remote workspace.
3. Take the user's Claude Code skills, MCP servers and plugins to the cloud.
4. Move a session between local and cloud, as one session with one harness id.

Paths below are relative to the repository root. `core/` means
`crates/monocode-core/src/` and `gpui/` means `crates/monocode-gpui/src/`.

---

## 0. Cross-cutting findings

### 0.1 Branches and worktrees that overlap

| Branch | State | Effect on this design |
|---|---|---|
| `feat/unified-cloud-api` | Merged (PR #56, `e5946d01`), 0 commits ahead of main | Already the base. The `cloud.*` app API (`core/mono/cloud_api.rs`) and `CloudProvider` trait are what feature 4 plugs into. No dependency left. |
| `fix/cloud-file-preview` | Merged (PR #57, `92480bef`) | Already the base: reconnect-on-click for chat file links. Feature 1 builds on it. |
| `feat/auto-capture-claude-token`, `feat/native-claude-setup-token`, `fix/native-delete-missing-codespace`, `fix/codespace-test-flake` | All merged (0 ahead) | None. |
| `origin/codex/event-driven-git-refresh` (`69d3ebcb`, upstream, Tauri era, never merged) | 1 commit, adds `host/git-watch.ts` (long-poll watcher) | **Source to port** for ticket F2-4. We take the host part; the Tauri client part is not needed. |
| `origin/feat/editor-git-blame` | Content already on main (`host/git-blame.ts` identical) | None. |
| `origin/mc/plan-remote-agent-sessions`, `origin/codex/remote-all-providers` | Old upstream Tauri-era host work (Sep 2026) | Not depended on. Not merged, not needed. |
| `mc/native-178366935418669-1794023945510` | 1 commit: "Add a dev container for testing cloud sessions in Codespaces" | Useful as a test fixture for manual testing. Not a dependency. |
| `docs/docker-sandbox-spec` | 317 behind; its spec (`docs/docker-sandbox-spec.html`) is already on main | Reference only (network policy notes used by feature 3). |

No open branch blocks this work.

### 0.2 The fork does not ship its own Host (blocks features 1, 2-watch, 3 and 4)

This is the main finding. Every feature that needs a Host change is blocked on it.

- The bootstrap script `core/remote/remote_bootstrap.sh` downloads
  `monocode-host-<os>-<arch>.tar.gz` from `@@RELEASE@@`. It checks the sha256 from the
  same release and requires `monocode-host --version == @@VERSION@@`.
- `core/remote/ssh.rs:699-721` (`bootstrap_script_from_template`) fills in:
  - the version from `env!("CARGO_PKG_VERSION")`, which is `0.16.0` (the
    `[workspace.package]` version in `crates/monocode-gpui/Cargo.toml:84-89`);
  - the release URL `https://github.com/hardbeat920/monocode/releases/download/v{version}`.
- `c36ed8f6` ("Download the Monoptah Host from upstream again") switched that URL back to
  **upstream**. The fork's own `v0.16.0` Host assets were older than upstream's no-systemd
  fix, so codespaces failed at `loginctl enable-linger`.
- So a **codespace** bootstrapped since `c36ed8f6` runs **upstream's v0.16.0 Host**.
  - A sandbox bootstrapped by an earlier build may run the fork's old v0.16.0 Host. The
    `loginctl` failure was codespace-only, because sandboxes run the Host detached, and
    nothing ever upgrades it.
  - The paused Tauri app pins `hhoangg/monoptah/.../v0.33.0`
    (`src-tauri/src/remote_ssh.rs:401`).
  - Either way, fork-only Host code never reaches a cloud instance. That includes `files.readBytes` (`e945c4e7`): it is in
  `host/server.ts:282,513-524` and `host/workspace.ts:446-482` on main, and upstream main
  does not have it.
- The script reinstalls only when the binary is missing or when
  `MONOCODE_HOST_FORCE_UPGRADE=1` (`remote_bootstrap.sh:12`). That variable is set only by
  the Connections "Upgrade" path (`ssh.rs:767-775`). A cloud instance that already has a
  Host is never upgraded, and its version is never compared.
- The handshake (`environment.describe`) reports `protocolVersion` (always 1) and a
  `capabilities` list (`host/server.ts:256-297`). It does **not** report the Host version
  or who built it (`core/remote.rs:387,707-731`).
- Host packages are built only by the paused Tauri `release.yml` (`:396-411`).
  `native-release-monoptah.yml` does not build them. `host/cli.ts:31,73` prints the
  root `package.json` version (0.33.0), not 0.16.0.
- The Host source on main already has the no-systemd fix (`host/service.ts:116-119`,
  `systemdBooted`). A fork build from main would therefore not hit the `loginctl` failure
  that caused the revert.

**Decision proposed (H): the fork builds and publishes its own Host packages, versioned
independently of the app. The bootstrap upgrades cloud Hosts to that version.** Section
1 has the details. Tickets H1, HR1, H2a and H2b come first.

### 0.3 The native core already speaks "remote" for files and git

`core/files/remote.rs` is a 1:1 port of the web app's `remoteCommands.ts`. Any core
files/scm function that gets a `remote://<environmentId>/<hostPath>` path forwards to the
Host's `workspace.run` command table (`host/workspace-commands.ts:40-84`). That covers:

- `list_dir`, `read_text_file`, `read_binary_file`, `write_text_file`, `stat_files`;
- `create_path`, `rename_path`, `delete_path`, `copy_path`, `move_path`;
- all `git_*` commands: index, diffs, stage/unstage/discard, commit, push/pull/sync, PR,
  history, blame;
- `search_project`.

Callers include:

- `core/files.rs:39-42`, `core/files/document.rs:86-176,302-320`;
- `core/scm.rs` (every public function), `core/scm/review.rs`, `core/git/remote.rs`.

Those Host commands exist in **upstream's v0.16.0 Host** too. Feature 2's basic
read/write therefore does not need the fork Host.

A cloud session's cwd is already such a path: `remote://<env>/workspaces/<repo>`
(`core/cloud_sessions.rs:3`, `CloudBinding.project_key` `:310-313`).

### 0.4 Transport facts every feature has to respect

- App to Host is HTTP JSON-RPC (`POST {endpoint}/rpc`, Bearer token, `ureq`).
  - Limits: connect timeout 5 s, total 30 s, response cap 16 MB (`core/remote.rs:269-340`).
  - It runs over an SSH local port forward (`core/remote/ssh.rs:801-845`): through
    `gh codespace ssh` for a codespace, or `ssh <id>@sbx_cloud` for a sandbox.
- There is no streaming. Session state is polled with `sessions.sync`: every 3 s when
  idle and every 750 ms during a turn (`gpui/conversation/remote.rs:446-460`).
- Host methods are allowlisted in `supported_remote_method` (`core/remote.rs:532-567`). A
  new Host method has to be added there.
- The Host's server `requestTimeout` is 20 s (`host/server.ts:196`). In Node this bounds
  *receiving* the request, not the handler. A long-poll is bounded by ureq's 30 s total
  and by the tunnel.
- Host git push/pull/sync timeouts are 30 s (`host/workspace-commands.ts:495,542,593,718`),
  equal to ureq's 30 s total. A slow push can be reported as failed while it succeeded.
- Background reads never wake a workspace. `Connections::request` only uses an existing
  tunnel; otherwise it returns `CODESPACE_NOT_CONNECTED` / `SANDBOX_NOT_CONNECTED`
  (`core/remote.rs:469-530`). Waking is always a user action (`gpui/cloud_wake.rs`). Every
  design below keeps that rule. In particular, ssh into a sandbox wakes it and bills
  (`core/cloud_sessions/docker_start.rs:88-90`).

---

## 1. Feature 1: Host `files.readBytes` (and the fork Host)

### 1.1 Current state (verified on main `d08c9838`)

- **Host:** `files.readBytes {projectId, cwd?, path, offset, length}` returns
  `{data: base64, offset: next, size}` (`host/server.ts:513-524`, `host/workspace.ts:446-482`).
  - Chunks are at most `MAX_READ_CHUNK = 393216` bytes, a multiple of 3 so base64 chunks
    can be joined.
  - The file is opened with `O_NOFOLLOW`; `.git` and links leaving the workspace are
    refused.
  - It is advertised in `capabilities` (`server.ts:282`) and tested in
    `host/workspace.test.ts:351`.
- **App:** `core/cloud_sessions/view.rs`.
  - `Fetcher::has_bytes()` (`:367-382`) probes `environment.describe` once per fetcher.
  - `range` (`:385-424`) uses `files.readBytes`. It does **not** send `cwd`, so a session
    running in a Host worktree reads from the project root, not its worktree. That is a
    latent bug.
  - Previewable files up to 5 MB open in memory (`MAX_MEMORY_BYTES`, `:76`). Anything else
    downloads into the session's `cloud-cache/` (2 GB cap, confirm above 500 MB;
    `view_cache.rs:28-30`).
- **The workaround** is the `OldHost` fallback from the same commit `e945c4e7`:
  1. When `has_bytes()` is false and the file is not Markdown/HTML/text at offset 0, the
     read returns `ViewError::OldHost` (`view.rs:401-424`).
  2. That becomes `Phase::CopyOffer` in `gpui/shell/cloud_view.rs:187,400`.
  3. The card shows `OUT_OF_DATE` (`view.rs:496`) with a **Download** button
     (`cloud_view.rs:710-725`).
  4. Download runs `CloudSessions::copy_cloud_file` (`core/cloud_sessions/view_cache.rs:405-495`).
     It stages into `cloud-cache/.monoptah-copy-*`, calls `CloudBackend::copy_from`,
     checks the result is a regular file under 2 GB, renames it into the cache, and always
     removes the staging folder.
     - Codespaces: `gh codespace cp -e -r` (`core/codespaces.rs:1110-1142`).
     - Sandbox: a base64 tar over ssh (`core/cloud_sessions/backend/docker.rs:350-504`,
       600 MB / 100k entries).
- Because cloud Hosts are upstream v0.16.0 (§0.2), **every** cloud Host is "old" today.
  The workaround is the only path for binary files from chat links.
- The **editor** path is separate. `core/files/document.rs:160-176` reads `remote://`
  files with `workspace.run read_binary_file`: the whole file as base64, capped at 10 MB.
  Upstream's v0.16.0 Host has this command. An old Host can therefore deliver binaries up
  to 10 MB without `files.readBytes`. `view.rs` does not use this.

### 1.2 Proposed solution

**H1 - The fork publishes its own Host.**

- New workflow `.github/workflows/host-release-monoptah.yml`, triggered by a tag
  `host-vX.Y.Z`.
  - Runs `npm run host:package -- --all` (`host/package.mjs`, pinned Node 24.21.0).
  - Publishes **every** `--all` target with its `.sha256` to the GitHub Release
    `host-vX.Y.Z` on `hhoangg/monoptah`. That means the Linux and macOS `.tar.gz` plus the
    Windows `.zip` (`host/package.mjs:16-29`). The reason: `bootstrap_script_from_template`
    also serves user SSH machines and `remote_bootstrap.ps1`, so a Windows "Upgrade" must
    not 404.
  - The release is created with `--latest=false` explicitly. It is never "latest", because
    Tauri's updater reads `releases/latest`.
  - The workflow refuses to run when the release already exists. Assets for a version are
    never re-uploaded, because the app pins their hashes.
  - It also writes `host-release.json` (version plus sha256 per target) as a workflow
    artifact, with a build-provenance attestation (`actions/attest-build-provenance`).
  - The tag is pushed by a script (`npm run host:release -- X.Y.Z`), never by hand,
    matching the native release convention.
  - Asset names keep `monocode-host` (CLAUDE.md: the CLI name is an identifier).
  - CLAUDE.md gets a "Releasing the Host" subsection. The after-merge checklist gets
    "bump `host/VERSION` when upstream Host changes were merged; gate any new upstream
    Host jobs".
- **The first fork release already carries the Host-side H2 changes**: the
  `describe.host` field and the flavor marker below. A Host that cannot identify itself is
  never published.
- **The Host version is decoupled from the app.**
  - New file `host/VERSION`. `host/build.mjs` injects it with an esbuild `define`.
  - `host/cli.ts` prints it for `--version`, instead of reading the root `package.json`.
    That file is `monocode-desktop`, used by `scripts/bump-version.mjs`, and stays
    untouched.
  - On the app side, a new `core/remote/host_release.rs` holds:
    - `HOST_VERSION`;
    - `HOST_RELEASE_BASE = https://github.com/hhoangg/monoptah/releases/download/host-v{HOST_VERSION}`;
    - `HOST_SHA256: [(target, hex)]`.
  - `bootstrap_script_from_template` uses these instead of `CARGO_PKG_VERSION` and the
    upstream URL.
  - The `scripts/check-fork-branding.sh` exception added in `c36ed8f6` is removed.
- **Flavor marker.** `--version` prints `X.Y.Z+monoptah` (semver build metadata).
  - The git tag and release name stay `host-vX.Y.Z`, without the `+`, which download URLs
    would have to percent-encode.
  - `HOST_VERSION` in the app holds the full `X.Y.Z+monoptah`.
  - The bootstrap treats any version **without** `+monoptah` as older than the pin, so an
    upstream Host is always replaced, whatever its number.
  - Version comparison ignores the build metadata.
- **Pinned checksums.** Today the checksum comes from the same release as the tarball, so
  it proves integrity, not authenticity. The app embeds the expected sha256 per target
  (`HOST_SHA256`), and the bootstrap gets it through a new `@@SHA256_<target>@@`
  substitution.
  - The Host is released before the app that pins it, so this has no chicken-and-egg
    problem.
  - A helper `scripts/pin-host-release.mjs` takes the hashes from the CI artifact
    `host-release.json` of the release run (`gh run download`) and verifies its
    attestation (`gh attestation verify`). It cross-checks them against the published
    `.sha256` assets, then writes `host_release.rs`. Reading only the release would be
    trust-on-first-use.

**H2 - The app knows which Host it talks to, and upgrades cloud Hosts.**

- `environment.describe` adds `host: {flavor: "monoptah", version: "X.Y.Z"}`.
  - An upstream Host has no `host` field, so the app treats it as `{flavor: "upstream"}`.
  - The field is additive, so `protocolVersion` stays 1.
- New `core/remote/host_info.rs`, a `HostInfo {flavor, version, capabilities}` cache keyed
  by machine id. It is filled on every successful connect/describe and dropped on
  disconnect. It replaces the ad-hoc probes:
  - `Fetcher::has_bytes` (`view.rs:367`);
  - the stored set in `gpui/conversation/remote.rs:586`;
  - `gpui/settings/pages/connections.rs:88`.

  Features 2-4 read capabilities from it.
- **Two-phase upgrade for cloud machines only.** A cloud reconnect runs the bootstrap over
  ssh *before* any tunnel or RPC exists (`core/remote.rs:607-636`). The script therefore
  cannot ask the running Host whether it is busy, and an old upstream Host has no CLI for
  it either. The flow is:
  1. **Phase 1:** the normal bootstrap, unchanged: start the installed Host, whatever its
     version.
  2. Open the tunnel, then call `environment.describe` and `sessions.list`.
  3. **Phase 2:** if the Host is older than the pin or not `+monoptah`, and no session on
     it is running, run the existing `upgrade_script` (`ssh.rs:767-775`) with the pinned
     version and hashes, then re-open the tunnel.
     - Codespaces: one extra `gh codespace ssh`, a few seconds.
     - Sandbox: already awake, so no extra wake.
     - The sandbox upgrade must go through the same token prelude as the connect step
       (`docker_start.rs:1486-1550`). The restarted Host needs `CLAUDE_CODE_OAUTH_TOKEN` in
       its environment.
  4. If a session is running, the upgrade is deferred: `pendingHostUpgrade` is set on the
     machine and retried at the next connect that finds the Host idle. The status line
     says "Monoptah Host X.Y.Z will be installed when the current turn ends".
  - User-owned SSH machines keep today's explicit "Upgrade" button: we do not silently
    upgrade a machine the user administers.
  - The copy "Update Monoptah Host in Connections settings"
    (`gpui/conversation/remote.rs:736-753`, `view.rs:496`) is wrong for cloud sessions. It
    becomes "Monoptah Host will be updated on the next connect", or the deferred
    message.
- **Rule for features:** gate on a capability string, never on a version comparison.
  Versions are only used to decide whether to upgrade.

**F1 - finish readBytes.**

Goal (user decision, 2026-11-10): **clicking a file always opens it directly, with no copy
step.** The fork ships its own Host precisely so that every cloud Host has
`files.readBytes`.

1. *(F1-1 dropped.)* A `read_binary_file` stopgap for upstream Hosts would be thrown away
   as soon as H2b lands, so it is not built.
2. **F1-2, send `cwd`.** Send the session's Host worktree cwd in `files.readBytes` and
   `files.read`, as `convert.rs:751-779` already resolves it.
3. **F1-3, switch the editor's remote binary reads to chunked `files.readBytes`.**
   `core/files/document.rs::read_remote_bytes` stops using the single 10 MB
   `read_binary_file` reply, which is bounded by the 16 MB RPC cap after base64. That
   lifts the cap for feature 2 and gives progress and cancel. It falls back to
   `read_binary_file` without the capability.
4. **F1-4, delete the copy workaround** (decided). This ships with H2b or right after it.
   - **Removed:** `CloudSessions::copy_cloud_file` (`view_cache.rs:405-495`),
     `ViewError::OldHost`, `Opened::CopyOnly` / `Outcome::CopyOnly`,
     `Phase::CopyOffer`, the `OUT_OF_DATE` text and the Download button
     (`gpui/shell/cloud_view.rs:187,400,483-505,710-725`).
   - **What a click does afterwards:**
     - Previewable files up to 5 MB open in memory at once.
     - Larger files stream through `files.readBytes` into `cloud-cache/` with a progress
       bar, then **open automatically**. That is the existing streaming path, with no
       copy step and no button.
     - Only files over 500 MB ask first (`CONFIRM_BYTES`).
   - **The rare Host without `files.readBytes`.** After H2b this only happens in two
     cases, and neither falls back to a copy:
     - **The upgrade is deferred because a turn is running.** The card says "Monoptah Host
       is updating after the current turn - the file opens then". The open is **queued**
       and runs by itself once the deferred upgrade finishes.
     - **The upgrade download is blocked** (a sandbox network policy blocking
       `github.com` / `objects.githubusercontent.com`). The card names the blocked host
       and the `sbx policy allow` fix. Nothing else is offered.
     - A later ticket can remove this case completely with alternative D (the app pushes
       the Host package over the tunnel), if it turns out to matter.
   - `CloudBackend::copy_from` stays: evidence pulls and cloud-to-local conversion use it,
     and they are not file viewing.

### 1.3 Alternatives considered

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| A. Fork-built Host, own tag (chosen) | Fork controls Host features; independent cadence; pinned checksums | New workflow and release step; fork must merge upstream Host fixes | **Chosen** |
| B. Keep upstream Host, design features without Host changes | No pipeline | Blocks readBytes, watching, kit flags, session import; workaround stays forever | Rejected |
| C. Offer `files.readBytes` upstream | One Host for everyone | CLAUDE.md: fork-local changes are never offered back; timing not ours | Rejected |
| D. App pushes the Host bundle over ssh stdin (no GitHub download) | Works when a sandbox network policy blocks GitHub releases | Bundles with Node are tens of MB per target, so the app grows a lot; slower connect | Keep as a later fallback for `deny-all` sandboxes; out of scope |
| E. Version the Host with the app (`CARGO_PKG_VERSION`) | One number | Every app release would need a Host release, and the 0.16.0 tag collides with upstream's and the fork's old one | Rejected |

### 1.4 Protocol and data changes

- `environment.describe` result adds `host: {flavor, version}`. This is additive.
- `remote_bootstrap.sh` adds `MONOCODE_HOST_UPGRADE_IF_OLDER` and per-target pinned
  sha256 substitutions.
- `remote-machines.json` machine entry adds `pendingHostUpgrade: bool`. This is additive
  and serde-defaulted.

### 1.5 UI changes

- Chat file card: no Download card any more. Large files show streaming progress and then
  open. A Host that cannot have `files.readBytes` yet gets the "opens after the current
  turn" or "update blocked by network policy" card (F1-4).
- Cloud panel status line: "Updating Monoptah Host…" during an upgrade, or the deferred
  message.
- Settings > Connections already shows an upgrade action for SSH machines. It now also
  shows the Host flavor and version.

### 1.6 Edge cases and failure modes

- **Network policy blocks the download.** A sandbox network policy (`balanced` or
  `deny-all`) can block `github.com` / `objects.githubusercontent.com`
  (`docs/docker-sandbox-spec.html` ~3283-3295).
  - On a **fresh** sandbox the bootstrap fails exactly as it does today. The error text
    names the blocked host.
  - On an **upgrade**, a failed download keeps the existing Host. It is not fatal: the app
    connects to the old Host and warns.
- **Codespace prebuilds or images that ship an old Host:** handled by the version compare.
- **Two app versions pinning different Host versions** (e.g. the dev build and the
  installed build) connect to the same codespace. Each would "upgrade" to its own pin and
  flap.
  - Rule: upgrade only when the installed version is **older** than the pin (semver
    compare, ignoring build metadata) **or** lacks the `+monoptah` marker.
  - A newer fork Host is used as is. It is compatible because features gate on
    capabilities.
- **Upgrade restart while the app is mid-request:** the app reconnects through the
  existing tunnel-invalidation path (`remote.rs:498-511`).

### 1.7 Security

- Pinned sha256 per target, compiled into the app, so the release is not trusted for its
  own checksum.
- Downloads are still HTTPS-only (`--proto '=https'`).
- `files.readBytes` already refuses `.git`, `O_NOFOLLOW`, and paths outside registered
  roots. No change.

### 1.8 Tests

- `host/` vitest: `--version` prints `host/VERSION`; `environment.describe` includes
  `host.flavor/version`.
- Rust:
  - `bootstrap_script_from_template` substitutes the fork URL and pinned hashes, with a
    snapshot test.
  - `host_info` parsing for upstream and fork describes.
  - `view.rs` three-step fallback with a fake `Host` (exists: `view_tests.rs`).
- Shell test of the bootstrap upgrade branch, using a fake `monocode-host` that prints
  versions: older triggers an upgrade, newer or equal does not, and a failed download
  keeps the old Host.
- `scripts/check-fork-branding.sh` passes without the `c36ed8f6` exception.

---

## 2. Feature 2: remote file tree, Git and editing

### 2.1 Current state

- **Explorer:** `gpui/explorer.rs` (port of FileTree.tsx).
  - Loads lazily, one folder per expand, via `files::list_dir` (`:421-440`). The listing
    cache is `dirs` (`:197`), and expanded/selected state is saved per root (`:195`).
  - Has no watcher of its own. It refreshes on `GitChanged` (`:376`), window focus
    (`:371`) and after its own file operations.
- **Changes panel:** `gpui/changes.rs` reads `ProjectGit.index`.
  - The model is `gpui/git_status.rs`. `ProjectGit::reload` (`:222-268`) calls
    `scm::diff_index` and `scm::file_diff_versions`.
  - Locally it uses a `notify` watcher (`gpui/fs_watch.rs`) with a 300 ms settle.
  - For a `remote://` root there is no watcher, so it **polls every 2 s** (`:15,167-174`),
    plus a 30 s repair read.
- **Editor:** `gpui/file_editor.rs` → `core/files/document.rs`.
  - Remote reads use `read_binary_file` (10 MB cap; 1 MB text cap through
    `document_from_bytes` `:125`).
  - Remote saves (`:302-320`) re-read the bytes, compare them with `document.disk`, then
    send `write_text_file`. Two separate RPCs leave a race window.
  - The Host's `files.write {path, expected, content}` (`host/workspace.ts:484-503`) already
    compares-then-writes on the Host. The editor does not use it.
- **Root binding:** `Shell::sync_git` (`gpui/shell.rs:1921-1950`) sets
  `ProjectGit::set_project` to the active session view's `settings.worktree_cwd` or
  `summary.cwd`. Explorer and Changes share that `Entity<ProjectGit>`
  (`gpui/right_panel.rs:289-297`).
- **Gate:** `gpui/git_gate.rs` (38 lines).
  - A thread-local holds the cloud key of the cloud session in view, set by
    `Shell::sync_cloud_panel` (`gpui/shell/cloud.rs:1133-1157`).
  - It disables every git write in Changes (`changes.rs:478,728,817`), Review
    (`review.rs:1300-1303,1400`) and editor hunks (`file_editor/diff.rs:82`).
  - It does not gate Explorer create/rename/delete or editor save.
  - Banner: "Local files - this session runs in {codespace|sandbox} {name}"
    (`shell/cloud.rs:620-626`).
- **The panels already show the remote workspace. The banner is wrong, and that is a live
  bug.** This was settled from the code during review; it is not yet confirmed in the
  running app.
  - The banner, the gate comment (`git_gate.rs:3-5`) and the sandbox spec
    (`docs/docker-sandbox-spec.html:1659,3598,3903`) all say the panels show the local
    checkout. They do not:
    - `Shell::add_loaded` (`gpui/shell.rs:1585`) and `ensure_session`
      (`gpui/shell/session_loading.rs:50-65`) create a `Conversation` for every shown
      session.
    - `sync_git` then binds `ProjectGit` to `worktree_cwd` or `summary.cwd`.
  - Both are `remote://` paths:
    - `summary.cwd` is the project key;
    - a Host worktree's `worktree_cwd` is built with `remote::remote_path`
      (`core/store/remote.rs:165`).
  - `working_copy()` is only the fallback when no session is active.
  - So today Files and Changes list the **remote** tree, polled every 2 s, under a banner
    saying "Local files".
  - `git_gate` blocks git writes to the **remote**. Meanwhile Explorer
    create/rename/delete and editor save **already write to the cloud workspace**, with no
    gate and the two-RPC conflict check only.
  - The existing test (`shell/cloud_tests.rs:1373-1396`) creates no `Conversation`, so it
    never covered this.
  - F2-0 confirms it in the running app and fixes the banner wording right away (ticket
    F2-0 below).
- **Host:** has no file or git watcher on main (no `fs.watch` in `host/*.ts`).
  `origin/codex/event-driven-git-refresh` has one (§0.1).

### 2.2 Proposed solution

**Bind panels explicitly.** Add a `PanelRoot` resolved in `Shell::sync_git`:

```rust
enum PanelRoot {
    Local { path: String },
    Cloud { root: String /* remote://env/path */, cloud: CloudRef, local_project: String },
    CloudPending { cloud: CloudRef },   // pending-* environment: workspace still starting
}
```

- For an active cloud session the default is `Cloud`, with root = the session's Host
  worktree cwd, else the project key. This is today's actual behaviour, now made explicit
  and tested.
  - The panel header gets a two-way switch **Cloud | Local** (decided). Local shows the local checkout, which is useful before converting back.
  - The choice is remembered per session in memory (default Cloud).
  - The banner text follows the mode: "Cloud files - codespace X" or "Local files - this
    session runs in…".
- `git_gate` becomes "writes refused when the panel root is `Local` and a cloud session is
  in view". That is today's protection, kept for the Local mode. In `Cloud` mode, writes go
  to the remote under the policy in §2.2.5.
- `CloudPending` shows a placeholder ("Workspace is starting") instead of a tree.

#### 2.2.1 Loading the tree

- Keep the lazy per-directory `list_dir`. The Host already filters with
  `git check-ignore` (`host/workspace.ts:117-154`) and confines paths to registered roots
  (`workspace-commands.ts:216-273`).
- **Large directories.** I am not sure whether `list_dir` has a cap; F2-0 measures it.
  - The design adds a per-folder cap of 5,000 entries with a "N more items…" row.
  - It is enforced client-side, and also Host-side as an optional `limit` argument when
    the capability `workspace.listDir.limit` is present.
- **First paint:** show the cached listing (§2.2.3) at once, then refresh. Only expanded
  folders are read.
- Quick Open and search already use `files.index` (20k cap) and `search_project`. Those
  are remote-capable through the same routing.

#### 2.2.2 Watching for changes

- **New Host capability `workspace.watch`.** It borrows the lease, batching and
  long-poll design of `host/git-watch.ts` on `origin/codex/event-driven-git-refresh`, but
  **not** its recursive watch.
  - That branch does `fs.watch(root, {recursive: true})`. On Linux, Node implements this
    with an inotify watch on every directory, `node_modules` included. Dropping ignored
    paths filters *events*, not *watches*. It can exhaust `fs.inotify.max_user_watches`
    and break the agent's own dev servers and test watchers.
  - Instead it uses **non-recursive** watches on:
    - the git metadata: `.git/HEAD`, `.git/index`, `.git/refs/**` (plus
      `packed-refs`), and the common dir for worktrees;
    - only the directories the Explorer has **loaded and expanded**. The client sends
      that list and updates it as folders open and close; it is capped at 256.
  - API:
    - `workspace.run watch_changes {cwd, dirs}` returns `{id}`;
    - `update_watch {id, dirs}`;
    - `wait_changes {id}` long-polls and returns
      `{git: bool, dirs: [relative dir], overflow: bool}`;
    - `unwatch_changes {id}`.
  - Kept from the branch: 100 ms batching, 60 s lease expiry, at most 128 watchers.
  - A change to a working-tree file in an unexpanded folder still changes `git status`.
    Without a recursive watch, that is caught by a slower 10 s `git_diff_index` poll that
    runs alongside the watch. It is still far cheaper than today's 2 s.
- **Timeouts.** Wait at most **15 s** per `wait_changes`. That is well below ureq's 30 s
  total, which is the real bound: Node's `requestTimeout` covers receiving the request,
  not the handler.
- **Client.** One watch per remote root, in `ProjectGit`. `git` triggers `reload`. `dirs`
  triggers an Explorer refresh of only those loaded folders (`overflow` refreshes all
  loaded ones).
- **Fallback without the capability (upstream Host):** today's 2 s git poll, plus a 5 s
  re-list of loaded folders like the web app (`FileTree.tsx:952-957`).
- **Cost control:**
  - Watching and polling run only while the right panel is visible and the window is
    focused. When unfocused, polling drops to 15 s; it stops when disconnected.
  - It stops after 10 minutes without user input in the window.
- **Not sure:** whether RPC traffic over the ssh tunnel counts as activity for the
  codespace idle timeout, or the sandbox `--ttl`. If it does, an open panel could keep a
  workspace alive, which costs money. The 10-minute input rule bounds that either way.
  F2-0 measures it.

#### 2.2.3 Caching and offline

- `ProjectGit` already keeps the last snapshot per project in memory (`set_project`
  shows it first). Extend it with an in-memory LRU of the last listing per remote root (16
  roots), so switching sessions is instant.
- **When the tunnel is down** (`is_not_connected`):
  - The panels show the cached tree and index **read-only and dimmed**, with a strip
    "Not connected - Connect".
  - Connect is a user action through `cloud_wake::reconnect` (sandbox:
    `reconnect_sandbox`). Nothing in the panels ever wakes a workspace by itself.
  - Opening a file while disconnected offers Connect, which is the same rule as chat links
    (`shell/cloud_view.rs:123-145`).
- No disk cache in v1. Remote file contents are never written to local disk except
  through the explicit download path (`cloud-cache/`).

#### 2.2.4 Viewing and editing files

- **Read:**
  - Text up to the local `MAX_TEXT_BYTES` (2 MB) is read with chunked `files.readBytes`
    (F1-3), with progress.
  - Text between 1 and 2 MB opens **read-only** ("Too large to save remotely"), because
    remote writes cap at 1 MB (`host/workspace.ts:27`, `workspace-commands.ts:345`).
  - Without the capability, `read_text_file` is used (1 MB).
  - Images and PDFs use the same path, up to 5 MB in memory.
  - Anything larger, or binary, gets "Too large to open here - Download" through the F1
    download path (`view_cache.rs`).
- **Save:** a conditional write in one Host call.
  - Today's `files.write` is keyed by `projectId` and `cwd`, while the editor works on
    `remote://` paths through `workspace.run`. So the condition goes on the path-based
    command instead: `write_text_file` gains an optional `expectedSha256` (capability
    `files.write.hash`).
  - Without the capability, today's re-read-and-compare in `save_document` stays.
  - The compare and the write happen in one Host call. The window is not zero (Node
    reads, then writes), but it is milliseconds, against two network round trips today.
  - The Host writes to a temp file in the same directory and renames it, so the agent
    never sees a half-written file. It keeps the file's mode and, when the path is a
    symlink inside the workspace, writes through to the target, as local `save_document`
    does.
- **Conflicts with the agent.**
  - With watching, a change to an open file sets `disk_changed`, so "File changed on disk.
    Reload…" already exists (`file_editor.rs:1387`).
  - A rejected save ("File changed on the host") opens a compare view (mine vs cloud) with
    **Keep mine (overwrite)**, **Take cloud** and **Cancel**.
  - While a turn is running, the editor shows "The agent is working in this workspace".
    Saving a file the agent touched in the current turn (from tool-call paths in the
    transcript) asks for confirmation.
- **Local-only actions** are disabled for remote roots. Today they return errors
  (`core/files.rs:436,471`):
  - Reveal in Finder, Open with default app and Open in external editor.
  - Codespaces gets "Open in browser (VS Code)" through `CloudBackend::web_url`.
- **Copy and move between local and remote** stay refused, as today
  (`files/remote.rs:135-145`).

#### 2.2.5 Git operations on the remote

| Operation | Policy in Cloud mode |
|---|---|
| status, diff, file diff, blame, history, branch list | Allowed (reads). |
| stage, unstage, hunk stage (`git_stage_contents`) | Allowed. |
| discard file / discard all | Allowed with the existing confirmation. Refused while a turn is running ("the agent may be editing these files"). |
| commit | Allowed. While a turn is running: confirmation ("the agent may commit or change the index at the same time"). Commit-message generation runs locally on the remote `staged_context`; this is acceptable because the diff stays with the same user. |
| push / pull / sync / create PR | Allowed as explicit user actions. They use the **remote's** credentials: the codespace's `GITHUB_TOKEN`, or the sandbox `git-credential-cache` from `docker_provision.sh`. A push failure shows the remote's error text (scrubbed by `sandboxes::redact`). |
| checkout / create branch / stash / worktrees | Allowed only when no turn is running, with a confirmation that says the agent's working directory changes. |

**Where "refused while a turn is running" is enforced.** `engine.withIdleProject`
(`host/engine.ts:300-320`) does not serialise anything. It **refuses** while any session
of the project is running.

- Through `workspace.run`, only `git_checkout` and `git_create_branch` use it
  (`workspace-commands.ts:780-797`). `git_commit`, `git_discard_*` and `git_stash` do
  not (`:165-208`).
- An app-side check against the session's running state is racy: a turn can start
  between the check and the RPC.
- So:
  - a Host change wraps `git_discard_*` and `git_stash` in the same idle guard
    (capability `git.idleGuard`, ticket F2-5h);
  - with an upstream Host, the app-side check is the best effort, and the UI says so.

**Push, pull and sync timeouts.** The Host's 30 s git timeout equals ureq's 30 s total,
so a slow push can be reported as failed after it succeeded. Fix:

- these three commands get a 120 s per-request client timeout (a `timeout` parameter on
  `rpc_with`);
- the Host's limit for them rises to 90 s;
- on a client timeout the UI says "Push may still be running - refresh to check" and
  re-reads ahead/behind, instead of "failed".

#### 2.2.6 Backends

The same Host protocol serves both backends. The differences are all in connect, wake
and paths:

- **Codespaces:** root `remote://<env>/workspaces/<repo>`. When the codespace idle-stops,
  the panels show Not connected.
- **Docker Sandbox:** the root is the container `remoteWorkspaceFolder` in devcontainer
  mode, or `$HOME/workspaces/<repo>` in Plain mode. Both are already resolved by
  `docker_start.rs`.
  - `--ttl` / `--on-timeout stop` show as Not connected.
  - The sandbox network policy does not matter here: all traffic is the RPC over the
    tunnel.

### 2.3 Alternatives considered

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| A. `remote://` root plus existing `workspace.run` commands (chosen) | Already implemented end to end in core; works with the upstream Host; parity with the web app | Polling until the Host watch ships | **Chosen** |
| B. Mirror the remote tree to a local folder (rsync/mutagen-like) and point panels at it | Panels unchanged; fast | Two-way sync conflicts with the agent; secrets and large repos copied to disk; heavy | Rejected |
| C. New streaming channel (WebSocket) for tree events | Push instead of long-poll | New transport and auth on both ends; long-poll gives the same latency at this scale | Rejected for now |
| D. Ship only read-only remote panels | Simpler, no conflict UI | The user asked for editing and Git | Rejected |

### 2.4 Protocol and data changes

- New `workspace.run` commands `watch_changes`, `update_watch`, `wait_changes` and
  `unwatch_changes`, capability `workspace.watch`.
- `list_dir` optional `limit`, capability `workspace.listDir.limit`.
- `write_text_file` optional `expectedSha256`, capability `files.write.hash`. Writes
  become temp-then-rename, keeping the mode.
- `git_discard_*` and `git_stash` run under the idle guard (capability
  `git.idleGuard`). Push/pull/sync Host timeouts go to 90 s.
- `rpc_with` gets an optional per-request timeout.
- `core/files/remote.rs::host_arguments` allowlist gets the new commands.
- `core/remote.rs` needs no new top-level method, because they go through `workspace.run`.

### 2.5 UI changes

- A Cloud | Local switch and a mode-aware banner over Files and Changes.
- A connection strip on both panels (Connected / Reconnecting / Not connected - Connect /
  Workspace starting).
- Dimmed read-only cached tree when disconnected.
- A conflict compare dialog on save.
- "Agent is working" notice in the editor and Changes during turns.
- Disabled local-only actions, plus "Open in browser" for codespaces.

### 2.6 Edge cases and failure modes

- Tunnel drops mid-save: the save reports an error and the buffer stays dirty. A retry
  re-sends with the same `expectedSha256`, so it is idempotent if the first write landed.
- The Host restarts (upgrade, crash): watch ids are lost, and `wait_changes` returns
  "unknown id". The client re-subscribes and does a full reload.
- Huge repos: lazy listing, per-folder caps and the 20k index cap. A `git_diff_index` on a
  very large working tree can take longer than the 30 s RPC timeout. Show "Git status timed
  out" and back off to 30 s.
- Files outside the workspace, symlinks and `.git` are refused by the Host's path
  confinement. Show the error inline.
- Session in a Host worktree: the root is the worktree, not `/workspaces/<repo>`.
- Several sessions on one instance (`attach_session`): each has its own root, and watches
  are per root, shared when equal.
- The workspace is deleted while the panel is open: the backend state becomes missing; the
  panel shows "Workspace deleted" and switches to Local.

### 2.7 Security

- All file access is confined by the Host to registered project roots and their worktrees
  (realpath checks). No new surface beyond the watch, which reports relative directory
  names only.
- No remote content is written to local disk except through the explicit download cache.
- Push uses the remote credentials. No local credential is sent.

### 2.8 Tests

- Rust:
  - `PanelRoot` resolution: cloud, pending, local switch, Mono active.
  - `git_gate` new semantics.
  - `save_document` conditional write and conflict, with a fake runner.
  - Watch client: re-subscribe on unknown id, fallback to polling without the capability,
    and stop when unfocused.
  - Extend `shell/cloud_tests.rs` to **create the Conversation entity** (the case today's
    test misses).
- `host/` vitest: watch lifecycle (port of `git-watch.test.ts`), `dirs` reporting,
  `list_dir` limit, and hash-conditional write with temp-rename.
- Manual: a codespace from the `mc/native-…` dev container branch, plus one sandbox.

---

## 3. Feature 3: take Claude Code skills, MCP servers and plugins to the cloud

### 3.1 Current state

- Nothing is copied today, except the auth token:
  - Codespaces: user secret `CLAUDE_CODE_OAUTH_TOKEN` through
    `gh secret set --app codespaces --user` (`core/codespaces.rs:16,1420-1455`).
  - Sandbox: env prelude on stdin (`core/cloud_sessions/docker_start.rs:195-268`).
- Claude Code on the instance is installed by the bootstrap
  (`curl https://claude.ai/install.sh | bash`, `remote_bootstrap.sh` ~85-97) and spawned
  by the Host through the shared TS provider.
  - `src/integrations/harness/providers/claude/claudeProtocol.ts:250-295` builds the args:
    `--setting-sources`, `--settings`, `--resume`, and so on.
  - It does not pass `--plugin-dir` or `--mcp-config` for interactive sessions.
- **Local discovery already exists**, written for local use:
  - `core/skills.rs::list_skills` (`:87`; scans `.claude/skills`, `.agents/skills`, user
    roots and plugin skills via `installed_plugins.json` `:234`);
  - `core/mcp.rs::mcp_discover` (`:432`; `~/.claude.json` user and local scope, project
    `.mcp.json`, plus other providers);
  - `core/mcp.rs::parse_claude_health` (`:970`).
- **Claude Code layout.** Verified with local `claude` 2.1.319 `--help` and the docs at
  code.claude.com.
  - Skills are `<root>/<name>/SKILL.md`. User root `~/.claude/skills`, project
    `.claude/skills`, and plugins carry `skills/`.
  - MCP:
    - local scope: `~/.claude.json` `projects.<path>.mcpServers`;
    - user scope: `~/.claude.json` `mcpServers`;
    - project scope: `.mcp.json`, which needs approval.
  - MCP configs support `${VAR}` / `${VAR:-default}` expansion in command, args, env, url
    and headers.
  - OAuth MCP tokens are stored separately, not in the config.
  - Plugins:
    - a plugin has `.claude-plugin/plugin.json` and is installed into
      `~/.claude/plugins/cache/...`;
    - `installed_plugins.json` (`version: 2`) holds **absolute** `installPath`s;
    - marketplaces are in `known_marketplaces.json`, enabled plugins in
      `settings.json enabledPlugins`;
    - `${CLAUDE_PLUGIN_ROOT}` is available inside plugins.
  - CLI flags: `--plugin-dir <path>` (repeatable, session-only), `--mcp-config <file|json>`
    (additive unless `--strict-mcp-config`), `--settings`, `CLAUDE_CONFIG_DIR`.
  - **Not sure** (`claude plugin --help` and `claude mcp add-json --help` printed "Not
    logged in" in this shell): the exact flags of `claude plugin install` and
    `marketplace add`.
- **Account config dirs.** Local sessions may run under a provider account with
  `CLAUDE_CONFIG_DIR=<data>/provider-accounts/claude/<id>` (`core/accounts.rs:95-160`).
  Such a directory has its own `.claude.json` and `plugins/` but no `skills/` or
  `settings.json`.
  - **Not sure** how Claude Code merges `~/.claude` and `CLAUDE_CONFIG_DIR` for skills and
    settings when both exist.
  - The design resolves the source as "the config dir of the account the session uses,
    then `~/.claude`". Ticket F3-1 verifies this with a real profile.
- **Docker Sandbox.**
  - Its network policy can block MCP URLs, `npx` registries and plugin marketplaces
    (`docs/docker-sandbox-spec.html` ~3283-3295).
  - `sbx` has its own `--static-mcp`, `sbx mcp` and `sbx skills` (experimental), all tied
    to Docker's MCP gateway and local skills store. **Not sure** they work on cloud
    sandboxes, so the design does not use them.

### 3.2 Proposed solution

#### 3.2.1 Inventory (local, pure, F3-1)

A new module `core/cloud_sessions/kit.rs` builds a `KitInventory` for (local project,
account). It reuses `skills.rs` and `mcp.rs` and classifies each item:

- **Skill** `{name, scope: user|project-untracked|plugin, path, size}`.
  - Project skills that are **committed** are skipped: they arrive with the clone. Checked
    with `git ls-files` on the local repo.
  - Only untracked or ignored project skills are offered.
- **MCP server** `{name, scope: user|local|project, transport, compat, secrets: [field]}`
  with `compat`:
  - `Ok`.
  - `LocalOnly(reason)`, in three cases:
    - stdio `command` is an absolute local path (`/Users/…`, `/opt/homebrew/…`,
      `/Applications/…`);
    - the URL host is `localhost`, `127.0.0.0/8`, `::1`, `*.local` or an RFC 1918 /
      link-local address;
    - stdio with a `docker` command on a sandbox.
  - `NeedsRuntime(npx|uvx|bunx|node|python|docker)`: checked remotely (§3.2.4).
  - `SignedIn`: an http/sse server with no auth header, for which Claude holds an OAuth
    sign-in locally. The sign-in is carried (§3.2.5).
  - `NotSignedIn`: the same, but with no local sign-in. It is carried without a token,
    and the panel says "Sign in locally first".
  - `Project`: project-scope `.mcp.json` servers already arrive with the clone. They are
    listed for information only.

  `secrets` holds **every** `env` value and every `headers` value, unless the value is
  exactly a `${VAR}` reference. URLs with a query string are treated as secret too. This
  is deliberately conservative: we cannot tell a token from a harmless value.
- **Plugin** `{id: name@marketplace, version, installPath, size, hasNativeBin}`.
  - Only plugins enabled in `settings.json enabledPlugins` are offered.
  - `hasNativeBin` is set when the plugin has `bin/` or `.lsp.json`: macOS binaries will
    not run on a Linux instance, so warn.

Size limits:

- At most 10 MB per skill and 25 MB per plugin, and a 50 MB total kit after compression.
- `.git` and `.DS_Store` are skipped.
- `node_modules` is skipped in skills but **kept in plugins**: some plugins ship bundled
  dependencies for their MCP servers.
  - A plugin with a native `node_modules` (`*.node` files) gets the same "may not run on
    Linux" warning as `bin/`.
- Symlinks are resolved (the local `~/.claude/skills` has some) but refused when they
  leave the user's home.

#### 3.2.2 Selection UI and one global kit (F3-2)

Decided: **one kit for all of the cloud**, the same on every codespace and sandbox.
Secrets and MCP sign-ins go along, so the user never has to sign in to an MCP server again
in the cloud.

- **Where it is set.** Settings > Cloud > "Claude setup", the single global selection.
  There are three tabs (Skills / MCP servers / Plugins), each with checkboxes, size and
  compat badges:
  - "Works in the cloud";
  - "Needs npx on the instance";
  - "Local only - will not work";
  - "Has secrets";
  - "Signed in (OAuth)".
  - Local-only items cannot be checked. Their reason is shown.
- **Defaults.** Every compatible item is checked, including servers with secrets and
  signed-in OAuth servers.
- **Project-scoped items follow their project automatically when the global switch is
  on:**
  - local-scope MCP servers (`projects.<path>.mcpServers`);
  - untracked project skills.
- **There is no per-project or per-session selection.** The start panel
  (`gpui/conversation/cloud_panel.rs`) and the codespace picker only show a summary row
  "Claude setup: 6 skills, 4 MCP servers, 3 plugins - Settings…" and an on/off switch for
  that one start.
- **One-time notice.** The first time the kit is enabled, the app shows the following, and
  the acknowledgement is stored in the global preferences:

  > "Your MCP secrets and sign-ins will be copied to your cloud workspaces. They will be
  > visible to the agent, to every MCP server and to every command the agent runs there."
- **The app API** (`sessions.start target`) gets an optional `kit: "none" | "default"`,
  default `"default"` = the global kit **including secrets**. This is the user's own
  standing choice in Settings, not consent claimed by the caller. An API caller can only
  turn the kit off, never widen it.

#### 3.2.3 Transfer and install (H3 + F3-3)

- **H3 - generic uploads on the Host.** New methods:
  - `uploads.begin {name, size, sha256}` returns `{uploadId}`;
  - `uploads.chunk {uploadId, offset, data(base64)}` with 512 KB chunks, like
    `attachments.upload` (`host/attachments.ts`);
  - `uploads.commit {uploadId}` verifies the sha256;
  - `uploads.discard`.

  Details:
  - Staging is under `~/.monocode-host/uploads/<id>/`, mode 0700.
  - Uploads are size-capped (500 MB) and expire after 1 h if not consumed.
  - The design is resumable: offsets are idempotent.

  The transfer runs over the existing tunnel, so it needs no extra ssh session (which on a
  sandbox would wake and bill) and works the same on both backends. Features 3 and 4 both
  use it.
- **F3-3 - the kit on the Host.** New method `kit.apply {uploadId, hash}` unpacks a tar
  into `~/.monoptah-kit/<hash>/` and points `~/.monoptah-kit/current` at it. The tar holds:
  - `skills/<name>/…`;
  - `plugins/<id>/…`, copied from the local `installPath`, pinned version;
  - `mcp.json` (`{mcpServers}` with secrets replaced by
    `${MONOPTAH_KIT_<SERVER>_<FIELD>}`);
  - `kit.json` (manifest).

  How each part reaches Claude:
  - **Plugins:** the Host's Claude spawn adds `--plugin-dir ~/.monoptah-kit/current/plugins/<id>`
    for each plugin. This is a documented, session-only flag. It does not touch the remote
    `~/.claude/plugins` or the internal `installed_plugins.json` format.
  - **MCP:** the spawn adds `--mcp-config ~/.monoptah-kit/current/mcp.json`, which is
    additive to the remote's own servers.
  - **Skills:** for each skill, a symlink `~/.claude/skills/<name>` → the kit, recorded in
    `kit.json` so a later apply removes only what the kit created. If the remote already
    has a real skill with that name, it is kept and the kit's copy is skipped with a
    warning.
    - This is persistent, instance-wide state, which fits the decided single global kit.
  - Plugin skills arrive with their plugin through `--plugin-dir`.

  The kit is gated on capability `claude.kit`. The spawn change is in
  `claudeProtocol.ts` (`buildClaudeArgs`) through a new `input.kit` resolved by the Host
  provider. Isolated helper spawns stay strict (`--strict-mcp-config`, no kit).
- **When changes take effect.** The Host keeps **one live Claude process per session**
  (`liveByThread`, `src/integrations/harness/providers/claude/claude.ts:424-438`). It
  respawns with `--resume` only when the cwd, `settingsKey` or planning changes. So:
  - the kit hash, and a hash of the secrets file, go into `settingsKey`, so an "Update
    Claude setup" respawns the process on the next turn;
  - the setup step finishes before the start reports `Ready`, so the first turn already
    sees the kit.
- **Credentials go one way, once, at the start of a new cloud workspace** (user
  decision, 2026-11-09). That covers the plain secrets (env and header values) and the MCP
  sign-ins (§3.2.5). They are not re-sent at reconnect and never come back.
  - `kit.secrets {values: {MONOPTAH_KIT_X_Y: "…"}}` is sent once in the "Claude setup"
    step. The Host writes the values to `~/.monoptah-kit/secrets.env`, mode 0600, written
    to a temp file and renamed. They survive Host restarts and codespace stop/start
    without the app.
  - The Host loads that file into the environment of the **Claude child processes only**.
    - Today the Host spawns children with `{...process.env}` (`host/child-backend.ts:358`),
      through the shared `spawnChild` (`src/integrations/harness/core/child.ts:350`, also
      used by the Tauri app).
    - So `spawnChild` gains an explicit `env` overlay parameter. The Host never puts
      secrets into its own `process.env`.
  - **What the agent can see.** `${VAR}` expansion needs the values in the Claude process
    environment. So every stdio MCP server, every Bash command the agent runs and
    `/proc/<pid>/environ` inherit **all** kit secrets, as does anyone who can read files
    in the workspace. The one-time notice says so (§3.2.2).
  - Host-side (TypeScript) logs and errors are scrubbed of the values. `Scrub` and
    `sandboxes::redact` cover only Rust.
  - Deleting the workspace removes them. Archiving with Keep leaves them, and the panel
    says "Credentials remain in this workspace".

#### 3.2.4 When it runs, refresh and health (F3-4)

- **New start step "Claude setup"** after `Connect` and before `Ready`, so the agent finds
  everything in place on its first turn.
  - It runs only when a session starts a **new** workspace. A session attached to an
    existing instance finds the kit already there.
  - It works for both backends' step lists (`core/cloud_sessions/startup.rs`,
    `docker_start.rs`):
  1. Probe the remote for runtimes with one `workspace.run`-free Host call
     `kit.probe {commands: ["npx","uvx","bunx","node","python3","docker"]}`, returning
     `{present: [...]}`.
  2. Build the tar for the selection, and drop items whose runtime is missing (with a
     warning).
  3. `uploads.*`, then `kit.apply`, then `kit.secrets`.
  4. Run a health check through the Host: `kit.health` runs `claude mcp list` in the
     project cwd with the kit flags. Parse it with the same `parse_claude_health` and show
     per-server status in the cloud panel ("clickup: connected", "playwright: failed - npx
     not found").
  5. The step **never fails the start**. Problems become warnings in the start log and
     the panel.
- **No automatic refresh.** A reconnect never re-syncs the kit or credentials.
  - The cloud panel has **"Update Claude setup"**, a user action that re-runs the step:
    the same one-way copy, credentials included.
  - `kit.json` stores the content hash, so the panel can say "Your local setup changed
    since this workspace started".
  - Changes take effect on the next turn: the kit hash is part of `settingsKey`, so the
    Host respawns the session's Claude process with `--resume` (§3.2.3).
- **Sandbox network policy.** When a server's URL or a runtime registry is blocked, the
  health check shows the failure. The panel suggests `sbx policy allow` for that host and
  notes that `--allow-network` can only be set when the sandbox is created
  (`docker-sandbox-spec.html` ~3312). The app never changes the user's policy.

#### 3.2.5 Carrying MCP sign-ins (OAuth) (F3-5)

Goal (user decision): an MCP server the user signed in to locally works in the cloud
without signing in again.

**What is known** (code.claude.com docs, `iam` and `mcp` pages):

- Claude Code keeps credentials in the macOS Keychain, in an entry keyed to
  `CLAUDE_CONFIG_DIR`. On Linux it keeps them in `~/.claude/.credentials.json` (mode 0600).
- MCP OAuth tokens are "stored securely and refreshed automatically". Their exact place and
  format are **not documented**: not sure they share the Keychain entry or the
  `.credentials.json` file.
- A documented bypass exists: `headersHelper`. When the helper prints an `Authorization`
  header, Claude Code uses it and does not fall back to OAuth. It re-runs the helper on a
  401/403 and retries once.

**Chosen approach: copy the sign-in to the remote credential store** ("full copy"):

1. A spike (F3-5a) runs with the user present, because reading the Keychain shows a macOS
   prompt. It reads the local credential entry for the session's account and prints
   **key names only**, to find where MCP OAuth entries live (expected: an `mcpOAuth`-like
   map keyed by server name and URL). It also checks the Linux layout with
   `claude mcp login` on a codespace.
2. Copying:
   - The app reads the local entries for the selected servers: Keychain through
     `security find-generic-password` with the account's `CLAUDE_CONFIG_DIR`, or the file.
   - It sends them once, in the "Claude setup" step of a new workspace, in `kit.secrets`
     under a separate `mcpSignIns` field.
   - The Host merges them into the remote `${CLAUDE_CONFIG_DIR:-~/.claude}/.credentials.json`:
     - only the MCP keys; Claude's own login there is never touched;
     - write to a temp file, then rename;
     - mode 0600;
     - the merged keys are recorded in `~/.monoptah-kit/signins.json`.
   - Remote Claude then refreshes the tokens on its own.
3. The kit removes the copied entries when the workspace is archived with Keep, and
   whenever a server leaves the kit. A deleted workspace takes them with it.

**Risk: refresh-token rotation.** Many OAuth servers rotate the refresh token on each
refresh. After the copy, local and cloud hold the same refresh token. The first side to
refresh invalidates it for the other, and that side has to sign in again.

- The one-way, start-only copy keeps this rare. It happens once per new workspace,
  usually within the token's lifetime.
- When it bites, the cloud health check reports "needs sign-in", and "Update Claude
  setup" copies the local sign-in again. The fix is still one way.
- **Not sure** which of the user's servers rotate. F3-5a lists them.

**If the spike finds the storage format unusable:** those servers are carried without a
sign-in and marked "Sign in in the cloud". The `headersHelper` broker considered earlier
needs the app to keep feeding tokens, which contradicts the one-way decision, so it is
dropped.

### 3.3 Alternatives considered

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| Plugins: copy pinned plugin dirs + `--plugin-dir` (chosen) | Exact local version; documented flag; no edits to Claude's internal files; works without marketplace network access | Kit size (local cache ~15 MB for 8 plugins); native binaries in plugins may not run | **Chosen** |
| Plugins: re-install from marketplaces on the remote (`claude plugin marketplace add` / `install`) | Small transfer | Version drift; needs GitHub and marketplace access (sandbox policy); CLI flags not verified here | Fallback idea, not v1 |
| Plugins: copy `~/.claude/plugins` and rewrite `installPath` in `installed_plugins.json` | No Host change | Edits an undocumented internal format (version 2) that Claude rewrites | Rejected |
| MCP: `claude mcp add-json -s user` on the remote | Works with any Host | Persists secrets in the remote `~/.claude.json` on disk; harder to remove cleanly | Rejected (secrets) |
| MCP: `--mcp-config` + one-way secrets file loaded into the Claude child env (chosen) | Survives Host restarts without the app; removable; additive; leaves `~/.claude.json` alone | Needs the fork Host; secrets on the workspace disk (0600) | **Chosen** |
| Secrets as Codespaces user secrets (`gh secret set --app codespaces`) | Survive restarts | Persist at GitHub; codespaces only; widen the blast radius to every codespace of the repos listed | Rejected for v1 |
| MCP sign-ins: user signs in again in the cloud (`claude mcp login` on the remote) | Nothing copied; no rotation race | The user explicitly does not want this; a headless remote may not complete the browser flow | Rejected (user decision) |
| MCP sign-ins: copy into the remote credential store (chosen) | Works offline from the laptop; remote refreshes itself | Undocumented format (spike); refresh-token rotation race | **Chosen**, pending F3-5a |
| MCP sign-ins: `headersHelper` broker fed by the app | Documented mechanism; refresh tokens stay local | Cloud depends on the app running; contradicts the one-way decision | Rejected |
| Transfer over `run_script` stdin (base64 tar) | Works with the old Host | A new ssh session per transfer (a sandbox wakes and bills); 50 MB base64 on stdin; not resumable | Rejected; uploads go over the tunnel |
| Docker `sbx mcp` / `sbx skills` | Native to sbx | Experimental; tied to Docker's gateway and local store; sandbox only; cloud support not verified | Rejected |
| Copy all of `~/.claude` | Trivial | Copies account state, history, caches and other projects' transcripts; secrets | Rejected |

### 3.4 Protocol and data changes

- **Host methods** `uploads.begin`, `uploads.chunk`, `uploads.commit` and
  `uploads.discard` (capability `uploads`).
- **Host methods** `kit.apply`, `kit.secrets`, `kit.probe` and `kit.health` (capability
  `claude.kit`), all added to `supported_remote_method`.
- The shared TS Claude args gain `input.kit {pluginDirs, mcpConfig}`. The Host provider
  fills it from `~/.monoptah-kit/current/kit.json`, and the child env gets the in-memory
  secrets.
- **Global preferences** get `cloudKit` (the single selection, the on/off switch and the
  notice acknowledgement). Project settings get nothing.
- **Start step list** gets a new `ClaudeSetup` step id. It is additive; old start records
  without it skip it.

### 3.5 UI changes

- A "Claude setup" row with its modal in the start panel and the codespace picker.
- An "Update Claude setup…" action in a running cloud session's panel.
- Per-server health in the cloud panel.
- Settings > Cloud: the global default.

### 3.6 Edge cases and failure modes

- **Old Host** without `claude.kit`: the row shows "Needs Monoptah Host X.Y.Z - will be
  installed on connect" (H2). Without the upgrade, nothing is transferred.
- **A skill name that collides** with a remote skill: the remote's is kept and a warning
  is shown.
- **Several sessions on one instance** (`attach_session`): they all get the same global
  kit. Project-scoped items (local-scope MCP servers, untracked project skills) are added
  for the projects open on that Host. A project-scoped MCP server whose name equals a
  user-scope one wins inside its project, as in Claude Code's own precedence.
- **The account's config dir differs** from `~/.claude` (§3.1, not sure): the inventory
  reads both and labels the source.
- **Project `.mcp.json` servers need interactive approval.** **Not sure** how headless
  stream-json Claude handles unapproved project servers. F3-4 health output shows it; a
  later ticket can add the setting that pre-approves them, if the user wants.
- **Plugins with hooks** run on the remote. That is expected. The modal says "Plugins can
  run hooks in the cloud workspace."
- **Kit too large:** items over the caps cannot be selected, with the size shown.

### 3.7 Security

- Secrets and MCP sign-ins are carried by default (user decision, 2026-11-09). This is
  covered by the one-time notice, which states that the agent, every MCP server and every
  command it runs can read them.
- Credentials travel one way, once per new workspace, only over the authenticated tunnel
  RPC.
- On the remote they are stored 0600:
  - plain secrets in `~/.monoptah-kit/secrets.env`;
  - sign-ins in Claude's `.credentials.json`.
- They reach only the Claude child env and the Claude credential store, and are scrubbed
  from logs and start output.
- An API caller can only turn the kit off, never widen it.
- Symlink resolution is bounded to the home folder.
- The tar is unpacked by the Host in TypeScript, **not** by GNU `tar`, which follows
  symlinks already at the destination. The same rules as Rust's `unpack`
  (`backend/docker.rs:527`) are reimplemented:
  - no `..` and no absolute paths;
  - regular files and directories only, no links;
  - extraction into a fresh directory that is created with `O_EXCL` and never reused.
- **Every new Host input is validated:**
  - `uploads.begin.name` is one plain file name;
  - the `kit.apply` hash is 64 hex characters;
  - skill names follow `skills::is_valid_skill_name`;
  - plugin ids and MCP server names match `[A-Za-z0-9._@-]+`.
- `kit.apply` writes only under `~/.monoptah-kit` and `~/.claude/skills/<managed name>`.

### 3.8 Tests

- Rust, `kit.rs`:
  - classification table tests: absolute command, `npx`, localhost/private URLs, OAuth,
    secret fields, a `${VAR}`-only value, committed vs untracked project skills;
  - size caps and symlink rules;
  - tar building is deterministic, so equal selections produce an equal hash;
  - a fixture `.claude.json` / `installed_plugins.json` (no real secrets).
- `host/` vitest:
  - uploads: chunk, resume, hash mismatch, expiry, caps;
  - `kit.apply`: path validation, managed symlinks, collision keep;
  - the spawn args include `--plugin-dir` and `--mcp-config` for interactive spawns but
    not isolated ones;
  - secrets reach the child env, not the Host's own `process.env`, logs or disk.
- GPUI: modal state, consent required before secrets are included, the remembered
  default.

---

## 4. Feature 4: move a session between local and cloud (one session, one id)

### 4.1 Current state

- **Only cloud to local exists:** `core/cloud_sessions/convert.rs` (1971 lines; entry
  points `convert_session` `:619` and `convert_session_confirmed` `:635`). It is reached
  from the cloud archive dialog (`gpui/shell/cloud.rs:1837-2217`).
  1. It wakes the workspace.
  2. `SNAPSHOT_SCRIPT` (`:91-175`) on the remote stages:
     - the Claude JSONL;
     - a `git bundle` of unpushed commits (`--not --remotes=origin`);
     - a `format-patch` series;
     - a binary patch of uncommitted and untracked changes through a temporary
       `GIT_INDEX_FILE` (the real index is untouched);
     - the list of ignored files;
     - a size check against a 500 MB limit.
  3. It copies those back and verifies them locally (`check_restorable` `:1404-1503`).
  4. It creates an **archived** local session with the rows, installs the JSONL with the
     cwd rewritten (`install_jsonl` `:1190-1226`, `rewrite_cwd` `:289`) so
     `claude --resume` works, tombstones the cloud session and forgets it.
  5. Code restore is a later opt-in (`restore_code` `:1707`) into a new worktree.
- **Local to cloud: not found.** "Run on cloud" in the composer appears only for a session
  with **no messages** (`gpui/conversation/cloud_target.rs:256-258`).
- **Start paths:**
  - Codespaces are created from a **GitHub** repo and branch (`core/codespaces.rs:649-710`,
    `gh api POST repos/{repo}/codespaces ref=<branch>`), so the branch must exist on
    GitHub.
  - Sandboxes `git clone` the `repo_url` and `checkout -b <feature> origin/<base>`
    (`backend/docker_provision.sh` clone phase).
  - Neither carries local unpushed commits or dirty files.
- **The Host cannot adopt an existing provider session.**
  - `create` (`host/engine.ts:82-110,427-452`) always starts empty, and
    `sessions.update` patches metadata only.
  - The Host resumes with `--resume` only for sessions it created
    (`engine.ts:282-286,850`).
- **Reusable pieces:**
  - git: `git::unpushed`, `session_worktree::inspect`, `scm::push_for`, and the bundle and
    patch logic in `convert.rs`;
  - transcript copy: `core/accounts/transfer.rs:31-54` copies `<id>.jsonl`,
    `projects/<cwd>/<id>/` and `file-history/<id>/`, the JSONL last;
  - the handoff brief (`Settings.handoff`, `core/agent.rs:192`; `wrap_handoff_prompt`);
  - start records with Retry (`core/cloud_sessions/startup.rs`).

### 4.2 Proposed solution

These user decisions (2026-11-09) shape this section:

- **One session, not two.** Moving to the cloud and back keeps **one** session in the
  sidebar with **one** history.
- **The same harness session id on both sides.**
  - Local → cloud: the cloud Claude session uses the **local** id.
  - Cloud → local: the local session uses the **cloud** id. `convert.rs` already does this
    through `provider_id`.
- **Commits travel by pushing the branch.** `.env.local` and every other ignored file
  stay local.

#### 4.2.1 One session with a location

A local session (desktop DB, `core/store.rs`) gains a **location**, stored as a session
setting `cloud-location`:

```text
Local                                            (default, today)
Cloud { cloud: CloudRef, environment_id, project_id, host_session_id,
        remote_cwd, local_cwd, moved_at_row, pushed_branch }
```

- **While the location is Cloud:**
  - The session's `Conversation` uses the **remote runtime** (`gpui/runtime/remote.rs`)
    against `host_session_id`, so the composer sends go to the cloud.
  - Rows from the Host are **written into the same local DB session** after the local
    rows. Today the remote runtime projects Host blocks into rows
    (`project_remote_history`) and keeps them only in memory (`core/store/remote.rs:1-3`).
    Here the new suffix is appended to the DB instead, so:
    - the history stays one list;
    - global search (`core/search/index.rs`, desktop DB only) also finds the cloud turns;
    - moving back needs no transcript merge.
  - The backing `RemoteSession` record in `remote-sessions.v1` still exists as plumbing,
    but carries `linked_to: <local session id>`. The sidebar hides it.
  - The cloud badge, cloud panel, wake and stop logic key off the location instead of the
    RemoteSession.
  - The local provider process for the session is closed when it moves. There is never a
    local Claude and a cloud Claude on the same id at once.
- **Sessions born in the cloud** (today's `RemoteSession`s) keep working unchanged.
  - Their "Continue locally" becomes the existing `convert.rs` flow with two changes: the
    new local session is **not archived**, and it **replaces** the cloud row in the
    sidebar (`adopt_converted`, `gpui/shell/cloud.rs:1703`, already swaps rows).
  - The harness id is already the cloud's.
  - From then on it is a local session with a location, so it can move again.

#### 4.2.2 Local → cloud ("Continue in cloud")

**Entry points:**

- Session menu "Continue in cloud…".
- The composer Workspace menu for sessions with messages. Today's "Run on cloud" there
  is limited to empty sessions (`cloud_target.rs:256-258`).
- App API `sessions.moveToCloud {sessionId, target}`. It uses the same push rules: the
  push is the user's standing policy, and a caller cannot change it.

All of them are refused while the session has a running turn.

**Dialog** (one confirmation, no extra checkboxes, because pushing is the decided
default):

1. **Target:** backend, new or existing compatible instance (same repo, Host ready), and
   machine options. It reuses `cloud_panel.rs`, with the global Claude setup row from
   feature 3.
2. **"What will move".** A read-only inspection of the session's working copy
   (`session_working_copy`):
   - "Push N commits to `origin/<branch>`";
   - "M changed and K new files go along uncommitted";
   - "Ignored files stay local: .env.local, …".
3. **Conversation:** "continues in the cloud with the same Claude session". For a
   non-Claude session: "the cloud runs Claude; it gets a summary of this conversation and
   a new Claude session" (cloud is Claude-only, `require_claude`,
   `core/cloud_sessions.rs:512`).

**Which branch is pushed.** The rules are fixed and there is never a force push:

- The local branch has an upstream on origin, and HEAD is ahead of it (fast-forward):
  push to that upstream.
- It has no upstream, and the name does not exist on origin: push to the same name and
  set the upstream.
- The remote branch exists and has diverged, or the local branch is origin's **default
  branch**: push to a new branch `<branch>-cloud` (`-cloud-2`, …), never to the default
  branch.
- Nothing to push (HEAD is already on origin): no push. The workspace starts on that
  branch.
- Detached HEAD: refused, the same as `create_cloud` today (`cloud_target.rs:589-701`).

**Steps.** A persisted `MoveRecord` (setting `cloud-move:<session id>`, like
`StartRecord`) makes every step idempotent and resumable after an app restart:

1. **Snapshot (local, read-only)** in a new `core/cloud_sessions/move_out.rs`:
   - `git fetch origin` (60 s, `GIT_TERMINAL_PROMPT=0`);
   - record `head`, `branch` and the push target from the rules above;
   - a binary patch of uncommitted and untracked, **non-ignored** files, made through a
     temporary `GIT_INDEX_FILE` (the same technique as `SNAPSHOT_SCRIPT`,
     `convert.rs:148-154`);
   - `tree`: the `git write-tree` of that temporary index after `add -A`;
   - the transcript files: `<id>.jsonl`, `projects/<cwd>/<id>/` and `file-history/<id>/`,
     as `core/accounts/transfer.rs:31-54`, from the session's account config dir;
   - the size check (500 MB for the patch, 50 MB for the JSONL).

   The user's working tree, index, branches and stash are never modified. Staging goes to
   `<db>.threads/<session>/cloud-move/`.
2. **Push** `HEAD:refs/heads/<target>` through `scm::push_for`. `pushed_branch` records
   whether the move created the branch, so that a cancel can offer to delete it.
3. **Workspace.**
   - Codespaces: `begin_start` with `ref=<target>`, or `attach_session` on an existing
     instance.
   - Sandbox: `begin_sandbox_start` with base ref = `<target>`. Today's provision does
     `checkout -b <feature> origin/<base>` (`docker_provision.sh` clone phase). It gains
     "feature == base: check out the tracking branch".
   - On an **existing** instance, the work happens in a new Host worktree on `<target>`
     (`git.worktreeCreate`), so other sessions' trees are untouched.
   - The start's own steps (create, provision, connect, Claude setup) run as today.
4. **Upload** the patch and the transcript files with H3 `uploads.*`. When the tree is
   clean and the session is non-Claude, there is nothing to upload.
5. **Restore code** with a new Host method `workspace.applySnapshot {uploadId?, head, tree, target}`:
   - it runs under `withIdleProject`;
   - it verifies `HEAD == head`;
   - it runs `git apply --binary` on the patch (all changes become unstaged working-tree
     changes; the dialog says so);
   - it verifies the `write-tree` of a temporary index after `add -A` equals `tree`. Tree
     ids are content-addressed, so they compare equal across git versions and diff
     configurations, unlike diff text;
   - on mismatch it stops and leaves the remote tree for inspection;
   - inputs are validated: hex ids, the patch stays inside the target and never touches
     `.git/`.
6. **Restore the conversation with the same id.** A new Host method
   `sessions.import {projectId, cwd, providerSessionId, title, model, uploadId}`:
   - It installs the JSONL and its folders under
     `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/<encoded remote cwd>/`.
   - The cwd fields are rewritten local → remote. The app does this before upload, with
     `rewrite_cwd` (`convert.rs:289`) generalised to both directions.
   - It writes the Host session record with `providerSessionId = <local id>` and calls
     `provider.bind` (`host/engine.ts:282-286`, `claude.ts:384-396`).
   - The first cloud turn then runs `claude --resume <local id>`.
   - `claude --help` documents `--fork-session` as "When resuming, create a new session
     ID". So a plain `--resume` **keeps** the id and appends to the same JSONL. The F4-0
     spike confirms this with the installed version before F4-2h is written.
   - A non-Claude session gets a fresh Claude session with `Settings.handoff` set to a
     brief built from its rows (`gpui/conversation.rs:2468-2475`).
7. **Flip the location.** In one local store transaction:
   - set `cloud-location` to Cloud;
   - record the `RemoteSession` with `linked_to`;
   - clear the `MoveRecord`.

   Before this step the local session is untouched and usable. The step first checks that
   the session's last row id and HEAD still match the snapshot. If they do not: "Your
   local session changed after the move started - move again / cancel".

**Failure and rollback:**

- **Before step 3:** cancel deletes the staging folder. The pushed branch stays (pushing
  is not destructive). If the move created it, the user is offered to delete it.
- **Steps 3-6 failing:** the session shows "Moving to the cloud failed at <step>" with
  **Retry**, which continues at that step, and **Cancel**. Cancel:
  1. discards uploads;
  2. offers to delete a workspace the move created (`created_note_for`);
  3. offers to delete a branch the move created;
  4. leaves the session Local.

#### 4.2.3 Cloud → local for a moved session ("Continue locally")

- The existing `SNAPSHOT_SCRIPT` and `check_restorable` run unchanged (bundle, patch, the
  JSONL with the same id).
- **Where the code lands.** If the local working copy still has HEAD and `tree` from the
  move snapshot (nothing changed locally since), the cloud's work is applied **in place**:
  - fetch from the bundle, or the pushed branch;
  - fast-forward the local branch;
  - `git apply --binary` the uncommitted patch.

  Otherwise it goes to a new worktree, as `restore_code` does today (`convert.rs:1707`).
  The user's changed tree is never overwritten.
- `install_jsonl` puts `<id>.jsonl` back. It is the same id, so the local `--resume <id>`
  continues the same conversation.
- The rows are already in the local DB (§4.2.1). The location flips back to Local, and
  the Host session is tombstoned as `convert.rs` does today.

### 4.3 Alternatives considered

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| Commits: push the branch (chosen) | Codespaces need the branch on GitHub anyway; sandboxes clone origin; simple; no Host change for commits | Publishes the commits; may start CI | **Chosen** (user decision) |
| Commits: private bundle via Host uploads | Nothing published | More Host code; base must be on origin; user preferred push | Rejected |
| Uncommitted changes: patch over the tunnel (chosen) | Never published; exact (tree id verified) | Needs H3 + `applySnapshot` | **Chosen** |
| Uncommitted changes: push a WIP commit | No upload path | Publishes unfinished work; pollutes history | Rejected |
| Identity: one session with a location (chosen) | One sidebar entry, one history, same harness id; search covers cloud turns | Store and runtime change (L) | **Chosen** (user decision) |
| Identity: archive local, new linked cloud session | Little store change | Two sessions; user rejected | Rejected |
| History: write Host rows into the local DB (chosen) | One list; trivial move back | Rows duplicated on the Host; a row-append path for remote runtime | **Chosen** |
| Same id: `--resume <id>` (chosen) vs `--session-id <id>` on a fresh JSONL | Resume keeps context and id (`--fork-session` is opt-in) | Depends on Claude CLI behaviour (spike F4-0) | **Chosen** |

### 4.4 Protocol and data changes

- **Host:**
  - `workspace.applySnapshot` (capability `workspace.applySnapshot`);
  - `sessions.import` (capability `sessions.import`);
  - uploads from H3.
- **Store:**
  - session setting `cloud-location`;
  - `MoveRecord` (`cloud-move:<id>`);
  - `RemoteSession.linked_to`;
  - the remote runtime appends projected rows to the linked local session.
- **App API:** `sessions.moveToCloud` and `sessions.moveToLocal`, added to the action
  allowlist in `core/app_access.rs:21-50`.
- **Sandbox provision:** feature == base branch support.

### 4.5 UI changes

- "Continue in cloud…" and "Continue locally…" in the session menu and composer menu.
- The one-confirmation move dialog, and progress through the existing start step list,
  with extra steps: Snapshot, Push, Upload, Restore code, Restore conversation.
- A cloud badge and the cloud panel on the **same** sidebar entry while its location is
  Cloud.
- A thin divider in the transcript where the session moved ("Moved to codespace X -
  <date>", "Back on this Mac - <date>").

### 4.6 Edge cases and failure modes

- **No `origin`, or not on GitHub (Codespaces):** refused with a clear reason.
- **Push rejected** (no write access, protected name, hook): the move stops at step 2 with
  git's message. Nothing else changed.
- **A local session in a session worktree** (`session_worktree`): the snapshot and push
  come from that worktree's branch.
- **LFS objects:** pushed commits carry LFS pointers; LFS content must be pushed to the
  LFS remote too. **Not sure** `git push` does that in every setup (it does with
  git-lfs's pre-push hook installed). v1 detects `filter=lfs` and warns.
- **Submodules:** a dirty submodule is not carried. Warn.
- **The local session is non-Claude:** summary only, a new Claude id. The dialog says so.
- **A large JSONL (>50 MB):** summary, with a note.
- **Account mismatch** (local account A, cloud token for account B): resume still works,
  as the local cross-account transfer shows. Say "the cloud workspace uses account B".
- **Network policy blocks the git host on a sandbox:** the start fails at provision, after
  the push. The pushed branch is fine; Retry or Cancel.
- **App quit mid-move:** the `MoveRecord` resumes it at the next launch, like
  `pending_starts`.
- **Same session open in two windows:** the location is store state, and every view
  follows it on the next refresh.
- **The workspace is deleted while the location is Cloud:**
  - the session shows "Cloud workspace deleted";
  - it offers "Continue locally from the last pushed state": fetch `pushed_branch` and
    keep the rows, which are already local;
  - the JSONL of turns after the move is lost with the workspace. The next local turn
    gets a summary of those rows.

### 4.7 Security

- Pushing follows the user's standing decision. The dialog always names the remote branch
  and the commit count, and never pushes to the default branch or with force.
- The patch and transcript travel only over the tunnel to the user's own workspace.
- The dialog states that the conversation (which may contain pasted secrets) and the code
  will be stored in the cloud workspace.
- Ignored files are never carried. `.env`-like names are listed.
- The Host validates upload ids, applies patches only inside the target, and refuses
  patches that touch `.git/`.
- The installed JSONL path is built from a validated provider id: alphanumerics, `-` and
  `_`, the same rule as `convert.rs:792-802`.

### 4.8 Tests

- Rust, `move_out.rs`, in temporary git repos:
  - the branch rules (upstream ahead, no upstream, diverged, default branch, nothing to
    push);
  - clean, dirty, untracked, binary, ignored;
  - a session worktree;
  - the user's index, stash and branches are unchanged;
  - deterministic `tree`.
- Round trip, local → cloud → local, with the Host-side logic and `SNAPSHOT_SCRIPT` in
  temporary repos:
  - equal trees;
  - the same provider id throughout;
  - in-place apply when nothing changed locally, a new worktree when something did.
- `tree` equality across git configurations: `diff.noprefix`, `core.autocrlf=input`.
- `host/` vitest:
  - `applySnapshot`: verify-mismatch stops, patch confinement, worktree target;
  - `sessions.import`: JSONL path, cwd rewrite, the first turn passes `--resume <same id>`
    and no `--fork-session`.
- Store:
  - `cloud-location` flips;
  - remote rows are appended once (no duplicates on re-sync);
  - the linked RemoteSession is hidden from the sidebar.
- GPUI:
  - one sidebar entry through local → cloud → local;
  - refusal while busy;
  - `MoveRecord` resume after a restart.


### 4.9 Risks of sharing one harness session id across machines

The decided rule is:

- a session started in the cloud brings the **cloud** id back to local;
- a session started locally takes the **local** id up to the cloud.

The id is a Claude Code session UUID, so collisions with *other* sessions are not a
concern. Every risk below comes from **the same id existing as two files on two machines
that can each grow**.

| # | Risk | Likelihood | Impact | Mitigation (in which ticket) |
|---|---|---|---|---|
| R1 | **Silent loss on return, certain with today's code.** `install_jsonl` never overwrites: if `<id>.jsonl` already exists locally it returns `AlreadyThere` and keeps the old file (`core/cloud_sessions/convert.rs:1190-1226`, `:1208`). In a local → cloud → local round trip the local file is the pre-move one, so the cloud turns would be missing from Claude's context even though the app shows them. | Certain | High | Replace when the incoming transcript **extends** the existing one (R3 rule), keeping the old file as `<id>.jsonl.pre-cloud`; same rule in the Host's `sessions.import` for a second move to the same instance (F4-J, F4-2h) |
| R2 | **Split brain: both sides write the same id.** Ways it can happen: the user runs `claude -r <id>` in a terminal, or a TUI launch (`core/tui_launch.rs`) runs while the location is Cloud; an old Host session is still bound to the id after a return; a crash between the flip and the tombstone. The two JSONLs then diverge. | Low-medium | High (one side's context is lost) | **Single writer by location.** While Cloud, the app refuses the local runtime and the TUI launch for that session. On return, the Host session is tombstoned before the location flips. `sessions.import` refuses when a live Host session already binds that `providerSessionId`. **Divergence is never overwritten**: the incoming copy is installed under a **new** id (fork), the user is told "the cloud and local conversations diverged", and the session continues on the fork. That is the one case where the ids stop matching (F4-J, F4-L, F4-2h) |
| R3 | **Byte comparison is useless** because the cwd rewrite changes lines in both directions. | Certain | Medium (wrong "diverged" or "extends" verdicts) | Compare the sequence of entry `uuid`s instead of bytes: "extends" means the existing uuid chain is a prefix of the incoming one (F4-J) |
| R4 | **Claude Code version skew.** The cloud gets the latest Claude through `install.sh` (`remote_bootstrap.sh`); local may be older or newer. A JSONL written by a newer Claude may not resume on an older one. | Medium | Medium (resume fails) | Each JSONL entry records the writer's `version`. If the target Claude is older than the newest writer: on the cloud, the setup step runs `claude update`; locally, warn and offer to update. If resume still fails: fall back to a summary handoff on a new id, and say so (F4-3a, F4-5) |
| R5 | **Paths of the other machine inside the transcript.** Tool calls and results mention `/Users/<user>/…` on the cloud, or `/workspaces/…` back home, and Claude may try to use them. | High | Low | The cwd fields are rewritten. The first turn after a move gets a one-line prepended note: "This session moved from <old cwd> to <new cwd>; paths in earlier messages refer to the old location" (F4-3a, F4-5) |
| R6 | **Rewind and checkpoints across the move.** Claude's `file-history/<id>/` and the app's own checkpoints (`core/checkpoint`, git-backed, local refs) describe files on the other machine. Rewinding to a turn before the move would restore the wrong paths, or nothing. | Medium | Medium | `file-history` is copied for continuity, as `accounts/transfer.rs` does. The app's rewind and checkpoint actions are disabled for turns before the latest move point, with an explanation (F4-L) |
| R7 | **The workspace is deleted while the location is Cloud.** The JSONL holding the turns after the move is gone. The local DB rows survive (§4.2.1). Resuming the old local `<id>.jsonl` would silently miss those turns. | Low | Medium | Never resume the stale local file in this case. Continue on a **new** id with a summary of the rows after the move point, and tell the user (F4-5) |
| R8 | **Cleanup touches the wrong copy.** `core/provider_cleanup.rs` deletes a deleted session's provider files in every local account home. | Low | Low | It runs only on delete, never on a move. A delete while the location is Cloud also tells the user that the workspace copy stays until the workspace is deleted (F4-L) |
| R9 | **Unknown server-side coupling to the id** (for example, features tied to claude.ai sessions). Resume is believed to send the history from the local file, so the id is only a file name. **Not sure.** | Low | Low | F4-0 spike: resume the same id on two machines and watch for errors |

**Verdict:** feasible, with **medium** residual risk once R1-R4 are handled.

- R1 is a certain bug in today's convert code that the round trip would expose. It must
  be fixed before F4 ships.
- R2's worst case (divergence) is designed to fail safe, by forking to a new id instead
  of overwriting. The one-id rule therefore holds in every normal flow and is knowingly
  broken only to avoid losing context.
- The shared logic (uuid-chain compare, extend / replace / fork decision, backups) lives
  in one pure module, `core/cloud_sessions/transcript_sync.rs`. The app uses it in both
  directions, and the Host mirrors its rule in TypeScript, with shared test fixtures.

---

## 5. Tickets

Sizes: S ≈ ≤1 day, M ≈ 2-4 days, L ≈ 1-2 weeks. Each ticket gets its own worktree and PR.

**Host work reaches users only through a Host release that an app release then pins.** A
Host-side ticket on its own changes nothing for users. So Host changes are batched, and
each batch ends with an explicit release-and-pin ticket (HR1-HR3).

### Wave 1: no Host dependency, all in parallel

| ID | Ticket | Size | Depends on |
|---|---|---|---|
| **F2-0** | **Hotfix.** Confirm in the running app that Files/Changes show the remote tree for cloud sessions. Correct the banner ("Cloud files - codespace X") and the `git_gate` comment. Add a test that creates the `Conversation` entity. Check `list_dir` caps and whether tunnel traffic resets idle timers; write the results into this doc. | S | - |
| **F1-2** | Send the Host worktree `cwd` in `files.readBytes`/`files.read` | S | - |
| **F1-3** | Editor: chunked `files.readBytes` for remote reads, with progress and cancel; falls back to `read_binary_file` | S-M | - |
| **H1** | Fork Host release pipeline. Covers: `host/VERSION`; the `+monoptah` marker; `describe.host`; `host-release-monoptah.yml` (all targets incl. Windows `.zip`, `--latest=false`, refuses existing releases, `host-release.json` artifact + attestation); `npm run host:release`; the "Releasing the Host" section and after-merge checklist in CLAUDE.md | M | - |
| **F3-1** | `kit.rs` inventory and classification (skills/MCP/plugins, compat, secrets, sizes, account config dir) | M | - |
| **F3-5a** | **Spike, with the user present** (a Keychain prompt appears): where and how Claude Code stores MCP OAuth sign-ins on macOS (key names only) and on Linux; which servers rotate refresh tokens (§3.2.5) | S | - |
| **F4-0** | Spike: `claude --resume <id>` on another machine, after the JSONL is installed with the cwd rewritten, keeps the id and appends to the same file; check the row-append path of the remote runtime | S | - |
| **F4-J** | `transcript_sync.rs`: uuid-chain compare, extend/replace/fork decision with `.pre-cloud` backup, cwd rewrite both ways, shared JSON fixtures for the Host mirror; fixes R1 in `install_jsonl` (§4.9) | M | - |
| **F4-1** | `move_out.rs`: branch/push-target rules, the patch of uncommitted non-ignored files, `tree` id, transcript collection; never touches the user's tree, index or stash | M | - |
| **A1** | Not a cloud feature, added by the user on 2026-11-10. A Mono can start a session on a chosen provider account: `sessions.start` `account`, a read-only `accounts.list`, `@<provider>/<label>` mentions in Mono composers, **local sessions only** (`account` with a cloud `target` is a validation error; cloud sessions keep the cloud setup). Specified in full in `docs/cloud-session-improvements-impl.md` | M | `origin/main` merged (PR #62) |

### Wave 2: first fork Host; remote panels, global kit UI and the session location

| ID | Ticket | Size | Depends on |
|---|---|---|---|
| **HR1** | Cut tag `host-v1.0.0` (Host version `1.0.0+monoptah`), with only H1's changes | S | H1 |
| **H2a** | App pins the fork Host. Covers: `host_release.rs` + `scripts/pin-host-release.mjs`; the bootstrap URL and pinned hashes; the flavor-aware version compare; the `HostInfo` cache; removing the `c36ed8f6` branding exception | M | HR1 |
| **H2b** | Automatic two-phase cloud upgrade with no prompt. Covers: describe and `sessions.list` after phase 1; upgrade when idle; deferral only while a turn runs; the sandbox token prelude; corrected copy | M | H2a |
| **F1-4** | Delete the copy workaround; large files stream and open automatically; queued open during a deferred upgrade; "blocked by network policy" card | S-M | H2b (same release or right after) |
| **F2-1** | `PanelRoot` + **Cloud/Local switch**. Covers: `git_gate` rework (remote writes allowed under §2.2.5, including push/pull/PR as confirmed user actions); the connection strip; local-only actions disabled plus "Open in browser" | M | F2-0 |
| **F2-2** | Disconnected state: cached read-only tree and index, Connect action, LRU per root | S-M | F2-1 |
| **F3-2** | Settings > Cloud "Claude setup": the single global kit (everything compatible checked, secrets and sign-ins included), the one-time notice, start-panel summary and on/off switch, app-API rule | M | F3-1 |
| **F4-L** | **Session location model.** Covers: `cloud-location` on local sessions; `RemoteSession.linked_to` hidden from the sidebar; the remote runtime appending projected rows to the local DB session (no duplicates); closing the local provider process and refusing the local runtime and TUI launch while Cloud (R2); rewind disabled before the move point (R6); cloud badge and panel driven by the location | L | F4-0 |

### Wave 3: Host batch 2 (files, git, uploads)

| ID | Ticket | Size | Depends on |
|---|---|---|---|
| **H3** | Host `uploads.*` (chunked, resumable, verified, validated) + Rust client | M | H2a |
| **F2-3h** | Host `write_text_file` with `expectedSha256`, temp-rename keeping the mode | S | H2a |
| **F2-4h** | Host `workspace.watch` (non-recursive: git metadata + the loaded dirs list) | M | H2a |
| **F2-5h** | Host idle guard for `git_discard_*`/`git_stash`; push/pull/sync Host timeouts at 90 s | S | H2a |
| **HR2** | Release Host batch 2 and pin it in the app | S | H3, F2-3h, F2-4h, F2-5h |
| **F2-3** | Editor save through the conditional write; conflict compare dialog; "agent is working" notice; read-only for 1-2 MB | M | HR2, F2-1 |
| **F2-4** | Watch client: long-poll, dirs list updates, 10 s status poll alongside, polling fallback, focus and idle rules | M | HR2, F2-1 |
| **F2-5** | `rpc_with` per-request timeout (120 s for push/pull/sync) and the "may still be running" UI | S | HR2 |

### Wave 4: Host batch 3 (kit, sign-ins, move), then features 3 and 4

| ID | Ticket | Size | Depends on |
|---|---|---|---|
| **F3-3h** | Host `kit.apply/secrets/probe/health`; `spawnChild` env overlay; `--plugin-dir`/`--mcp-config` in `buildClaudeArgs`; kit hash in `settingsKey`; TS log scrubbing; TS tar extraction | M | HR2 |
| **F3-5h** | Host side of MCP sign-ins: merge into the remote `.credentials.json` (MCP keys only, temp-rename, 0600, tracked) | S-M | HR2, F3-5a |
| **F4-2h** | Host `workspace.applySnapshot` (idle guard, worktree target on shared instances, `tree` verify, validation) and `sessions.import` (JSONL install with the F4-J rule, refuse when a live session binds the id, store record + `provider.bind`, same id) | M | HR2, F4-0, F4-J |
| **HR3** | Release Host batch 3 and pin it in the app | S | F3-3h, F3-5h, F4-2h |
| **F3-4** | "Claude setup" step in new-workspace starts (one-way: kit + secrets file), "Update Claude setup" action, health in the panel, network-policy hints | M | F3-2, HR3 |
| **F3-5** | App side of MCP sign-ins: read the account's local entries (Keychain or file), send them once in the setup step, "needs sign-in" health, re-copy only through "Update Claude setup" | M | F3-4, F3-5h |
| **F4-3a** | Local → cloud orchestrator: `MoveRecord`, push, both backends, uploads, restore, verify, same-id import, version check (R4), moved-path note (R5), location flip, rollback/cancel | M-L | F4-1, F4-J, F4-L, HR3 |
| **F4-4** | "Continue in cloud" dialog (one confirmation), progress steps, transcript divider | M | F4-3a |
| **F4-5** | Cloud → local for moved and cloud-born sessions: in-place apply when unchanged, else a new worktree; the convert no longer archives and replaces the cloud row; same id through F4-J (fork on divergence, new id after a lost workspace) | M | F4-L, F4-3a, F4-J |
| **F4-3b** | App API `sessions.moveToCloud` / `sessions.moveToLocal` | S | F4-4, F4-5 |

### Recommended order

1. **Wave 1 now, in parallel.** F2-0 first, because it fixes a misleading banner users see
   today. F3-5a needs the user at the machine.
2. **Wave 2.**
   - The release chain HR1 → H2a → H2b → F1-4 runs alongside F2-1 → F2-2, F3-2 and F4-L.
   - After this wave, every cloud Host is the fork's, and the remote Explorer/Changes are
     correct and editable.
3. **Wave 3:** Host batch 2, HR2, then F2-3, F2-4 and F2-5.
4. **Wave 4:**
   - Host batch 3 and HR3 first.
   - Then feature 3: F3-4, then F3-5.
   - In parallel, feature 4: F4-3a, F4-4, F4-5, then F4-3b.

---

## 6. Decisions (user, 2026-11-09)

1. **Host distribution:** the fork builds and publishes its own Host. Required.
2. **Upgrade:** automatic, no prompt. Only a running turn defers it, so that a turn is
   never killed mid-way.
3. **Feature 2:** a Cloud | Local switch, yes. Push/pull/PR on the remote, yes, as
   confirmed user actions.
4. **Feature 3 defaults:** take everything compatible, **including secrets**, so the user
   never has to authenticate MCP servers in the cloud.
5. **Feature 3 MCP auth:** MCP servers go with their credentials, including OAuth
   sign-ins (§3.2.5). **One way, local → cloud, once, in the setup step of a new
   workspace before it is ready**, plus the manual "Update Claude setup".
6. **Feature 3 plugins:** copy the local version.
7. **Feature 3 scope:** one kit for all of the cloud.
8. **Feature 4 code:** push the branch. Ignored files such as `.env.local` are never
   carried.
9. **Feature 4 identity:** one session, not two. The harness session id is the same on
   both sides:
   - local → cloud uses the local id;
   - cloud → local uses the cloud id.
10. **Copy path:** delete it (2026-11-10). Clicking a file always opens it directly
    through the fork Host's `files.readBytes`; large files stream and open
    automatically.
---

## 7. Things I could not verify ("not sure")

- **The running app:** the panels showing the remote tree for an open cloud session is
  settled from the code (§2.1), not yet seen in the running app. F2-0.
- **Idle timers:** whether tunnel RPC traffic resets the codespace idle timeout or the
  sandbox `--ttl`. F2-0.
- **MCP sign-ins:** where and in what format Claude Code stores MCP OAuth sign-ins
  (undocumented), and whether those servers rotate refresh tokens. F3-5a.
- **Same session id across machines:** whether `--resume <id>` keeps the id and appends to
  the same JSONL after a cross-machine install. The CLI help says yes: `--fork-session` is
  the opt-in for a new id. F4-0.
- **`claude plugin` flags:** the exact `install` / `marketplace add` flags, and
  `claude mcp add-json` flags. `--help` printed "Not logged in" here. Not needed by the
  chosen design.
- **Account config dirs:** how Claude Code merges `~/.claude` with an account
  `CLAUDE_CONFIG_DIR` for skills and settings. F3-1.
- **Unapproved project MCP servers:** how a headless (stream-json) Claude treats
  unapproved project `.mcp.json` servers.
- **Docker features on cloud sandboxes:** whether `sbx mcp`, `sbx skills` and
  `--static-mcp` work on cloud sandboxes (not used by this design).
- **LFS:** whether `git push` uploads LFS content in every setup.
