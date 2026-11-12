# Sandbox only: a devcontainer often has /run/systemd/system without systemd as PID 1, which
# makes `service install` choose the systemd path and fail. Keep the Host out of that path:
# start it detached exactly like host/service.ts startDetached (same data dir, port and log),
# so `service install` finds a Host answering and returns early.
# Only the start is needed: the template's restart lines (`service uninstall`) already stop a
# detached Host (the CLI runs a lifecycle stop and waits for running.json to vanish; the
# systemctl calls are ignored on a fake-systemd container), and this start brings it back with
# the new environment.
MONOCODE_HOST_DETACHED=1
host_answering() {
  "$ENTRY" connection-info >/dev/null 2>&1
}
start_detached_host() {
  if ! command -v setsid >/dev/null 2>&1 && ! command -v nohup >/dev/null 2>&1; then
    echo 'Neither setsid nor nohup is available, so the Monoptah Host can not be started detached.' | tee -a "$BASE/host.log" >&2
    exit 1
  fi
  (
    PATH="$HOME/.local/bin:$PATH:/usr/local/bin:/usr/bin:/bin"
    export PATH
    if command -v setsid >/dev/null 2>&1; then
      setsid "$ENTRY" serve --data-dir "$BASE" --port "$HOST_PORT" </dev/null >>"$BASE/host.log" 2>&1 &
    else
      nohup "$ENTRY" serve --data-dir "$BASE" --port "$HOST_PORT" </dev/null >>"$BASE/host.log" 2>&1 &
    fi
  )
}
ensure_detached_host() {
  [ "${MONOCODE_HOST_DETACHED:-0}" = 1 ] || return 0
  host_answering && return 0
  start_detached_host
  DETACHED_WAIT=0
  while [ "$DETACHED_WAIT" -lt 30 ]; do
    host_answering && return 0
    sleep 1
    DETACHED_WAIT=$((DETACHED_WAIT + 1))
  done
  echo "The Monoptah Host did not start. Check $BASE/host.log." >&2
  tail -n 20 "$BASE/host.log" >&2 2>/dev/null || true
  exit 1
}
# Runs before every `service install` of a restored Host too (the upgrade rollback calls it).
before_service_install() {
  ensure_detached_host
}
