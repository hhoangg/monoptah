set -eu
umask 077
BASE="$HOME/.monocode-host"
ENTRY="$BASE/bin/monocode-host"
VERSION=@@VERSION@@
RELEASE=@@RELEASE@@
EXISTED=0
[ -x "$ENTRY" ] && EXISTED=1
FORCE_UPGRADE=${MONOCODE_HOST_FORCE_UPGRADE:-0}
HOST_PORT=${MONOCODE_HOST_PORT:-3774}

# True (0) when the installed Host must be replaced by the pinned one: it lacks the
# "+monoptah" flavor marker (an upstream Host, whatever its number; "+monoptah.N" counts as
# the marker) or it is lower than the pin. A pre-release ("1.1.0-rc.1") is lower than its
# release. Other build metadata is ignored in the comparison. Not used by the install rule
# below unless MONOCODE_HOST_UPGRADE_IF_OLDER=1 (the two-phase cloud upgrade, which the app
# only starts after it saw the Host idle); the plain install rule stays "missing or
# FORCE_UPGRADE".
is_older_or_foreign() {
  case "$1" in *+monoptah|*+monoptah.*) ;; *) return 0 ;; esac
  _installed=${1%%+*}
  _pin=${2%%+*}
  _a_pre=0
  _b_pre=0
  case "$_installed" in *-*) _a_pre=1 ;; esac
  case "$_pin" in *-*) _b_pre=1 ;; esac
  IFS=. read -r _a1 _a2 _a3 <<EOF_INSTALLED
${_installed%%-*}
EOF_INSTALLED
  IFS=. read -r _b1 _b2 _b3 <<EOF_PIN
${_pin%%-*}
EOF_PIN
  for _n in "$_a1" "$_a2" "$_a3" "$_b1" "$_b2" "$_b3"; do
    case "$_n" in *[!0-9]*|'') return 0 ;; esac
  done
  [ "$_a1" -eq "$_b1" ] || { [ "$_a1" -lt "$_b1" ]; return; }
  [ "$_a2" -eq "$_b2" ] || { [ "$_a2" -lt "$_b2" ]; return; }
  [ "$_a3" -eq "$_b3" ] || { [ "$_a3" -lt "$_b3" ]; return; }
  # Same X.Y.Z: a pre-release is older than a release; two different pre-releases are
  # replaced (the pin is always a release in practice).
  [ "$_a_pre" -eq 1 ] || return 1
  [ "$_b_pre" -eq 0 ] || [ "$_installed" != "$_pin" ]
}

# The two-phase cloud upgrade (MONOCODE_HOST_UPGRADE_IF_OLDER=1) replaces an older or foreign
# Host and leaves a current one alone (another connect may have upgraded it in the meantime).
# A launcher that cannot print its version counts as foreign. The app drives it in steps
# through MONOCODE_HOST_UPGRADE_PHASE so a turn is never cut off:
#   stage    download and verify the package into a staging directory; the running Host and
#            its launcher are not touched. Prints {"staged":true} or {"staged":false} (current).
#   swapnow  the worker of the swap. The app has seen the Host idle and holds its own sends
#            back, and this checks AGAIN, on the Host itself, right before it stops the Host
#            (exit 77 and nothing touched if any session runs, waits, or cannot be read: other
#            windows and devices are not held back by the app). Then it installs the staged
#            package and restarts the Host. If the new Host does not start, the previous
#            runtime and launcher are restored and started again. It is started detached by the
#            app (see ssh.rs), ignores a closed ssh, and writes its progress to swap-status.
#   status   print the state of the last swap; a worker that died mid-swap is finished or
#            rolled back first.
#   discard  drop the staged package.
PHASE=${MONOCODE_HOST_UPGRADE_PHASE:-}
STAGED_FILE="$BASE/staged-runtime"
STATUS_FILE="$BASE/swap-status"
# Names one swap: the app only believes a status that carries its own.
NONCE=${MONOCODE_SWAP_NONCE:-}
SWAP_ACTIVE=0
TMP=
HB_PID=

# The worker's state, read by the app: checking <pid>, swapping <pid>, done, restored,
# unrestored, busy, refused <code>, failed <code>; then the nonce of the swap on a second line.
set_status() {
  printf '%s\n%s\n' "$1" "$NONCE" > "$BASE/swap-status.tmp" && mv "$BASE/swap-status.tmp" "$STATUS_FILE"
}
@@WORKER_ALIVE@@

