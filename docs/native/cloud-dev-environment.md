# Native cloud development environment

How a cloud session gets a working Rust toolchain for `crates/monocode-core` and
`crates/monocode-gpui`. Native only; the archived Tauri app is not covered.

## Which backend reads what

| Backend | Reads | Notes |
| --- | --- | --- |
| Docker Sandbox (`sbx --cloud`) | `.devcontainer/devcontainer.json` | The app clones the repo into the sandbox and runs the Dev Container CLI (`devcontainer up --skip-non-blocking-commands`), then opens the project inside that container. The sandbox's own image has no Rust toolchain or C linker; the dev container is the only place they come from. The path is the project's "devcontainer path" setting (default `.devcontainer/devcontainer.json`). |
| Codespaces | the same file | Standard devcontainer support. |
| Local Mac | nothing | Use rustup and Xcode as before. |

So there is one config, not a separate Sandbox template. If a project has no devcontainer
the app runs the session directly in the sandbox, which has no toolchain. A project can also
start the sandbox from a saved template or a public image, which skips the dev container
(see "Docker Sandbox: start from a template or an image").

Platforms: the image builds for the host architecture, so Linux aarch64 and x86_64
sandboxes both work (aarch64 verified; x86_64 uses the same file, with the
`x86_64-unknown-linux-gnu` rustup-init, and was not run here).

## Resources: what is applied, and what you need

- **Docker Sandbox**: size comes from the project's sandbox setup (`cpus`/`memory`,
  default 2 CPUs / 4g). `sbx` accepts only the shapes 1/2g, 2/4g, 4/8g, 8/16g and 16/32g.
  The app passes the size to `sbx create --cpus --memory` when the sandbox is created, so
  changing it later needs a new sandbox. `sbx` itself ignores `hostRequirements` in
  `devcontainer.json`, so the app reads it from the local checkout (the project's devcontainer
  path) and warns, in Cloud settings and in the composer's Size row, when a size is below it.
  Every size stays selectable and nothing is raised for you there. A Mono starting a sandbox
  through the app API without naming a size gets the saved size raised to the smallest shape
  that meets `hostRequirements`, but never above 8 CPUs / 16g by itself (a larger requirement
  keeps the saved size and says so); the start result reports the size in `target.cpus` and
  `target.memory` and why in `note`, and the saved setting is not changed. A saved size that is
  not a shape is rounded up to the next shape, never down. A Mono giving only `cpus` or only
  `memory` gets the other half of that shape, and a pair that is not a shape is refused before
  anything is created. A size named explicitly is used as it is. For this repository
  (`cpus: 4`, `memory: 8gb`) the raise is 4 CPUs / 8g.
- **Concurrency**: the container sets `CARGO_BUILD_JOBS=1`. Memory, not CPU, is the limit:
  one `rustc` for the GPUI test binary needs more than 4g by itself, and several in parallel
  add up (the 4-job GPUI test build on 8g was killed with SIGKILL, the OOM-killer signature). 2 jobs was not tried on 8g. Raise the variable only
  after measuring. `nproc` ignores CPU quotas, so it is not a guide.
- **Measured** (aarch64, container limited with `docker update --memory 4g --cpus 2`, logs in
  the PR evidence):

| Task | 4g | 8g |
| --- | --- | --- |
| core `clippy -D warnings`, GPUI `cargo check --all-targets` | pass (2 jobs) | pass (4 jobs) |
| core tests, build (1 job) | pass | pass (full suite, run at 4 jobs) |
| core tests, build (2 jobs) | OOM-killed | not measured |
| GPUI tests, build (1 job) | OOM-killed | pass (full run: 3339 pass, 23 fail) |

So the default 2 CPU / 4g sandbox can run fmt, clippy and `cargo check`, and the core tests
at 1 job; running the GPUI tests needs a 4 CPU / 8g sandbox. 8g with 1 job is the only
verified GPUI-test setup.
`scripts/native-dev-doctor.sh` reads the cgroup limit and warns about all of this.

