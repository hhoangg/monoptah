#!/usr/bin/env bash
# onCreateCommand: blocking, idempotent. Only checks that the image really has the
# toolchain; it installs nothing, so a rerun changes nothing.
set -eu
cd "$(dirname "$0")/.."
bash scripts/native-dev-doctor.sh --quiet
