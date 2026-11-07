#!/usr/bin/env bash
# Lifecycle bootstrap for the Monoptah devcontainer.
# Usage: bootstrap.sh <update-content|post-create|post-start>
# No global -e: required steps fail explicitly, optional steps warn and continue.
# Idempotent: every step is safe to re-run.
set -uo pipefail

PHASE="${1:-}"
START_TS=$(date +%s)

cd "$(dirname "$0")/.." || { echo "ERROR: cannot cd to repo root" >&2; exit 1; }
REPO_ROOT=$(pwd)

# Lifecycle commands may not see the rust feature's PATH.
if [ -d /usr/local/cargo/bin ]; then
  export PATH="/usr/local/cargo/bin:$PATH"
fi

section() {
  echo
  echo "------------------------------------------------------------"
  echo "==> [monoptah devcontainer] ${PHASE}: $1"
  echo "    $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "------------------------------------------------------------"
}

run_optional() {
  local desc="$1"
  shift
  "$@"
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "WARN: ${desc} failed (exit ${rc}), continuing"
  fi
  return 0
}

run_required() {
  local desc="$1"
  shift
  "$@"
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "ERROR: ${desc} failed (exit ${rc})" >&2
    exit "$rc"
  fi
}

finish() {
  echo
  echo "==> [monoptah devcontainer] ${PHASE}: finished in $(($(date +%s) - START_TS))s"
}

usage() {
  echo "Usage: $0 <update-content|post-create|post-start>" >&2
}

case "$PHASE" in
  update-content)
    section "npm ci"
    if [ -f package-lock.json ]; then
      run_required "npm ci" npm ci --no-audit --no-fund
    else
      echo "No package-lock.json, skipping npm ci"
    fi
    section "cargo fetch"
    run_required "cargo fetch" cargo fetch --manifest-path crates/monocode-gpui/Cargo.toml --locked
    finish
    ;;
  post-create)
    section "Tool versions"
    run_optional "gh --version" gh --version
    run_optional "node --version" node --version
    run_optional "npm --version" npm --version
    run_optional "claude --version" claude --version
    run_optional "chromium --version" chromium --version
    run_optional "chromedriver --version" chromedriver --version
    run_optional "cargo --version" cargo --version
    run_optional "rustc --version" rustc --version
    section "Codespace checks"
    run_optional "whoami" whoami
    run_optional "pwd" pwd
    # The app reads the token from an ssh login shell, not from this lifecycle env.
    if [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then
      echo "CLAUDE_CODE_OAUTH_TOKEN (lifecycle env): set"
    else
      echo "CLAUDE_CODE_OAUTH_TOKEN (lifecycle env): NOT set - add it as a Codespaces secret for this repo"
    fi
    if env -u CLAUDE_CODE_OAUTH_TOKEN sh -lc '[ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]'; then
      echo "CLAUDE_CODE_OAUTH_TOKEN (login shell): set"
    else
      echo "WARN: CLAUDE_CODE_OAUTH_TOKEN (login shell): NOT set - the app's ssh bootstrap will not see it"
    fi
    run_optional "login shell claude lookup" sh -lc 'command -v claude || echo "claude not on login PATH"'
    run_optional "login shell cargo lookup" sh -lc 'command -v cargo || echo "cargo not on login PATH"'
    if command -v sshd >/dev/null 2>&1 || test -x /usr/sbin/sshd; then
      echo "sshd: installed"
    else
      echo "WARN: sshd not installed, gh codespace ssh will not work"
    fi
    finish
    ;;
  post-start)
    section "Ready"
    echo "Repo: ${REPO_ROOT}"
    BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || echo "WARN: could not read git branch, continuing"
    echo "Branch: ${BRANCH:-unknown}"
    echo "Monoptah devcontainer started"
    finish
    ;;
  *)
    usage
    exit 2
    ;;
esac
