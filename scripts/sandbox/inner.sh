#!/usr/bin/env bash
# Runs INSIDE the sandbox (Debian, user agent, sudo). Copied and started by the host scripts; do not
# run it by hand unless you are debugging. Plain Linux only: nothing here may assume macOS.
#   inner.sh warm            apt, clone, toolchain, nextest, node, then build for the combined set
#   inner.sh refresh         move the clone to origin/$BOX_REF and rebuild (same steps as warm's build)
#   inner.sh check PATCH     reset the clone, apply PATCH, then fmt / clippy / tests
# Settings come from ~/box-env (written by the host from BOX_* variables).
# The do_* functions are called through `step`, which shellcheck cannot follow.
# shellcheck disable=SC2317
set -uo pipefail
# shellcheck disable=SC1091
. "$HOME/box-env"
REPO="$HOME/work/repo"
STEPS_FILE="$HOME/box-steps"

ts() { printf '[%s] %s\n' "$(date +%T)" "$*"; }
env_rust() {
  export RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo
  export PATH=/opt/rust/cargo/bin:/opt/node/bin:$PATH
  export CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never CARGO_INCREMENTAL=0
  export CARGO_BUILD_JOBS=$BOX_JOBS
}

# step NAME CMD...: run, time, record "NAME seconds exit" in the steps file and print a STEP line.
step() {
  local name=$1; shift
  local t0 t1 rc
  t0=$(date +%s); "$@"; rc=$?; t1=$(date +%s)
  printf 'STEP %s %ss exit=%s\n' "$name" "$((t1 - t0))" "$rc"
  printf '%s %s %s\n' "$name" "$((t1 - t0))" "$rc" >> "$STEPS_FILE"
  return $rc
}

# Fill the PKG array (bash 3.2 safe, no mapfile).
load_pkgs() { PKG=(); local p; for p in $BOX_PACKAGES; do PKG+=(-p "$p"); done; }

