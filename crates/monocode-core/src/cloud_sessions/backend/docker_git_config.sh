set -eu
# Runs INSIDE the devcontainer (as its remote user), or in the outer sandbox when the
# session has no devcontainer. The app prepends WANT_CRED, CLONE_URL, GIT_HOST, REPO_PATH
# and SOCKET (empty: the outer sandbox's own cache socket, for a session without a
# devcontainer, where this script runs in the outer sandbox).
#
# With a bound credential it (1) points git at the credential-cache socket that is
# bind-mounted from the outer sandbox, for the exact repository URL only, (2) for GitHub
# installs a `gh` wrapper in ~/.local/bin that reads the token from that cache on every
# call, and (3) writes a short note for the cloud agent in ~/.claude/rules. Without a
# credential it removes the wrapper and the note, but only if they carry our marker.
# No secret is ever written: the wrapper holds the socket path, host and repo path only.
GH_MARK='# monoptah-gh-wrapper v1'
NOTE_MARK='<!-- monoptah-cloud-git v1 -->'
GH_FILE="$HOME/.local/bin/gh"
NOTE_FILE="$HOME/.claude/rules/monoptah-cloud-git.md"

# ours FILE MARKER: a regular file (not a symlink) that carries our marker near the top.
ours() {
  [ -f "$1" ] && [ ! -L "$1" ] && head -n 3 "$1" 2>/dev/null | grep -qF -- "$2"
}
# quote VALUE: single-quote a value for a shell script.
quote() {
  printf "'"
  printf '%s' "$1" | sed "s/'/'\\\\''/g"
  printf "'"
}

# remove_gh / remove_note: delete our own file only. Best effort: a failure is reported by
# the caller and never fails the start (every command is guarded, because `set -e` is off
# inside a function that is called in a `||` context).
remove_gh() {
  if ours "$GH_FILE" "$GH_MARK"; then
    rm -f "$GH_FILE" || return 1
    echo 'MONOPTAH_GH=removed'
  fi
}
remove_note() {
  if ours "$NOTE_FILE" "$NOTE_MARK"; then
    rm -f "$NOTE_FILE" || return 1
    echo 'MONOPTAH_NOTE=removed'
  fi
}

if [ "$WANT_CRED" != 1 ]; then
  remove_gh || echo '[monoptah] could not remove the gh wrapper; skipping'
  remove_note || echo '[monoptah] could not remove the agent note; skipping'
  exit 0
fi
[ -n "$SOCKET" ] || SOCKET="$HOME/.monoptah/git/sock"
if ! command -v git >/dev/null 2>&1; then
  echo '[monoptah] git is not installed here; skipping credential setup'
  exit 0
fi
git config --global "credential.$CLONE_URL.useHttpPath" true
git config --global "credential.$CLONE_URL.helper" "cache --socket=$SOCKET"
echo 'MONOPTAH_GIT=configured'

# install_gh: write the wrapper atomically. Best effort, see remove_gh.
install_gh() {
  if [ -e "$GH_FILE" ] || [ -L "$GH_FILE" ]; then
    if ! ours "$GH_FILE" "$GH_MARK"; then
      echo '[monoptah] ~/.local/bin/gh is not ours; leaving it'
      return 0
    fi
  fi
  mkdir -p "$HOME/.local/bin" || return 1
  tmp="$HOME/.local/bin/.gh.monoptah.$$"
  {
    printf '#!/bin/sh\n%s\n' "$GH_MARK"
    printf 'SOCK=%s\nGIT_HOST=%s\nREPO_PATH=%s\n' \
      "$(quote "$SOCKET")" "$(quote "$GIT_HOST")" "$(quote "$REPO_PATH")"
    cat <<'WRAPPER'
# Hands the Monoptah Git token for this repository to the real gh, for one call only.
# The token is read from the in-memory credential cache every time and never stored.
case "${1:-}" in
auth)
  case "${2:-}" in
  token)
    echo 'gh auth token is disabled in this sandbox: the token stays inside Monoptah.' >&2
    exit 2
    ;;
  git-credential)
    echo 'gh auth git-credential is disabled in this sandbox: it would print the token.' >&2
    exit 2
    ;;
  login | setup-git)
    echo "gh auth $2 is disabled: this sandbox is already authenticated through Monoptah." >&2
    exit 2
    ;;
  status)
    for arg in "$@"; do
      case "$arg" in
      --show-token | --show-token=* | -t)
        echo 'gh auth status --show-token is disabled in this sandbox.' >&2
        exit 2
        ;;
      -[!-]*)
        # A bundle made only of the boolean flags (-a, -t), such as -at. Anything with a
        # value (-hgithub.com) is not a bundle.
        flags=${arg#-}
        case "$flags" in
        *[!at]*) ;;
        *t*)
          echo 'gh auth status --show-token is disabled in this sandbox.' >&2
          exit 2
          ;;
        esac
        ;;
      esac
    done
    ;;
  esac
  ;;
