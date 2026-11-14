#!/usr/bin/env bash
# Shared helpers for the sandbox build box scripts. Sourced, never run.
# See docs/native/sandbox-build-box.md for what these scripts are for and the numbers behind them.
#
# Every setting is an environment variable with a Monoptah default, so another Rust project
# can reuse the scripts by exporting different values. No script reads or stores a token:
# authentication is whatever `sbx login` already holds.

# --- sandbox shape and lifetime ---------------------------------------------------------------
: "${BOX_CPUS:=8}"                  # large = 8 vCPU / 16 GB. The GPUI test build peaks near 7.6 GB.
: "${BOX_MEMORY:=16g}"              # sbx accepts only billable shapes: 1/2g 2/4g 4/8g 8/16g 16/32g
: "${BOX_TTL:=2h}"
: "${BOX_ON_TIMEOUT:=stop}"         # stop keeps the files; delete would lose the warm target dir
: "${BOX_MAX_BOXES:=4}"             # warm.sh refuses to create more than this many cloud sandboxes
# --- what to build ------------------------------------------------------------------------------
: "${BOX_REPO_URL:=https://github.com/hhoangg/monoptah.git}"
: "${BOX_REF:=main}"                # branch or commit of the repo the sandbox clones and refreshes to
: "${BOX_MANIFEST:=crates/monocode-gpui/Cargo.toml}"
# ONE combined package set, always. cargo rebuilds the workspace crates when the -p set changes,
# so "-p a" then "-p b" then "-p a" rebuilds each time. Space separated.
: "${BOX_PACKAGES:=monocode-core monocode-gpui}"
: "${BOX_CARGO_PROFILE:=native-dev}"
: "${BOX_NEXTEST_CONFIG:=crates/monocode-gpui/.config/nextest.toml}"
: "${BOX_NEXTEST_PROFILE:=ci}"
: "${BOX_JOBS:=4}"                  # CARGO_BUILD_JOBS; keeps the peak memory inside 16 GB
: "${BOX_NODE_TEST_CMD:=npm run native:test:dev}"   # empty: no node step at all (no node, no npm ci)
: "${BOX_NODE_VERSION:=22.12.0}"
# Empty means: read it from the repo checkout (.devcontainer/Dockerfile ARG RUST_VERSION and the
# `nextest@X` pin in .github/workflows/native.yml), so the box follows the repo's own pins.
: "${BOX_RUST_VERSION:=}"
: "${BOX_NEXTEST_VERSION:=}"
# sha256 of the Node tarball / nextest archive for the box architecture. Needed only for a version
# that inner.sh has no pinned hash for; the download is refused without a matching hash.
: "${BOX_NODE_SHA256:=}"
: "${BOX_NEXTEST_SHA256:=}"
: "${BOX_APT_PACKAGES:=build-essential clang cmake ninja-build pkg-config libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-randr0-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libasound2-dev libx11-xcb-dev libegl1-mesa-dev libvulkan-dev libdbus-1-dev squashfs-tools desktop-file-utils ca-certificates xz-utils zstd git curl}"
# --- local behaviour -----------------------------------------------------------------------------
: "${BOX_POLL:=20}"                 # seconds between log polls of a detached run
: "${BOX_HOME:=/home/agent}"        # the sandbox user's home; sbx cp needs absolute paths
: "${BOX_STATE_DIR:=${XDG_STATE_HOME:-$HOME/.local/state}/sandbox-build-box}"

BOX_SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

