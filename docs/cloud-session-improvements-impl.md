# Cloud session improvements: implementation spec

This file is for a session with **no memory of the design conversation**. Read it first,
then read `docs/cloud-session-improvements.md` (the design, cited below as "design §N").
The design explains *why*; this spec says *what to do, in which order and how to check
it*. When the two disagree, the user decisions in section 2 below win. Then the design
wins, and the spec is fixed to match.

- Feature branch: `feat/cloud-session-improvements`, branched from `origin/main` at
  `d08c9838`.
- Main worktree for this work:
  `/Users/hhoangg/workspaces/my-projects/monoptah-worktrees/cloud-session-improvements`.
- The repository's `CLAUDE.md` applies throughout. Read it: it covers fork branding,
  identifiers that must never be renamed, releases, and how to read exit codes.

---

## 1. Ground rules

### 1.1 Scope

- **The GUI target is the native app only:** `crates/monocode-core` and
  `crates/monocode-gpui`.
  - Do not change the Tauri app (`src-tauri/`, `src/` UI).
  - Exception: shared TypeScript the Host also uses. That is
    `src/integrations/harness/**`, e.g. `claudeProtocol.ts` and `core/child.ts`. Changes
    there must keep `npm run check:web` green.
- **The remote Host (`host/`, TypeScript) is in scope.** Several tickets change it.
- **Never rename** the identifiers listed in `CLAUDE.md`:
  - the `monocode-host` CLI and asset names;
  - the `~/.monocode-host` data dir;
  - the `monocode.*` keys, crate and package names;
  - `INTERRUPT_MESSAGE`.

  Fork branding rules apply to every user-visible string.

### 1.2 Who does what

- **Implementation:** a subagent with `model: "sonnet"`, one ticket at a time per agent.
  Independent tickets of the same wave may run in parallel, each in its own worktree.
- **Review:** after the implementer finishes, a **separate** subagent with
  `model: "opus"` reviews the ticket's diff.
  - Give the reviewer: the diff (`git diff feat/cloud-session-improvements...HEAD`), the
    ticket section of this spec, and the design sections it cites.
  - Do **not** give it the implementer's summary or conclusions.
  - The reviewer reports findings. Valid findings go back to a Sonnet implementer to fix.
  - Re-review when the fix is non-trivial.
  - The writer of the code is never its only reviewer.
- **The coordinating session** (you) creates worktrees, dispatches agents, runs the
  verification commands itself, merges, and updates the tracker (section 4).

### 1.3 Git workflow

**Confirm once with the user** at the start of the implementation session that local
commits on ticket branches and local merges into `feat/cloud-session-improvements` are
allowed. The user's global rule is "never commit unless asked". Phase 2 was approved, but
get an explicit yes for committing.

**Step 0, before the first ticket: merge `origin/main`.** The feature branch starts at
`d08c9838`. `main` has since gained PR #59 (sealed store), #62 (default provider
account) and #63 (setup-token usage), which ticket A1 builds on.

- With the user's yes, `git fetch origin` and `git merge --no-ff origin/main` on
  `feat/cloud-session-improvements`.
- Run `npm ci`, `npm run native:check` and `./scripts/check-fork-branding.sh`.
- Line numbers in this spec come from `d08c9838` (A1: from `9eae242e`). After the merge,
  find code by name.

**Then:** the two docs (`docs/cloud-session-improvements.md` and this
file) are uncommitted in the main worktree. Ticket worktrees branched from the feature
branch would not see them. With the user's yes, commit them directly on
`feat/cloud-session-improvements`: "Add cloud session improvements design and
implementation spec".

**One worktree per ticket:**

```bash
cd /Users/hhoangg/workspaces/my-projects/monoptah-worktrees/cloud-session-improvements
git worktree add -b csi/<ticket-id-lowercase> \
  /Users/hhoangg/workspaces/my-projects/monoptah-worktrees/csi-<ticket-id-lowercase> \
  feat/cloud-session-improvements
cd ../csi-<ticket-id-lowercase> && npm ci     # node_modules is not shared between worktrees
```

- **Base:** branch from the **current** tip of `feat/cloud-session-improvements`, so every
  dependency the ticket needs is already merged.
- **Commits:** small, English messages, ending with the attribution line the
  harness provides. Never `--amend` a commit that was already merged.
- **Merging back,** after the checks of section 1.4 pass in the ticket worktree and the
  Opus review is clean:

  ```bash
  cd /Users/hhoangg/workspaces/my-projects/monoptah-worktrees/cloud-session-improvements
  git merge --no-ff csi/<id> -m "Merge <ID>: <title>"
  ```

  Then re-run the checks for the touched areas **on the feature branch**, and update the
  tracker.
- **Conflicts:** if a merge conflicts, resolve it in the ticket worktree (merge the
  feature branch into the ticket branch), re-run the checks, then merge again.
- **Cleanup:** after a successful merge you may `git worktree remove` the clean ticket
  worktree. **Do not delete branches** without asking.
- **Tickets that land directly on the feature branch** (no worktree):
  - the step-0 `origin/main` merge;
  - tracker and doc updates (sections 4 and 5 of this file, and the results that F2-0,
    F3-5a and F4-0 write into the design doc);
  - nothing else.
- **Never:**
  - commit to `main`;
  - push anything without the user asking (that includes the feature branch and tags);
  - force-push, `reset --hard` or rebase a shared branch.
- **Release tickets (HR1-HR3) need pushes.** Stop and ask (section 3, HR1).

### 1.4 Build and test commands

Run these from the ticket worktree root. Read exit codes directly, never through a pipe
(`cmd > /tmp/log 2>&1; echo $?`); `CLAUDE.md` explains why.

| What | Command |
|---|---|
| Install JS deps (once per worktree) | `npm ci` |
| Core crate tests | `cargo test --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-core --profile native-dev` |
| GPUI crate tests | `cargo test --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-gpui --profile native-dev -- --test-threads=1` |
| One test | append `<test_name_filter>` (core) or put it before `-- --test-threads=1` (gpui) |
| Format | `cargo fmt --manifest-path crates/monocode-gpui/Cargo.toml --all` (check: add `--check`) |
| Clippy | `cargo clippy --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-core -p monocode-gpui --all-targets --profile native-dev -- -D warnings` |
| **Full native gate** (run before every merge of a Rust ticket) | `npm run native:check` |
| Host type-check + bundle | `npm run host:build` |
| Host tests | `npm run test:host` (vitest with `host/vitest.config.ts`) |
| Host packages (all targets) | `npm run host:package -- --all` |
| Shared TS (`src/integrations/**`) | `npx vitest run src/integrations` and `npx tsc --noEmit`, or `npm run check:web` |
| Native script tests | `npm run native:test:dev` (plus any new `scripts/*.test.mjs` with `node --test`) |
| Fork branding gate | `./scripts/check-fork-branding.sh` (must exit 0) |
| Run the app from source | `npm run native:dev` (builds `~/Applications/Monoptah Native Dev.app`). Read `docs/native/README.md` first to confirm it keeps its own data. |

Notes:

- `monocode-core` and `monocode-gpui` share the workspace manifest
  `crates/monocode-gpui/Cargo.toml`. Running `cargo test` inside `crates/monocode-core`
  also works, because it finds that workspace, but use `--manifest-path` for consistency.
- GPUI tests must run with `--test-threads=1`.

### 1.5 Cloud resources and the user's machine

- **Manual checks against a codespace or Docker sandbox cost the user money.** They use
  the user's GitHub and Docker accounts. Ask before creating, starting or deleting any
  cloud instance. Prefer one existing instance the user names.
- **Reading the macOS Keychain shows a system prompt.** Only do it in F3-5a, with the user
  present.
- **Never print secret values:** not `.env`, not tokens, not MCP `env`/`headers` values.
  Print key names only (e.g. `jq 'paths(scalars)|join(".")'`).
- **Running `claude` makes model requests on the user's account.** F4-0 needs that. Ask
  first.

---

## 2. Decisions (rules)

These are final. Do not re-open them. If one turns out impossible, stop and ask the user.

| # | Rule |
|---|---|
| D1 | The fork builds and publishes its own Host (`hhoangg/monoptah`, tag `host-vX.Y.Z`, Host version `X.Y.Z+monoptah`). It ships every target that `host/package.mjs --all` builds, Windows included. Releases use `--latest=false`, and a version is never re-uploaded. Design §1.2 H1. |
| D2 | Cloud Hosts upgrade **automatically, without a prompt**, in the two-phase connect. The only deferral is "a session on that Host is running": a turn is never killed. User-owned SSH machines keep the manual Upgrade button. Design §1.2 H2. |
| D3 | Features gate on **capability strings** from `environment.describe`, never on version numbers. Versions only decide upgrades. An installed Host without `+monoptah` counts as older than any pin. |
| D4 | Feature 1: clicking a file in a cloud session's chat **opens it directly**. The copy workaround is **deleted** (F1-4); there is no Download-a-copy fallback. Large files stream through `files.readBytes` and open automatically. Only above 500 MB the app asks first. F1-1 (a `read_binary_file` stopgap) is **dropped**. Design §1.2. |
| D5 | Feature 2: Files and Changes show the **cloud** workspace by default, with a **Cloud / Local** switch. Remote push, pull, sync and PR are allowed as confirmed user actions. Background reads **never** wake a workspace; waking is always a user action. Design §2.2. |
| D6 | Feature 3: **one global kit** for all cloud workspaces, set in Settings > Cloud. There is no per-project or per-session selection; project-scoped items (local-scope MCP, untracked project skills) follow their own project. The default is everything compatible, **including secrets and MCP OAuth sign-ins**. A one-time notice is acknowledged once. Plugins are **copied from local** and passed with `--plugin-dir`. Design §3.2. |
| D7 | Feature 3 credentials (env and header secrets, MCP OAuth sign-ins) go **one way, local to cloud, once**: in the "Claude setup" start step of a **new** workspace, before `Ready`. They are stored on the workspace 0600 and loaded into Claude child processes only. A reconnect never re-sends them and nothing ever comes back. The only refresh is the manual "Update Claude setup" action, still one way. There is no `headersHelper` broker. Design §3.2.3, §3.2.5. |
| D8 | Feature 4, code: commits travel by **pushing the branch**, never with force and never to origin's default branch. On divergence the target is `<branch>-cloud`. Uncommitted, non-ignored changes travel as a patch over the tunnel and are never pushed. **Ignored files (e.g. `.env.local`) are never carried.** Design §4.2.2. |
| D9 | Feature 4, identity: **one session, not two.** A local session gains a location (Local / Cloud). The harness (Claude) session id is the same on both sides: a session born in the cloud keeps the **cloud** id when it comes back, and a session born locally takes the **local** id up to the cloud. Design §4.2.1. |
| D10 | Shared-id safety (design §4.9): **one writer at a time** (by location). Transcripts are compared by the entry `uuid` chain, never by bytes. An extension replaces the old copy, which is kept as `<id>.jsonl.pre-cloud`. **Divergence is never overwritten**: it forks to a new id and tells the user. After a lost workspace, continue on a new id with a summary, never on the stale file. |
| D11 | The app API can only narrow user choices. It can turn the kit off, but cannot widen it. `sessions.moveToCloud` follows the same push rules (D8). |