# 0 when the Host that is running answers on its own port and every session on it is idle.
# Anything else (a Host that cannot be asked, no credential) counts as busy, so a turn is
# never cut off. The credential comes from the environment and is kept off the command line.
last_check() {
  [ -n "${CHECK_TOKEN:-}" ] || return 1
  command -v curl >/dev/null 2>&1 || return 1
  _projects=$(rpc_call projects.list '{}') || return 1
  case "$_projects" in '{"result":'*) ;; *) return 1 ;; esac
  for _id in $(printf '%s' "$_projects" | grep -o '"id":"[^"]*"' | sed 's/^"id":"//; s/"$//'); do
    _sessions=$(rpc_call sessions.list "{\"projectId\":\"$_id\"}") || return 1
    case "$_sessions" in '{"result":'*) ;; *) return 1 ;; esac
    printf '%s' "$_sessions" | grep -q '"needsInput":true' && return 1
    _total=$(printf '%s' "$_sessions" | grep -o '"projectId":"' | wc -l)
    _statuses=$(printf '%s' "$_sessions" | grep -o '"status":"' | wc -l)
    _quiet=$(printf '%s' "$_sessions" | grep -o -E '"status":"(idle|interrupted)"' | wc -l)
    # Every session must show a project and a quiet status. Anything that does not add up
    # (a session without the keys this looks for) counts as busy.
    [ "$_total" -eq "$_quiet" ] && [ "$_statuses" -eq "$_quiet" ] || return 1
  done
  return 0
}
rpc_call() {
  printf 'header = "Authorization: Bearer %s"\n' "$CHECK_TOKEN" | curl -fsS --max-time 10 -K - \
    -H 'Content-Type: application/json' \
    -d "{\"version\":1,\"environmentId\":\"${CHECK_ENV:-}\",\"method\":\"$1\",\"params\":$2}" \
    "http://127.0.0.1:$HOST_PORT/rpc"
}

# Puts the previous runtime and launcher back and starts the old Host. 0 when it answers.
rollback_swap() {
  set +e
  [ -f "$BASE/runtime-path.prev" ] && [ -f "$BASE/bin/monocode-host.prev" ] || return 1
  cp -p "$BASE/runtime-path.prev" "$BASE/runtime-path" || return 1
  cp -p "$BASE/bin/monocode-host.prev" "$ENTRY" || return 1
  "$ENTRY" service uninstall >/dev/null 2>&1
  (
    if command -v before_service_install >/dev/null 2>&1; then before_service_install || exit 1; fi
    "$ENTRY" service install --port "$HOST_PORT" 2>/dev/null >/dev/null || exit 1
  ) || return 1
  _r=0
  until "$ENTRY" connection-info >/dev/null 2>&1; do
    _r=$((_r + 1))
    [ "$_r" -lt 30 ] || return 1
    sleep 1
  done
}
on_exit() {
  _code=$?
  set +e
  [ -z "$TMP" ] || rm -rf "$TMP"
  [ "$PHASE" = swapnow ] || return 0
  [ -z "$HB_PID" ] || kill "$HB_PID" 2>/dev/null
  rm -f "$BASE/swap.lock"
  if [ "$SWAP_ACTIVE" = 1 ] && [ "$_code" -ne 0 ]; then
    SWAP_ACTIVE=0
    if rollback_swap; then
      set_status restored
      echo 'Monoptah Host update failed; the previous Host was restored.'
      exit 75
    fi
    set_status unrestored
    echo 'Monoptah Host update failed and the previous Host could not be restored.'
    exit 76
  fi
  case "$_code" in
    0) ;;
    77) set_status busy ;;
    3|78) set_status "refused $_code" ;;
    *) set_status "failed $_code" ;;
  esac
}