## Docker Sandbox: start from a template or an image

By default the sandbox is a stock `shell` sandbox and the app builds the repository's dev
container in it. That costs about 3m37s before the session is usable (the Dockerfile alone
builds in about 150 s on a 2 vCPU / 4g sandbox). A project can instead start from something
that already has the toolchain: in the project's **Cloud settings**, Docker Sandbox tab, the
**Environment** row (see "Environment" in [cloud-setup.md](cloud-setup.md)), or with
`sessions.start` target `{kind: "sandbox", template: NAME}` / `{kind: "sandbox", image: REF}`:

| Environment | What the app runs | Measured (aarch64) |
| --- | --- | --- |
| Default | `sbx create ... shell`, then Dev Container CLI | about 3m37s to ready |
| Template | `sbx --cloud create --template NAME`, no `--cpus` / `--memory` | 396 MB toolchain template: created in 9 s, first command 4 s later (about 13 s; 4-13 s across the templates tried) |
| Public image | `sbx --cloud create --image-ref REF --cpus N --memory M` | `docker.io/library/rust:slim`: created in 9 s, first command 3 s later (about 12 s) |

A template or image brings its own toolchain, so the app skips the dev container build
(`Plain` mode: the session runs directly in the sandbox, the Dev Container CLI is not even
installed). Turn **Also build the repository's dev container** on (under the Environment
field) to get the old behaviour on top of it (config `useDevcontainer`, default off for a template or image).

What the app does **not** do: it never creates, uploads or deletes a template (a later
slice will). The template must already exist in the Docker cloud registry
(`sbx --cloud template ls`). An unknown name fails the create with the CLI's own error. The
start treats that as a refusal (no session and no record left behind) only when the error says
that the template **we asked for** is not found or does not exist, naming it; any other
failure, including a late one whose text merely mentions a template, is unclear (a sandbox may
exist and bill), so the app keeps a failed start to Retry (which looks the name up first) or
delete.

### Size

A template fixes the sandbox size: `sbx` rejects `--cpus` and `--memory` with it
("template-mode sandboxes use the template's baked-in sizing"). The app therefore sends none,
does not check the saved size (when an image overrides a template project through the Mono
API, the saved size is rounded up to a billable shape and the response says so), shows **Size comes from the template** in the Cloud settings
dialog and in the composer panel, and refuses `cpus` / `memory` next to `template` in `sessions.start`.
You need one template per size, and per platform (a template inherits the platform it was
saved on; without `--platform` the new sandbox inherits it). A public image is different: `sbx`
requires `--cpus` and `--memory` for `--image-ref`, so the normal sizes apply.

### Preparing a template by hand

Do this in a terminal with `sbx` signed in, **before** any credential reaches the sandbox:

```bash
# 1. A sandbox of the size the template should have (this fixes the template's size).
ID=$(sbx --cloud create --no-progress --name tpl-rust --cpus 4 --memory 8g --ttl 1h shell | grep -E '^sbx_')
# 2. Install the toolchain on the sandbox's OWN filesystem, not in an inner Docker image.
sbx --cloud exec "$ID" sh -c 'sudo apt-get update && sudo apt-get install -y \
  git curl xz-utils build-essential pkg-config clang && \
  curl --proto =https -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c rustfmt,clippy'
# 3. Save it. The default `disk` capture mode; do NOT pass --capture-mode all.
sbx --cloud template save "$ID" rust-medium --description "rust toolchain, 4 vCPU / 8g"
# 4. The sandbox was only the source: stop it, wait until `sbx --cloud ls` says stopped, remove it.
sbx --cloud stop "$ID"
sbx --cloud rm --force "$ID"
sbx --cloud template ls
```

The template needs `sh`, `git` and `curl` (the app clones the repository and installs its Host
in the sandbox). The commands above follow `sbx` 0.53; check `sbx --cloud template save --help` if your
version differs.

