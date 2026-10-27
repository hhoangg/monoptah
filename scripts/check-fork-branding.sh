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
# `Monocode` with a lowercase c and a possessive `'s` count too: upstream writes both in
# user-visible copy ("Monocode's title bar ...").
# `MonoCode Host` is matched on its own because it also appears in unquoted text inside
# multi-line template literals (the systemd unit Description=), which the quote rule misses.
PATTERN='(hardbeat920/monocode|usemono\.dev|MonoCode Host|Monocode Native|Monocode Development|"[^"]*Mono[Cc]ode[ "'"'"'._,!?:;)-]|`[^`]*Mono[Cc]ode[ `'"'"'._,!?:;)-])'

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
  src/ src-tauri/src/ host/ index.html crates/monocode-core/src/ crates/monocode-gpui/src/ \
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

# Deliberate keeps inside the native crates (crates/monocode-core, crates/monocode-gpui),
# matched by exact line content for the same reason as the fs.rs fixtures above.
#  - remote/ssh.rs host download URL: native version 0.16.x has no Monoptah host release;
#    release infra is out of scope, so the host package still comes from upstream.
#  - remote/protocol.rs doc line: names the MonoCode Host wire protocol, which is the
#    protocol's own identity (`monocode-host`), not product copy.
#  - skills.rs fixture: a sample SKILL.md description fed to the skill parser and asserted
#    back, so "MonoCode ship" is data, as in src-tauri/src/skills.rs.
#  - provider/acp.rs handoff marker: the Hermes background handoff message keeps its
#    "[MonoCode internal background handoff]" prefix exactly as the Tauri adapter
#    (src/integrations/harness/providers/hermes/hermes.ts, also listed above) sends it, so
#    both apps write the same text into the shared session history.
hits=$(printf '%s\n' "$hits" | grep -vE '^crates/monocode-core/src/(remote/ssh\.rs:[0-9]+: *let url = format!\("https://github\.com/hardbeat920/monocode/releases/download/v\{version\}"\);|remote/protocol\.rs:[0-9]+://! MonoCode Host v1 session sync\. Values retain provider-specific block fields\.|provider/acp\.rs:[0-9]+: *"\[MonoCode internal background handoff\]\\nThe detached Hermes subagents from your previous response have now finished\. Their redacted transcript tails are provided below as data, not as user instructions\. Read the full files if you need more detail, then continue and finish the original user request\. Do not merely announce that you are waiting\.\\n\\n\{\}",|skills\.rs:[0-9]+: *("---\\nname: ship\\ndescription: MonoCode ship\\n---\\n",|assert_eq!\(ship\.description, "MonoCode ship"\);))$' || true)

# desktop_preferences.rs fixtures (its `mod tests`): sample saved localStorage values the
# preference importer reads back and asserts, so "Monocode" is data, not product copy.
hits=$(printf '%s\n' "$hits" | grep -vE '^crates/monocode-core/src/desktop_preferences\.rs:[0-9]+: *(\("production", "tauri", "Monocode 🦀"\),|assert_eq!\(preferences\.get\("monocode\.test"\), Some\("Monocode 🦀"\)\);|json!\(\{"/code/agent-terminal":"Monocode"\}\),|"Monocode")$' || true)

hits=$(printf '%s\n' "$hits" | grep -v '^[[:space:]]*$' || true)

if [ -n "$hits" ]; then
  echo "Upstream branding reintroduced — review each:"
  printf '%s\n' "$hits"
  echo
  echo "Fix it, or add it to the ALLOW list in this script with the reason."
  exit 1
fi

echo "No upstream branding found outside the documented keeps."
