#!/usr/bin/env bash
# Bring a warm sandbox up to the current main of the repo: fetch, check out origin/REF, rebuild what
# changed (clippy and test binaries, same combined package set), pick up a changed toolchain pin.
# A warm sandbox that is only a few commits behind refreshes in a few minutes.
#
# Usage: scripts/sandbox/refresh.sh [--name NAME] [--ref REF]
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() { sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
name=${BOX_NAME:-}
while [ $# -gt 0 ]; do
  case $1 in
    --name) name=${2:?}; shift 2 ;;
    --ref) BOX_REF=${2:?}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument $1" ;;
  esac
done
require_sbx
name=$(resolve_box "$name")

t0=$(date +%s)
log "refreshing $name to origin/$BOX_REF"
remote_run "$name" refresh || die "refresh failed in $name (see the log above)"
log "refreshed in $(( $(date +%s) - t0 ))s"
sx ttl "$name" >&2 || true
printf '%s\n' "$name"
