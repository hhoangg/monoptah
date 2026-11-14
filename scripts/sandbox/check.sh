#!/usr/bin/env bash
# One edit-loop pass in a warm sandbox, from the repo you are standing in:
#   1. diff your working tree (committed, staged, unstaged and new files) against the base commit,
#   2. copy the patch into the sandbox, check out the base there, `git apply --index` the patch,
#   3. cargo fmt --check, clippy -D warnings, then the tests ONLY if clippy passed,
#   4. copy any rustfmt diff back and print the step times.
# Run `cargo fmt` locally before the first pass; rustfmt reorders things and a fmt diff costs a loop.
#
# Usage: scripts/sandbox/check.sh [--name NAME] [--filter EXPR | --full] [--base REF] [--apply-fmt]
#                                 [--no-untracked]
#   --filter EXPR  nextest filter expression, e.g. 'test(/accounts::identity/)'. Without --filter
#                  and --full only fmt and clippy run.
#   --full         the whole suite: node tests, fmt, clippy, every nextest test.
#   --base REF     what the diff is taken against. Without it (and without BOX_BASE): <remote>/main of
#                  the remote whose URL is BOX_REPO_URL, else origin/main. The chosen base is printed.
#   --apply-fmt    apply a returned rustfmt diff to your working tree (default: only save it).
#   --no-untracked leave untracked files out of the patch (BOX_NO_UNTRACKED=1 does the same).
# Untracked, non-ignored files are uploaded and listed first. A secret-looking name (.env*, *.pem,
# *.key, id_rsa*, credentials*, *.p12, *.token, ...) stops the run unless BOX_UPLOAD_UNTRACKED_SECRETS=1.
# Exit status is 0 only when every step passed.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
name=${BOX_NAME:-}; base=${BOX_BASE:-}; filter=''; full=0; apply_fmt=0; no_untracked=${BOX_NO_UNTRACKED:-0}
while [ $# -gt 0 ]; do
  case $1 in
    --name) name=${2:?}; shift 2 ;;
    --filter) filter=${2:?}; shift 2 ;;
    --full) full=1; shift ;;
    --base) base=${2:?}; shift 2 ;;
    --apply-fmt) apply_fmt=1; shift ;;
    --no-untracked) no_untracked=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument $1" ;;
  esac
done
[ "$full" = 0 ] || [ -z "$filter" ] || die "--filter and --full exclude each other"
require_sbx
name=$(resolve_box "$name")
git rev-parse --show-toplevel >/dev/null 2>&1 || die "run this inside the git repository you are checking"
top=$(git rev-parse --show-toplevel)
cd "$top"

# Base: an explicit --base / BOX_BASE, else <remote>/main of the remote that points at the repo the
# box clones (remote names differ per checkout), else origin/main.
base_why='--base or BOX_BASE'
if [ -z "$base" ]; then
  remote=$(matching_remote "$BOX_REPO_URL" || true)
  if [ -n "$remote" ] && git rev-parse --verify -q "$remote/main^{commit}" >/dev/null; then
    base=$remote/main; base_why="remote '$remote' is $BOX_REPO_URL"
  else
    [ -z "$remote" ] || log "remote '$remote' has no main locally (git fetch $remote); falling back to origin/main"
    git rev-parse --verify -q "origin/main^{commit}" >/dev/null || die "no base ref: pass --base REF or set BOX_BASE (no remote matches $BOX_REPO_URL and there is no origin/main)"
    base=origin/main; base_why="no remote matches $BOX_REPO_URL, using the origin/main fallback"
  fi
fi
base_sha=$(git merge-base HEAD "$base") || die "no merge base between HEAD and $base"

# The patch holds committed and uncommitted work, including new files, taken through a throwaway
# index so your real index is not touched. --binary keeps binary files intact.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Untracked, non-ignored files go into the patch too, so name them first and stop on secrets.
git ls-files --others --exclude-standard -z > "$work/untracked.z"
n_untracked=$(tr -cd '\0' < "$work/untracked.z" | wc -c | tr -d ' ')
if [ "$n_untracked" -gt 0 ]; then
  if [ "$no_untracked" = 1 ]; then
    log "$n_untracked untracked file(s) left out (--no-untracked)"
  else
    log "uploading $n_untracked untracked, non-ignored file(s) (skip them with --no-untracked, or add them to .gitignore):"
    shown=0; secrets=''
    while IFS= read -r -d '' f; do
      shown=$((shown + 1))
      [ "$shown" -gt 40 ] || printf '  %s\n' "$f" >&2
      if is_secret_name "$f"; then secrets="$secrets $f"; fi
    done < "$work/untracked.z"
    [ "$shown" -le 40 ] || printf '  ... and %s more\n' "$((shown - 40))" >&2
    if [ -n "$secrets" ]; then
      printf 'WARNING: secret-looking untracked file(s) would be uploaded to the cloud box:%s\n' "$secrets" >&2
      [ "${BOX_UPLOAD_UNTRACKED_SECRETS:-0}" = 1 ] || die "refusing to upload them. Add them to .gitignore, rerun with --no-untracked, or set BOX_UPLOAD_UNTRACKED_SECRETS=1 if you really mean it"
      log "BOX_UPLOAD_UNTRACKED_SECRETS=1: uploading them anyway"
    fi
  fi
fi
GIT_INDEX_FILE="$work/index" git read-tree "$base_sha"
GIT_INDEX_FILE="$work/index" git add -A
if [ "$no_untracked" = 1 ] && [ "$n_untracked" -gt 0 ]; then
  xargs -0 env GIT_INDEX_FILE="$work/index" git rm --cached -q --ignore-unmatch -- < "$work/untracked.z"
fi
GIT_INDEX_FILE="$work/index" git diff --cached --binary "$base_sha" > "$work/check.patch"
[ -s "$work/check.patch" ] || die "no difference from $base ($base_sha): nothing to check"
files=$(grep -c '^diff --git' "$work/check.patch" || true)
log "base ${base_sha:0:9} ($base: $base_why), patch: $files file(s), $(wc -c < "$work/check.patch" | tr -d ' ') bytes"

t0=$(date +%s)
sx cp "$work/check.patch" "$name:$BOX_HOME/check.patch" >/dev/null || die "could not copy the patch into $name (is it running? sbx --cloud ls)"
export BOX_BASE_SHA=$base_sha BOX_FILTER=$filter BOX_FULL=$full
# shellcheck disable=SC2034  # read by remote_run in lib.sh
BOX_EXTRA_ENV="BOX_BASE_SHA BOX_FILTER BOX_FULL"
rc=0
remote_run "$name" check "$BOX_HOME/check.patch" || rc=$?

if sx exec "$name" test -s "$BOX_HOME/box-fmt.patch" 2>/dev/null; then
  mkdir -p "$BOX_STATE_DIR"
  fmt_out="$BOX_STATE_DIR/fmt.patch"   # outside the repo, so it cannot be committed by accident
  sx cp "$name:$BOX_HOME/box-fmt.patch" "$fmt_out" >/dev/null
  log "rustfmt diff copied back to $fmt_out"
  if [ "$apply_fmt" = 1 ]; then
    if git apply "$fmt_out"; then log "applied the rustfmt diff"; rm -f "$fmt_out"; else log "the diff did not apply cleanly; apply it by hand"; fi
  else
    log "apply it with: git apply $fmt_out   (or rerun with --apply-fmt)"
  fi
fi

log "check finished in $(( $(date +%s) - t0 ))s, exit $rc"
exit "$rc"
