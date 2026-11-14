# Sandbox build box

How to use a Docker Sandbox (`sbx --cloud`) as a Linux build machine for the native app
(`crates/monocode-core`, `crates/monocode-gpui`), so a ticket's fmt, clippy and tests do not
occupy the Mac. Native only. This is about *your* checks while you work; the app's own cloud
sessions are in [cloud-dev-environment.md](cloud-dev-environment.md).

The scripts are in `scripts/sandbox/`. They use whatever `sbx login` already holds and never
read, store or print a token.

## Sandbox or Mac?

| Use a sandbox when | Use the Mac when |
| --- | --- |
| The change is fmt, clippy or tests of code that also compiles on Linux (most of `monocode-core` and `monocode-gpui`). | The change touches macOS-gated code or a test gated to macOS (see "What a sandbox cannot check"). |
| Several tickets run at once and one cargo build at a time would queue on the Mac. | You need the app window, `native:dev`, packaging or a release. |
| You want to keep the Mac's CPU and memory free. | You only edit docs or scripts: `npm run native:test:dev` is a few seconds anywhere. |

A sandbox does not replace `npm run native:check:mac` or `npm run native:check` before a
release. It replaces the slow Linux part of the loop.

## The recipe

```bash
# 1. Once per ticket (about 14 minutes cold, on a large sandbox). Prints the sandbox name.
scripts/sandbox/warm.sh --name tkt-123

# 2. Run cargo fmt locally, then loop. Each pass sends your diff, not the whole tree.
cargo fmt --manifest-path crates/monocode-gpui/Cargo.toml --all
scripts/sandbox/check.sh --name tkt-123 --filter 'test(/accounts::identity/)'

# 3. Before the PR, once: node tests, fmt, clippy and every nextest test.
scripts/sandbox/check.sh --name tkt-123 --full

# 4. Always, also after a failure.
scripts/sandbox/clean.sh
```

- `warm.sh` creates a large sandbox (8 vCPU / 16 GB, TTL 2 h, `--on-timeout stop`), clones the
  repo's current `main`, installs the toolchain (the Rust version from
  `.devcontainer/Dockerfile`, the nextest version from `native.yml`, Node), and builds the
  clippy and test-binary artifacts for the combined package set. It prints only the sandbox
  name on stdout; progress goes to stderr.
- `refresh.sh [--name NAME]` moves a warm sandbox to the current `main` and rebuilds what
  changed. A refresh from 4 commits behind (one Rust file changed) took 6 min 42 s.
- `check.sh` takes the diff of your working tree (committed, staged, unstaged and new files)
  against `upstream/main` (or `--base REF`), copies it with `sbx cp`, checks out the base
  commit in the sandbox, runs `git apply --index`, then `cargo fmt --check`, clippy
  `-D warnings`, and the tests **only if clippy passed**. `--filter` runs the tests that match a
  nextest filter expression, `--full` runs everything, and with neither only fmt and clippy run.
  A patch that changes no Rust files skips cargo and runs only the node tests. If fmt fails,
  the sandbox formats its tree (so clippy and the tests run on the formatted code), and the
  resulting diff is copied back to `~/.local/state/sandbox-build-box/fmt.patch`; `--apply-fmt`
  applies it to your working tree. The exit status is 0 only when every step passed.
- `clean.sh [NAME...]` runs `sbx --cloud rm --force` on the named sandboxes, or on every one
  the scripts recorded, then prints `sbx --cloud ls` and fails if a removed name is still
  listed.

The long steps run detached in the sandbox and the script polls their log every 20 seconds, so
a dropped connection does not kill a build.

## Measured numbers

aarch64 cloud sandboxes (Debian 13), `sbx` 0.53.0, Rust 1.99.1, `--profile native-dev`,
`--locked`, cargo-nextest 0.9.151, `-p monocode-core -p monocode-gpui`. Times are wall clock.