---

## 3. Tickets

Sizes: S ≈ ≤1 day, M ≈ 2-4 days, L ≈ 1-2 weeks.

Path shorthands:

- `core/` = `crates/monocode-core/src/`
- `gpui/` = `crates/monocode-gpui/src/`

Line numbers come from `d08c9838` and may drift. Find the code by name.

**Every ticket also requires:**

- the checks of section 1.4 for the areas it touches;
- an Opus review;
- the tracker updated.

**"Stop and ask"** means: finish what can be done without the missing input, record it in
the tracker as `blocked: <reason>`, and ask the user.

### Wave 1 (no Host release needed; all independent)

#### F2-0 Hotfix: cloud panels banner and binding test (S)

- **Goal:** Files/Changes already show the remote tree for cloud sessions, while the
  banner says "Local files". Make the UI tell the truth, pin the behaviour with a test,
  and answer the open questions of design §7 assigned here. Design §2.1.
- **Depends on:** nothing.
- **Files:**
  - `gpui/shell/cloud.rs` (`banner_text` ~620, `sync_cloud_panel` ~1133);
  - `gpui/git_gate.rs` (the doc comment only);
  - `gpui/shell/cloud_tests.rs` (near the test at ~1373);
  - `docs/cloud-session-improvements.md` (§2.1, §7: results).
- **Plan:**
  1. Read `Shell::sync_git` (`gpui/shell.rs` ~1921), `Shell::add_loaded` (~1585) and
     `ensure_session` (`gpui/shell/session_loading.rs` ~50). Confirm the remote binding
     from the code.
  2. Change `banner_text` to "Cloud files - <noun> <name>". Fix the `git_gate.rs` comment
     ("blocks git writes while a cloud session is in view; the panels show the cloud
     workspace").
     - Do **not** change gate behaviour here: that is F2-1.
  3. Add a GPUI test that creates the cloud session's `Conversation` entity and asserts
     that `ProjectGit`'s root is the `remote://` project key. Add a second test for a Host
     worktree session (`worktree_cwd` from `core/store/remote.rs` ~165), asserting the
     root is that worktree path.
  4. Read the Host's `list_dir` (`host/workspace-commands.ts`, `host/workspace.ts`
     `listHostFiles`) and record whether it caps entries.
  5. **Manual (ask the user):** open an existing cloud session in `npm run native:dev`
     and confirm the Explorer shows remote files.
     - If the user agrees, observe whether the codespace idle timer resets while the
       panel polls. Keep the panel open past the idle timeout and see whether the
       codespace stops.
     - Write the results into design §2.1 and §7.
- **Acceptance:**
  - [ ] The banner reads "Cloud files - codespace X" / "Cloud files - sandbox X".
  - [ ] There is no "Local files" wording for cloud sessions anywhere in `gpui/`.
  - [ ] The new tests fail on the old banner and binding assumptions and pass now.
  - [ ] Design §7 has answers (or "user declined manual check") for: panels in the
    running app, the `list_dir` cap, idle timers.
  - [ ] `npm run native:check` passes.
- **Out of scope:** gate changes, the Local switch, connection states (F2-1, F2-2).
- **Risks:** none. This is text and tests.

#### F1-2 Send the worktree `cwd` in cloud file reads (S)

- **Goal:** chat file links of a session that runs in a Host worktree read from that
  worktree, not the project root. Design §1.1, §1.2 F1-2.
- **Depends on:** nothing.
- **Files:**
  - `core/cloud_sessions/view.rs` (`Fetcher`, `range` ~385-424, `resolve_cloud_file`
    ~501);
  - `core/cloud_sessions/view_tests.rs`;
  - reuse the cwd resolution of `core/cloud_sessions/convert.rs` ~751-779. Extract it
    into a shared helper instead of copying it.
- **Plan:**
  1. Extract "session Host cwd": `worktree_cwd` unless `worktree_removed`, confined to
     the workspace root.
  2. Pass it as `cwd` in the `files.readBytes` and `files.read` params, and confine the
     resolved path to that cwd.
  3. Tests with a fake `Host` asserting the params carry `cwd`, and that a path outside
     the worktree is refused.
- **Acceptance:**
  - [ ] The `files.readBytes` and `files.read` requests include the worktree `cwd` when
    the session has one, and omit it otherwise.
  - [ ] `convert.rs` uses the same helper.
  - [ ] Core tests pass. `npm run native:check` passes.
- **Out of scope:** any change to the copy path (F1-4).
- **Risks:** a removed worktree. Fall back to the project cwd, as convert does.

#### F1-3 Editor: chunked `files.readBytes` for remote reads (S-M)

- **Goal:** the editor reads `remote://` files in chunks with progress and cancel,
  without the single 10 MB `read_binary_file` reply. Design §1.2 F1-3, §2.2.4.
- **Depends on:** nothing. It is useful once Hosts have `files.readBytes` (after H2b),
  and must work before that through the fallback.
- **Files:**
  - `core/files/document.rs` (`read_document` ~86, `read_remote_bytes` ~160-176);
  - `core/files/remote.rs`;
  - `core/remote.rs` (machine lookup by environment id);
  - `core/git/remote.rs` (precedent for `projects.list`);
  - `gpui/file_editor.rs` (`load` ~419) for progress and cancel.
- **Plan:**
  1. `files.readBytes` is keyed by `projectId` (+ `cwd`), not by path. To map a
     `remote://<env>/<path>` path:
     - find the machine by environment id;
     - call `projects.list` (cached per machine for 5 s);
     - choose the registered project whose cwd is the longest prefix;
     - send the rest as the relative `path`, plus `cwd` when the path is inside one of
       that project's worktrees.
  2. When the capability `files.readBytes` is present (probe `environment.describe`
     once per machine, or reuse a cache if H2a has landed), read in chunks of
     `MAX_READ_CHUNK` = 393216 bytes. Report progress through a callback and honour a
     cancel flag.
  3. Otherwise, or when no project matches, fall back to today's `read_binary_file`.
  4. Keep the existing text and media detection (`document_from_bytes`) and caps:
     2 MB text, 32 MB preview.
- **Acceptance:**
  - [ ] With a fake Host advertising `files.readBytes`, a 3 MB file is read in 8 chunks
    and decoded exactly.
  - [ ] A cancel stops after the current chunk.
  - [ ] Without the capability, `read_binary_file` is used.
  - [ ] A path outside every project is an error, not a crash.
  - [ ] Core tests pass. `npm run native:check` passes.
- **Out of scope:** saving (F2-3), the chat view (already chunked).
- **Risks:** the base64 chunk join (chunks are multiples of 3, so concatenating decoded
  bytes is safe).

#### H1 Fork Host release pipeline (M)

- **Goal:** the fork can publish its own Host packages. The first package can identify
  itself. Design §1.2 H1 and the H2 Host-side bits, rules D1 and D3.
- **Depends on:** nothing.
- **Files:**
  - new `host/VERSION`, containing `1.0.0+monoptah`;
  - `host/build.mjs`: an esbuild `define` of `__HOST_VERSION__` from `host/VERSION`;
  - `host/cli.ts` (~31, ~73-74): `--version` prints `__HOST_VERSION__`, no longer
    `../package.json`;
  - `host/server.ts` (`environment.describe` ~256-297): add
    `host: {flavor: "monoptah", version}`;
  - `host/package.mjs`: the version used for checks comes from `host/VERSION`. Asset
    names stay `monocode-host-<target>.tar.gz` / `.zip`;
  - new `.github/workflows/host-release-monoptah.yml`;
  - new `scripts/host-release-monoptah.mjs` + `scripts/host-release-monoptah.test.mjs`;
  - `package.json`: a `host:release` script;
  - `CLAUDE.md`.
- **Workflow requirements** (model them on `.github/workflows/native-release-monoptah.yml`
  conventions):
  - trigger: `push` of tags `host-v*`, plus `workflow_dispatch`;
  - build all targets with `npm run host:package -- --all` (linux, darwin and win32 on
    their runners as needed; check `host/package.mjs` for what each runner can build);
  - check that the tag version matches `host/VERSION` minus `+monoptah`;
  - **refuse** when the release already exists;
  - `gh release create host-vX.Y.Z --latest=false`, uploading every package and its
    `.sha256`;
  - produce `host-release.json` (`{version, assets: {target: sha256}}`) as a workflow
    artifact **and** a release asset;
  - attest it with `actions/attest-build-provenance`;
  - least `permissions` (contents: write, id-token: write, attestations: write).
- **Release script** `npm run host:release -- X.Y.Z`:
  - requires a clean tree;
  - writes `host/VERSION` (`X.Y.Z+monoptah`);
  - commits "Release Monoptah Host X.Y.Z";
  - creates tag `host-vX.Y.Z`;
  - **prints** the push commands. It never pushes itself; the user's rule is that tags
    are not pushed by hand, and the script leaves the push to an explicit step the user
    approves.
- **CLAUDE.md:**
  - add a "Releasing the Host" subsection under "Releasing": tag naming, `--latest=false`,
    never re-upload, pinning in the app (H2a);
  - add to "After every merge from upstream": "If upstream changed `host/`, bump
    `host/VERSION` and cut a Host release; give any new upstream Host workflow job the
    `if: github.repository == 'hardbeat920/monocode'` gate."
- **Acceptance:**
  - [ ] `npm run host:build` passes, and the built Host prints `1.0.0+monoptah` for
    `--version`.
  - [ ] `npm run test:host` passes, with new tests for `--version` and `describe.host`.
  - [ ] `npm run host:package -- --all` produces every target locally (or every target
    the local OS can build; note which).
  - [ ] `node --test scripts/host-release-monoptah.test.mjs` passes: version validation,
    dirty-tree refusal, no push.
  - [ ] The workflow passes `actionlint` if it is available (otherwise a careful review).
    It contains `--latest=false` and the existing-release refusal.
  - [ ] `./scripts/check-fork-branding.sh` exits 0.
  - [ ] The Tauri app and the root `package.json` version are untouched.
