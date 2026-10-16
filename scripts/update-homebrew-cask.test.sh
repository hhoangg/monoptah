#!/usr/bin/env bash
# Plain-bash tests for update-homebrew-cask.sh. Run: bash scripts/update-homebrew-cask.test.sh
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/update-homebrew-cask.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

OLD_SHA=dbfe884b2d78377e267cb468ce77a49010777e79e1d5bbaea9713502d5727616
NEW_SHA=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef

write_fixture() {
  cat > "$1" <<'CASK'
cask "monoptah" do
  version "0.23.0"
  sha256 "dbfe884b2d78377e267cb468ce77a49010777e79e1d5bbaea9713502d5727616"

  url "https://github.com/hhoangg/monoptah/releases/download/v#{version}/Monoptah_#{version}_aarch64.zip"
  name "Monoptah"
  desc "One UI for every agent harness"
  homepage "https://github.com/hhoangg/monoptah"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on :macos

  app "Monoptah.app"

  caveats <<~EOS
    Afterwards it launches normally, and the app updates itself from
    #{homepage}/releases.
  EOS
end
CASK
}

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "ok   - $1"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL - $1"; }

# usage: expect_fail <name> <cask> <version> <sha>
# The script must exit non-zero and leave the cask byte-for-byte unchanged.
expect_fail() {
  local name="$1" cask="$2" before
  before=$(cat "$cask")
  if bash "$SCRIPT" "$cask" "$3" "$4" > "$WORK/out" 2>&1; then
    bad "$name (expected failure, got success)"
  elif [[ "$(cat "$cask")" != "$before" ]]; then
    bad "$name (file was modified)"
  else
    ok "$name"
  fi
}

CASK="$WORK/monoptah.rb"
write_fixture "$CASK"

# 1. Update succeeds and rewrites only the two lines.
out=$(bash "$SCRIPT" "$CASK" 0.24.0 "$NEW_SHA" 2>&1); rc=$?
if (( rc == 0 )) && [[ "$out" == "updated" ]] \
  && grep -qx '  version "0.24.0"' "$CASK" \
  && grep -qx "  sha256 \"$NEW_SHA\"" "$CASK" \
  && ! grep -q "$OLD_SHA" "$CASK" \
  && [[ "$(diff <(write_fixture "$WORK/orig.rb"; cat "$WORK/orig.rb") "$CASK" | grep -c '^>')" == "2" ]] \
  && grep -qF 'download/v#{version}/Monoptah_#{version}_aarch64.zip' "$CASK"; then
  ok "update rewrites version and sha256 only"
else
  bad "update rewrites version and sha256 only (rc=$rc out=$out)"
fi

# 2. Running again reports no change and does not touch the file.
cp "$CASK" "$WORK/before.rb"
out=$(bash "$SCRIPT" "$CASK" 0.24.0 "$NEW_SHA" 2>&1); rc=$?
if (( rc == 0 )) && [[ "$out" == "unchanged" ]] && cmp -s "$CASK" "$WORK/before.rb"; then
  ok "second run reports unchanged"
else
  bad "second run reports unchanged (rc=$rc out=$out)"
fi

# 2b. An older requested version is skipped and leaves the file untouched.
cp "$CASK" "$WORK/before.rb"
out=$(bash "$SCRIPT" "$CASK" 0.23.9 "$OLD_SHA" 2>&1); rc=$?
if (( rc == 0 )) && [[ "$out" == skipped,\ cask\ is\ newer* ]] && cmp -s "$CASK" "$WORK/before.rb"; then
  ok "older version is skipped and file is unchanged"
else
  bad "older version is skipped and file is unchanged (rc=$rc out=$out)"
fi

# 2c. Version ordering is numeric, not lexical (0.9.5 is older than 0.24.0).
out=$(bash "$SCRIPT" "$CASK" 0.9.5 "$OLD_SHA" 2>&1); rc=$?
if (( rc == 0 )) && [[ "$out" == skipped,\ cask\ is\ newer* ]] && cmp -s "$CASK" "$WORK/before.rb"; then
  ok "0.9.5 is treated as older than 0.24.0"
else
  bad "0.9.5 is treated as older than 0.24.0 (rc=$rc out=$out)"
