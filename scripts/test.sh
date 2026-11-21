#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
export CARGO_INCREMENTAL=0
# Build separately with `CARGO_INCREMENTAL=0 cargo test --workspace --all-targets --locked --no-run` before timing this entry.
# This includes every workspace test, its fixtures, and cleanup; no ignored slow-test lane.
# The user-approved execution wall-time budget is 10 minutes (600 seconds).
/usr/bin/time -p bash -e -c '
  cargo test --workspace --all-targets --locked --no-fail-fast
  cargo test --workspace --doc --locked --no-fail-fast
'
