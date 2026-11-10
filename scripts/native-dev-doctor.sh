#!/usr/bin/env bash
# Diagnoses the Linux native dev environment.
#   scripts/native-dev-doctor.sh [--quiet]
# Exit status reflects BUILD prerequisites only (toolchain, C compiler and linker, system
# libraries, node, git). Resources, git checkout and GitHub access are reported as WARN or
# NOTE lines and never change the exit status: they limit what you can do, not whether the
# toolchain is there. --quiet hides the "ok" lines, not the warnings.
set -u
quiet=0; [ "${1:-}" = "--quiet" ] && quiet=1
missing=0
ok()   { [ "$quiet" = 1 ] || printf '  ok    %s\n' "$1"; }
bad()  { printf '  MISSING %s\n' "$1" >&2; missing=1; }
warn() { printf '  WARN  %s\n' "$1" >&2; }
note() { [ "$quiet" = 1 ] || printf '  note  %s\n' "$1"; }
tool() { if command -v "$1" >/dev/null 2>&1; then ok "$1: $("$1" ${2:---version} 2>&1 | head -1)"; else bad "$1"; fi; }

# --- build prerequisites -------------------------------------------------------------
[ "$quiet" = 1 ] || echo "build prerequisites"
for t in rustc cargo cc c++ clang cmake ninja pkg-config node npm git; do tool "$t"; done
cargo fmt --version >/dev/null 2>&1 && ok "rustfmt" || bad "rustfmt"
cargo clippy --version >/dev/null 2>&1 && ok "clippy" || bad "clippy"
for lib in fontconfig freetype2 xcb xcb-shape xcb-xfixes xcb-randr xkbcommon xkbcommon-x11 \
           wayland-client alsa x11-xcb egl vulkan dbus-1; do
  pkg-config --exists "$lib" 2>/dev/null && ok "pkg-config $lib" || bad "pkg-config $lib"
done

# --- resources (warnings only) -------------------------------------------------------
# What the build can actually use is the container's cgroup limit, not the host's RAM and
# not nproc (a CPU quota does not change nproc).
[ "$quiet" = 1 ] || echo "resources"
mem_mib=""
for f in /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory/memory.limit_in_bytes; do
  if [ -r "$f" ]; then v=$(cat "$f"); case "$v" in ''|max|*[!0-9]*) ;; *) mem_mib=$((v / 1048576)) ;; esac; break; fi
done
[ -n "$mem_mib" ] && [ "$mem_mib" -lt 1048576 ] || mem_mib=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
jobs="${CARGO_BUILD_JOBS:-}"
note "memory limit: ${mem_mib} MiB  nproc: $(nproc)  CARGO_BUILD_JOBS: ${jobs:-unset}"
if [ "$mem_mib" -lt 7000 ]; then
  warn "under 8g: the GPUI test binary (cargo test -p monocode-gpui) is OOM-killed even with CARGO_BUILD_JOBS=1. fmt, clippy and cargo check for core and GPUI, and the core tests, were verified at 4g with CARGO_BUILD_JOBS=1 (core tests) / 2 (check, clippy). Resize the sandbox to 4 CPUs / 8g (Docker Sandbox setup) for the GPUI tests."
fi
if [ -z "$jobs" ]; then
  warn "CARGO_BUILD_JOBS is unset, so cargo uses nproc jobs; the devcontainer sets 1. The 4-job GPUI test build on 8g was killed (likely OOM)."
elif [ "$jobs" -gt 1 ] 2>/dev/null && [ "$mem_mib" -lt 16000 ]; then
  warn "CARGO_BUILD_JOBS=$jobs: only 1 is verified for the GPUI test binary on 8g (4 was killed, likely OOM; 2 is untested)."
fi

# --- git checkout (warning only) -----------------------------------------------------
if [ -f .git ]; then
  gitdir=$(sed -n 's/^gitdir: //p' .git)
  if ! git rev-parse --git-dir >/dev/null 2>&1; then
    warn "linked worktree whose gitdir ($gitdir) is not visible here: git fails and the 3 docker_start_tests::scripts tests fail. Bind-mount the main repository's .git at the same path (docs/native/cloud-dev-environment.md)."
  fi
fi

# --- GitHub integration (optional) ---------------------------------------------------
[ "$quiet" = 1 ] || echo "github (optional, not needed to build)"
if command -v gh >/dev/null 2>&1; then
  note "gh: $(gh --version | head -1)"
else
  warn "gh is not installed: no GitHub CLI (the devcontainer image installs it)"
fi
if [ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then
  note "a GitHub token variable is set; this script does not test it, so authentication is unverified"
else
  warn "no GH_TOKEN/GITHUB_TOKEN: gh refuses every call, so no PR review or posting. Public reads: curl https://api.github.com/..."
fi
exit "$missing"