# 0 when some Host answers (a few tries: one failed call proves nothing).
host_answers() {
  _try=0
  while [ "$_try" -lt 5 ]; do
    "$ENTRY" connection-info >/dev/null 2>&1 && return 0
    _try=$((_try + 1))
    [ "$_try" -ge 5 ] || sleep "${MONOCODE_PROBE_WAIT:-2}"
  done
  return 1
}
if [ "$PHASE" = status ]; then
  _state=none
  _pid=
  _file_nonce=
  [ ! -f "$STATUS_FILE" ] || read -r _state _pid < "$STATUS_FILE" || true
  [ ! -f "$STATUS_FILE" ] || _file_nonce=$(sed -n 2p "$STATUS_FILE")
  if [ "$_state" != none ] && [ -n "$NONCE" ] && [ "$_file_nonce" != "$NONCE" ]; then
    # Left by some other swap: not this app's to read.
    echo '{"swap":"stale"}'
    exit 0
  fi
  case "$_state" in
    checking|swapping)
      if worker_alive "$_pid"; then
        :
      elif [ "$_state" = checking ]; then
        rm -f "$BASE/swap.lock"
        _state=failed
        set_status "failed 1"
      else
        # The worker is gone in the middle of the swap (killed, machine restarted). If a Host
        # answers it is NEVER restarted here: the files only have to say which one it is.
        # Only when nothing answers is the previous Host put back.
        rm -f "$BASE/swap.lock"
        _now=$(cat "$BASE/runtime-path" 2>/dev/null || true)
        _prev=$(cat "$BASE/runtime-path.prev" 2>/dev/null || true)
        if host_answers; then
          if [ -n "$_now" ] && [ "$_now" != "$_prev" ]; then
            "$ENTRY" connection-info > "$BASE/swap-info" 2>/dev/null
            _state=done
          else
            _state=restored
          fi
          set_status "$_state"
        elif rollback_swap; then
          _state=restored
          set_status restored
        else
          _state=unrestored
          set_status unrestored
        fi
      fi ;;
  esac
  case "$_state" in
    done)
      _port=$(sed -n 's/.*"port":\([0-9]*\).*/\1/p' "$BASE/swap-info" 2>/dev/null | head -n 1)
      echo "{\"swap\":\"done\",\"port\":${_port:-0}}" ;;
    refused) echo "{\"swap\":\"refused\",\"code\":${_pid:-0}}" ;;
    failed) echo '{"swap":"failed"}' ;;
    *) echo "{\"swap\":\"$_state\"}" ;;
  esac
  exit 0
fi
if [ "$PHASE" = discard ]; then
  if [ -f "$STAGED_FILE" ]; then
    _staged=$(cat "$STAGED_FILE")
    case "$_staged" in "$BASE"/runtime/staged-*) rm -rf "$_staged" ;; esac
    rm -f "$STAGED_FILE"
  fi
  echo '{"staged":false}'
  exit 0
fi
if [ "$FORCE_UPGRADE" != 1 ] && [ "$EXISTED" = 1 ] && [ "${MONOCODE_HOST_UPGRADE_IF_OLDER:-0}" = 1 ] && [ "$PHASE" != swapnow ]; then
  INSTALLED=$("$ENTRY" --version 2>/dev/null || true)
  if is_older_or_foreign "$INSTALLED" "$VERSION"; then
    FORCE_UPGRADE=1
  elif [ "$PHASE" = stage ]; then
    echo '{"staged":false}'
    exit 0
  fi
fi
if [ "$PHASE" = swapnow ]; then
  # The credential for the last look is kept in plain shell variables, and removed from the
  # environment BEFORE any process starts: the Host this script starts (the new one, or the old
  # one put back) must never inherit it, or any agent could print it.
  CHECK_TOKEN=${MONOCODE_CHECK_TOKEN:-}
  CHECK_ENV=${MONOCODE_CHECK_ENV:-}
  unset MONOCODE_CHECK_TOKEN MONOCODE_CHECK_ENV
  FORCE_UPGRADE=1
  # The Host is restarted around the install and Claude was installed by the connect. Nothing
  # slow may run after the Host is stopped.
  MONOCODE_HOST_INSTALL_CLAUDE=0
  trap on_exit EXIT
  trap '' HUP PIPE
  set_status "checking $$"
  exec >>"$BASE/swap.log" 2>&1
  printf '%s\n' "$$" > "$BASE/swap.lock"
  _worker=$$
  ( while kill -0 "$_worker" 2>/dev/null; do touch "$BASE/swap-heartbeat"; sleep 5; done ) </dev/null >/dev/null 2>&1 &
  HB_PID=$!
fi

if [ ! -x "$ENTRY" ] || [ "$FORCE_UPGRADE" = 1 ]; then
  mkdir -p "$BASE/runtime" "$BASE/bin"
  TMP=$(mktemp -d "$BASE/runtime/.install.XXXXXXXX")
  trap on_exit EXIT
  trap 'exit 130' INT TERM
  [ "$PHASE" = swapnow ] || trap 'exit 130' HUP
  if [ "$PHASE" != swapnow ]; then
  case "$(uname -s)" in Darwin) OS=darwin ;; Linux) OS=linux ;; *) echo 'Monoptah Host supports Linux and macOS.' >&2; exit 1 ;; esac
  case "$(uname -m)" in arm64|aarch64) ARCH=arm64 ;; x86_64|amd64) ARCH=x64 ;; *) echo 'Unsupported host architecture.' >&2; exit 1 ;; esac
  FILE="monocode-host-$OS-$ARCH.tar.gz"
  download() {
    if command -v curl >/dev/null 2>&1; then
      curl --proto '=https' --proto-redir '=https' -fLsS --connect-timeout 15 --max-time 180 "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
      wget --https-only --timeout=180 -q -O "$2" "$1"
    else
      echo 'Install curl or wget on this host and reconnect.' >&2; exit 1
    fi
  }
  # The expected hash is pinned in the app and substituted here per target, so a changed
  # release cannot vouch for itself. The published .sha256 is only a cross-check.
  case "$OS-$ARCH" in