esac

real=
oldifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || dir=.
  candidate="$dir/gh"
  [ -f "$candidate" ] && [ -x "$candidate" ] || continue
  [ "$candidate" -ef "$0" ] && continue
  # Another copy of this wrapper (A would exec B would exec A).
  if head -n 3 "$candidate" 2>/dev/null | grep -qF -- '# monoptah-gh-wrapper'; then
    continue
  fi
  real=$candidate
  break
done
IFS=$oldifs
if [ -z "$real" ]; then
  echo 'The GitHub CLI (gh) is not installed in this sandbox.' >&2
  exit 127
fi

if [ -n "${GH_TOKEN:-}" ] || [ -n "${GITHUB_TOKEN:-}" ]; then
  exec "$real" "$@"
fi

reply=
if command -v git >/dev/null 2>&1; then
  reply=$(printf 'protocol=https\nhost=%s\npath=%s\n\n' "$GIT_HOST" "$REPO_PATH" |
    git credential-cache --socket "$SOCK" get 2>/dev/null) || reply=
fi
token=$(printf '%s\n' "$reply" | sed -n 's/^password=//p' | head -n 1)
reply=
if [ -n "$token" ]; then
  GH_TOKEN=$token
  export GH_TOKEN
  token=
else
  echo "Monoptah: no Git credential for $GIT_HOST/$REPO_PATH is available to gh here (it expires after 24 h; reconnecting the session delivers it again, unless the credential socket is not readable by this user)." >&2
fi
exec "$real" "$@"
WRAPPER
  } >"$tmp" || { rm -f "$tmp"; return 1; }
  chmod 0755 "$tmp" || { rm -f "$tmp"; return 1; }
  mv -f "$tmp" "$GH_FILE" || { rm -f "$tmp"; return 1; }
  echo 'MONOPTAH_GH=wrapped'
}

# install_note: write the agent note atomically. Best effort, see remove_gh.
install_note() {
  if [ -e "$NOTE_FILE" ] || [ -L "$NOTE_FILE" ]; then
    if ! ours "$NOTE_FILE" "$NOTE_MARK"; then
      echo '[monoptah] ~/.claude/rules/monoptah-cloud-git.md is not ours; leaving it'
      return 0
    fi
  fi
  mkdir -p "$HOME/.claude/rules" || return 1
  tmp="$HOME/.claude/rules/.monoptah-cloud-git.$$"
  {
    printf '%s\n\n' "$NOTE_MARK"
    printf 'Git credentials in this sandbox: HTTPS access to %s is preconfigured through a git credential helper, so `git fetch`, `git pull` and `git push` to that remote work without prompts.' "$CLONE_URL"
    if [ "$GIT_HOST" = github.com ]; then
      printf ' The `gh` CLI is wired to the same token for each call (if `command -v gh` does not point to `~/.local/bin/gh`, run it as `~/.local/bin/gh`), so use `gh pr create`, `gh pr comment`, `gh pr review` (or `gh api`) for pull request work. Do not run `gh auth login`, do not change the remote URL, and never print, echo or write the token (no `gh auth token`, no `gh auth status -t`, no `git credential fill` output). What the token may do depends on its own permissions, and it may lack write or pull request access: if a push or API call is denied, report the error text instead of trying other ways to authenticate.\n'
    else
      printf ' There is no GitLab CLI token in this sandbox: do merge request work through the user, not `glab`. Never print, echo or write the token. What the token may do depends on its own permissions, and it may lack write access: if a push is denied, report the error text instead of trying other ways to authenticate.\n'
    fi
  } >"$tmp" || { rm -f "$tmp"; return 1; }
  chmod 0644 "$tmp" || { rm -f "$tmp"; return 1; }
  mv -f "$tmp" "$NOTE_FILE" || { rm -f "$tmp"; return 1; }
  echo 'MONOPTAH_NOTE=written'
}

if [ "$GIT_HOST" = github.com ]; then
  install_gh || echo '[monoptah] could not install the gh wrapper; skipping'
else
  # The sandbox may have been adopted for another repository.
  remove_gh || echo '[monoptah] could not remove the gh wrapper; skipping'
fi
install_note || echo '[monoptah] could not write the agent note; skipping'
