#!/usr/bin/env bash
# Remove sandboxes with `sbx --cloud rm --force` (a stop first is slow: a large sandbox needs 7 to
# 9 minutes to reach "stopped", rm --force takes about 2 seconds) and verify with `sbx --cloud ls`.
# Without arguments it removes every sandbox the scripts in this directory created and recorded.
#
# Usage: scripts/sandbox/clean.sh [NAME ...]
# Exit status is 1 when a removed sandbox still shows up in the listing.
set -uo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

case ${1:-} in -h|--help) sed -n '2,7p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;; esac
require_sbx
names=("$@")
if [ ${#names[@]} -eq 0 ] && [ -s "$(box_state_file)" ]; then
  while IFS= read -r n; do [ -n "$n" ] && names+=("$n"); done < "$(box_state_file)"
fi
if [ ${#names[@]} -eq 0 ]; then log "nothing recorded to remove"; fi

for n in ${names[@]+"${names[@]}"}; do
  valid_name "$n" || { log "skipping '$n': not a valid sandbox name"; continue; }
  if sx rm --force "$n" >&2; then log "removed $n"; else log "rm --force $n failed (already gone?)"; fi
  forget_box "$n"
done

log "sbx --cloud ls:"
listing=$(sx ls 2>&1) || { printf '%s\n' "$listing" >&2; die "could not list sandboxes"; }
printf '%s\n' "$listing"
left=0
for n in ${names[@]+"${names[@]}"}; do
  if sx ls -q 2>/dev/null | grep -qxF "$n"; then log "STILL LISTED: $n (a large sandbox can take a minute; run clean.sh $n again)"; left=1; fi
done
[ "$left" = 0 ] && log "verified: none of the removed sandboxes is listed"
exit "$left"
