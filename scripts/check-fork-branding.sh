#!/usr/bin/env bash
# Finds upstream MonoCode branding that a merge from upstream may have reintroduced.
#
# Run after every merge. Exits non-zero on anything not explicitly allowed below,
# so it can gate a merge.
set -uo pipefail

cd "$(dirname "$0")/.."

# Only string literals and links. A bare `MonoCode` inside an identifier
# (MonoCodeMark, MonoCodeToolCall, MonoCodeDockMenuTarget) is a symbol name, not
# branding, so the pattern requires a quote or sentence punctuation to end the word.
# `MonoCode Host` is matched on its own because it also appears in unquoted text inside
# multi-line template literals (the systemd unit Description=), which the quote rule misses.
PATTERN='(hardbeat920/monocode|usemono\.dev|MonoCode Host|"[^"]*MonoCode[ "._,!?:;)-]|`[^`]*MonoCode[ `._,!?:;)-])'

# Deliberate keeps, each with the reason. Anything not listed here is a finding.
# Format: <file>:<line> or <file> for a whole file.
#
# Rust unit tests live beside the code they test, so they cannot be excluded by
# path — the ones that assert on upstream strings as fixture data are listed here.
read -r -d '' ALLOW <<'EOF' || true
src/features/sessions/model/inFlight.ts
src/integrations/harness/providers/hermes/hermes.ts
src-tauri/src/macos.rs
src-tauri/src/skills.rs
src-tauri/src/control_cli.rs
src-tauri/src/quick_composer.rs
host/windows.ts
EOF

allow_re=$(printf '%s' "$ALLOW" | paste -sd'|' -)

hits=$(grep -rnE "$PATTERN" \
  src/ src-tauri/src/ host/ index.html \
  --include="*.ts" --include="*.tsx" --include="*.rs" --include="*.html" 2>/dev/null \
  | grep -v '\.test\.' \
  | grep -vE "^[^:]+: *(//|///|//!|\*)" \
  | grep -vE "^($allow_re):")

# Test fixtures inside src-tauri/src/fs.rs (its `mod tests`). They feed the GitHub repo
# parsers (parse_github_repositories, github_pr_head_filter, split_github_repo) sample
# `owner/repo` slugs and assert the parsed result, so the upstream name is data, not
# branding. Matched by exact line content rather than line number so they survive upstream
# merges shifting the file, and so a real hit sitting nearby is not hidden.
hits=$(printf '%s\n' "$hits" | grep -vE '^src-tauri/src/fs\.rs:[0-9]+: *("nameWithOwner": "hardbeat920/monocode",|vec!\["hardbeat920/monocode"\]|github_pr_head_filter\("hardbeat920/monocode", "main"\)\.as_deref\(\),|vec!\["EricRasputin/monocode-eric", "hardbeat920/monocode"\]|split_github_repo\(" hardbeat920/monocode "\)\.unwrap\(\),)$' || true)

hits=$(printf '%s\n' "$hits" | grep -v '^[[:space:]]*$' || true)

if [ -n "$hits" ]; then
  echo "Upstream branding reintroduced — review each:"
  printf '%s\n' "$hits"
  echo
  echo "Fix it, or add it to the ALLOW list in this script with the reason."
  exit 1
fi

echo "No upstream branding found outside the documented keeps."
