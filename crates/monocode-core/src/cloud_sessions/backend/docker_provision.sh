set -eu
# Provisions the OUTER sandbox. The app prepends the variables PHASE, REPO_URL,
# REPO_DIR, BASE_REF, BRANCH and CACHE_TIMEOUT (seconds the credential stays cached). Nothing here holds a secret: the Git credential
# arrives on stdin in the `approve` phase only and is kept in memory by
# git-credential-cache, never written to disk.
umask 077
STATE="$HOME/.monoptah"
GITDIR="$STATE/git"
SOCK="$GITDIR/sock"
case "$PHASE" in
setup)
  mkdir -p "$STATE/run" "$GITDIR" "$HOME/workspaces"
  chmod 700 "$STATE" "$GITDIR"
  if ! command -v xz >/dev/null 2>&1; then
    echo '[monoptah] installing xz-utils'
    sudo -n apt-get install -y xz-utils >/dev/null 2>&1 || {
      sudo -n apt-get update >/dev/null 2>&1 || true
      sudo -n apt-get install -y xz-utils >/dev/null
    }
  fi
  if [ ! -x "$HOME/.devcontainers/bin/devcontainer" ]; then
    echo '[monoptah] installing the Dev Container CLI'
    curl --proto '=https' -fsSL --connect-timeout 15 --max-time 300 \
      https://raw.githubusercontent.com/devcontainers/cli/v0.90.0/scripts/install.sh \
      -o "$STATE/install-devcontainer.sh"
    sh "$STATE/install-devcontainer.sh" --version 0.90.0 >/dev/null
  fi
  [ -x "$HOME/.devcontainers/bin/devcontainer" ] || { echo 'The Dev Container CLI is not installed.' >&2; exit 1; }
  echo "MONOPTAH_HOME=$HOME"
  ;;
approve)
  # stdin is a `git credential approve` payload. The cache daemon keeps it in memory.
  mkdir -p "$GITDIR"
  chmod 700 "$GITDIR"
  setsid -w git -c credential.helper= \
    -c "credential.helper=cache --socket=$SOCK --timeout=$CACHE_TIMEOUT" \
    -c credential.useHttpPath=true credential approve
  echo 'MONOPTAH_CREDENTIAL=cached'
  ;;
clone)
  WS="$HOME/workspaces/$REPO_DIR"
  if [ -e "$WS" ] && [ ! -d "$WS/.git" ]; then
    echo "$WS exists but is not a Git checkout; not touching it." >&2
    exit 1
  fi
  export GIT_TERMINAL_PROMPT=0
  # One clone at a time per workspace (atomic mkdir); a lock of a dead run is taken over.
  LOCK="$STATE/run/clone-$REPO_DIR.lock"
  mkdir -p "$STATE/run"
  if ! mkdir "$LOCK" 2>/dev/null; then
    OLD=$(cat "$LOCK/pid" 2>/dev/null || true)
    if [ -n "$OLD" ] && kill -0 "$OLD" 2>/dev/null; then
      echo 'Another start is cloning this repository; wait for it and retry.' >&2
      exit 1
    fi
    rm -rf "$LOCK"
    mkdir "$LOCK"
  fi
  echo $$ >"$LOCK/pid"
  trap 'rm -rf "$LOCK"' EXIT
  if [ ! -d "$WS/.git" ]; then
    echo '[monoptah] cloning the repository'
    # Only our own half-finished clone is ever removed, never a workspace.
    rm -rf "$WS.partial"
    if [ -S "$SOCK" ]; then
      git -c credential.helper= -c "credential.helper=cache --socket=$SOCK" \
        -c credential.useHttpPath=true clone --quiet "$REPO_URL" "$WS.partial"
    else
      git clone --quiet "$REPO_URL" "$WS.partial"
    fi
    # Recheck right before the move: never put a clone into or over something that appeared.
    if [ -e "$WS" ]; then
      echo "$WS appeared while cloning; not touching it." >&2
      exit 1
    fi
    mv "$WS.partial" "$WS"
  fi
  cd "$WS"
  if git rev-parse --verify --quiet "refs/heads/$BRANCH" >/dev/null; then
    git checkout --quiet "$BRANCH"
  elif [ -n "$BASE_REF" ] && [ "$BASE_REF" = "$BRANCH" ]; then
    # feature == base (a session moved to the cloud): the branch was pushed to origin, so
    # check out a local branch that tracks it instead of branching from it under a new name.
    # The workspace may be an older clone that never saw the push: fetch the branch first.
    echo "[monoptah] checking out $BRANCH"
    if [ -S "$SOCK" ]; then
      git -c credential.helper= -c "credential.helper=cache --socket=$SOCK" \
        -c credential.useHttpPath=true fetch --quiet origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" || true
    else
      git fetch --quiet origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" || true
    fi
    if ! git rev-parse --verify --quiet "refs/remotes/origin/$BRANCH^{commit}" >/dev/null; then
      echo "The branch $BRANCH does not exist in the repository." >&2
      exit 1
    fi
    git checkout --quiet --track -b "$BRANCH" "origin/$BRANCH"
  else
    START=HEAD
    if [ -n "$BASE_REF" ]; then
      if git rev-parse --verify --quiet "refs/remotes/origin/$BASE_REF^{commit}" >/dev/null; then
        START="origin/$BASE_REF"
      elif git rev-parse --verify --quiet "$BASE_REF^{commit}" >/dev/null; then
        START="$BASE_REF"
      else
        echo "The base ref $BASE_REF does not exist in the repository." >&2
        exit 1
      fi
    fi
    echo "[monoptah] creating branch $BRANCH"
    git checkout --quiet --no-track -b "$BRANCH" "$START"
  fi
  echo "MONOPTAH_WORKSPACE=$WS"
  ;;
*)
  echo "unknown phase $PHASE" >&2
  exit 2
  ;;
esac
