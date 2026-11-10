# 0 when `$1` is the pid of a live swap worker. A pid alone proves nothing (after a restart of
# the machine a low pid is reused), so on Linux its command line must be the worker, and
# elsewhere the worker's heartbeat file must be fresh (it touches it every few seconds).
worker_alive() {
  [ -n "$1" ] || return 1
  kill -0 "$1" 2>/dev/null || return 1
  if [ -r "/proc/$1/cmdline" ]; then
    tr '\0' ' ' < "/proc/$1/cmdline" | grep -q 'swap-worker\.sh'
    return
  fi
  [ -n "$(find "$BASE/swap-heartbeat" -mmin -1 2>/dev/null)" ]
}