- **Out of scope:** changing the app's bootstrap URL (H2a); publishing (HR1).
- **Risks:**
  - `host/package.mjs` may assume the root version in other places. Grep for
    `package.json` usage in `host/`.

#### F3-1 Kit inventory and classification (M)

- **Goal:** a pure core module that lists what the global kit can take to the cloud and
  classifies each item. Design §3.2.1, rules D6 and D7.
- **Depends on:** nothing.
- **Files:**
  - new `core/cloud_sessions/kit.rs` (+ `kit_tests.rs`), registered in
    `core/cloud_sessions.rs`;
  - reuse `core/skills.rs` (`list_skills`, `list_skills_from`, plugin scan ~234),
    `core/mcp.rs` (`mcp_discover`, `discover`) and `core/accounts.rs`
    (`Profile::home(Kind::Claude)`).
- **Plan:**
  1. Write the `KitInventory` types per design §3.2.1:
     - skills: user, project-untracked, plugin;
     - MCP servers with `compat`: `Ok`, `LocalOnly(reason)`, `NeedsRuntime(..)`,
       `SignedIn`, `NotSignedIn`, `Project`; plus `secrets: [field]`;
     - plugins: id, version, `installPath`, size, `hasNativeBin`.
  2. **Sources:** the account config dir from `Profile::home`, then `~/.claude`.
     - Verify how Claude Code merges the two for skills and settings: test with a real
       profile dir, names only. Record the result in design §7.
     - Label each item with its source.
  3. **Classification rules:** follow §3.2.1 exactly.
     - Absolute local commands, localhost/private/link-local URLs, `docker` on a
       sandbox: `LocalOnly`.
     - `npx`/`uvx`/`bunx`/`node`/`python3`/`docker`: `NeedsRuntime`.
     - `${VAR}`-only values are not secrets. Query-string URLs are secret.
     - `SignedIn` vs `NotSignedIn` needs F3-5a's storage answer. Until then, mark http/sse
       servers without an `Authorization` header as `SignedIn` when F3-5a's probe
       function says so. Leave a `fn signed_in(server) -> Option<bool>` seam that returns
       `None` ("unknown") for now.
  4. **Committed project skills are skipped** (they arrive with the clone): use
     `git ls-files` in the local repo.
  5. **Size caps:** 10 MB per skill, 25 MB per plugin, 50 MB total compressed.
     - Skip `.git` and `.DS_Store`.
     - Skip `node_modules` in skills only.
     - Resolve symlinks, but refuse links that leave `$HOME`.
  6. **No secret values in any `Debug`/`Display`/log output.** Use a `Secret` newtype
     that redacts.
- **Acceptance:**
  - [ ] Table tests cover every classification rule, using fixture `.claude.json`,
    `.mcp.json`, `installed_plugins.json` and `settings.json` **with fake values**.
  - [ ] A deterministic ordering and content hash for the same inputs.
  - [ ] Size caps and symlink rules are tested.
  - [ ] A test asserts that `format!("{:?}")` of an inventory with secrets contains no
    secret value.
  - [ ] Design §7 "account config dirs" is answered.
  - [ ] Core tests pass. `npm run native:check` passes.
- **Out of scope:** UI (F3-2), transfer (F3-3h, F3-4), reading the Keychain (F3-5).
- **Risks:** large home dirs. Cap the scanning time and entries, as `skills.rs` does
  (`MAX_SKILLS`).

#### F3-5a Spike: MCP OAuth sign-in storage (S). **Needs the user at the machine.**

- **Goal:** learn where Claude Code stores MCP OAuth sign-ins and in what shape, on macOS
  and on Linux, and which of the user's servers rotate refresh tokens. Design §3.2.5.
- **Depends on:** nothing. **Stop and ask** the user to be present before running any
  Keychain command (macOS shows a prompt).
- **Plan:**
  1. Read the Claude Code docs again, in case the location has since been documented:
     - <https://code.claude.com/docs/en/mcp>, the authentication sections;
     - <https://code.claude.com/docs/en/iam>, "Credential management".
  2. **macOS:**
     - list the Keychain items whose service mentions Claude, attributes only (no `-w`,
       no `-g`);
     - for the account's config dir (`CLAUDE_CONFIG_DIR`), identify the entry name;
     - with the user's OK, read it and print **only key paths**:
       `security find-generic-password -s "<service>" -w | jq -r 'paths|map(tostring)|join(".")'`.
     - **Never print values.**
  3. **Linux:** ask the user for an existing codespace. Run `claude mcp login <server>`
     there for one server (the user completes the browser step), then print the key
     paths of `~/.claude/.credentials.json`.
  4. For each of the user's OAuth MCP servers, record whether the token response rotates
     the refresh token. Only if visible without printing secrets: compare the presence and
     length of the refresh token before and after a refresh, or check the server's docs.
     Otherwise write "unknown".
- **Deliverable:** a "F3-5a results" note in design §3.2.5 and §7, covering:
  - the storage location per OS;
  - the key path layout (names only);
  - how to select one server's entry;
  - rotation per server.

  It is committed directly on the feature branch.
- **Acceptance:**
  - [ ] Design §3.2.5 records a concrete "copy" mapping (source keys → remote
    `.credentials.json` keys), **or** states that the format is unusable. In that case
    the design's fallback applies: "carried without sign-in, marked Sign in in the
    cloud".
  - [ ] No secret value appears in the transcript, logs or docs.
- **Out of scope:** writing any code.
- **Risks:** a Keychain prompt with no user present. Never run without them.

#### F4-0 Spike: same-id resume and the row-append path (S). **Needs the user's OK** (model requests).

- **Goal:** prove that `claude --resume <id>`, after installing the JSONL under a
  different cwd and config dir with the cwd fields rewritten, keeps the id and appends to
  the same file. Also confirm the remote runtime can append rows to a local DB session.
  Design §4.2.2 step 6, §4.9 R4 and R9, rule D9.
- **Depends on:** nothing. **Stop and ask** before running `claude` (it uses the user's
  account).
- **Plan:**
  1. In a scratch dir, run `CLAUDE_CONFIG_DIR=/tmp/csi-a claude -p "Reply with OK" --session-id <uuid>`
     in project dir A.
  2. Copy `projects/<enc A>/<uuid>.jsonl` to `/tmp/csi-b/projects/<enc B>/`, rewriting
     the `cwd` fields from A to B. Use `rewrite_cwd` (`core/cloud_sessions/convert.rs`
     ~289) or `jq`.
  3. Run `CLAUDE_CONFIG_DIR=/tmp/csi-b claude -p "Reply with OK again" --resume <uuid>` in
     dir B.
  4. Check:
     - no new session file was created;
     - the same file grew;
     - the entry `uuid` chain is preserved (old uuids are a prefix);
     - the `sessionId` fields are unchanged;
     - the per-entry `version` field exists.
  5. Optional, if the user offers an existing codespace: repeat steps 2-4 there with the
     codespace's Claude version.
  6. **Code reading:** `gpui/runtime/remote.rs` (~683-760, row projection and diff) and
     `core/store/remote.rs`. Describe how projected rows could be appended to a local DB
     session idempotently, by Host block ids or row indexes. Feed this into F4-L.
- **Deliverable:** a "F4-0 results" note in design §4.2.2, §4.9 and §7, committed on the
  feature branch. Delete `/tmp/csi-*` afterwards.
- **Acceptance:**
  - [ ] The results state yes or no for "same id kept", "same file appended" and "uuid
    prefix preserved".
  - [ ] **If any answer is no, stop and ask the user before starting F4-J, F4-L, F4-2h or
    F4-3a.** Rule D9 may need a different mechanism.
  - [ ] There is a written row-append approach for F4-L.

#### F4-J Transcript sync rules (M)

- **Goal:** one pure module that decides, for a transcript arriving at a machine that
  may already hold the same id: install, replace (extension) or fork (divergence). It
  also does the cwd rewrite both ways and fixes R1 in `install_jsonl`. Design §4.9, rule
  D10.
- **Depends on:** nothing. Its tests must not depend on F4-0's live results, but read
  F4-0's notes if they exist.
- **Files:**
  - new `core/cloud_sessions/transcript_sync.rs` (+ tests);
  - `core/cloud_sessions/convert.rs` (`install_jsonl` ~1190-1226, `rewrite_cwd` ~289);
  - new fixtures under `crates/monocode-core/tests/fixtures/transcript_sync/` (JSONL
    files), shared later with the Host mirror (F4-2h).
- **Plan:**
  1. `parse_chain(jsonl) -> Vec<Uuid>`, from entry `uuid`s in file order. Skip lines
     without a `uuid`, but keep them in the output.
  2. `decide(existing: Option<&[u8]>, incoming: &[u8]) -> Install | Replace | AlreadyCurrent | Diverged`:
     - **Replace:** the existing chain is a strict prefix of the incoming one.
     - **AlreadyCurrent:** the chains are equal.
     - **Diverged:** otherwise.
  3. `rewrite_cwd(jsonl, from, to)`: generalise the existing one, in both directions.
  4. `fork(jsonl, new_id)`: rewrite the `sessionId` fields to a new UUID.
  5. `install_jsonl` uses `decide`:
     - **Replace** writes to a temp file, renames, and keeps the old file as
       `<id>.jsonl.pre-cloud`;
     - **Diverged** installs a fork and returns the new id with a user-facing message.

     The existing `Resume` enum gains `Replaced` and `Forked(new_id)`.
- **Acceptance:**
  - [ ] Tests: install fresh; extension → replace with backup; equal → no write; diverged
    → fork with a new id and the old file untouched.
  - [ ] Tests: a cwd rewrite round trip is the identity. Comparison ignores the cwd
    rewrite (uuid-based).
  - [ ] Today's cloud→local convert tests (`convert_tests.rs`) still pass, with
    `AlreadyThere` semantics replaced as specified.
  - [ ] Core tests pass. `npm run native:check` passes.
- **Out of scope:** the Host mirror (F4-2h), UI.
- **Risks:**
  - Very large JSONL: parse line by line, never into one `Value`.
  - Unknown future entry shapes: only `uuid` and `sessionId` are interpreted.

#### F4-1 `move_out.rs`: local snapshot for moving to the cloud (M)

- **Goal:** a read-only local snapshot that decides the push target and builds the
  uncommitted patch, the `tree` id and the transcript bundle. Design §4.2.2 steps 1-2 and
  the branch rules, rule D8.
