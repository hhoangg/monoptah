#!/usr/bin/env bash
# Rewrite the `version "..."` and `sha256 "..."` lines of a Homebrew cask.
#
# usage: update-homebrew-cask.sh <cask-file> <version> <sha256>
#
# Prints "updated" when the file changed and "unchanged" when it already held
# these values (exit 0 in both cases). Any problem exits non-zero and leaves
# the cask untouched. Uses awk plus a temp file instead of `sed -i`, whose
# in-place flag differs between BSD (macOS) and GNU (Linux) sed.
set -euo pipefail

if (( $# != 3 )); then
  echo "usage: $0 <cask-file> <version> <sha256>" >&2
  exit 2
fi

CASK="$1"
VERSION="$2"
SHA="$3"

if [[ ! -f "$CASK" ]]; then
  echo "Cask file not found: $CASK" >&2
  exit 1
fi
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version must look like X.Y.Z, got: $VERSION" >&2
  exit 1
fi
if [[ ! "$SHA" =~ ^[0-9a-f]{64}$ ]]; then
  echo "sha256 must be 64 lowercase hex characters, got: $SHA" >&2
  exit 1
fi

# Counts lines of the form `  <key> "<value>"` (the `version` inside the url
# interpolation does not start a line, so it is not counted).
count_lines() {
  local n
  n=$(grep -c -E "^[[:space:]]*$1 \"[^\"]*\"[[:space:]]*$" "$CASK" || true)
  printf '%s\n' "$n"
}

for key in version sha256; do
  n=$(count_lines "$key")
  if [[ "$n" != "1" ]]; then
    echo "Expected exactly one '$key \"...\"' line in $CASK, found $n" >&2
    exit 1
  fi
done

TMP=$(mktemp "${CASK}.XXXXXX")
trap 'rm -f "$TMP"' EXIT

awk -v version="$VERSION" -v sha="$SHA" '
  /^[[:space:]]*version "[^"]*"[[:space:]]*$/ {
    match($0, /^[[:space:]]*/)
    print substr($0, 1, RLENGTH) "version \"" version "\""
    next
  }
  /^[[:space:]]*sha256 "[^"]*"[[:space:]]*$/ {
    match($0, /^[[:space:]]*/)
    print substr($0, 1, RLENGTH) "sha256 \"" sha "\""
    next
  }
  { print }
' "$CASK" > "$TMP"

if cmp -s "$CASK" "$TMP"; then
  echo "unchanged"
  exit 0
fi

# Keep the original file mode, then move into place (same directory, so the
# rename is atomic).
chmod --reference="$CASK" "$TMP" 2>/dev/null || chmod "$(stat -f '%Lp' "$CASK" 2>/dev/null || echo 644)" "$TMP"
mv "$TMP" "$CASK"
trap - EXIT
echo "updated"