@@SHA256@@
    *) echo 'No pinned checksum for this host target.' >&2; exit 1 ;;
  esac
  case "$EXPECTED" in *[!0-9a-f]*|'') echo 'Invalid host package checksum.' >&2; exit 1 ;; esac
  [ "${#EXPECTED}" -eq 64 ] || exit 1
  if ! download "$RELEASE/$FILE" "$TMP/$FILE" || ! download "$RELEASE/$FILE.sha256" "$TMP/checksum"; then
    echo "The Monoptah Host package for version $VERSION is unavailable. Install a Monoptah release that includes host packages." >&2; exit 1
  fi
  PUBLISHED=$(awk 'NR == 1 {print $1}' "$TMP/checksum")
  [ "$PUBLISHED" = "$EXPECTED" ] || { echo 'Monoptah Host pinned checksum mismatch: the published checksum differs.' >&2; exit 1; }
  if command -v shasum >/dev/null 2>&1; then
    ACTUAL=$(shasum -a 256 "$TMP/$FILE" | awk '{print $1}')
  elif command -v sha256sum >/dev/null 2>&1; then
    ACTUAL=$(sha256sum "$TMP/$FILE" | awk '{print $1}')
  else
    echo 'Install shasum or sha256sum on this host and reconnect.' >&2; exit 1
  fi
  [ "$EXPECTED" = "$ACTUAL" ] || { echo 'Monoptah Host package checksum mismatch.' >&2; exit 1; }
  mkdir "$TMP/unpacked"
  tar -xzf "$TMP/$FILE" -C "$TMP/unpacked"
  [ "$("$TMP/unpacked/monocode-host" --version)" = "$VERSION" ] || { echo 'Monoptah Host version mismatch.' >&2; exit 1; }
  fi
  if [ "$PHASE" = stage ]; then
    # Phase A: the verified package waits next to the running Host, which is not touched.
    _old_staged=$(cat "$STAGED_FILE" 2>/dev/null || true)
    case "$_old_staged" in "$BASE"/runtime/staged-*) rm -rf "$_old_staged" ;; esac
    DEST="$BASE/runtime/staged-$VERSION-$OS-$ARCH-$(basename "$TMP")"
    mv "$TMP/unpacked" "$DEST"
    [ "$("$DEST/monocode-host" --version)" = "$VERSION" ] || exit 1
    printf '%s\n' "$DEST" > "$STAGED_FILE"
    echo '{"staged":true}'
    exit 0
  fi
  if [ "$PHASE" = swapnow ]; then
    DEST=$(cat "$STAGED_FILE" 2>/dev/null || true)
    case "$DEST" in "$BASE"/runtime/staged-*) ;; *) echo 'No staged Monoptah Host package.'; exit 3 ;; esac
    [ "$("$DEST/monocode-host" --version 2>/dev/null)" = "$VERSION" ] || { echo 'The staged Monoptah Host package is not usable.'; exit 3; }
    # Keep what is running now, so a Host that fails to start can be put back. Without a
    # previous runtime and launcher there is no way back: the Host is left alone.
    if [ "$EXISTED" = 1 ]; then
      if [ -f "$BASE/runtime-path" ] && cp -p "$BASE/runtime-path" "$BASE/runtime-path.prev" && cp -p "$ENTRY" "$BASE/bin/monocode-host.prev"; then
        :
      else
        echo 'There is no previous Monoptah Host to roll back to.'
        exit 78
      fi
    fi
    # The last look, on the Host itself, right before it is stopped. The staged package stays.
    _looked=0
    last_check || _looked=1
    CHECK_TOKEN=
    unset CHECK_TOKEN CHECK_ENV
    [ "$_looked" = 0 ] || { echo 'A session is running on the Host.'; exit 77; }
    rm -f "$STAGED_FILE"
    set_status "swapping $$"
    [ "$EXISTED" != 1 ] || SWAP_ACTIVE=1
  else
    DEST="$BASE/runtime/$VERSION-$OS-$ARCH-$(basename "$TMP")"
    # Concurrent installations never replace a directory used by a running host.
    mv "$TMP/unpacked" "$DEST"
    [ "$("$DEST/monocode-host" --version)" = "$VERSION" ] || exit 1
  fi
  # Keep a real wrapper (rather than a symlink): it resolves the packaged Node
  # relative to the versioned executable, not this bin directory.
  printf '%s\n' "$DEST" > "$TMP/runtime-path"
  mv "$TMP/runtime-path" "$BASE/runtime-path"
  cat > "$TMP/launcher" <<'SH'
