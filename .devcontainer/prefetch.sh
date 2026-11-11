#!/usr/bin/env bash
# postCreateCommand: non-blocking for the sandbox start. Downloads the native crates so the
# first check does not wait on the network. Idempotent; failure never fails the session.
cd "$(dirname "$0")/.."
cargo fetch --locked --manifest-path crates/monocode-gpui/Cargo.toml \
  || echo "[monoptah] cargo fetch failed (network?); the first cargo command will retry" >&2
exit 0