- **Depends on:** nothing.
- **Files:**
  - new `core/cloud_sessions/move_out.rs` (+ tests);
  - reuse `core/git.rs` (`unpushed`, `uncommitted`), `core/session_worktree.rs`
    (`inspect`), `core/scm.rs` (`git_cmd`, `push_for`; do not push here) and
    `core/accounts/transfer.rs` (~31-54, which files make up a transcript).
- **Plan:**
  1. `fetch_origin(cwd)`: 60 s timeout, `GIT_TERMINAL_PROMPT=0`.
  2. `push_target(cwd) -> PushTarget { branch, needs_push, creates_branch }`. Follow the
     branch rules in design §4.2.2:
     - upstream and fast-forward: push to the upstream;
     - no upstream and the name is free: same name;
     - diverged or default branch: `<branch>-cloud[-N]`;
     - nothing to push: `needs_push: false`;
     - detached HEAD: error.
  3. `snapshot(cwd, staging) -> Snapshot { head, tree, patch_path?, ignored: Vec<(path, size)>, sizes }`:
     - copy the real index to a temp `GIT_INDEX_FILE`;
     - `add -A`, then `write-tree` for `tree`;
     - `diff --cached --binary HEAD` for the patch;
     - list ignored files;
     - never touch the real index, the working tree, branches or the stash.
  4. `collect_transcript(account_home, cwd, provider_id, staging)`: the `<id>.jsonl`, the
     `projects/<enc>/<id>/` folder and `file-history/<id>/`, as `accounts/transfer.rs`
     defines them. Enforce the 500 MB patch and 50 MB JSONL limits.
- **Acceptance:**
  - [ ] Temp-repo tests for every branch rule.
  - [ ] Tests for clean, dirty, untracked, binary and ignored files (`.env.local` is
    listed and **not** in the patch).
  - [ ] Session-worktree tests.
  - [ ] A test that the real index, stash and branch list are byte-identical before and
    after.
  - [ ] `tree` is deterministic across runs, and stays equal with `core.autocrlf=input`
    or `diff.noprefix` set.
  - [ ] Core tests pass. `npm run native:check` passes.
- **Out of scope:** pushing, uploading, any remote work (F4-3a).
- **Risks:**
  - LFS: detect `filter=lfs` in `.gitattributes` and surface a warning field.
  - Submodules: report dirty submodules as a warning.

#### A1 Mono can start a local session on a chosen provider account (M)

This is outside the four cloud features. It was added by the user on 2026-11-10 for the
same Phase 2 run. It does not depend on a Host release.

- **Goal:**
  - A Mono, through the `app` tool or `monocode-gpui app`, can start a **local** session
    that runs with a named provider account.
  - A Mono can list the accounts.
  - The user can point a Mono at an account with an `@<provider>/<label>` mention.
  - Sessions started this way really run with that account's config dir and credentials.
  - **Scope:** local sessions only (user decision, 2026-11-10). Cloud sessions keep the
    cloud setup unchanged.
- **Depends on:** the `origin/main` merge of section 1.3 (step 0), because PR #62
  (default provider account), #59 (sealed store) and #63 (setup-token usage) are not on
  `d08c9838`. All paths below are from `origin/main` at `9eae242e`.

**Rules decided for this ticket:**

1. `sessions.start` takes an optional `account` (text, at most 128 characters).
   - It is resolved **for the session's harness**. The harness is the given `harness`,
     else the existing rules (`monos/host.rs` `prepare_start`).
   - Without `account`, behaviour is unchanged:
     - same provider as the Mono chat: inherit the chat's account;
     - otherwise: `selected(kind, project)` = the project's choice, else the provider
       default (PR #62).
2. **Matching:** trim the selector.
   - Accepted, in this order:
     - the literal `default` (the provider CLI's own login);
     - an **exact account id** (case-sensitive; ids are `[A-Za-z0-9_-]`);
     - a **case-insensitive exact label** after trimming and collapsing inner whitespace.
   - **No prefix or fuzzy matching.**
   - Two accounts of that provider with the same label (case-insensitive) → an
     "ambiguous" error listing their ids.
   - No match → an error naming the provider and listing its valid labels and ids.
   - A label or id that exists only for **another** provider → an error saying which
     provider it belongs to, plus the valid list for the requested provider.
   - A harness without account support (anything but claude, codex, devin; and devin
     has only `default`) → an error.
   - Removed accounts never match.
3. **`account` is Mono-only.**
   - An operator (`app_access::validate_operator`) passing it gets "account is available
     to Monos only".
   - Reason: an operator thread should not switch the user's credentials. This follows
     the narrow-only spirit of rule D11.
4. **New read-only action `accounts.list {provider?}`** for Monos and operators.
   - It returns `{accounts:[{provider, id, label, isDefault, authKind}]}`:
     - `isDefault` = the provider's preferred account (`Book::preferred`);
     - `authKind` is one of `"cli"`, `"setupToken"`, `"apiKey"`, from
       `claude_credential()`, which returns the kind only. Codex and devin are always
       `"cli"`.
   - Removed accounts are excluded. `provider` must be an account-capable provider.
   - **Never** a secret, token, email or config path.
5. **Local sessions only.** `account` together with a cloud `target` (`codespace` or
   `sandbox`, any `instance`) is a **validation error**. It is raised in
   `mono::api::validate`, before anything is resolved or created:

   > "account applies to local sessions only; a cloud session uses the cloud setup (the
   > API key or setup token configured for Codespaces or Docker Sandbox)"

   - Cloud starts without `account` are unchanged:
     - codespaces keep using the `CLAUDE_CODE_OAUTH_TOKEN` codespace secret;
     - sandboxes keep using the project's saved Docker Sandbox settings.
   - A1 adds **no** cloud credential plumbing.
6. **Mention syntax:** `@<provider>/<label>`, e.g. `@claude/can.le`,
   `@codex/hoangxuan2402`.
   - A label with spaces or characters outside `[A-Za-z0-9._-]` is written quoted, as
     the composer already does for spaced paths (`core/composer.rs` `mention_text`):
     `@"claude/Work account"`.
   - The id form is also accepted: `@claude/account-<uuid>`.
   - Only **Mono composers** recognise account mentions.
   - Because `claude/…` can also be a real folder, a token is an account mention only
     when `<provider>` is account-capable **and** the rest resolves to an existing
     account by rule 2. Anything else stays a file mention, exactly as today.

**Files (origin/main paths):**

- `crates/monocode-core/src/accounts.rs` (`Book`, `Account`, `PROVIDERS`,
  `supports_accounts`, `preferred`, `selected`): add a pure
  `resolve(book, kind, selector) -> Result<String /*id*/, ResolveError>`. Its error text
  lists the valid labels.
- `crates/monocode-core/src/accounts/token.rs` (`claude_credential`) for `authKind`.
- `crates/monocode-core/src/mono/api.rs`:
  - add `account` to the `sessions.start` field list (~:10) and its text checks;
  - refuse `account` together with `target` (rule 5). That is the only cloud-related
    change: no file under `mono/cloud_api/` or `cloud_sessions/` is touched;
  - add `accounts.list` validation (`provider` optional text).
- `crates/monocode-core/src/app_access.rs`:
  - allow `accounts.list` for operators;
  - refuse `account` for operators (`validate_operator`, ~:15-57).
- `crates/monocode-gpui/src/monos/host.rs`:
  - `app_call_ready` (~:707) answers `accounts.list`;
  - `prepare_start` (~:1376-1590): after choosing `kind`, if `account` is given, resolve
    it with `accounts::get(cx)` and set `settings.account = center.profile(id)` (the
    profile needs the root; never build it from the bare id). Do this **before**
    `catalog::models_for(kind, &settings.account)` (~:1472), so model validation uses
    the chosen account.
- `crates/monocode-gpui/src/shell/app_commands.rs` (sessions.start ~:521-608): operator
  path refusal. Add `accounts.list` handling there too.
- `crates/monocode-gpui/src/monos/workflow_runner/host.rs` (~:403, uses `prepare_start`):
  make sure it keeps working. Workflows do not pass `account` in this ticket.
- **Help and instructions:**
  - `crates/monocode-core/src/mono/bridge.rs` `HELP` (~:17-70): add `account?` to the
    sessions.start line, a paragraph with rules 1, 2 and 5, and an `accounts.list` line;
  - `crates/monocode-core/src/mono.rs` `instructions()` (~:379-384): "the session
    inherits your harness, model and account unless you pass harness/model/account; when
    the user mentions @provider/label, pass account (local sessions only; a cloud target
    uses the cloud setup)".
  - `docs/agent-access.md` (~:16-19).
- **Mentions:**
  - `crates/monocode-core/src/composer.rs` (`Trigger::Mention`, `token_at`,
    `mention_text`, `highlight_ranges`);
  - `crates/monocode-gpui/src/composer_completion.rs`:
    - `rank()` Mention branch ~:913-1050: when the query starts with
      `claude/`, `codex/` or `devin/`, or is a bare provider prefix like `cl`, offer that
      provider's accounts as `@claude/<label>` with the auth kind as detail. Mono
      composers only, like `@workflow/`;
    - `pick()` ~:1219 for quoting;
    - `prepare_prompt_for` ~:397-463: attach the account context, Mono only, typed by the
      user, next to `attach_workflows`;
  - `crates/monocode-gpui/src/input/mention.rs` (`MentionIcon`): add `Account`;
  - new `crates/monocode-core/src/mono/account_mention.rs`, modelled on
    `mono/workflows/mention.rs`:
    - `mentioned_accounts(text, book) -> Vec<(provider, id, label, authKind)>`;
    - `attach_accounts(prompt, …)`, which appends after `"\n\n---\n"` the line
      "Referenced provider account (context only: when the user's request starts a
      local session for it, pass account:"<id>" with harness:"<provider>" to sessions.start;
      cloud sessions cannot take an account):"
      followed by `{provider,id,label,authKind}` per mention. Never anything secret.

**Plan:**

1. `accounts::resolve` + `ResolveError` with table tests (pure).
2. `accounts.list` in core validation, the Mono host and the operator path.
3. `sessions.start` `account`: validation, the operator refusal, `prepare_start`
   resolution before the model catalog, and the local-only refusal with a cloud `target`.
4. Help, instructions and docs text.
5. Mentions: the core parser and context block, completion, the icon, and
   `prepare_prompt_for`.
