set -eu
# Runs the Dev Container CLI in the OUTER sandbox. The app prepends MODE, WS, CONFIG,
# MOUNT (1 to bind-mount the git credential socket directory), MOUNT_DST and, for
# `follow`, OFFSET (log lines already delivered). `devcontainer up` consumes stdin, so
# everything runs with </dev/null, detached (setsid nohup) so an ssh drop does not kill a
# build, with its output in a log file that `follow` reads.
umask 077
DC="$HOME/.devcontainers/bin/devcontainer"
RUN="$HOME/.monoptah/run"
LOG="$RUN/up.log"
DONE="$RUN/up.exit"
PIDF="$RUN/up.pid"
MODEF="$RUN/up.mode"
LOCK="$RUN/up.lock"
mkdir -p "$RUN"
# A pid file is only trusted when that process is really our build wrapper.
running() {
  [ -f "$PIDF" ] || return 1
  PID=$(cat "$PIDF" 2>/dev/null || true)
  [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null || return 1
  case "$(ps -o args= -p "$PID" 2>/dev/null || true)" in *"$LOG"*) return 0 ;; esac
  return 1
}
case "$MODE" in
start|rebuild)
  # One launcher at a time (atomic mkdir); the lock of a dead launcher is taken over.
  if ! mkdir "$LOCK" 2>/dev/null; then
    OLD=$(cat "$LOCK/pid" 2>/dev/null || true)
    if [ -n "$OLD" ] && kill -0 "$OLD" 2>/dev/null; then
      echo 'Another start is launching the container build; retry in a moment.' >&2
      exit 1
    fi
    rm -rf "$LOCK"
    mkdir "$LOCK"
  fi
  echo $$ >"$LOCK/pid"
  trap 'rm -rf "$LOCK"' EXIT
  if running; then
    if [ "$MODE" = rebuild ] && [ "$(cat "$MODEF" 2>/dev/null || true)" != rebuild ]; then
      echo 'A container build is already running; wait for it to finish, then rebuild.' >&2
      exit 3
    fi
    echo 'MONOPTAH_UP=running'
    exit 0
  fi
  rm -f "$LOG" "$DONE"
  echo "$MODE" >"$MODEF"
  set -- "$DC" up --workspace-folder "$WS" --skip-non-blocking-commands --log-format json
  [ -z "$CONFIG" ] || set -- "$@" --config "$WS/$CONFIG"
  [ "$MOUNT" != 1 ] || set -- "$@" --mount "type=bind,source=$HOME/.monoptah/git,target=$MOUNT_DST"
  [ "$MODE" != rebuild ] || set -- "$@" --remove-existing-container
  setsid nohup sh -c 'log=$1; fin=$2; pidf=$3; shift 3; echo $$ >"$pidf"; "$@" </dev/null >"$log" 2>&1; echo $? >"$fin"' sh "$LOG" "$DONE" "$PIDF" "$@" </dev/null >/dev/null 2>&1 &
  echo $! >"$PIDF"
  echo 'MONOPTAH_UP=started'
  ;;
follow)
  # Polling instead of `tail -F`: no background process to clean up, and every log line is
  # printed with an `L ` prefix so the app can tell it from ssh noise and count it.
  [ -f "$LOG" ] || : >"$LOG"
  N=${OFFSET:-0}
  GONE=0
  while :; do
    TOTAL=$(wc -l <"$LOG" | tr -d ' ')
    if [ "$TOTAL" -gt "$N" ]; then
      sed -n "$((N + 1)),${TOTAL}p" "$LOG" | sed 's/^/L /'
      N=$TOTAL
    fi
    if [ -f "$DONE" ]; then
      TOTAL=$(wc -l <"$LOG" | tr -d ' ')
      if [ "$TOTAL" -gt "$N" ]; then
        sed -n "$((N + 1)),${TOTAL}p" "$LOG" | sed 's/^/L /'
      fi
      echo "MONOPTAH_UP_EXIT=$(cat "$DONE")"
      break
    fi
    if ! running; then
      GONE=$((GONE + 1))
      if [ "$GONE" -ge 3 ] && [ ! -f "$DONE" ]; then
        echo 'MONOPTAH_UP_EXIT=gone'
        break
      fi
    else
      GONE=0
    fi
    sleep 1
  done
  ;;
lifecycle)
  set -- "$DC" run-user-commands --workspace-folder "$WS"
  [ -z "$CONFIG" ] || set -- "$@" --config "$WS/$CONFIG"
  setsid nohup "$@" </dev/null >"$RUN/lifecycle.log" 2>&1 &
  echo 'MONOPTAH_LIFECYCLE=started'
  ;;
*)
  echo "unknown mode $MODE" >&2
  exit 2
  ;;
esac
