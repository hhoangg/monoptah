set -eu
# Runs INSIDE the devcontainer (as its remote user). The app prepends WANT_CRED,
# CLONE_URL and SOCKET. Points git at the credential-cache socket that is bind-mounted
# from the outer sandbox, for the exact repository URL only. Nothing secret is written.
[ "$WANT_CRED" = 1 ] || exit 0
if ! command -v git >/dev/null 2>&1; then
  echo '[monoptah] git is not installed in the container; skipping credential setup'
  exit 0
fi
git config --global "credential.$CLONE_URL.useHttpPath" true
git config --global "credential.$CLONE_URL.helper" "cache --socket=$SOCKET"
echo 'MONOPTAH_GIT=configured'