#!/bin/sh
set -eu
BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
RUNTIME=$(cat "$BASE/runtime-path")
exec "$RUNTIME/monocode-host" "$@"
SH
  chmod 700 "$TMP/launcher"
  mv "$TMP/launcher" "$ENTRY"
fi

if [ "$EXISTED" = 1 ] && [ "$FORCE_UPGRADE" = 1 ]; then
  "$ENTRY" service uninstall >/dev/null
fi
TOKEN_MARKER="$BASE/host-token.sha256"
TOKEN_HASH=
# A sandbox may track an API key instead (MONOCODE_HOST_TOKEN_VAR); a codespace never sets it.
case "${MONOCODE_HOST_TOKEN_VAR:-}" in
  ANTHROPIC_API_KEY) TOKEN_VALUE=${ANTHROPIC_API_KEY:-} ;;
  *) TOKEN_VALUE=${CLAUDE_CODE_OAUTH_TOKEN:-} ;;
esac
if [ "${MONOCODE_HOST_TRACK_TOKEN:-0}" = 1 ] && [ -n "$TOKEN_VALUE" ]; then
  if command -v sha256sum >/dev/null 2>&1; then
    TOKEN_HASH=$(printf '%s' "$TOKEN_VALUE" | sha256sum | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    TOKEN_HASH=$(printf '%s' "$TOKEN_VALUE" | shasum -a 256 | awk '{print $1}')
  fi
  # A host started without (or with another) token keeps its old environment.
  if [ -n "$TOKEN_HASH" ] && [ "$(cat "$TOKEN_MARKER" 2>/dev/null || true)" != "$TOKEN_HASH" ]; then
    "$ENTRY" service uninstall >/dev/null 2>&1 || true
  else
    TOKEN_HASH=
  fi
fi
# Codespaces: Claude Code is the only harness of a cloud session and a bare codespace
# image does not ship it. Install it (best effort) and restart a host that was started
# before it was available, since the host discovers providers only at startup.
CLAUDE_MARKER="$BASE/host-claude-path"
CLAUDE_PATH=
if [ "${MONOCODE_HOST_INSTALL_CLAUDE:-0}" = 1 ]; then
  if ! command -v claude >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/claude" ]; then
    echo 'Installing Claude Code on this host...' >&2
    if command -v curl >/dev/null 2>&1; then
      curl --proto '=https' -fsSL --connect-timeout 15 --max-time 300 https://claude.ai/install.sh | bash >&2 || echo 'Claude Code install failed; install it on this host manually.' >&2
    fi
  fi
  PATH="$HOME/.local/bin:$PATH"
  export PATH
  CLAUDE_PATH=$(command -v claude 2>/dev/null || true)
  if [ -n "$CLAUDE_PATH" ] && [ "$(cat "$CLAUDE_MARKER" 2>/dev/null || true)" != "$CLAUDE_PATH" ]; then
    "$ENTRY" service uninstall >/dev/null 2>&1 || true
  else
    CLAUDE_PATH=
  fi
fi
"$ENTRY" service install --port "$HOST_PORT" >/dev/null
[ -z "$TOKEN_HASH" ] || printf '%s\n' "$TOKEN_HASH" > "$TOKEN_MARKER"
[ -z "$CLAUDE_PATH" ] || printf '%s\n' "$CLAUDE_PATH" > "$CLAUDE_MARKER"
if [ "$PHASE" = swapnow ]; then
  # The new Host answers, or the trap puts the previous one back.
  _wait=0
  until "$ENTRY" connection-info >/dev/null 2>&1; do
    _wait=$((_wait + 1))
    [ "$_wait" -lt 30 ] || { echo 'The updated Monoptah Host did not start.'; exit 1; }
    sleep 1
  done
  # Captured, not printed: a closed ssh must not turn a finished update into a rollback.
  INFO=$("$ENTRY" connection-info)
  printf '%s\n' "$INFO" > "$BASE/swap-info"
  SWAP_ACTIVE=0
  set_status done
  printf '%s\n' "$INFO" || true
  exit 0
fi
"$ENTRY" connection-info