Measured on a micro/small sandbox: installing apt packages plus rustup took 33 s, saving the
template 34 s and the template is 396 MB; a sandbox created from it ran `rustc`, `gcc` and the
system libraries on the first command. Warnings from that spike:

- **Never snapshot a sandbox that already received credentials** (a Git token, a Claude
  token, the Host token). Docker's docs say credential files in the filesystem end up in
  templates, and `--capture-mode all` also captures process memory (for example a
  `git-credential-cache` daemon). Prepare the template in a sandbox the app never touched.
- **A `disk` template does not keep the Docker image built inside the sandbox.** An image built
  by the dev container lives under `/var/lib/docker` and is not captured; only the sandbox's
  own filesystem is. That is why the toolchain has to be installed outside Docker.
- `--capture-mode all` keeps the inner images but produced a **26.6 GB** template (137 s to
  save) and removed its source sandbox before the save finished. Avoid it.
- **Storage cost is unknown.** The Docker documentation gives no storage price for templates;
  ask Docker billing before keeping several multi-hundred-MB templates per size and platform.
- Every template is tied to its size and platform: one per size and architecture. Everything
  above was run on aarch64; x86_64 was not.

### A public image

`--image-ref` needs no template: the image must be public, and the sandbox it makes has only
what the image contains. `docker.io/library/rust:slim` has no `git`, `curl`, `sudo` or Docker,
so it cannot be used as is (the clone and the Host install need `git` and `curl`); use an
image that has them.

Checked once more with a micro sandbox from `docker.io/library/rust:slim` (`create` without an
agent argument, which is what the app sends for an image or a template):

- It was created in 4 s and `rustc` ran 3 s later.
- `sbx --cloud ls` shows an **empty AGENT column** for it, as for a template. The app's
  lookup by name therefore accepts an empty agent as well as `shell`.
- `sbx exec` runs as `root`, but `ssh <id>@sbx_cloud` (which the app uses) logs in as `user`
  (uid 2000, `HOME=/home/user`). The provisioning scripts take `$HOME` from the login user and
  assume neither `root` nor `agent`.

### A Docker Sandbox as your own Linux build box

This page is about the sandboxes the app starts. To use a sandbox yourself, or from an agent,
as the Linux machine for fmt, clippy and tests of a ticket (a warm large sandbox, an edit loop
of 6.5 to 8 minutes, scripts in `scripts/sandbox/`), see [sandbox-build-box.md](sandbox-build-box.md).
It also lists what a sandbox cannot check.

## Codespaces need an SSH server

The app reaches a codespace through `gh codespace ssh`, so the dev container must run an SSH
server. Without one the start fails at "connect" with `SSH setup failed: ... failed to start
SSH server ...`; the app then says to add the feature below and keeps the codespace for
Retry or Rebuild container. Add it to `devcontainer.json` (then rebuild the container):

```json
"features": {
  "ghcr.io/devcontainers/features/sshd:1": {}
}
```

See GitHub's [SSH into a codespace](https://docs.github.com/en/codespaces/developing-in-a-codespace/using-github-codespaces-with-github-cli#ssh-into-a-codespace).
Docker Sandboxes do not need it.

## What is provisioned

- `Dockerfile`: Debian trixie dev image, the system libraries from `native.yml`
  (clang, cmake, ninja, pkg-config, fontconfig, xcb, xkbcommon, wayland, alsa, EGL, Vulkan,
  dbus, ...), `gh`, and Rust `RUST_VERSION` (pinned, `rustfmt` + `clippy`; rustup-init is
  checked against its published sha256). CI uses `stable`; bump the pin when you bump CI.
- Node 22 through the devcontainer node feature (for `npm run native:test:dev`).
- An SSH server through the devcontainer sshd feature (`ghcr.io/devcontainers/features/sshd:1`,
  pinned in `devcontainer-lock.json`). Required: the app reaches a codespace only through
  `gh codespace ssh`, and without it a Codespaces start fails at "connect". Do not remove it.
  Docker Sandboxes do not use it. No password is set and no port is forwarded.
