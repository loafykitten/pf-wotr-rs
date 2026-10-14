#!/usr/bin/env bash
# Manage macOS's per-app Metal shader cache for Wrath (shared by every build, since they
# all keep the bundle ID "unity.Owlcat Games.Pathfinder Wrath Of The Righteous").
#
# usage: oracle/tools/metal-cache.sh status | clear | restore
#   clear    move the cache aside (into builds/metal-cache-stash/<time>/) so the next
#            launch compiles every shader from scratch: a "cold" benchmark run
#   restore  put the most recent stashed cache back, replacing whatever is there now
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)" # the repository root
cache="$(getconf DARWIN_USER_CACHE_DIR)unity.Owlcat Games.Pathfinder Wrath Of The Righteous"
stash_root="$root/builds/metal-cache-stash"

# pgrep: 0 = running, 1 = not running, anything else = couldn't tell (refuse).
rc=0
pgrep -qx 'Pathfinder Wrath Of The Righteous' || rc=$?
case $rc in
  0) echo "quit Wrath first: the cache is in use while it runs" >&2; exit 1 ;;
  1) ;;
  *) echo "pgrep failed (exit $rc); can't tell whether Wrath is running" >&2; exit 1 ;;
esac

case "${1:-status}" in
  status)
    if [[ -d "$cache" ]]; then du -sh "$cache"; else echo "no cache at $cache (next launch is cold)"; fi
    [[ -d "$stash_root" ]] && find "$stash_root" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; | sort | sed 's/^/stashed: /'
    true
    ;;
  clear)
    if [[ ! -d "$cache" ]]; then echo "already cold: no cache at $cache"; exit 0; fi
    dest="$stash_root/$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$stash_root"
    mv "$cache" "$dest"
    echo "moved cache to $dest; the next launch is cold"
    ;;
  restore)
    latest=""
    [[ -d "$stash_root" ]] && latest="$(find "$stash_root" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; | sort | tail -1)"
    [[ -n "$latest" ]] || { echo "nothing stashed in $stash_root" >&2; exit 1; }
    rm -rf "$cache"
    mv "$stash_root/$latest" "$cache"
    echo "restored $latest"
    ;;
  *)
    echo "usage: $0 status | clear | restore" >&2
    exit 2
    ;;
esac
