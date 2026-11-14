#!/usr/bin/env bash
# One edit-loop pass in a warm sandbox, from the repo you are standing in:
#   1. diff your working tree (committed, staged, unstaged and new files) against the base commit,
#   2. copy the patch into the sandbox, check out the base there, `git apply --index` the patch,
#   3. cargo fmt --check, clippy -D warnings, then the tests ONLY if clippy passed,
#   4. copy any rustfmt diff back and print the step times.
# Run `cargo fmt` locally before the first pass; rustfmt reorders things and a fmt diff costs a loop.
#
# Usage: scripts/sandbox/check.sh [--name NAME] [--filter EXPR | --full] [--base REF] [--apply-fmt]
#   --filter EXPR  nextest filter expression, e.g. 'test(/accounts::identity/)'. Without --filter
#                  and --full only fmt and clippy run.
#   --full         the whole suite: node tests, fmt, clippy, every nextest test.
#   --base REF     what the diff is taken against (default: upstream/main, else origin/main).
#   --apply-fmt    apply a returned rustfmt diff to your working tree (default: only save it).
# Exit status is 0 only when every step passed.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() { sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
name=${BOX_NAME:-}; base=${BOX_BASE:-}; filter=''; full=0; apply_fmt=0
while [ $# -gt 0 ]; do
  case $1 in
    --name) name=${2:?}; shift 2 ;;
    --filter) filter=${2:?}; shift 2 ;;
    --full) full=1; shift ;;
    --base) base=${2:?}; shift 2 ;;
    --apply-fmt) apply_fmt=1; shift ;;
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

if [ -z "$base" ]; then
  for cand in upstream/main origin/main; do
    git rev-parse --verify -q "$cand^{commit}" >/dev/null && { base=$cand; break; }
  done
fi
[ -n "$base" ] || die "no base ref: pass --base REF"
base_sha=$(git merge-base HEAD "$base") || die "no merge base between HEAD and $base"

# The patch holds committed and uncommitted work, including new files, taken through a throwaway
# index so your real index is not touched. --binary keeps binary files intact.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
GIT_INDEX_FILE="$work/index" git read-tree "$base_sha"
GIT_INDEX_FILE="$work/index" git add -A
GIT_INDEX_FILE="$work/index" git diff --cached --binary "$base_sha" > "$work/check.patch"
[ -s "$work/check.patch" ] || die "no difference from $base ($base_sha): nothing to check"
files=$(grep -c '^diff --git' "$work/check.patch" || true)
log "base ${base_sha:0:9} ($base), patch: $files file(s), $(wc -c < "$work/check.patch" | tr -d ' ') bytes"

t0=$(date +%s)
sx cp "$work/check.patch" "$name:$BOX_HOME/check.patch" >/dev/null || die "could not copy the patch into $name (is it running? sbx --cloud ls)"
export BOX_BASE_SHA=$base_sha BOX_FILTER=$filter BOX_FULL=$full
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
