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
the app runs the session directly in the sandbox, which has no toolchain.

Platforms: the image builds for the host architecture, so Linux aarch64 and x86_64
sandboxes both work (aarch64 verified; x86_64 uses the same file, with the
`x86_64-unknown-linux-gnu` rustup-init, and was not run here).

## Resources: what is applied, and what you need

- **Docker Sandbox**: size comes only from the project's sandbox setup (`cpus`/`memory`,
  default 2 CPUs / 4g; shapes 1/2g, 2/4g, 4/8g, ...). The app passes them to
  `sbx create --cpus --memory` when the sandbox is created, so changing them later needs a
  new sandbox. `hostRequirements` in `devcontainer.json` is ignored here; it only makes
  Codespaces offer a big enough machine. This change does not alter the app's defaults or
  any project's settings.
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
tests (`-p monocode-gpui -- --test-threads=1`) on Linux; Windows keeps a `cargo check`. GPUI is
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
