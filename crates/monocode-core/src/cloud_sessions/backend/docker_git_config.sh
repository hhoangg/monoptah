set -eu
# Runs INSIDE the devcontainer (as its remote user), or in the outer sandbox without one. The app prepends WANT_CRED,
# CLONE_URL and SOCKET (empty: the outer sandbox's own cache socket, for a session without
# a devcontainer, where this script runs in the outer sandbox). Points git at the credential-cache socket that is bind-mounted
# from the outer sandbox, for the exact repository URL only. Nothing secret is written.
[ "$WANT_CRED" = 1 ] || exit 0
[ -n "$SOCKET" ] || SOCKET="$HOME/.monoptah/git/sock"
if ! command -v git >/dev/null 2>&1; then
  echo '[monoptah] git is not installed in the container; skipping credential setup'
  exit 0
fi
git config --global "credential.$CLONE_URL.useHttpPath" true
git config --global "credential.$CLONE_URL.helper" "cache --socket=$SOCKET"
echo 'MONOPTAH_GIT=configured'
