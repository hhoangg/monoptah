# Cloud setup (native)

How a user configures cloud sessions (GitHub Codespaces and Docker Sandbox) in Monoptah
Native. Native only; the Tauri app is not covered. Mockup agreed with the user:
three screens (Settings > Cloud, the project's Cloud settings dialog, the composer's
RUN ON CLOUD panel).

## Problem

Before this change, cloud configuration was spread over four places with different fields:

| Place | File | What it held |
| --- | --- | --- |
| Codespaces setup dialog | `crates/monocode-gpui/src/codespaces_dialog.rs` | GitHub user Codespaces secrets only (`CLAUDE_CODE_OAUTH_TOKEN` through `claude setup-token`). Nothing stored locally; machine `basicLinux32gb`, idle timeout 30 minutes and retention 72 hours were hard-coded in `crates/monocode-core/src/codespaces.rs`. |
| Docker Sandbox setup dialog | `crates/monocode-gpui/src/docker_dialog.rs` | Repo URL, base branch, devcontainer path, Git credential, Claude account, compute, runtime limit (`DockerProjectConfig`, setting `docker-sandbox-projects.v1`). |
| Settings > Cloud | `crates/monocode-gpui/src/settings/pages/cloud.rs` | The Claude setup kit. |
| Composer RUN ON CLOUD | `conversation/cloud_target.rs`, `conversation/cloud_panel.rs` | Per-session overrides; rows differ per backend and overrides are not marked. |

A codespace could not use an app Claude account. `sbx login` is one Docker account per
machine (macOS Keychain), so several Docker accounts could not be used side by side.

## Design: two layers, one place each, both backends in one frame

### Layer 1: Settings > Cloud (user level, every project)

1. **Backends**: a checklist per backend with Check again and Fix.
   - Codespaces: `gh` installed and signed in, `codespace` scope.
     Fix runs `gh auth login` or `gh auth refresh -h github.com -s codespace`.
   - Docker Sandbox: `sbx` installed, ssh set up (the `sbx_cloud` alias,
     `sandboxes::ssh_alias_configured`). Fix runs `sbx --cloud setup ssh`.
   - Fix opens the shared fix terminal (a small dialog with an embedded terminal, the same
     `Terminal::launch` pattern the old Codespaces dialog used for `claude setup-token`),
     then checks again when the command exits. A token is passed only through the
     terminal's environment, never typed into the command line.
2. **Docker accounts** (new): one row per account with Docker ID, status, token expiry
   (when known; Docker's token exchange does not report it, so it reads "Not known" until a
   source exists), and the projects that use it. Actions: Add, Replace token, Remove.
   - Add form: Docker ID and Personal access token (PAT, `dckr_pat_...`), with the guidance:
     create it at `app.docker.com/settings/personal-access-tokens` (a link that opens
     `https://app.docker.com/settings/personal-access-tokens` in the browser), tick
     "Allow Cloud Sandboxes access" (scope `sandbox:use`), Repository permissions can stay
     "Repo Public Read-only", prefer an expiration date, never use the Docker password.
   - Save exchanges the PAT (see "Docker auth per account") and checks the JWT `scope`
     claim. Errors are precise: wrong Docker ID or PAT, token revoked or expired, missing
     Cloud Sandboxes access, network failure.
   - Remove is refused while a project names the account (the error lists the projects).
3. **Claude setup** (kit): unchanged.

There is no Claude accounts block here: Claude accounts come from Providers and are chosen
per project.

### Layer 2: Cloud settings dialog per project (new, standalone)

A dialog titled "Cloud settings" with the project name and path. It does not change the
Project settings dialog. It replaces the Codespaces setup and Docker Sandbox setup dialogs.

Entry points:
- Project menu: the "Cloud setup" submenu becomes one item, "Cloud settings...". It opens on
  the project's default backend. Right after it, "Cloud workspaces..." opens the overview of the
  running codespaces and sandboxes (see `cloud-workspaces.md`).
- Composer Workspace menu: new item "Cloud settings for this project...".
- Composer Workspace menu "Docker Sandbox" while the project has no sandbox config (the
  conversation emits `OpenDockerSetup`, which the shell now answers by opening the dialog on
  Docker Sandbox): opens the dialog on Docker Sandbox.