| What | Time | Source |
| --- | --- | --- |
| Cold setup on a small sandbox (2 vCPU / 4 GB) | about 29 min | earlier runs, not repeated |
| `warm.sh`, large sandbox, cold | 14 min 8 s (create 9 s, apt 39 s, clippy 193 s, test build 565 s) | `warm.sh` run on 2026-11-14 |
| `refresh.sh`, 4 commits behind, one Rust file changed | 6 min 42 s | `refresh.sh` run |
| `check.sh --filter`, edit loop: fmt 16 s, clippy 75 s, filtered tests 357 s | 8 min 18 s including the copy and polling | `check.sh` run |
| `check.sh --full`: node tests 7 s, fmt 16 s, clippy 87 s, test build and 9350 tests 525 s (all passed) | 11 min 26 s including the copy and polling | `check.sh` run |
| `check.sh` when clippy fails: fmt 16 s, clippy 87 s, tests skipped | 2 min 13 s | `check.sh` run |
| Edit loop in the earlier experiments | 6.5 to 8 min | experiments, several runs |
| Full check in the earlier experiments (node tests, fmt, clippy, build, 9345 tests) | about 10 min | experiments |
| Warm clippy with no edit | under 1 s | experiments |

The edit loop is dominated by one `rustc` process for the `monocode_gpui` test binary
(about 315 to 380 s). More vCPUs do not shorten it. That process peaks at about 7.6 GB, so the
sandbox is sized by memory: a small sandbox (4 GB) cannot build it and large (16 GB) is the
shape that was verified.

Cost: only the small shape (0.56 USD/h) is known. The other prices below are an assumption of
cost linear in vCPUs and were not checked against billing: large about 2.24 USD/h, so one
edit-loop pass is about 0.30 USD and a whole ticket of about an hour about 2.2 USD.

## Rules

- **One combined package set, always.** Use `-p monocode-core -p monocode-gpui` for clippy and
  tests. Cargo rebuilds the workspace crates whenever the `-p` set changes, and the two
  packages on their own invalidate each other (78 to 90 s, then 340 to 360 s, on every
  switch). The scripts use the combined set; set `BOX_PACKAGES` only for another project.
- **One large sandbox per ticket.** A shared build box gains nothing: cargo's build-directory
  lock serializes builds, two tickets at once took 874 s against 830 s one after the other, and
  two worktrees do not share the workspace-crate artifacts.
- **TTL 2 hours with `--on-timeout stop`.** The files survive a stop. To go on past it:
  `sbx --cloud ttl +1h NAME` (there is a hard 24 h ceiling from creation).
- **`rm --force`, do not stop and wait.** A large sandbox needed 7 to 9 minutes to reach
  `stopped`; `rm --force` took about 2 seconds. Never `rm` without `--force` in a script
  (it fails without a terminal).
- **At most 4 sandboxes and 24 vCPU per account** at once, and that count includes the app's
  own cloud sandboxes. `warm.sh` refuses to create one when `BOX_MAX_BOXES` (default 4) exist.
- **Use the box directly, do not save a template for one ticket.** Saving a template of the
  warm box takes about 2 minutes (35 s for a delta) and creating from it about 47 s, which
  is more than it saves for a single ticket. A template pays off only for many tickets from
  the same base. The scripts do not use templates.
- **Do not put credentials in the sandbox.** The scripts clone a public repo over HTTPS and
  refuse a repo URL with credentials in it. A private repo needs `sbx`'s own secret mechanism.
  The patch you send is your own diff, so do not check a diff that contains secrets.

## Practical notes

- `sbx cp` of a `git diff` patch plus `git apply --index` is reliable, and `check.sh` does
  it for you. There is no sandbox-to-sandbox copy.
- nextest's `ci` profile prints no PASS lines. To prove a new test really ran, `check.sh`
  also prints `cargo nextest list -E ...` for the filter; a filter that matches nothing makes
  nextest fail instead of passing quietly.