6. Confirm that the launch really uses the account:
   - Local launches go through `request.settings.account.apply` (`provider.rs` ~:1427),
     which sets `CLAUDE_CONFIG_DIR` / `CLAUDE_SECURESTORAGE_CONFIG_DIR` and the sealed
     `CLAUDE_CODE_OAUTH_TOKEN` / `ANTHROPIC_API_KEY` for Claude, or `CODEX_HOME` for Codex.
   - Check that a child session does not get the Mono-only Codex home override:
     `provider.rs` ~:1437-1446 only applies when `settings.mono` is set, and
     `launch_settings` clears it.
   - The persisted account is `sessions.provider_account_id` (shared store) or the
     legacy settings. The runtime refuses removed accounts (`runtime.rs` ~:160-168).

**Acceptance:**

- [ ] `resolve` matches `default`, an exact id and a case-insensitive exact label
  (inner whitespace collapsed). It rejects prefixes, wrong-provider names, removed
  accounts, and ambiguous labels (the error lists ids). Unknown names list the valid
  labels and ids for that provider.
- [ ] `accounts.list` returns `[{provider,id,label,isDefault,authKind}]`, filters by
  provider, and rejects an unknown provider. A serialized result built with a fixture
  token (`sk-ant-oat01-fixture-first`) does not contain the token, any path or any
  email.
