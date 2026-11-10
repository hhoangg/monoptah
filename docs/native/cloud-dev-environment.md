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
`x86_64-unknown-linux-gnu` rustup-init, and was not run here). Sandbox size: set 4 CPUs and
8g (`hostRequirements` and `CARGO_BUILD_JOBS=4`); the default 2 CPUs / 4g sandbox is slow
and can run out of memory linking GPUI.

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
cargo clippy --locked --manifest-path $M -p monocode-core --all-targets --profile native-dev -- -D warnings
cargo test   --locked --manifest-path $M -p monocode-core --profile native-dev
cargo check  --locked --manifest-path $M -p monocode-core -p monocode-gpui --all-targets --profile native-dev
```

These are what `native.yml` runs for Linux. GPUI is supported on Linux by the source (X11 and
Wayland features, `vendor/gpui-pre-linux`), so it builds here, but it is not fully green:

| Check (Linux aarch64, 4 CPUs / 8g) | Result |
| --- | --- |
| core fmt, clippy `-D warnings`, `cargo check` GPUI `--all-targets` | pass |
| core tests | pass (4199 tests; 3 `docker_start_tests::scripts` need a real clone, see worktrees) |
| `npm run native:test:dev` (after `npm ci`) | 80 pass, 2 skipped |
| GPUI `clippy -D warnings` | **fails**: unused/dead code that is only used on macOS (`native.yml` says the same) |
| GPUI tests | 3339 pass, 23 fail deterministically (terminal dock, panes, settings, quit gate, ...), at least some because tests press macOS `cmd-` shortcuts. Not triaged one by one |

So GPUI clippy and the full GPUI test run stay Mac-only checks (`npm run native:check`); do
not report them as passing from a Linux session. Linking the GPUI test binary needs more
than 4 jobs of memory: run it with `CARGO_BUILD_JOBS=1` (rustc was OOM-killed at 4 jobs in an
8g sandbox). Anything needing a Metal device, a display or the real macOS app (`native:dev`,
`native:build`, packaging, `native:release:mac`) is Mac-only. Do not raise `CARGO_BUILD_JOBS`
above 4.

## Git worktrees

A `git worktree` checkout has a `.git` file pointing outside the folder. Mounted alone into
the container it is not a repository, and three `docker_start_tests::scripts` tests that run
real `git` fail. Cloud sessions use a plain clone, so they are unaffected. To use a worktree
in a dev container, also bind-mount the main repo's `.git` at the same path.

## GitHub access

`gh` is installed with no credentials, and none are baked into the image or config.

- The app gives the session a Git credential (HTTPS, scoped to the one repository URL, held
  in memory in a `git-credential-cache` socket) so `git fetch`/`push` of that repo works. It
  does not set `GH_TOKEN`, so `gh` is **unauthenticated**.
- Without a token `gh` refuses every command, including public reads. Public reads work with
  `curl https://api.github.com/repos/OWNER/REPO/...` (rate limited). Reviewing private PRs,
  posting comments and `gh pr create` need a token the app does not inject yet; injecting one
  is app-side work (the secrets seam in `cloud_sessions/docker_start.rs`), outside this change.
  In Codespaces, `GITHUB_TOKEN`/`GH_TOKEN` can come from Codespaces secrets.
- Never put a token in this folder, the Dockerfile or `containerEnv`.
