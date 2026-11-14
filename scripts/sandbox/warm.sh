#!/usr/bin/env bash
# Create a large Docker Sandbox, clone the repo's current main into it, install the toolchain and
# dependencies, and prebuild the target dir for the combined cargo package set. Prints the sandbox
# name on stdout (everything else goes to stderr). Cold time measured for Monoptah: about 15 to
# 30 minutes. See docs/native/sandbox-build-box.md.
#
# Usage: scripts/sandbox/warm.sh [--name NAME] [--ref REF] [--cpus N --memory SIZE] [--ttl DUR] [--repo URL]
# Every default comes from an environment variable, see lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() { sed -n '2,9p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
name=${BOX_NAME:-}
while [ $# -gt 0 ]; do
  case $1 in
    --name) name=${2:?}; shift 2 ;;
    --ref) BOX_REF=${2:?}; shift 2 ;;
    --cpus) BOX_CPUS=${2:?}; shift 2 ;;
    --memory) BOX_MEMORY=${2:?}; shift 2 ;;
    --ttl) BOX_TTL=${2:?}; shift 2 ;;
    --repo) BOX_REPO_URL=${2:?}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument $1" ;;
  esac
done
guard_no_secret_in_url "$BOX_REPO_URL"
require_sbx

if [ -z "$name" ]; then
  repo_slug=$(basename "$BOX_REPO_URL" .git | tr -c 'A-Za-z0-9\n' '-' | cut -c1-30)
  name="box-$repo_slug-$(date +%m%d%H%M%S)"
fi
valid_name "$name" || die "bad sandbox name '$name' (2-63 letters, digits, hyphens)"

existing=$(sx ls -q 2>/dev/null | grep -c . || true)
if [ "$existing" -ge "$BOX_MAX_BOXES" ]; then
  die "$existing cloud sandboxes exist already (limit BOX_MAX_BOXES=$BOX_MAX_BOXES; the account allows 4 sandboxes / 24 vCPU). Run clean.sh first."
fi

t0=$(date +%s)
record_box "$name"   # recorded before create, so clean.sh can remove a half-made sandbox too
log "creating $name ($BOX_CPUS vCPU / $BOX_MEMORY, ttl $BOX_TTL, on timeout: $BOX_ON_TIMEOUT)"
sx create --name "$name" --cpus "$BOX_CPUS" --memory "$BOX_MEMORY" --ttl "$BOX_TTL" \
  --on-timeout "$BOX_ON_TIMEOUT" --no-progress shell >&2 || die "sbx create failed; clean.sh removes anything left behind"
t1=$(date +%s)
log "created in $((t1 - t0))s; warming (clone $BOX_REPO_URL @ $BOX_REF, toolchain, deps, prebuild). This takes a while."

if ! remote_run "$name" warm; then
  log "warm failed after $(( $(date +%s) - t1 ))s. The sandbox is kept for inspection: 'sbx --cloud exec $name tail -n 50 box.log'. Remove it with scripts/sandbox/clean.sh $name"
  exit 1
fi
log "warm done in $(( $(date +%s) - t0 ))s total ($((t1 - t0))s create, $(( $(date +%s) - t1 ))s setup and build)"
sx ttl "$name" >&2 || true
log "next: scripts/sandbox/check.sh --name $name --filter 'test(/some::module/)'"
printf '%s\n' "$name"