- rustfmt may reorder or rewrap things you did not touch. Run `cargo fmt` locally before the
  first pass; if the sandbox still finds a diff, copy it back (`--apply-fmt`).
- The sandbox clock is UTC. The Mac log lines are local time and the sandbox's are UTC.
- `pkill -f` inside `sbx exec bash -c '...'` also matches the exec itself. Use the process
  name (`pkill -x`) or a pid.
- `sbx secret ls`/`rm` echo a masked token (first and last characters). Do not paste their
  output into logs.

## What a sandbox cannot check

A Linux sandbox cannot compile or run:

- code under `cfg(target_os = "macos")` and tests gated to macOS (`#[cfg(target_os = "macos")]`
  or `cfg_attr(not(target_os = "macos"), ignore = ...)`),
- anything that needs Metal, WKWebView, AVPlayer or the Keychain (for example the tests in
  `crates/monocode-gpui/src/metal_tests.rs`, which are `#[ignore]`d and need a Metal device),
- Windows-only code and the Windows build,
- macOS-only `#[ignore]` tests and the native app window.

For those, run `npm run native:check:mac` on the Mac (clippy of the macOS-only code), plus the
macOS tests you touched, and `npm run native:check` before a release. A pass in the sandbox is
a Linux pass; do not report it as a macOS pass. The sandbox also does not run doctests
(`cargo test`, which the core job in CI runs), because nextest does not.

## Reusing the scripts for another Rust project

Every setting is an environment variable with a Monoptah default (see the top of
`scripts/sandbox/lib.sh`). The ones to change:

| Variable | Default | Meaning |
| --- | --- | --- |
| `BOX_REPO_URL` | `https://github.com/hhoangg/monoptah.git` | Public repo to clone (no credentials in the URL). |
| `BOX_REF` | `main` | Branch or commit to clone and refresh to. |
| `BOX_PACKAGES` | `monocode-core monocode-gpui` | The one combined cargo package set. |
| `BOX_MANIFEST` | `crates/monocode-gpui/Cargo.toml` | `--manifest-path` of the workspace. |
| `BOX_CARGO_PROFILE` | `native-dev` | Cargo profile for clippy and tests. |
| `BOX_NEXTEST_CONFIG`, `BOX_NEXTEST_PROFILE` | the repo's `.config/nextest.toml`, `ci` | nextest settings. |
| `BOX_CPUS`, `BOX_MEMORY` | `8`, `16g` | Shape; only billable shapes (1/2g, 2/4g, 4/8g, 8/16g, 16/32g). |
| `BOX_TTL`, `BOX_ON_TIMEOUT` | `2h`, `stop` | Lifetime and what happens at the end. |
| `BOX_NODE_TEST_CMD` | `npm run native:test:dev` | Node step of `--full` and of Rust-free patches; empty means no Node at all. |
| `BOX_RUST_VERSION`, `BOX_NEXTEST_VERSION` | read from the repo | Override the toolchain pins. |
| `BOX_APT_PACKAGES` | the native Linux build dependencies | System packages to install. |
| `BOX_NAME`, `BOX_STATE_DIR` | last recorded box, `~/.local/state/sandbox-build-box` | Which sandbox, and where the list of created ones lives. |

The toolchain pins are read from `.devcontainer/Dockerfile` (`ARG RUST_VERSION=`) and
`.github/workflows/*.yml` (`nextest@X`); a project without them gets `stable` and the latest
nextest unless the variables are set.

## Not tested

- Medium (4 vCPU / 8 GB) and xl (16 vCPU / 32 GB) shapes. Medium may fit the 7.6 GB peak only
  barely; it was not tried.
- sccache. The edit loop recompiles one workspace crate, which it would not cache across
  edits.
- `--on-timeout restart` and a full 2 h TTL running out (a 5 minute TTL was tested once in the
  experiments).
- x86_64 sandboxes, private repositories, and a patch with binary files.
- Prices other than the small shape.