- Copy that pointed users to "Codespaces setup" or "the project's Docker Sandbox settings"
  (`mono/bridge.rs`, `shell/cloud_create.rs`, `cloud_target.rs`,
  `mono/cloud_api/sandbox_provider.rs`) now says "Cloud settings".

Layout, top to bottom:
- **Segment** Codespaces | Docker Sandbox, full width of the dialog body (two equal halves).
  Below it, on its own row, "Default for new cloud sessions" (label and select on one line).
- **Ready**: a checklist shown as inline chips, with Fix at the end of the row where possible.
  It is computed when the tab opens and on "Check again"; the dialog says "Checked when this
  tab opens. Every start checks sign-in and the Docker account again." Editing the repository
  URL or a devcontainer path marks that backend's checklist "Changed: Check again" (no command
  runs on a keystroke). Replace token (revoked or Cloud Sandboxes access missing) is offered
  only for a rejected token; a Hub or network problem has no fix but Check again, and an
  account that no longer exists points at the dialog's Docker account picker. Replace token
  with unsaved edits first asks "Discard changes and open Settings > Cloud?".
  - Codespaces: gh signed in (with codespace scope), origin on GitHub, devcontainer found,
    sshd feature present in the devcontainer (the app reaches a codespace through
    `gh codespace ssh`; see `cloud-dev-environment.md`).
  - Docker Sandbox: Docker account valid, repository reachable (with the chosen Git
    credential, read access), devcontainer found, ssh set up. The Docker chip names the
    Docker ID ("Docker account <id>").
- **Workspace** (same rows, same order for both backends):
  Repository (from origin, read-only for codespaces; editable HTTPS URL for sandboxes as
  today), Base branch, Devcontainer (path; on the Docker Sandbox tab the Environment row sits
  before it, see "Environment" below), then one row with two compact selects side by
  side: Machine (codespace machine types from `codespaces::list_machines`) and Stop when idle
  (codespace: "After 30 minutes", 1, 2 or 4 hours), or Size (sbx shapes 1/2g, 2/4g, 4/8g,
  8/16g, 16/32g) and Runtime limit (sandbox: "1 hour, then stop", 2, 4, 8 hours). Sizes below
  the devcontainer's `hostRequirements` stay selectable; the warning shows under that row.
  There are no explanatory captions under the fields: hints are input placeholders, and only
  validation errors and the size-below-devcontainer warning are written under a field. A
  Docker token that needs replacing is shown in the Ready checklist (the Docker account
  item and its Replace token fix), not under a field. The Codespaces secrets block keeps its
  `CLAUDE_TOKEN_NOTE` warning, the secret descriptions that come from the project's
  `devcontainer.json`, and a one-line privacy note.
- **Environment** (Docker Sandbox tab only, between Base branch and Devcontainer): a select
  with **Default**, **Template** and **Public image**, where the sandbox starts from. The
  default is the stock `shell` sandbox plus the repository's dev container (today's
  behaviour).
  - **Template** shows a name field and the hint "The template must already exist in the
    Docker cloud registry (`sbx --cloud template ls`). Monoptah does not upload it, and its
    size is fixed." The app never creates or checks the template: an unknown name fails the
    create with the CLI's own error and leaves no session behind.
  - **Public image** shows a reference field (for example `docker.io/library/rust:slim`) and a
    hint that the image needs `sh`, `git` and `curl`.
  - Under the field, **Also build the repository's dev container** (off by default): a template
    or image brings its toolchain, so the session runs directly in the sandbox. The
    Devcontainer path row is only shown while the dev container is built.
  - A template fixes the size, so the **Size** half of the Size / Runtime limit row is replaced
    by the line "Size comes from the template" (nothing to choose, no devcontainer size
    warning). An image keeps the Size select, because `sbx` needs `--cpus` and `--memory` for
    it. Picking Template after Public image (or the other way round) clears the field: a
    template name is not an image reference.
  - Name and reference rules (also checked at start and by the Mono API): not empty, at most
    256 characters, no leading `-`, printable ASCII without spaces. An error shows under the
    field.
  - Save keeps what is stored: the environment, the dev container choice and the saved size
    (a template project whose stored size is not a valid shape is saved with the default size,
    so a later switch to an image never meets a value nobody checked) are carried by the form,
    not rebuilt from defaults. Opening the dialog on a template project and saving without an
    edit writes nothing.
