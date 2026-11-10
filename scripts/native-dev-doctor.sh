#!/usr/bin/env bash
# Diagnoses the Linux native dev environment: toolchain, C linker, system libraries, gh.
# Exit 0 only when everything needed for the core and GPUI checks is present.
#   scripts/native-dev-doctor.sh [--quiet]
set -u
quiet=0; [ "${1:-}" = "--quiet" ] && quiet=1
missing=0
ok()   { [ "$quiet" = 1 ] || printf '  ok   %s\n' "$1"; }
bad()  { printf '  MISSING %s\n' "$1" >&2; missing=1; }
tool() { if command -v "$1" >/dev/null 2>&1; then ok "$1: $("$1" ${2:---version} 2>&1 | head -1)"; else bad "$1"; fi; }

[ "$quiet" = 1 ] || echo "arch: $(uname -m)  cpus: $(nproc)  jobs: ${CARGO_BUILD_JOBS:-unset}"
for t in rustc cargo cc c++ clang cmake ninja pkg-config node npm git; do tool "$t"; done
cargo fmt --version >/dev/null 2>&1 && ok "rustfmt" || bad "rustfmt"
cargo clippy --version >/dev/null 2>&1 && ok "clippy" || bad "clippy"
for lib in fontconfig freetype2 xcb xcb-shape xcb-xfixes xcb-randr xkbcommon xkbcommon-x11 \
           wayland-client alsa x11-xcb egl vulkan dbus-1; do
  pkg-config --exists "$lib" 2>/dev/null && ok "pkg-config $lib" || bad "pkg-config $lib"
done
if command -v gh >/dev/null 2>&1; then
  ok "gh: $(gh --version | head -1)"
  if [ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then ok "gh token present in env"
  else [ "$quiet" = 1 ] || echo "  note gh has no token: gh refuses every call without one (use curl against api.github.com for public reads); no posting"; fi
else bad "gh"; fi
exit "$missing"
