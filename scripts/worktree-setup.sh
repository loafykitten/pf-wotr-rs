#!/usr/bin/env bash
# scripts/worktree-setup.sh NAME: prepares a git worktree of this repo (run inside it) for a parallel
# work stream, from the main checkout it belongs to:
#   - links the gitignored inputs: fixtures/captures, fixtures/saves, decompiled, builds (read them;
#     never build or install WrathOracle from a worktree, nor record: the oracle build and the
#     game/GPU lock are shared, so recordings go through the main session);
#   - target: its own build directory beside the main one (an APFS clone of it, so dependencies
#     don't rebuild), linked as target/;
#   - cache: its own directory beside the main one whose entries link to the main cache's (reads
#     see the main cache; new top-level entries stay private), sim/ a private copy, linked as cache/.
# Prints the environment to use (CARGO_TARGET_DIR and WR_CACHE point at the links' targets;
# CARGO_BUILD_JOBS keeps a stream's builds to 4 jobs, so parallel streams don't exhaust memory).
# Memory-heavy runs (capture replays and sweeps, ignored capture suites) go through
# scripts/heavy.sh, which runs them one at a time across the machine.
set -euo pipefail
name=${1:?stream name, e.g. paths}
wt=$(git rev-parse --show-toplevel)
main=$(cd "$(git rev-parse --git-common-dir)/.." && pwd)
[ "$wt" != "$main" ] || { echo "run this inside a worktree, not the main checkout" >&2; exit 1; }
cd "$wt"
mkdir -p fixtures
for p in fixtures/captures fixtures/saves decompiled builds; do
  [ -e "$p" ] || ln -s "$main/$p" "$p"
done
# The main checkout's target/ and cache/ are links onto the external volume; siblings go there too.
main_target=$(readlink "$main/target" || echo "$main/target")
main_cache=$(readlink "$main/cache" || echo "$main/cache")
target="$(dirname "$main_target")/target-wt-$name"
cache="$(dirname "$main_cache")/cache-wt-$name"
if [ ! -e "$target" ]; then
  echo "cloning the main target dir (copy-on-write)..." >&2
  cp -cR "$main_target" "$target" 2>/dev/null || mkdir -p "$target"
fi
mkdir -p "$cache"
# sim/ (the area files the sim reads: physics, nav, scenes, prefabs, strings) is copied, not linked: a stream
# that changes a file's format would otherwise write it into the main cache, which the main build can't read.
[ -e "$cache/sim" ] || cp -R "$main_cache/sim" "$cache/sim"
for e in "$main_cache"/*; do
  [ -e "$cache/$(basename "$e")" ] || ln -s "$e" "$cache/$(basename "$e")"
done
[ -e target ] || ln -s "$target" target
[ -e cache ] || ln -s "$cache" cache
echo "export CARGO_TARGET_DIR=$target WR_CACHE=$cache CARGO_BUILD_JOBS=4"