- **Credentials**:
  - Docker account (sandbox only; from layer 1, "Machine sign-in (sbx login)" keeps the old
    behavior and is the default for existing projects).
  - Claude: one picker for both backends listing the app Claude accounts that carry a
    setup-token or API key. Codespaces additionally offer "Codespaces secret
    (CLAUDE_CODE_OAUTH_TOKEN)", the default for existing users; while it is selected the
    dialog shows the secret status and the "Generate with claude setup-token" flow of the
    old Codespaces dialog. Sandboxes keep "None".
  - Git: codespace shows read-only text "GitHub token built into the codespace (git and gh)";
    sandbox keeps the saved Git credential picker plus Manage. Manage does not open a second
    dialog: the Cloud settings dialog widens and a Git credential panel appears on its right
    (select a saved profile, add an HTTPS token, check access, remove). The dialog stays the
    only modal. Escape closes one layer per press, see "Escape order" below; Close (or
    switching to the Codespaces tab) returns the dialog to its normal width. The panel's body
    fills the dialog's height so its footer sits on the dialog's footer line. Saving or using a credential selects it in the form and
    closes the panel.
  - One line under the Credentials group, not per field: changing an account applies from
    the next start or reconnect; running sessions keep theirs.
- **Selects float**: every select (default backend, Environment, Machine, Size, Runtime limit,
  Stop when idle, Docker account, Claude, Git credential) opens a floating list under its trigger, the
  same dropdown as the Providers "sign-in options" menu (`popover::Choices`, the app's
  `Menu`): the current value is checked, arrows and Enter work, and opening it moves nothing
  in the form. The list draws above the dialog (layer 61, the dialog is 60) and below the fix
  terminal (62), is as wide as its trigger, and a press on it is not "outside" the dialog. A
  press outside it closes only the list (the press reaches nothing else).
- **Escape order**: one layer per Escape press: an open select's list, the Git credential panel's
  "Confirm remove" question, the Git credential panel, the replace-token question, then the
  dialog itself. The keyboard returns to the dialog when the panel is closed, unless a field
  in the main column had it.
- Footer: Cancel and Save. Save validates with the same rules as before
  (`DockerProjectConfig::validated` plus the new codespace rules) and writes both backends.
  It also saves the Codespaces secrets typed in the secrets block (the block has no Save of
  its own); when GitHub refuses a secret nothing is stored and the dialog stays open with the
  error.

### Composer (RUN ON CLOUD)

For a project on a saved template, the Size row of the Docker Sandbox panel is the plain line
"Size comes from the template" (it opens no menu and never counts as changed for this
session); the other rows are as for any sandbox.

The flow stays: Workspace menu, RUN ON CLOUD, Codespace or Docker Sandbox, panel attached
above the composer, instance created on first send. The Workspace menu gains:
- "Existing instance..." for the chosen backend, else the project's default one. It carries
  the project of the conversation that asked, so the picker lists that project's instances
  even when another project is open in the window. The description shows the most recent
  instance name when known.
  - Codespace: the existing codespace picker of `shell/cloud_picker.rs` (listing through gh,
    on a worker).
  - Docker Sandbox: a small picker (`shell/sandbox_picker.rs`) of the sandboxes the project's
    live cloud sessions already run in. The list comes from the store only
    (`CloudSessions::project_instances`, like `SandboxProvider::bound_names`), so nothing
    calls `sbx` while it opens and a sandbox's state is not shown (a stopped one starts with
    the first send). Picking one starts another session on it through
    `CloudSessions::attach_session`, the path Mono uses for an existing instance: the project
    must already have a ready session on it, so a sandbox whose only sessions are still
    starting is listed but cannot be picked. The item is disabled ("No sandbox yet") while
    the project has no session in a sandbox. The picker runs on a worker; the menu facts
    (default backend, most recent instance) are read once and kept until the project's cloud
    settings are saved or its cloud sessions change.