- `onCreateCommand` runs `scripts/native-dev-doctor.sh --quiet`: blocking, installs nothing,
  fails the build if anything is missing. `postCreateCommand` runs `cargo fetch --locked`
  in the background-safe slot; it never fails the session. Both are idempotent.

## Checks

From the repository root, inside the container:

```bash
scripts/native-dev-doctor.sh      # toolchain and library diagnostics
M=crates/monocode-gpui/Cargo.toml
cargo fmt --manifest-path $M --all --check
cargo clippy --locked --manifest-path $M -p monocode-core -p monocode-gpui --all-targets --profile native-dev -- -D warnings
cargo test   --locked --manifest-path $M -p monocode-core --profile native-dev
cargo test   --locked --manifest-path $M -p monocode-gpui --profile native-dev -- --test-threads=1
```

Core fmt/clippy/tests are what the `native.yml` core job runs on Linux. For GPUI, `native.yml`
runs fmt, clippy `-D warnings` (`-p monocode-core -p monocode-gpui --all-targets`) and the GPUI
tests on Linux (cargo-nextest, one process per test, 3 shards; the `cargo test ... --test-threads=1`
above is the local equivalent); Windows keeps a `cargo check`. GPUI is
supported on Linux by the source (X11 and Wayland features, `vendor/gpui-pre-linux`). Tests gated
to macOS and clippy of the macOS-only code (`npm run native:check:mac`) stay Mac-only, and
`npm run native:check` still runs in full before every release. Do not report macOS-only checks
as passing from a Linux session. The GPUI test binary needs an 8g sandbox even at 1 job (see Resources). Anything needing a Metal device, a display or the real macOS app (`native:dev`,
`native:build`, packaging, `native:release:mac`) is Mac-only.

## Linked git worktrees