fi

# 2d. Equal version with the same sha reports no change.
out=$(bash "$SCRIPT" "$CASK" 0.24.0 "$NEW_SHA" 2>&1); rc=$?
if (( rc == 0 )) && [[ "$out" == "unchanged" ]] && cmp -s "$CASK" "$WORK/before.rb"; then
  ok "equal version reports unchanged"
else
  bad "equal version reports unchanged (rc=$rc out=$out)"
fi

# 2e. A newer version after a skip still updates, and the file mode is kept.
chmod 644 "$CASK"
out=$(bash "$SCRIPT" "$CASK" 0.25.0 "$OLD_SHA" 2>&1); rc=$?
mode=$(ls -l "$CASK" | cut -c1-10)
if (( rc == 0 )) && [[ "$out" == "updated" ]] && grep -qx '  version "0.25.0"' "$CASK" && [[ "$mode" == "-rw-r--r--" ]]; then
  ok "newer version updates and keeps file mode"
else
  bad "newer version updates and keeps file mode (rc=$rc out=$out mode=$mode)"
fi

# 3. Missing lines fail.
write_fixture "$WORK/missing-version.rb"
grep -v '^  version ' "$WORK/missing-version.rb" > "$WORK/mv.tmp" && mv "$WORK/mv.tmp" "$WORK/missing-version.rb"
expect_fail "missing version line fails" "$WORK/missing-version.rb" 0.24.0 "$NEW_SHA"

write_fixture "$WORK/missing-sha.rb"
grep -v '^  sha256 ' "$WORK/missing-sha.rb" > "$WORK/ms.tmp" && mv "$WORK/ms.tmp" "$WORK/missing-sha.rb"
expect_fail "missing sha256 line fails" "$WORK/missing-sha.rb" 0.24.0 "$NEW_SHA"

# 4. Duplicate lines fail.
write_fixture "$WORK/dup-version.rb"
printf '  version "0.1.0"\n' >> "$WORK/dup-version.rb"
expect_fail "duplicate version line fails" "$WORK/dup-version.rb" 0.24.0 "$NEW_SHA"

write_fixture "$WORK/dup-sha.rb"
printf '  sha256 "%s"\n' "$OLD_SHA" >> "$WORK/dup-sha.rb"
expect_fail "duplicate sha256 line fails" "$WORK/dup-sha.rb" 0.24.0 "$NEW_SHA"

# 5. Bad version fails.
write_fixture "$WORK/badver.rb"
expect_fail "version 1.2 fails" "$WORK/badver.rb" 1.2 "$NEW_SHA"
expect_fail "version v1.2.3 fails" "$WORK/badver.rb" v1.2.3 "$NEW_SHA"
expect_fail "pre-release version fails" "$WORK/badver.rb" 1.2.3-rc.1 "$NEW_SHA"
expect_fail "empty version fails" "$WORK/badver.rb" "" "$NEW_SHA"

# 6. Bad sha fails.
expect_fail "short sha fails" "$WORK/badver.rb" 0.24.0 "${NEW_SHA:0:63}"
expect_fail "long sha fails" "$WORK/badver.rb" 0.24.0 "${NEW_SHA}0"
expect_fail "uppercase sha fails" "$WORK/badver.rb" 0.24.0 "$(printf '%s' "$NEW_SHA" | tr a-f A-F)"
expect_fail "non-hex sha fails" "$WORK/badver.rb" 0.24.0 "${NEW_SHA:0:63}g"

# 7. Missing file and wrong arg count fail.
if bash "$SCRIPT" "$WORK/nope.rb" 0.24.0 "$NEW_SHA" > /dev/null 2>&1; then bad "missing file fails"; else ok "missing file fails"; fi
if bash "$SCRIPT" "$CASK" 0.24.0 > /dev/null 2>&1; then bad "wrong arg count fails"; else ok "wrong arg count fails"; fi

# 8. No temp files are left behind.
leftover=$(find "$WORK" -name '*.rb.*' | wc -l | tr -d ' ')
if [[ "$leftover" == "0" ]]; then ok "no temp files left behind"; else bad "no temp files left behind ($leftover found)"; fi

echo "$PASS passed, $FAIL failed"
(( FAIL == 0 ))