- "Cloud settings for this project..." (opens layer 2 on the backend currently chosen).

The panel rows have the same shape for both backends:
1. Machine (codespace) / Size (sandbox)
2. Stop when idle (codespace) / Runtime limit (sandbox)
3. Base branch
4. Branch (the working branch of the session; for a codespace it defaults to the base
   branch). A different valid name is created in the codespace before the session starts, as
   part of the Connect step ("Creating branch <name>"): a branch that already exists on
   origin is fetched and checked out tracking it (never recreated from the base, which would
   lose its commits; a single-branch clone is handled), otherwise `git switch -c` creates it.
   The script is idempotent and runs with the long ssh timeout plus one retry, because it is
   the first ssh to a fresh codespace. The name is validated before anything is created and is
   part of the single start-record save. A codespace Branch row that names another branch than
   the base counts as "changed for this session"; a sandbox's feature branch never does.
5. Docker account (sandbox only)
6. Claude account
7. Claude setup toggle

Each row shows the project default marked "default". A row the user changed is highlighted
and marked "changed for this session"; it applies to this session only (as today).

Only a row marked "changed" is an override. When the first prompt is sent the panel re-reads
the project's settings and applies onto them just the changed rows, so a row the user did not
touch always follows the project, including settings saved from "Cloud settings for this
project..." while the panel was open (the shell also tells the open panel on save, which
re-seeds the defaults and keeps the changed rows). A Claude account is pinned to the instance
only when it differs from the project's current default; a Docker account override is pinned
to the created sandbox by the per-sandbox pin.

### Codespaces Claude account

When a codespace project (or a per-session override) selects an app Claude account, the
account's credential reaches the codespace's Host through the same mechanism the sandbox
uses: `ClaudeAuth::prelude` (`cloud_sessions/docker_start.rs`) is prepended to the shared
`remote/remote_bootstrap.sh` script, sent on stdin of `gh codespace ssh ... sh -l -s`.
The bootstrap tracks the token by its sha256 and restarts the Host when it changes.

Because a codespace re-runs the bootstrap from `Connections` on every pairing, wake,
reconnect and Host upgrade, the account is resolved inside `Connections` for every codespace
bootstrap (an injectable resolver keyed by codespace name), not only at start. Otherwise a
run without the prelude would restart the Host back onto the Codespaces secret.