Cloud sessions clone the repository, so this only matters when you open a `git worktree`
checkout yourself. Its `.git` is a file (`gitdir: <main repo>/.git/worktrees/<name>`) pointing
outside the folder. A dev container mounts only the folder, so there `git` fails ("gitdir
does not point to a valid repository") and three real-git tests
(`docker_start_tests::scripts::*`) fail; everything else passes. The doctor warns about it.
With the Dev Container CLI, mount the main repo's `.git` at the same absolute path:

```bash
G=/path/to/main/.git   # the path in the `gitdir:` line, up to .git
devcontainer up --workspace-folder . --mount "type=bind,source=$G,target=$G"
```

That was verified (the 3 tests passed with it). It is not in `devcontainer.json` because the
path is per machine and the app's sandbox flow never uses a worktree. The mount is read-write,
so git commands in the container can change the main repo. Do not change tests to avoid this.

## GitHub access (optional; not needed to build)

Build readiness and GitHub access are separate: the doctor's exit status covers only the
toolchain and libraries, and reports a missing token as a warning.

`gh` is installed with no credentials, and none are baked into the image or config.

- The app gives the session a Git credential (HTTPS, scoped to the one repository URL, held
  in memory in a `git-credential-cache` socket) so `git fetch`/`push` of that repo works. It
  does not set `GH_TOKEN`, so `gh` is **unauthenticated**. The doctor does not test a token,
  so even with a token variable set it only says authentication is unverified.
- Without a token `gh` refuses every command, including public reads. Public reads work with
  `curl https://api.github.com/repos/OWNER/REPO/...` (rate limited). Reviewing private PRs,
  posting comments and `gh pr create` need a token the app does not inject yet; injecting one
  is app-side work (the secrets seam in `cloud_sessions/docker_start.rs`), outside this change.
  In Codespaces, `GITHUB_TOKEN`/`GH_TOKEN` can come from Codespaces secrets.
- Never put a token in this folder, the Dockerfile or `containerEnv`.

## Stopping idle workspaces (cost control)

A running workspace costs money whether or not anyone uses it. A Docker sandbox bills per runtime
second (after the credit, to the card) until its runtime limit ends it, and a session that is
finished or forgotten leaves it running. **Settings > Cloud > Cost control** makes the app stop such
a workspace. It only ever **stops**: nothing is deleted, the state is kept, and the session shows the
usual Stopped state with Resume. The next message you send resumes it, exactly as after a manual Stop.

| Setting | Docker sandbox | Codespace |
| --- | --- | --- |
| Auto-stop after N minutes of inactivity (Off, 5, 10, 15, 30) | **10** | **Off** |
| Auto-stop when its last session is gone (archived or deleted) | **On** | **Off** |

These are Monoptah's own rules and are named apart from the per-project **Stop when idle** of a
project's cloud settings, which is GitHub's codespace timeout, set when the codespace is created.

Codespaces default to off because GitHub already idles a codespace out after 30 minutes; turn it on
to stop sooner. The settings are global (they apply to every project); a per-project override is not
there yet.

**Idle** means all of this, for every session on the workspace:

* no turn is running and nothing waits for input, no terminal or background task is busy, and **no
  terminal is open on the workspace**: a shell you opened from the session menu, or a codespace's
  panel shell once you typed in it (the app opens that one by itself, and an untouched prompt is not
  work). A dev server or a build there is work too, and a quiet one cannot be told from a forgotten
  prompt, so an open shell holds the workspace until you close it. The idle time counts from then;
* nothing is queued, starting, restoring, being moved, archived or converted, and no file is waiting
  to be copied to this computer (a session with a sync problem, or an unsaved file in the editor,
  counts as busy);
* the app saw no typing in a composer (an edit by you: not a restored draft or a placeholder), send or
  Resume click in those sessions for N minutes, counted from the last such activity (or from when the app first saw the workspace, so restarting the app never stops
  anything at once).

When the app does not know something (the workspace state has not been read, or a session's data is
missing) it does **not** stop. Only a workspace the app reads as running is ever stopped.

**Last session archived or deleted**: when no session of the window uses a workspace any more, it is
stopped after a short grace (the idle time, or 5 minutes when idle stopping is off). Your own choices
win: a workspace you archived with **Keep running**, or removed with **Remove from Monoptah**, is
**never** stopped for being unused (those dialogs say it keeps running), until you resume it from a
session. An open terminal on it also holds it. The app re-reads the store first and does nothing if a
session of another project still uses it. While other sessions still use it, the idle rule applies to it
as usual.

How it runs: once a minute the app looks at session state it already holds in memory. It does not
call `gh` or `sbx` for the check. Only when a workspace crosses its threshold does one worker read the
state again and stop it through the same call the Stop menu uses. The stop takes seconds, so for that
long the app holds the workspace the way an archive does: it is marked busy (archive, delete, continue
locally, a new terminal and the `cloud.*` app actions refuse), the composers of its sessions refuse a
send ("Monoptah is stopping this workspace after it sat idle. Nothing was sent; send again in a moment
and it will resume.", the typed text stays) and so do app and Mono turns. The worker is given the same
live view of busy sessions that `cloud.stop` gets and checks it before the state read and again just
before the stop call, so a turn that started a second ago, which the stored flags (polled every 30 s)
do not show yet, is never cut off. If the stop fails (an expired Docker token, `sbx` or `gh` failing)
the error line says so and that the workspace is still running; it is shown once per workspace until a
stop works, and every failure is also written to the log. A notice says what happened ("Stopped
Docker sandbox NAME after 10 min idle; send a message to resume."). A stop is asked for once; a
stopped workspace is not stopped again, and a refused or failed stop waits for a whole new window.
Background work (a Mono, an automation, the app API) never resumes a stopped workspace; only your own
send or the Resume button does.

Not covered: a workspace with **no** Monoptah session (an orphan, for example from a failed start)
is never stopped by this. Stop or delete it by hand in **Cloud workspaces**.