detect_pins() {
  [ -n "$BOX_RUST_VERSION" ] || BOX_RUST_VERSION=$(sed -n 's/^ARG RUST_VERSION=\([0-9][0-9.]*\).*/\1/p' "$REPO/.devcontainer/Dockerfile" 2>/dev/null | head -n 1)
  [ -n "$BOX_RUST_VERSION" ] || BOX_RUST_VERSION=stable
  [ -n "$BOX_NEXTEST_VERSION" ] || BOX_NEXTEST_VERSION=$(grep -ho 'nextest@[0-9][0-9.]*' "$REPO"/.github/workflows/*.yml 2>/dev/null | head -n 1 | cut -d@ -f2)
  [ -n "$BOX_NEXTEST_VERSION" ] || BOX_NEXTEST_VERSION=latest
}

arch_names() {
  case $(uname -m) in
    x86_64)  TRIPLE=x86_64-unknown-linux-gnu;  NEXTEST_OS=linux;     NODE_ARCH=x64 ;;
    aarch64) TRIPLE=aarch64-unknown-linux-gnu; NEXTEST_OS=linux-arm; NODE_ARCH=arm64 ;;
    *) ts "unsupported architecture $(uname -m)"; return 1 ;;
  esac
}

do_apt() {
  sudo apt-get update -qq || return 1
  # shellcheck disable=SC2086
  sudo DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y -qq $BOX_APT_PACKAGES >/dev/null
}

do_clone() {
  mkdir -p "$HOME/work" && cd "$HOME/work" || return 1
  [ -d repo/.git ] || git clone --quiet "$BOX_REPO_URL" repo || return 1
  cd repo && git fetch --quiet origin && git checkout --quiet --detach "origin/$BOX_REF" 2>/dev/null \
    || git checkout --quiet --detach "$BOX_REF" || return 1
  git log -1 --oneline
}

do_rust() {
  arch_names || return 1
  sudo mkdir -p /opt/rust && sudo chown "$(id -u):$(id -g)" /opt/rust
  env_rust
  if ! command -v rustup >/dev/null 2>&1; then
    local url=https://static.rust-lang.org/rustup/dist/$TRIPLE/rustup-init expected
    curl --proto '=https' --tlsv1.2 -fsSL "$url" -o /tmp/rustup-init || return 1
    expected=$(curl --proto '=https' --tlsv1.2 -fsSL "$url.sha256" | cut -d' ' -f1)
    echo "$expected  /tmp/rustup-init" | sha256sum -c - || return 1
    chmod +x /tmp/rustup-init
    /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "$BOX_RUST_VERSION" \
      --component rustfmt --component clippy || return 1
    rm -f /tmp/rustup-init
  else
    rustup toolchain install "$BOX_RUST_VERSION" --profile minimal --component rustfmt --component clippy || return 1
    rustup default "$BOX_RUST_VERSION" || return 1
  fi
  cat <<ENV | sudo tee /etc/profile.d/rust.sh >/dev/null
export RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo
export PATH=/opt/rust/cargo/bin:/opt/node/bin:\$PATH
export CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0
ENV
  grep -q profile.d/rust.sh "$HOME/.bashrc" 2>/dev/null || echo '. /etc/profile.d/rust.sh' >> "$HOME/.bashrc"
  rustc --version
}

do_nextest() {
  arch_names || return 1
  env_rust
  if cargo nextest --version 2>/dev/null | head -n 1 | grep -q "nextest $BOX_NEXTEST_VERSION"; then
    cargo nextest --version | head -n 1; return 0
  fi
  curl -LsSf "https://get.nexte.st/$BOX_NEXTEST_VERSION/$NEXTEST_OS" | tar zxf - -C "$CARGO_HOME/bin" || return 1
  cargo nextest --version | head -n 1
}

do_node() {
  [ -n "$BOX_NODE_TEST_CMD" ] || { ts "no node step configured, skipping node"; return 0; }
  arch_names || return 1
  if [ -x /opt/node/bin/node ]; then /opt/node/bin/node --version; return 0; fi
  sudo mkdir -p /opt/node && sudo chown "$(id -u):$(id -g)" /opt/node
  curl -fsSL "https://nodejs.org/dist/v$BOX_NODE_VERSION/node-v$BOX_NODE_VERSION-linux-$NODE_ARCH.tar.xz" \
    | tar -xJ -C /opt/node --strip-components=1 || return 1
  /opt/node/bin/node --version
}

# The warm build: dependencies, then clippy and the test binaries for the ONE combined -p set.
do_build() {
  env_rust; load_pkgs
  cd "$REPO" || return 1
  if [ -n "$BOX_NODE_TEST_CMD" ] && [ -f package-lock.json ]; then
    step npm-ci npm ci --silent || return 1
  fi
  step cargo-fetch cargo fetch --locked --manifest-path "$BOX_MANIFEST" || return 1
  step warm-clippy cargo clippy --locked --manifest-path "$BOX_MANIFEST" "${PKG[@]}" --all-targets \
    --profile "$BOX_CARGO_PROFILE" -- -D warnings || return 1
  step warm-test-build cargo nextest run --no-run --locked --manifest-path "$BOX_MANIFEST" "${PKG[@]}" \
    --cargo-profile "$BOX_CARGO_PROFILE" || return 1
  du -sh "$(dirname "$BOX_MANIFEST")/target" /opt/rust 2>/dev/null || true
}

cmd_warm() {
  : > "$STEPS_FILE"
  step apt do_apt || return 1
  step clone do_clone || return 1
  detect_pins
  ts "pins: rust $BOX_RUST_VERSION, nextest $BOX_NEXTEST_VERSION"
  step rust do_rust || return 1
  step nextest do_nextest || return 1
  step node do_node || return 1
  do_build
}

cmd_refresh() {
  : > "$STEPS_FILE"
  step clone do_clone || return 1
  detect_pins
  ts "pins: rust $BOX_RUST_VERSION, nextest $BOX_NEXTEST_VERSION"
  env_rust
  # A toolchain or nextest bump in the repo is picked up here; unchanged pins are no-ops.
  if [ "$(rustc --version 2>/dev/null | cut -d' ' -f2)" != "$BOX_RUST_VERSION" ]; then
    step rust do_rust || return 1
  fi
  step nextest do_nextest || return 1
  do_build
}

fmt_check() { cargo fmt --manifest-path "$BOX_MANIFEST" --all --check >/dev/null 2>&1; }

# check PATCH: the edit loop. Base checkout, patch with --index, fmt, clippy, then tests only if
# clippy passed. BOX_FILTER (nextest filter expression) selects tests; BOX_FULL=1 runs everything.
cmd_check() {
  local patch=$1 rc=0
  : > "$STEPS_FILE"; rm -f "$HOME/box-fmt.patch"
  env_rust; load_pkgs
  cd "$REPO" || return 1
  git reset -q --hard && git clean -fdq || return 1       # ignored files (target, node_modules) stay
  git fetch -q origin || return 1
  git checkout -q --detach "$BOX_BASE_SHA" || { ts "base $BOX_BASE_SHA is not on $BOX_REPO_URL: push or rebase onto the remote main first"; return 1; }
  git apply --index "$patch" || { ts "patch does not apply to $BOX_BASE_SHA"; return 1; }
  git diff --cached --stat | tail -n 1

  if ! git diff --cached --name-only | grep -Eq '\.rs$|(^|/)Cargo\.(toml|lock)$|(^|/)\.cargo/|nextest\.toml$'; then
    ts "no Rust files changed: skipping fmt, clippy and tests"
    if [ -n "$BOX_NODE_TEST_CMD" ]; then
      # shellcheck disable=SC2086
      step node-tests $BOX_NODE_TEST_CMD || rc=1
    fi
    return $rc
  fi

  if [ "${BOX_FULL:-0}" = 1 ] && [ -n "$BOX_NODE_TEST_CMD" ]; then
    # shellcheck disable=SC2086
    step node-tests $BOX_NODE_TEST_CMD || rc=1
  fi

  # fmt: a failing check is turned into a diff the host copies back. The tree is then formatted,
  # so clippy and the tests run on what will exist once the diff is applied locally.
  if ! step fmt-check fmt_check; then
    cargo fmt --manifest-path "$BOX_MANIFEST" --all
    git diff > "$HOME/box-fmt.patch"
    ts "rustfmt would change $(grep -c '^diff --git' "$HOME/box-fmt.patch") file(s); diff saved for copy back"
    rc=1
  fi

  if ! step clippy cargo clippy --locked --manifest-path "$BOX_MANIFEST" "${PKG[@]}" --all-targets \
      --profile "$BOX_CARGO_PROFILE" -- -D warnings; then
    ts "clippy failed: tests skipped"
    return 1
  fi

  local nextest=(cargo nextest run --locked --manifest-path "$BOX_MANIFEST" "${PKG[@]}"
    --cargo-profile "$BOX_CARGO_PROFILE" --config-file "$BOX_NEXTEST_CONFIG" --profile "$BOX_NEXTEST_PROFILE")
  if [ "${BOX_FULL:-0}" = 1 ]; then
    step tests-full "${nextest[@]}" || rc=1
  elif [ -n "${BOX_FILTER:-}" ]; then
    # The ci profile prints no PASS lines, so prove the filter selected something: list it.
    step tests-filtered "${nextest[@]}" -E "$BOX_FILTER" || rc=1
    ts "tests selected by the filter:"
    cargo nextest list --locked --manifest-path "$BOX_MANIFEST" "${PKG[@]}" --cargo-profile "$BOX_CARGO_PROFILE" \
      --config-file "$BOX_NEXTEST_CONFIG" -E "$BOX_FILTER" --message-format oneline 2>/dev/null | head -n 40
  else
    ts "no --filter and no --full: fmt and clippy only"
  fi
  return $rc
}

sub=${1:-}
case $sub in
  warm)    cmd_warm ;;
  refresh) cmd_refresh ;;
  check)   cmd_check "${2:?patch path}" ;;
  *) echo "usage: inner.sh warm|refresh|check PATCH" >&2; exit 2 ;;
esac
rc=$?
if [ -s "$STEPS_FILE" ]; then
  echo "--- steps"
  awk '{ printf "  %-18s %6ss  exit=%s\n", $1, $2, $3 }' "$STEPS_FILE"
fi
exit $rc