log() { printf '[%s] %s\n' "$(date +%T)" "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Reject anything that could carry a credential into a script argument or a log line.
guard_no_secret_in_url() {
  case $1 in
    *://*@*) die "BOX_REPO_URL must not contain credentials (user:token@host). The box clones public repos; for a private one use sbx's own secret mechanism." ;;
  esac
}

# --- which repo the patch is taken against ----------------------------------------------------------
# Remote NAMES differ per checkout (origin is the fork in one, the main repo in another), so the base
# is chosen by URL: the remote that points at the repo the box clones.
norm_git_url() { # https://github.com/o/r.git, git@github.com:o/r and ssh://git@github.com/o/r/ -> github.com/o/r
  local u
  case $1 in *://*) u=${1#*://} ;; *) u=${1/:/\/} ;; esac
  u=${u#*@}; u=${u%/}; u=${u%.git}; u=${u%/}
  printf '%s\n' "$u" | tr '[:upper:]' '[:lower:]'
}
matching_remote() { # matching_remote URL: first remote of the current repo whose URL is the same repo
  local want r
  want=$(norm_git_url "$1")
  for r in $(git remote); do
    [ "$(norm_git_url "$(git remote get-url "$r" 2>/dev/null)")" = "$want" ] && { printf '%s\n' "$r"; return 0; }
  done
  return 1
}
# Names that usually hold a secret. Checked on the lower-cased file name.
is_secret_name() {
  local b; b=$(basename -- "$1" | tr '[:upper:]' '[:lower:]')
  case $b in
    .env|.env.*|*.pem|*.key|id_rsa*|id_ed25519*|id_ecdsa*|credentials*|*.p12|*.pfx|*.token) return 0 ;;
  esac
  return 1
}

require_sbx() {
  command -v sbx >/dev/null 2>&1 || die "sbx is not installed or not on PATH"
  sbx --cloud ls -q >/dev/null 2>&1 || die "'sbx --cloud ls' failed: run 'sbx login' first"
}

sx() { sbx --cloud "$@"; }

# --- state: the sandboxes these scripts created, so clean.sh removes exactly those --------------
box_state_file() { printf '%s/sandboxes\n' "$BOX_STATE_DIR"; }
record_box() {
  mkdir -p "$BOX_STATE_DIR"
  grep -qxF "$1" "$(box_state_file)" 2>/dev/null || printf '%s\n' "$1" >> "$(box_state_file)"
}
forget_box() {
  local f; f=$(box_state_file)
  [ -f "$f" ] || return 0
  grep -vxF "$1" "$f" > "$f.tmp" || true
  mv "$f.tmp" "$f"
}
last_box() { if [ -s "$(box_state_file)" ]; then tail -n 1 "$(box_state_file)"; fi; }

# Pick the sandbox: --name, then BOX_NAME, then the last one a script created.
resolve_box() {
  local n=${1:-${BOX_NAME:-}}
  [ -n "$n" ] || n=$(last_box)
  [ -n "$n" ] || die "no sandbox: pass --name NAME, set BOX_NAME, or run warm.sh first"
  valid_name "$n" || die "bad sandbox name '$n' (2-63 letters, digits, hyphens)"
  printf '%s\n' "$n"
}
valid_name() { printf '%s' "$1" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9-]{0,61}[A-Za-z0-9]$'; }

# --- the environment file shipped to the sandbox ---------------------------------------------------
# Only the settings listed here cross over. Values are shell-quoted with printf %q.
write_env_file() { # write_env_file PATH [EXTRA_VAR ...]
  local out=$1; shift
  local v
  : > "$out"
  for v in BOX_REPO_URL BOX_REF BOX_MANIFEST BOX_PACKAGES BOX_CARGO_PROFILE BOX_NEXTEST_CONFIG \
           BOX_NEXTEST_PROFILE BOX_JOBS BOX_NODE_TEST_CMD BOX_NODE_VERSION BOX_RUST_VERSION \
           BOX_NEXTEST_VERSION BOX_NODE_SHA256 BOX_NEXTEST_SHA256 BOX_APT_PACKAGES "$@"; do
    printf '%s=%q\n' "$v" "${!v:-}" >> "$out"
  done
}

# Run `inner.sh SUBCOMMAND [ARG]` in the sandbox as a detached process and follow its log. A cloud
# `sbx exec` is a poor home for a 10 to 30 minute build (one dropped connection kills it), so the
# run is detached with setsid and polled. Returns the remote exit status. Output goes to stderr.
# Extra env vars to ship are listed in BOX_EXTRA_ENV (space separated names).
remote_run() { # remote_run NAME SUBCOMMAND [ARG]
  local name=$1 sub=$2 arg=${3:-}
  local tmp; tmp=$(mktemp -d)
  # shellcheck disable=SC2086
  write_env_file "$tmp/box-env" ${BOX_EXTRA_ENV:-}
  sx cp "$BOX_SCRIPT_DIR/inner.sh" "$name:$BOX_HOME/box-inner.sh" >/dev/null || { rm -rf "$tmp"; die "could not copy inner.sh into $name"; }
  sx cp "$tmp/box-env" "$name:$BOX_HOME/box-env" >/dev/null || { rm -rf "$tmp"; die "could not copy the env file into $name"; }
  rm -rf "$tmp"
  # The subshell form returns at once; the wrapper records its own pid and, when the run ends, its status.
  # SUB and ARG reach the wrapper as positional parameters, shell-quoted with %q, so a BOX_HOME with
  # spaces (or any other character) stays one argument.
  local q_args; q_args=$(printf '%q' "$sub"); [ -z "$arg" ] || q_args="$q_args $(printf '%q' "$arg")"
  sx exec "$name" bash -c "cd ~ && rm -f box.rc box.log box.pid && \
    (setsid nohup bash -c 'echo \$\$ > ~/box.pid; bash ~/box-inner.sh \"\$@\" > ~/box.log 2>&1; echo \$? > ~/box.rc' _ $q_args >/dev/null 2>&1 </dev/null &)" \
    || die "could not start $sub in $name"
  local next=1 fails=0 out head body rc alive
  while :; do
    sleep "$BOX_POLL"
    # One exec per poll: new complete log lines, then the run state. Header line "@@N <lines so far>".
    out=$(sx exec "$name" bash -c "cd ~; n=\$(wc -l < box.log); echo @@N \$n; \
      tail -n +$next box.log | head -n \$((n - $next + 1)); \
      if [ -s box.rc ]; then echo @@RC \$(cat box.rc); \
      elif kill -0 \$(cat box.pid) 2>/dev/null; then echo @@RC running; else echo @@RC died; fi" 2>/dev/null) || {
      fails=$((fails + 1))
      [ "$fails" -le 20 ] || die "lost contact with $name (20 failed polls in a row)"
      continue
    }
    fails=0
    head=$(printf '%s\n' "$out" | sed -n '1p')
    rc=$(printf '%s\n' "$out" | sed -n '$p')
    body=$(printf '%s\n' "$out" | sed '1d;$d')
    [ -z "$body" ] || printf '%s\n' "$body" >&2
    case $head in '@@N '*) next=$(( ${head#@@N } + 1 )) ;; esac
    case $rc in
      '@@RC running') ;;
      '@@RC died') log "the run in $name died without an exit status (out of memory? see ~/box.log in the sandbox)"; return 99 ;;
      '@@RC '*) alive=${rc#@@RC }; return "$alive" ;;
    esac
  done
}
