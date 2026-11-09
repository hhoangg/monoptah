set -eu
# Starts the relay in the OUTER sandbox: a loopback listener that bridges every
# connection into the Host inside the devcontainer, so `ssh -L` (which can only reach
# the outer sandbox's loopback) can get to a 127.0.0.1-only Host. The inner end is the
# Host's own packaged Node, so it works in any image. The app prepends CID, RUN_USER,
# PORT and HOST_PORT.
umask 077
RUN="$HOME/.monoptah/run"
mkdir -p "$RUN"
PIDF="$RUN/relay-$PORT.pid"
CIDF="$RUN/relay-$PORT.cid"
INNER="$RUN/relay-$PORT-inner.sh"
BRIDGE="$RUN/relay-bridge.js"
# A pid file is only trusted when that process is really our relay.
alive() {
  [ -f "$PIDF" ] || return 1
  PID=$(cat "$PIDF" 2>/dev/null || true)
  [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null || return 1
  case "$(ps -o args= -p "$PID" 2>/dev/null || true)" in *"$INNER"*) return 0 ;; esac
  return 1
}
# The relay depends on both the container and the Host port it bridges to.
KEY="$CID:$HOST_PORT"
if alive && [ "$(cat "$CIDF" 2>/dev/null || true)" = "$KEY" ]; then
  echo 'MONOPTAH_RELAY=running'
  exit 0
fi
# A relay of an older container (rebuild) must go.
if alive; then kill "$(cat "$PIDF")" 2>/dev/null || true; fi
rm -f "$PIDF" "$CIDF"
if ! command -v socat >/dev/null 2>&1; then
  sudo -n apt-get install -y socat >/dev/null
fi
cat >"$BRIDGE" <<'JS'
const net = require("net");
const socket = net.connect(Number(process.env.MONOPTAH_HOST_PORT), "127.0.0.1");
process.stdin.pipe(socket);
socket.pipe(process.stdout);
socket.on("error", () => process.exit(1));
socket.on("close", () => process.exit(0));
process.stdin.on("end", () => socket.end());
JS
cat >"$INNER" <<SH
#!/bin/sh
exec docker exec -i -u $RUN_USER -e MONOPTAH_HOST_PORT=$HOST_PORT $CID sh -c 'NODE="\$(cat "\$HOME/.monocode-host/runtime-path")/bin/node"; exec "\$NODE" -e "\$1"' sh "\$(cat "$BRIDGE")"
SH
chmod 700 "$INNER"
setsid nohup socat "TCP-LISTEN:$PORT,bind=127.0.0.1,reuseaddr,fork" "EXEC:sh $INNER" </dev/null >"$RUN/relay-$PORT.log" 2>&1 &
echo $! >"$PIDF"
echo "$KEY" >"$CIDF"
sleep 1
if ! alive; then
  cat "$RUN/relay-$PORT.log" >&2 || true
  echo "The relay on port $PORT did not start." >&2
  exit 1
fi
echo 'MONOPTAH_RELAY=started'