- [ ] `sessions.start` with `account` for a local target persists the chosen account id
  on the new session and launches with `settings.account.id == <id>`. Without `account`,
  existing tests pass unchanged (incl. the PR #62 test, `monos/tests.rs` ~:6026).
- [ ] Model validation uses the chosen account: a model only that account lists is
  accepted, and one it lacks is refused.
- [ ] Operator `sessions.start` with `account` is refused. Operator `accounts.list` works.
- [ ] **Local only:**
  - `account` with a `target` of kind `codespace` or `sandbox` (with `instance` new, a
    name or `{session}`) fails validation with the rule-5 message;
  - no `gh` or `sbx` call happens (assert with the `NoGh` / no-sandbox fakes);
  - cloud starts **without** `account` behave exactly as before (the existing
    `mono/cloud_api/tests.rs` and `monos/cloud_tests.rs` pass unchanged);
  - `git diff` shows no change under `mono/cloud_api/` or `cloud_sessions/`.
- [ ] Mentions:
  - in a Mono composer, `@claude/` offers Claude accounts;
  - picking a spaced label inserts `@"claude/Work account"`;
  - the prepared prompt contains the context block with the id;
  - a normal session composer offers no account mentions;
  - `@claude/src` with no such account stays a file mention.
- [ ] HELP, `instructions()` and `docs/agent-access.md` describe `account` and
  `accounts.list`.
- [ ] `npm run native:check` and `./scripts/check-fork-branding.sh` pass.

**Required tests** (fakes only, never real credentials):

- `accounts/tests.rs`: `resolve` table, using in-memory `Book` values.
- `mono/api.rs` inline tests: field validation.
- `app_access/tests.rs`: the operator refusal and `accounts.list`.
- `monos/tests.rs`: `call`/`setup` helpers. Add accounts the way the PR #62 test does
  (`center.book.add`, `set_preferred`), then call `sessions.start` and assert the
  configured agent settings. Add the sealed fixture token through the `accounts/token`
  test `Fixture` pattern when `authKind` needs it.
- `shell/app_commands/tests.rs`: operator path.
- `mono/api.rs` inline tests, plus one `monos/tests.rs` case: the `account` + `target`
  refusal (codespace and sandbox) without any cloud call.
- `composer_completion` tests: mention completion and quoting.
- `mono/account_mention.rs` unit tests.

**Manual verification** (ask the user; it uses their real accounts):

1. In `npm run native:dev`, open a Mono chat. Type `@claude/` and pick one of the user's
   named Claude accounts. Ask the Mono to start a small session.
2. Check:
   - the new session's account chip shows that label;
   - the account identity (email or plan in the account UI) matches.
3. Repeat with a Codex account.
4. Ask the Mono for a cloud session with `@claude/<label>`: it gets the "local sessions
   only" error. A cloud session without an account still starts with the cloud setup.
5. **Never** inspect the process environment (`ps eww`), because a setup-token value
   would be printed.

**Out of scope:**

- changing the account of an existing session (`switch_account` exists in the UI);
- `account` in workflows;
- any account choice for cloud sessions (codespace or sandbox): they keep the cloud
  setup;
- quick-composer account mentions.

**Risks:**

- **Path collisions** between `@claude/…` and real folders: handled by rule 6.
- **Labels are user-editable:** the context block passes the **id**, so a rename after
  mentioning does not break the call.
- **The desktop registry (`store/shared/accounts.rs`) and native overrides:** resolve
  through the same `Book` the GUI uses (`accounts::get(cx)`), not a fresh read.

### Wave 2

#### HR1 Cut the first fork Host release (S). **Release: stop and ask.**

- **Goal:** publish `host-v1.0.0` (Host version `1.0.0+monoptah`) with only H1's
  changes. Design §1.2, rule D1.
- **Depends on:** H1 merged.
- **What the agent must do:**
  1. Prepare: run `npm run host:release -- 1.0.0` in a worktree from the feature branch.
     That commits the version and creates the tag locally.
  2. **Stop and ask** the user:
     - The tag must point at a commit GitHub has. Push the feature branch and the tag? Or
       cherry-pick H1 onto a branch merged to `main` first?
     - The tag push starts `host-release-monoptah.yml`.
  3. After the user pushes, or allows you to push, watch the workflow with
     `gh run watch`. Then check:
     - the release `host-v1.0.0` exists, is not "latest", and has every asset and
       `.sha256`;
     - `host-release.json` is present;
     - the attestation verifies (`gh attestation verify`).
- **Acceptance:**
  - [ ] The release is published with every target and `host-release.json`.
  - [ ] `gh release view host-v1.0.0 --json isLatest` reports false.
  - [ ] The tracker records the run URL.
- **Out of scope:** pinning (H2a).

#### H2a App pins the fork Host (M)

- **Goal:** the app downloads the fork Host at the pinned version and hashes, identifies
  Host flavor and version, and caches Host info. Design §1.2 H1 (pinned checksums), H2
  (`HostInfo`), rules D1 and D3.
- **Depends on:** HR1 (needs real hashes).
- **Files:**
  - new `core/remote/host_release.rs` (`HOST_VERSION = "1.0.0+monoptah"`, the release
    base URL `https://github.com/hhoangg/monoptah/releases/download/host-v1.0.0`,
    `HOST_SHA256` per target);
  - `core/remote/ssh.rs` (`bootstrap_script_from_template` ~699-721; the
    `upgrade_script` substitutions);
  - `core/remote/remote_bootstrap.sh`;
  - `core/remote/remote_bootstrap_detached.sh`, if it embeds the version;
  - `core/remote/remote_bootstrap.ps1` (Windows SSH machines);
  - new `scripts/pin-host-release.mjs` (+ test);
  - new `core/remote/host_info.rs` (`HostInfo {flavor, version, capabilities}` cache
    keyed by machine id, filled on connect and describe, dropped on disconnect), wired in
    `core/remote.rs`;
  - replace the ad-hoc probes in `core/cloud_sessions/view.rs` (`Fetcher::has_bytes`
    ~367), `gpui/conversation/remote.rs` (~586) and `gpui/settings/pages/connections.rs`
    (~88);
  - `scripts/check-fork-branding.sh`: remove the `remote/ssh.rs ... hardbeat920/monocode/releases`
    allow entry added in `c36ed8f6`.
- **Plan:**
  1. `scripts/pin-host-release.mjs host-vX.Y.Z`:
     - downloads `host-release.json` from that release's workflow run artifact
       (`gh run download`);
     - verifies its attestation;
     - cross-checks it against the published `.sha256` assets;
     - rewrites `host_release.rs`.
  2. **Script changes:**
     - `EXPECTED` comes from a substituted `@@SHA256@@` for the detected target, instead
       of the downloaded `.sha256` file. Keep downloading the `.sha256` only as a
       cross-check that must match.
     - Version check: equal to `@@VERSION@@`.
     - Add a POSIX-sh helper `is_older_or_foreign <installed> <pin>`: true when the
       installed version lacks `+monoptah`, or its semver core is lower.
     - Use the helper only where H2b needs it. In H2a the reinstall rule stays "missing or
       FORCE_UPGRADE".
  3. Fill `HostInfo` from `environment.describe`. Treat a missing `host` field as
     `{flavor: "upstream"}`.
  4. Settings > Connections shows the Host flavor and version.
- **Acceptance:**
  - [ ] The bootstrap snapshot test shows the fork URL, the version and the pinned hash.
  - [ ] A bootstrap shell test with fake packages (`sh` + a stub `monocode-host`) passes.
    A wrong hash fails with "checksum mismatch".
  - [ ] `HostInfo` tests cover an upstream describe, a fork describe and the cache drop on
    disconnect.
  - [ ] `./scripts/check-fork-branding.sh` exits 0 without the exception.
  - [ ] `node --test scripts/pin-host-release.test.mjs` passes, with the network mocked.
  - [ ] `npm run native:check` passes.
  - [ ] **Manual (ask the user):** a **new** codespace or sandbox start installs
    `1.0.0+monoptah`, and `environment.describe` shows `host.flavor == "monoptah"`.
- **Out of scope:** upgrading existing Hosts (H2b).
- **Risks:**
  - Windows `.ps1` parity.
  - The Tauri copy `src-tauri/src/remote_bootstrap.sh` is **not** changed (the Tauri app
    is out of scope).

#### H2b Automatic two-phase cloud Host upgrade (M)

- **Goal:** cloud Hosts upgrade to the pin at connect, with no prompt, deferred only while
  a session runs. Design §1.2 H2 "Two-phase upgrade", rule D2.
- **Depends on:** H2a.
- **Files:**
  - `core/remote.rs` (connect flow ~600-640, `StoredMachine` gets
    `pending_host_upgrade: bool` with serde default);
  - `core/remote/ssh.rs` (`upgrade_script` ~767-775);
  - `core/cloud_sessions/docker_start.rs` (`sandbox_connect_step` ~1486-1550,
    `ClaudeAuth::prelude` ~195-228): the upgrade must use the same prelude;
  - `core/cloud_sessions/docker_start/reconnect.rs`;
  - `gpui/conversation/remote.rs` (~736-753 copy);
  - `core/cloud_sessions/view.rs` (~496 copy);
  - the cloud panel status line (`gpui/conversation/cloud_panel.rs`).
- **Plan:**
  1. After phase 1 (today's bootstrap) and the tunnel:
     - read `HostInfo` and `sessions.list` for every project;
     - if the Host is older or foreign and nothing is running, run `upgrade_script`
       (codespace: `gh codespace ssh`; sandbox: the container exec with the token
       prelude), then reconnect the tunnel;
     - if something is running, set `pending_host_upgrade` and retry on the next connect.
  2. The status line shows "Updating Monoptah Host…" or "Monoptah Host X will be
     installed when the current turn ends".
  3. Replace the "Update Monoptah Host in Connections settings" copy for cloud sessions.
  4. User SSH machines: unchanged.
- **Acceptance:**
  - [ ] Tests with a fake Host and a fake ssh runner:
    - older and idle → upgrade, then reconnect;
    - older and running → deferred, flag set;
    - next idle connect → upgrade, flag cleared;
    - newer fork → no upgrade;
    - upstream at a higher version → upgrade;
    - a failed download → old Host kept, warning, not fatal.
  - [ ] The sandbox path includes the prelude, and no secret appears in the logs (scrub
    test).
  - [ ] `npm run native:check` passes.
  - [ ] **Manual (ask the user):** an existing codespace running the upstream Host
    upgrades on connect.
- **Out of scope:** F1-4 UI.
- **Risks:**
  - A tunnel race during the restart: reuse the tunnel-invalidation path (`remote.rs`
    ~498-511).

#### F1-4 Delete the copy workaround; files open directly (S-M)

- **Goal:** clicking a cloud file always opens it. The copy path is deleted, large files
  stream and open automatically, and an open is queued while an upgrade is deferred.
  Design §1.2 F1-4, rule D4.
- **Depends on:** H2b (ships in the same release or right after).
- **Files:**
  - `core/cloud_sessions/view_cache.rs` (remove `copy_cloud_file` ~405-495 and its
    helpers if unused);
  - `core/cloud_sessions/view.rs` (remove `ViewError::OldHost`, `Opened::CopyOnly`, the
    `OUT_OF_DATE` copy wording);
  - `gpui/shell/cloud_view.rs` (remove `Outcome::CopyOnly`, `Phase::CopyOffer`,
    `start_cloud_copy`, the Download button; add the queued-open state);
  - `gpui/shell/cloud_view_tests.rs`;
  - `core/cloud_sessions/view_tests.rs`.
- **Plan:**
  1. Remove the copy path. Keep `CloudBackend::copy_from` (evidence and convert use it).
  2. A large file auto-opens after its streaming download completes (only above
     `CONFIRM_BYTES` = 500 MB, ask first).
  3. Capability missing:
     - with `pending_host_upgrade`: show "opens after the current turn", queue the path,
       and open it when a later connect finishes the upgrade;
     - with an upgrade failure: show "Monoptah Host could not be updated: <host> blocked
       by the sandbox network policy" plus the `sbx policy allow` hint;
     - no Download button in any case.
- **Acceptance:**
  - [ ] `rg -n "copy_cloud_file|CopyOffer|CopyOnly|OldHost" crates/` finds nothing.
  - [ ] Tests:
    - small image → in-memory open;
    - 20 MB PDF → streamed with progress, then opened;
    - 600 MB → confirmation;
    - missing capability + pending upgrade → queued, then opened after the upgrade;
    - missing capability + blocked → card with no download action.
  - [ ] `npm run native:check` passes.
- **Out of scope:** editor reads (F1-3).

#### F2-1 Explicit panel root, Cloud/Local switch, remote git policy (M)

- **Goal:** Files/Changes bind explicitly to a `PanelRoot` with a Cloud/Local switch.
  Remote git writes are allowed under the policy, and the panels show a connection strip.
  Design §2.2 (intro), §2.2.4 local-only actions, §2.2.5, rule D5.
- **Depends on:** F2-0.
- **Files:**
  - `gpui/shell.rs` (`sync_git` ~1921);
  - `gpui/shell/cloud.rs` (`sync_cloud_panel`, `banner_text`, `active_cloud`);
  - `gpui/git_gate.rs`;
  - `gpui/changes.rs` (~478, 728, 817);
  - `gpui/review.rs` (~1300-1303, 1400);
  - `gpui/file_editor/diff.rs` (~82);
  - `gpui/right_panel.rs` (banner ~332-350, ~677-690: add the switch and the strip);
  - `gpui/explorer.rs` (disable Reveal / Open with default app / external editor for
    remote; add "Open in browser" for codespaces via `CloudBackend::web_url`);
  - `core/files.rs` (~436, 471 errors stay).
- **Plan:**
  1. `PanelRoot { Local, Cloud { root, cloud, local_project }, CloudPending { cloud } }`,
     resolved in `sync_git`. The root is the session's Host worktree, else the project
     key.
  2. The switch state is per session, in memory, default Cloud. The banner text follows
     the mode.
  3. `git_gate`: refuse writes only when the mode is Local and a cloud session is in view.
  4. **Policy** (design §2.2.5 table):
     - discard: confirm, and refuse while a turn runs (app-side check for now);
     - commit while running: confirm;
     - checkout / branch / stash / worktrees: only when idle, with a confirmation;
     - push/pull/sync/PR: allowed with today's confirmations.
  5. **Connection strip:** Connected / Not connected - Connect / Workspace starting. The
     Connect action uses `gpui/cloud_wake.rs`. Never wake from background code.
- **Acceptance:**
  - [ ] Tests:
    - root resolution (cloud, worktree, pending, Local switch, Mono active);
    - the gate refuses only in Local mode;
    - discard is refused while running;
    - local-only actions are disabled for `remote://` roots.
  - [ ] No background path calls a wake function (assert in tests with a counting fake).
  - [ ] `npm run native:check` passes.
  - [ ] **Manual (ask the user):** in a cloud session, toggle Cloud/Local, stage and
    commit remotely, and push to a scratch branch.
- **Out of scope:** the disconnected cache (F2-2), conditional save (F2-3), watch (F2-4),
  Host idle guard (F2-5h).
- **Risks:** the app-side running check is racy until F2-5h. Say so in the code comment.

#### F2-2 Disconnected state and cached panels (S-M)

- **Goal:** when the tunnel is down, the panels show the last tree and index read-only
  and dimmed, with a Connect action. There is an LRU per remote root. Design §2.2.3.
- **Depends on:** F2-1.
- **Files:** `gpui/git_status.rs` (snapshot per project, `set_project`, `reload`),
  `gpui/explorer.rs` (`dirs` cache, `saved`), `gpui/right_panel.rs`, and
  `core/cloud_sessions.rs` (`is_not_connected`).
- **Plan:**
  1. An LRU of 16 remote roots holding the listing and the index snapshot.
  2. On `is_not_connected`: show the cached data dimmed, disable writes, and show the
     strip.
  3. Connect is a user action. After it, do a full reload.
  4. Stop polling while disconnected.
- **Acceptance:**
  - [ ] Tests: a disconnect keeps the cached tree and disables writes; Connect calls
    reconnect exactly once; polling stops while disconnected and resumes after.
  - [ ] `npm run native:check` passes.

#### F3-2 Settings > Cloud: the global kit (M)

- **Goal:** one global selection UI with defaults and the one-time notice. The start
  panel gets a summary and an on/off switch, and the app API gets a `kit` option. Design
  §3.2.2, rules D6 and D11.
- **Depends on:** F3-1.
- **Files:**
  - a new page `gpui/settings/pages/cloud.rs`, registered in
    `gpui/settings/pages/mod.rs`. Look at `mcp.rs` and `skills.rs` there for list UIs;
  - global preferences storage in `core/desktop_preferences*` (`cloudKit`: selection,
    enabled, `notice_acknowledged`);
  - `gpui/conversation/cloud_panel.rs` and `gpui/shell/cloud_picker.rs` (the summary row
    and the switch);
  - `core/mono/cloud_api/codespace_provider.rs`, `sandbox_provider.rs` and `validate.rs`
    (the `kit: "none" | "default"` target option).
- **Plan:**
  1. Three tabs, checkboxes, size and compat badges as in §3.2.2. Local-only items cannot
     be checked.
  2. **Defaults:** everything compatible is checked, secrets and sign-ins included.
  3. **One-time notice:** the exact text in §3.2.2, stored as acknowledged.
  4. The start panel and picker show the summary and a per-start on/off switch.
  5. The app API may pass `kit: "none"`, but can never add items.
  6. Ship behind a flag (`cloudKitEnabled`, off) until F3-4 lands.
- **Acceptance:**
  - [ ] Tests:
    - defaults from an inventory fixture;
    - local-only items are not selectable;
    - the notice shows once;
    - the API `kit: "none"` disables the kit;
    - an API input with extra items is rejected.
  - [ ] `npm run native:check` passes.

#### F4-L Session location model (L). Split into sub-PRs if it grows.

- **Goal:** one sidebar session that can live locally or in the cloud. While it is Cloud,
  its runtime is remote and remote rows are appended to the local DB session. It keeps a
  single writer. Design §4.2.1, §4.9 R2, R6 and R8, rules D9 and D10.
- **Depends on:** F4-0. Use its row-append notes.
- **Files:**
  - `core/store.rs` and `core/store/remote.rs` (the `cloud-location` session setting,
    `RemoteSession.linked_to`, append API);
  - `gpui/runtime/remote.rs` (~683-760: write the projected suffix to the linked local
    session, idempotently);
  - `gpui/runtime.rs` (choose the runtime by location);
  - `gpui/conversation/remote.rs`;
  - `gpui/shell/cloud.rs` (`collect` ~87-131, `cloud_marks`, `is_cloud`, `project_of`:
    driven by the location; hide linked RemoteSessions);
  - `core/tui_launch.rs` (refuse while Cloud);
  - `core/checkpoint*` and rewind UI (disable before the move point);
  - `core/provider_cleanup.rs` (the delete notice).
- **Suggested sub-PRs:**
  1. store and location types, with tests;
  2. runtime selection and row append;
  3. sidebar and cloud UI driven by location;
  4. single-writer guards (local runtime, TUI) and rewind gating.
- **Acceptance:**
  - [ ] Tests:
    - location flips;
    - remote rows are appended once (re-sync makes no duplicates);
    - a linked RemoteSession is hidden;
    - the local runtime and TUI launch are refused while Cloud;
    - rewind before the move point is disabled.
  - [ ] Existing cloud-born sessions behave exactly as before (the existing
    `cloud_tests.rs` pass unchanged).
  - [ ] `npm run native:check` passes.
- **Out of scope:** the move itself (F4-3a), the return (F4-5).
- **Risks:** the size. Keep each sub-PR reviewable, and run an Opus review per sub-PR.

### Wave 3 (Host batch 2, then the app parts)

Host tickets (`*h`, H3) change `host/`. Their checks are `npm run host:build` and
`npm run test:host`, plus `npm run native:check` when the Rust client changes. Users only
get them after HR2.

#### H3 Host uploads (M)

- **Goal:** chunked, resumable, verified uploads over the tunnel, with a Rust client.
  Design §3.2.3 H3.
- **Depends on:** H2a.
- **Files:**
  - new `host/uploads.ts` (+ test);
  - `host/server.ts` (methods `uploads.begin/chunk/commit/discard`, capability `uploads`);
  - `core/remote.rs` (`supported_remote_method` ~532);
  - new `core/remote/uploads.rs` client.
- **Plan:**
  - staging `~/.monocode-host/uploads/<id>/`, mode 0700;
  - a 512 KB chunk size;
  - idempotent offsets;
  - sha256 verified at commit;
  - a 500 MB cap;
  - expiry after 1 h when not consumed;
  - names validated (one plain file name).
- **Acceptance:**
  - [ ] Host tests: chunking, resume, hash mismatch, cap, expiry, name validation.
  - [ ] Rust client test against a fake Host.
  - [ ] All checks pass.

#### F2-3h Host conditional write (S)

- **Goal:** `write_text_file` accepts `expectedSha256` and writes to a temp file, then
  renames, keeping the mode and writing through in-workspace symlinks. Design §2.2.4,
  §2.4.
- **Depends on:** H2a.
- **Files:** `host/workspace-commands.ts` (`write_text_file` ~345) and its tests.
  Capability `files.write.hash`.
- **Acceptance:**
  - [ ] Tests: a hash mismatch fails with "File changed on the host"; the mode is
    preserved; the symlink target inside the workspace is updated; a link out of the
    workspace is refused; content over 1 MB is refused.

#### F2-4h Host watch (M)

- **Goal:** non-recursive watches on git metadata and the client's loaded directories,
  with a long-poll. Design §2.2.2.
- **Depends on:** H2a.
- **Files:**
  - new `host/workspace-watch.ts` (+ test);
  - `host/workspace-commands.ts` (`watch_changes`, `update_watch`, `wait_changes`,
    `unwatch_changes`);
  - `host/server.ts` (close the watches on shutdown).
  - Borrow the lease and batching from `git show origin/codex/event-driven-git-refresh:host/git-watch.ts`.
    Do **not** use `recursive: true`.
- **Plan:**
  - watch git metadata (HEAD, index, refs, packed-refs, and the common dir for
    worktrees) and at most 256 directories from the client;
  - 100 ms batching;
  - wait at most 15 s;
  - a 60 s lease;
  - at most 128 watchers;
  - an unknown id returns an error the client re-subscribes on.
- **Acceptance:**
  - [ ] Tests: a git change → `git: true`; a file in a watched dir → `dirs` includes it;
    a file in an unwatched dir → no event; lease expiry; the watcher cap; no recursive
    watch (assert the `fs.watch` options).

#### F2-5h Host git idle guard and timeouts (S)

- **Goal:** `git_discard_*` and `git_stash` run under the idle guard. Push, pull and sync
  timeouts become 90 s. Design §2.2.5.
- **Depends on:** H2a.
- **Files:** `host/workspace-commands.ts` (~165-208, timeouts at ~495, 542, 593, 718) and
  `host/engine.ts` (`withIdleProject` ~300). Capability `git.idleGuard`.
- **Acceptance:**
  - [ ] Tests: discard and stash are refused while a session is running; the timeouts are
    90 s.

#### HR2 Release Host batch 2 and pin it (S). **Release: stop and ask.**

- **Depends on:** H3, F2-3h, F2-4h and F2-5h merged.
- **Steps:**
  1. Run `npm run host:release -- 1.1.0`.
  2. **Stop and ask** about pushing, as in HR1.
  3. Check the release.
  4. Run `scripts/pin-host-release.mjs host-v1.1.0` in a ticket worktree. That updates
     `host_release.rs`.
  5. Run the checks, an Opus review of the pin diff, then merge.
- **Acceptance:**
  - [ ] The release is verified as in HR1.
  - [ ] The app pins `1.1.0+monoptah`.
  - [ ] A **new** start installs it (manual, ask the user).

#### F2-3 Editor conditional save and conflicts (M)

- **Goal:** remote saves use `expectedSha256`, show a conflict compare dialog, show an
  "agent is working" notice, and open 1-2 MB text read-only. Design §2.2.4.
- **Depends on:** HR2, F2-1.
- **Files:** `core/files/document.rs` (`save_document` ~302-320), `core/files/remote.rs`
  (`host_arguments`), `gpui/file_editor.rs` (`save` ~859, `disk_changed` ~1387), and a
  new conflict dialog in `gpui/file_editor/`.
- **Plan:** use the capability `files.write.hash`. Without it, keep today's
  re-read-and-compare.
- **Acceptance:**
  - [ ] Tests: a matching hash saves; a mismatch opens the dialog; Keep mine overwrites
    with the new hash; Take cloud reloads; a 1.5 MB file is read-only; without the
    capability the old path is used.

#### F2-4 Watch client (M)

- **Goal:** `ProjectGit` and the Explorer use `workspace.watch` long-polls, with polling
  fallback and focus/idle rules. Design §2.2.2.
- **Depends on:** HR2, F2-1.
- **Files:** `gpui/git_status.rs` (`set_polling` ~144-191), `gpui/explorer.rs`
  (`refresh_loaded`, and the expanded-dirs list sent through `update_watch`), and
  `core/files/remote.rs` (allowlist the new commands).
- **Plan:**
  - one watch per remote root;
  - `git` triggers a reload, `dirs` refreshes those folders, `overflow` refreshes every
    loaded folder;
  - a 10 s status poll runs alongside;
  - without the capability, the 2 s git poll plus a 5 s re-list of loaded dirs;
  - watching only while the panel is visible and the window focused;
  - 15 s polling when unfocused;
  - stop after 10 minutes without input, and stop when disconnected.
- **Acceptance:**
  - [ ] Tests with a fake Host: event routing; re-subscribe on an unknown id; the
    fallback; focus and idle transitions; nothing runs while disconnected.

#### F2-5 Client timeouts for push, pull and sync (S)

- **Goal:** `rpc_with` takes a per-request timeout, 120 s for push, pull and sync. A
  timeout says "may still be running" and refreshes ahead/behind. Design §2.2.5.
- **Depends on:** HR2.
- **Files:** `core/remote.rs` (`rpc_with` ~269-340), `core/scm.rs` (`push`, `pull`,
  `sync` remote paths) and `gpui/changes.rs`.
- **Acceptance:**
  - [ ] Tests: the timeout is passed through; a timeout gives the "may still be running"
    state and an ahead/behind refresh.

### Wave 4 (Host batch 3, then features 3 and 4)

#### F3-3h Host kit (M)

- **Goal:** the Host applies the kit, stores secrets 0600, passes `--plugin-dir` /
  `--mcp-config`, and loads secrets into the Claude child env only. Design §3.2.3, rules
  D6 and D7.
- **Depends on:** HR2 (uploads).
- **Files:**
  - new `host/kit.ts` (+ test): `kit.apply`, `kit.secrets`, `kit.probe`, `kit.health`;
  - `host/server.ts`;
  - `host/child-backend.ts` (~358);
  - `src/integrations/harness/core/child.ts` (~350: an `env` overlay parameter);
  - `src/integrations/harness/providers/claude/claudeProtocol.ts` (`buildClaudeArgs`
    ~250-295: `input.kit`; isolated spawns unchanged);
  - `src/integrations/harness/providers/claude/claude.ts` (the `settingsKey` includes the
    kit hash and the secrets-file hash);
  - Host log scrubbing.
- **Plan:**
  - unpack the tar in TypeScript into a fresh `O_EXCL` dir under `~/.monoptah-kit/<hash>`
    (regular files and dirs only, no `..`, no absolute paths, no links), then flip
    `current`;
  - managed skill symlinks in `~/.claude/skills` are recorded and never clobber real
    skills;
  - secrets go to `~/.monoptah-kit/secrets.env` (temp file, rename, 0600);
  - validate every input (hash hex 64, names).
- **Acceptance:**
  - [ ] Host tests:
    - path traversal and link entries are refused;
    - a skill collision keeps the remote skill;
    - interactive spawns get `--plugin-dir` and `--mcp-config`, isolated ones do not;
    - secrets appear in the child env and **not** in `process.env`, logs or error text;
    - a changed kit hash respawns the process.
  - [ ] `npm run check:web` passes (shared TS).

#### F3-5h Host MCP sign-in merge (S-M)

- **Goal:** merge the copied MCP sign-ins into the remote Claude `.credentials.json`.
  Only the MCP keys; temp file and rename; 0600; tracked in `signins.json`. Design
  §3.2.5, rule D7.
- **Depends on:** HR2, F3-5a (the key mapping).
- **Files:** `host/kit.ts` (`mcpSignIns` in `kit.secrets`) and tests.
- **If F3-5a found the format unusable:** this ticket only marks those servers "Sign in
  in the cloud". Record that in the tracker.
- **Acceptance:**
  - [ ] Tests: Claude's own login keys are untouched; the merge is idempotent; removal of
    tracked keys works; the file mode is 0600.

#### F4-2h Host `applySnapshot` and `sessions.import` (M)

- **Goal:** restore code and import a transcript under the same id, on the Host. Design
  §4.2.2 steps 5-6, §4.9, rule D10.
- **Depends on:** HR2, F4-0, F4-J (shared fixtures).
- **Files:**
  - new `host/move.ts` (+ test);
  - `host/server.ts`;
  - `host/engine.ts` (store record + `provider.bind`, ~282-286);
  - a TypeScript mirror of F4-J's `decide`, tested with the **same** fixture files.
- **Plan:**
  - `applySnapshot` runs under `withIdleProject`:
    - target the project root on a new instance, else a new Host worktree;
    - verify `HEAD == head`;
    - `git apply --binary`;
    - verify the `write-tree` of a temp index equals `tree`;
    - validate hex ids, `branch` with `check-ref-format`, and the patch paths.
  - `sessions.import`:
    - install the JSONL by the F4-J rule;
    - refuse when a live session binds that `providerSessionId`;
    - write the record and bind.
- **Acceptance:**
  - [ ] Host tests: tree verification passes and fails; the patch cannot touch `.git/` or
    leave the target; a shared instance uses a worktree; import extend / replace / fork
    gives the same decisions as the Rust fixtures; a live-binding refusal; the first
    turn passes `--resume <same id>` and no `--fork-session`.

#### HR3 Release Host batch 3 and pin it (S). **Release: stop and ask.**

- Same as HR2, with version `1.2.0`.
- **Depends on:** F3-3h, F3-5h and F4-2h merged.

#### F3-4 "Claude setup" start step (M)

- **Goal:** new-workspace starts run a one-way setup step before `Ready`: probe, build the
  kit, upload, apply, write secrets, health check. There is also an "Update Claude setup"
  action. Design §3.2.4, rule D7.
- **Depends on:** F3-2, HR3.
- **Files:**
  - `core/cloud_sessions/startup.rs` and `core/cloud_sessions/docker_start.rs` (a new
    step id `ClaudeSetup`, skipped by old start records);
  - `core/cloud_sessions/kit.rs` (tar builder, deterministic);
  - `gpui/conversation/cloud_panel.rs` (health list, the Update action, "local setup
    changed since start");
  - `gpui/transcript/cloud_start.rs` (the step row).
- **Plan:**
  - run only for starts that create a workspace, never on reconnect or attach;
  - the step never fails the start: warnings only;
  - parse the health output with `core/mcp.rs::parse_claude_health`;
  - give sandbox network-policy hints;
  - remove the F3-2 flag.
- **Acceptance:**
  - [ ] Tests: the step runs on a new start and not on reconnect or attach; a missing
    runtime drops the item with a warning; a health parse; a failure stays a warning; no
    secret appears in the start log (scrub test).
  - [ ] Design §7 "unapproved project MCP servers" is answered from the observed health
    output.
  - [ ] **Manual (ask the user):** a new codespace start with the kit, and the agent can
    use a skill and an MCP server on its first turn.

#### F3-5 App side of MCP sign-ins (M). **Needs the user at the machine** for the manual check (Keychain prompt).

- **Goal:** read the selected servers' local sign-ins and send them once in the setup
  step. Health shows "needs sign-in". Re-copying happens only through "Update Claude
  setup". Design §3.2.5, rule D7.
- **Depends on:** F3-4, F3-5h, F3-5a.
- **Files:** `core/cloud_sessions/kit.rs` (implement the `signed_in` seam from F3-1 and
  the readers) and a new `core/cloud_sessions/kit_keychain.rs` (macOS `security`
  invocation for the account's entry, the Linux/Windows file reader).
- **Acceptance:**
  - [ ] Tests with a fake credential source: only the selected servers are sent; values
    never appear in `Debug` output or logs; no send on reconnect.
  - [ ] **Manual, with the user:** a server signed in locally works in a new codespace
    without signing in.

#### F4-3a Local to cloud orchestrator (M-L)

- **Goal:** `MoveRecord`-driven moves with: push, workspace start or attach, uploads,
  restore, verify, same-id import, version check, moved-path note, location flip,
  rollback and cancel. Design §4.2.2, §4.9 R4 and R5, rules D8, D9 and D10.
- **Depends on:** F4-1, F4-J, F4-L, HR3.
- **Files:**
  - new `core/cloud_sessions/move.rs` (+ tests);
  - `core/cloud_sessions/startup.rs` and `docker_start.rs` (start on the pushed branch;
    sandbox "feature == base" support in `backend/docker_provision.sh` clone phase);
  - `core/scm.rs` (`push_for`).
- **Plan:**
  - follow steps 1-7 of §4.2.2 exactly;
  - persist after each step;
  - Retry continues at the failed step;
  - Cancel follows the rollback list;
  - the version check uses the JSONL `version` fields against the remote
    `claude --version`, and runs `claude update` on the remote when it is older;
  - the first cloud turn gets the moved-path note;
  - refuse while the session is busy, or when HEAD or the last row changed before the
    flip.
- **Acceptance:**
  - [ ] Tests with fakes for git, the Host and backends:
    - every step is idempotent;
    - resume after a restart at each step;
    - a cancel rolls back what was created;
    - a push is never forced and never targets the default branch;
    - ignored files are never uploaded;
    - the same id reaches `sessions.import`;
    - a non-Claude session gets the summary path with a new id;
    - LFS and submodule warnings.
  - [ ] **Manual (ask the user):** move a real local session with commits and dirty
    files to a new codespace. The cloud continues the conversation, with the same
    session id in the JSONL.

#### F4-4 "Continue in cloud" UI (M)

- **Goal:** a one-confirmation dialog, progress steps and a transcript divider. Design
  §4.2.2 dialog, §4.5.
- **Depends on:** F4-3a.
- **Files:** `gpui/shell/session_groups.rs` (menu), `gpui/conversation/cloud_target.rs`
  (the composer menu for sessions with messages), a new dialog module in `gpui/shell/`,
  and `gpui/transcript/cloud_start.rs` (steps).
- **Acceptance:**
  - [ ] Tests: the dialog lists the push target, the counts and the ignored files; it is
    refused while busy; the progress maps `MoveRecord` steps; the divider renders.
  - [ ] **Manual (ask the user):** run it end to end once.

#### F4-5 Cloud to local, one session, same id (M)

- **Goal:** "Continue locally" for moved and cloud-born sessions:
  - apply in place when the local tree is unchanged since the move, else into a new
    worktree;
  - convert no longer archives, and it replaces the cloud row;
  - the same id goes through F4-J (fork on divergence, a new id after a lost workspace).

  Design §4.2.3, §4.9 R1 and R7, rules D9 and D10.
- **Depends on:** F4-L, F4-3a, F4-J.
- **Files:** `core/cloud_sessions/convert.rs` (entry points ~619/635, `finish` ~1041,
  `restore_code` ~1707), `gpui/shell/cloud.rs` (`adopt_converted` ~1703, archive flows),
  and `gpui/shell/session_groups.rs`.
- **Acceptance:**
  - [ ] Tests: a round trip local→cloud→local keeps one session and one id, and the
    local JSONL contains the cloud turns (R1 fixed); local changes force a worktree
    restore; divergence forks with a message; a deleted workspace continues on a new id
    with a summary.
  - [ ] The existing archive flow (Keep/Stop/Delete) still archives when the user chose
    archive.
  - [ ] **Manual (ask the user):** a round trip on a real codespace.

#### F4-3b App API `sessions.moveToCloud` / `sessions.moveToLocal` (S)

- **Goal:** expose the moves to the app API under rule D11. Design §4.4.
- **Depends on:** F4-4, F4-5.
- **Files:** `core/mono/cloud_api.rs`, `core/app_access.rs` (allowlist ~21-50) and the
  API docs, if any exist for `cloud.*`.
- **Acceptance:**
  - [ ] Tests: validation; busy refusal; the push rules are enforced; the API cannot
    widen the kit.

---

## 4. Progress tracker

The implementing session updates this table on the feature branch: at the start of each
ticket, after review and after merge.

- Status values: `todo`, `in progress`, `in review`, `merged`, `blocked: <reason>`,
  `skipped: <reason>`.
- Worktree: the path, or `-`.
- Notes: the merge commit, the release run URL, review findings, follow-ups.

| Ticket | Wave | Status | Worktree | Notes |
|---|---|---|---|---|
| F2-0 | 1 | todo | - | |
| F1-2 | 1 | todo | - | |
| F1-3 | 1 | todo | - | |
| H1 | 1 | merged | csi-h1 | merge 9b6cba91. Opus review: fixed first-release tag path, gh not-found check, dirty tree on commit fail. actionlint unavailable. Note: describe.host.version is bare X.Y.Z; --version prints X.Y.Z+monoptah. |
| F3-1 | 1 | todo | - | |
| F3-5a | 1 | todo | - | needs the user (Keychain) |
| F4-0 | 1 | todo | - | needs the user's OK (model requests) |
| F4-J | 1 | todo | - | |
| F4-1 | 1 | todo | - | |
| A1 | 1 | todo | - | needs step 0 (`origin/main` merge); local sessions only, account + cloud target = validation error |
| HR1 | 2 | todo | - | release: stop and ask |
| H2a | 2 | todo | - | |
| H2b | 2 | todo | - | |
| F1-4 | 2 | todo | - | |
| F2-1 | 2 | todo | - | |
| F2-2 | 2 | todo | - | |
| F3-2 | 2 | todo | - | |
| F4-L | 2 | todo | - | L, sub-PRs |
| H3 | 3 | todo | - | |
| F2-3h | 3 | todo | - | |
| F2-4h | 3 | todo | - | |
| F2-5h | 3 | todo | - | |
| HR2 | 3 | todo | - | release: stop and ask |
| F2-3 | 3 | todo | - | |
| F2-4 | 3 | todo | - | |
| F2-5 | 3 | todo | - | |
| F3-3h | 4 | todo | - | |
| F3-5h | 4 | todo | - | |
| F4-2h | 4 | todo | - | |
| HR3 | 4 | todo | - | release: stop and ask |
| F3-4 | 4 | todo | - | |
| F3-5 | 4 | todo | - | needs the user for the manual check |
| F4-3a | 4 | todo | - | |
| F4-4 | 4 | todo | - | |
| F4-5 | 4 | todo | - | |
| F4-3b | 4 | todo | - | |

---

## 5. Unverified assumptions and who verifies them

These come from design §7. Do not rely on an item before its ticket has recorded an
answer in the design doc.

| Assumption (design §7) | Verified by | If the answer is bad |
|---|---|---|
| The running app shows the remote tree for cloud sessions (known from code only) | F2-0 | Fix the binding in F2-1 before anything else in feature 2 |
| `list_dir` entry cap | F2-0 | F2-1 adds the client cap and the optional Host `limit` |
| Tunnel RPC traffic resets the codespace idle timeout or the sandbox `--ttl` | F2-0 (manual, if the user agrees) | F2-4's 10-minute idle stop becomes mandatory, not just a cost guard |
| Where and how Claude Code stores MCP OAuth sign-ins; refresh-token rotation | F3-5a | F3-5h and F3-5 fall back to "Sign in in the cloud" for those servers |
| `--resume <id>` keeps the id and appends after a cross-machine install | F4-0 | Stop and ask before F4-J, F4-L, F4-2h and F4-3a |
| How Claude Code merges `~/.claude` with an account `CLAUDE_CONFIG_DIR` | F3-1 | The inventory labels sources and reads both; adjust the precedence |
| How headless (stream-json) Claude treats unapproved project `.mcp.json` servers | F3-4 (health output) | Add a follow-up ticket for pre-approval if the user wants it |
| Whether `git push` uploads LFS content in every setup | F4-1 / F4-3a (detect and warn) | Keep the warning; no LFS transfer in v1 |
| `claude plugin` / `claude mcp add-json` flags | not needed (the chosen design uses `--plugin-dir` and `--mcp-config`) | - |
| `sbx mcp`, `sbx skills`, `--static-mcp` on cloud sandboxes | not used | - |
