#!/usr/bin/env bash
# Format, lint and test the main workspace. Spikes (spikes/*) are separate workspaces:
#   scripts/check.sh            main workspace
#   scripts/check.sh spikes/X   one spike
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/${1:-.}"
cd "$dir"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --no-tests=pass