- Precedence: the account wins over the Codespaces secret for Claude processes only (the
  prelude unsets `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` in
  the bootstrap shell before exporting the account's variable).
- The token travels on stdin only. Never argv, a file that syncs back or is archived, or a
  log line. Errors are scrubbed with the prelude and the secret.
- Per instance: the choice is stored by instance name (`codespace-claude-account:<name>`,
  `sandbox-claude-account:<name>`) as `{account, pinned}`. `pinned` is true for a per-session
  override (reconnect keeps it); otherwise reconnect follows the project's current choice.
  This also fixes a sandbox gap: a per-session sandbox override used to be lost on reconnect.
- `cloud.status`: the `CLAUDE_CODE_OAUTH_TOKEN` secret is reported as optional when the
  project chooses an account.
- Mono API: `sessions.start` with a new cloud target (codespace or sandbox) accepts `account`
  like a local start (resolved by the GUI, must carry a setup-token or API key). With an
  existing instance it is refused: the account belongs to the instance. Help text updated.
- No `host/` change: the Host reads Claude credentials from its process environment and
  Host 1.2.0's detached `service install` already inherits the bootstrap shell's exports.
  No Host release or pin is needed.

### Docker auth per account

Verified by hand on sbx v0.51.0:
- `sbx login` state is in the macOS Keychain, per user. `SBX_STATE_DIR`, `DOCKER_CONFIG`
  and `HOME` do not isolate it (a `HOME` override breaks login with Keychain error -60008).
  The app never uses `sbx login` for multi-account.
- `DOCKER_ACCESS_TOKEN=<JWT>` works for `sbx --cloud ...` and takes precedence over the
  machine login. A bad token fails with "DOCKER_ACCESS_TOKEN was rejected" and does not fall
  back. A raw PAT in `DOCKER_ACCESS_TOKEN` fails ("requires both account and accessToken");
  `DOCKER_PAT` and `DOCKER_ID` are ignored by sbx.
- JWT: `POST https://hub.docker.com/v2/auth/token` with JSON
  `{"identifier": <Docker ID>, "secret": <PAT>}` returns `{"access_token": ...}`. Lifetime
  15 minutes; claims include `scope`, `sub`, `exp`, `iat`.

Storage (`crates/monocode-core/src/docker_accounts.rs`):
- Table `docker_accounts(id, docker_id, nonce, ciphertext, status, checked_at, expires_at,
  created_at)` in the credentials database, registered next to `git_credentials` in both
  schema places. The PAT is sealed with `accounts::sealed` (AES-256-GCM, `credentials.key`)
  and associated data `monoptah-docker-account-v1\0<id>\0<docker id>`. Never the Keychain
  (the no-Keychain guard in `kit_prefs_tests.rs` covers the new files).
- The JWT lives only in memory, in one process-wide cache. It is refreshed when less than
  60 seconds remain or after a rejection; concurrent refreshes of one account are coalesced
  behind a per-account lock. The HTTP exchange goes through an injectable client so tests
  never reach Docker.
- An exchange answered with 401/403 marks the account "Token revoked or expired"; a JWT
  without sandbox scope marks it "Missing Cloud Sandboxes access". A network failure keeps
  the stored status.

One sbx seam:
- `sandboxes::SystemSbx` stays the only place an `sbx` process is spawned. Every call names
  the Docker account it runs as (`None` keeps the machine `sbx login`, today's behavior, so
  existing users are not affected). With an account, the runner sets `DOCKER_ACCESS_TOKEN`
  on the child; on "DOCKER_ACCESS_TOKEN was rejected" it drops the cached JWT, exchanges
  again and retries once.
- The account is resolved from the project (`DockerProjectConfig.docker_account`) for calls
  made before a sandbox exists (diagnose, `ls`, `create`), and from the sandbox afterwards:
  the creating account is pinned per sandbox name (`sandbox-docker-account:<name>`) and on
  `SandboxTarget.docker_account`, because a sandbox lives in the account that created it.
  Changing a project's Docker account applies to new sandboxes only.
- Every `ssh <id>@sbx_cloud` gets the same env, because the alias's ProxyCommand runs
  `sbx --cloud ssh-connect`: the remote module's ssh builder (pairing, tunnels, scripts,
  Host upgrade), `SystemShell` (start steps, scripts, copy, evidence) and the interactive
  sandbox terminal.
