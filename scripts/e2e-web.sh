#!/usr/bin/env bash
# The one command for the Monoptah web E2E harness (E-01), in CI and in sandboxes.
#
#   scripts/e2e-web.sh
#
# Environment:
#   CI=true                     add `--with-deps` to the Playwright browser install
#   E2E_SKIP_HEADLESS_BUILD=1   do not build web_link_headless (use the binary that is already there)
#   E2E_HEADLESS_BIN            path of the headless binary (default crates/monocode-gpui/target/native-dev/web_link_headless)
#   E2E_OUTPUT_DIR              where artifacts, report, traces and run.log go (default /tmp/evidence/ws-q/e2e)
# The exit code is the one of Playwright. At test time nothing leaves localhost and no credentials are needed.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${E2E_OUTPUT_DIR:-/tmp/evidence/ws-q/e2e}"
export E2E_OUTPUT_DIR="$out"
mkdir -p "$out"

cd "$repo/web"
pnpm install --frozen-lockfile

install_args=(chromium)
if [ "${CI:-}" = "true" ]; then install_args+=(--with-deps); fi
pnpm --filter @monoptah/e2e exec playwright install "${install_args[@]}"

if [ "${E2E_SKIP_HEADLESS_BUILD:-}" != "1" ]; then
  (cd "$repo" && cargo build --locked --profile native-dev \
    --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-core --bin web_link_headless)
fi

# `tee` keeps the list output in run.log; pipefail keeps the exit code of Playwright.
pnpm --filter @monoptah/e2e run e2e 2>&1 | tee "$out/run.log"
