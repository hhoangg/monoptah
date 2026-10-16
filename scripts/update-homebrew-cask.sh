#!/usr/bin/env bash
# Rewrite the `version "..."` and `sha256 "..."` lines of a Homebrew cask.
#
# usage: update-homebrew-cask.sh <cask-file> <version> <sha256>
#
# Prints "updated" when the file changed, "unchanged" when it already held
# these values, and "skipped, cask is newer" when the cask pins a higher
# version than the one requested (a cask is never downgraded). All three exit 0.
# Any problem exits non-zero and leaves the cask untouched. Uses awk plus a temp file instead of `sed -i`, whose
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

CURRENT=$(sed -n -E 's/^[[:space:]]*version "([^"]*)"[[:space:]]*$/\1/p' "$CASK")
if [[ "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$CURRENT" != "$VERSION" ]]; then
  HIGHEST=$(printf '%s\n%s\n' "$CURRENT" "$VERSION" | sort -V | tail -n 1)
  if [[ "$HIGHEST" == "$CURRENT" ]]; then
    echo "skipped, cask is newer ($CURRENT > $VERSION)"
    exit 0
  fi
fi

# cp -p gives the temp file the cask's mode (mktemp makes it 0600); the awk
# redirect below then truncates and rewrites it without changing that mode.
TMP=$(mktemp "${CASK}.XXXXXX")
trap 'rm -f "$TMP"' EXIT
cp -p "$CASK" "$TMP"

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

# Same directory, so the rename is atomic.
mv "$TMP" "$CASK"
trap - EXIT
echo "updated"