- Covered callers: start (diagnose, create, inspect, exec steps), reconnect and wake, the
  background state poll, stop, archive and delete (including the store's delete path), move
  to cloud and back, convert, evidence pull, Mono `cloud.*` actions, and the readiness checks.
- Account failures surface as "Docker account <id>: token revoked or expired. Replace it in
  Settings > Cloud." in Settings, in the project's Ready checklist and in startup errors.

Unverified (handled defensively, listed for manual verification):
- Whether `sbx` create, attach, exec and the long-lived ssh tunnel keep working after the
  JWT that started them expires. The app passes a fresh JWT to every new process, and a
  dropped tunnel is re-established through the normal reconnect path, which spawns a new ssh
  with a fresh JWT.
- Whether `sbx --cloud setup ssh` is per account. The alias is global; Fix runs it with the
  project's account when one is in context.
- The exact `scope` value (`sandbox:use` is expected; any scope word starting with `sandbox`
  is accepted).

## Data and migration

- **Sandbox**: stays in `docker-sandbox-projects.v1` as `DockerProjectConfig`, which gains
  `dockerAccount` (serde default `None`), `environment` (`{kind: "template", name}` or
  `{kind: "image", reference}`; absent for the default sandbox) and `useDevcontainer`
  (optional, only written with a template or image). Existing fields (`claudeAccount`, `credentialId`,
  `cpus`, `memory`, `ttl`, `baseRef`, `devcontainerPath`, `repoUrl`, `platform`) are kept as
  they are, so every existing reader keeps working and nothing has to be rewritten.
- **Codespace and default backend**: new setting `cloud-projects.v1`, a map keyed by the
  project cwd: `{defaultBackend, codespace: {baseRef, devcontainerPath, machine,
  idleTimeoutMinutes, claudeAccount}}`. Missing values mean today's behavior: machine
  `basicLinux32gb`, 30 minutes, the HEAD branch, the repository's default devcontainer, the
  Codespaces secret. `defaultBackend` is absent for existing projects and reads as Docker
  Sandbox when a sandbox config exists, else Codespaces.
- One facade, `cloud_sessions::project_cloud`, loads and saves both parts.
- Codespace creation uses the stored machine, idle timeout and devcontainer path (`gh
  codespace create --devcontainer-path`, and the REST path) instead of the constants.

## Out of scope

Changing the Project settings dialog; GitLab or `glab`; Host changes.

## Known limitations

- "Existing instance" for sandboxes lists only the sandboxes that have a ready session of
  the project; a sandbox with no session in the app is not offered.
- A Docker token's expiry is shown as "Not known": Docker Hub does not report it.
- Mono `sessions.start` target options do not take an idle timeout or a working branch.
- Removing a Claude account in Providers does not yet check the cloud instances pinned to it
  (Docker account removal does check the sandboxes created in it).

## Manual verification (needs a second Docker account and a fresh PAT)

1. Settings > Cloud > Docker accounts > Add: enter the second Docker ID and a PAT created
   with "Allow Cloud Sandboxes access". Expect "Cloud Sandboxes access". Repeat with a PAT
   without that box: adding it is refused with "This token has no Cloud Sandboxes access..."
   (nothing is saved). The "Missing Cloud Sandboxes access" status appears on an account
   that already exists, after Check again or Replace token finds the access gone. Repeat
   with a wrong PAT: expect "Wrong Docker ID or token".
2. Project A: Cloud settings > Docker Sandbox > Docker account = account 1. Project B:
   account 2. Start a sandbox session in each. In the Docker dashboard of each account,
   confirm the sandbox appears under the right account only.
3. Leave a sandbox session open for more than 15 minutes, then send a message, open its
   terminal, and pull evidence. Confirm all work (JWT expiry vs long-lived tunnel).
4. Stop the Mac's network for a minute so the tunnel drops; reconnect. Confirm it comes back
   (new ssh with a fresh JWT).
5. Archive and delete the sandbox sessions; confirm both sandboxes are removed from the right
   accounts.
6. Revoke account 2's PAT on Docker Hub. Within 15 minutes, start or reconnect in project B:
   expect "token revoked or expired. Replace it in Settings > Cloud." Replace token is in
   Settings > Cloud (the account's row) and in the Ready checklist of the project's Cloud
   settings dialog; replace it and retry.
7. With `sbx login` signed in as account 1 and project B on account 2, run
   `sbx --cloud ls` in a terminal (machine login) and confirm the app's project B sandboxes
   are not affected by the machine login.
8. Check whether `sbx --cloud setup ssh` needs re-running after switching accounts.
9. Codespaces: Cloud settings > Codespaces > Claude = an app account. Start a codespace
   session; in the codespace run `env | grep -c CLAUDE_CODE_OAUTH_TOKEN` inside a Claude
   shell tool call and confirm Claude answers using the chosen account (Usage page of that
   account). Stop the codespace, wake it, and confirm the account is still used.
10. Switch the project back to "Codespaces secret"; reconnect; confirm the secret is used.
11. Confirm no token appears in `~/Library/Logs` / the app log, in session archives, or in
   the codespace or sandbox file system (`grep -r dckr_pat_ ~/.monocode-host` and so on).
12. In the composer, start a sandbox session in project A with the panel's Docker account
   changed to account 2 for this session only. Confirm the sandbox is created under account
   2, and that stop, reconnect and delete of that session keep using account 2 after
   project A's default is changed again.
13. With a codespace session running a long turn, change the project's Claude account in
   Cloud settings. Confirm the turn is not interrupted and the new account applies once the
   Host is idle (the next send says "Applying the Claude account to the codespace" while it
   happens).
14. Confirm the scope Docker Hub puts in the access token for a PAT with Cloud Sandboxes
   access (expected `sandbox:use`; the app accepts any scope word starting with `sandbox`).
