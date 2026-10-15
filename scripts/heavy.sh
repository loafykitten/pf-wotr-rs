#!/usr/bin/env bash
# scripts/heavy.sh CMD [ARGS...]: runs a memory-heavy command (a capture replay or sweep, an ignored
# capture test suite, a recording's analysis) one at a time across the machine. Every worktree
# shares the main checkout's builds/, so the lock there is the machine's. Waits for the lock, then
# for enough free memory, so parallel streams queue instead of exhausting the 32 GB together.
#   WR_HEAVY_MIN_GB  free memory (GiB) to wait for before starting (default 12)
#   WR_HEAVY_WAIT    seconds to wait for memory before giving up (default 1800)
# One driven replay of a long capture peaks at about 8.5 GB; the game while recording takes more.
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: scripts/heavy.sh CMD [ARGS...]" >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
lock="$root/builds/.heavy.lock"
min_gb=${WR_HEAVY_MIN_GB:-12}
wait_s=${WR_HEAVY_WAIT:-1800}

free_gb() {
  # Free plus reclaimable (inactive, purgeable, speculative) pages, in GiB.
  vm_stat | awk -v ps="$(pagesize)" '
    /Pages free/ || /Pages inactive/ || /Pages purgeable/ || /Pages speculative/ { gsub(/\./, "", $NF); n += $NF }
    END { printf "%d", n * ps / 1073741824 }'
}

if [ "${WR_HEAVY_HELD:-}" != 1 ]; then
  # Re-enter under the lock (lockf waits for it); the inner run skips this.
  exec env WR_HEAVY_HELD=1 /usr/bin/lockf -k "$lock" "$0" "$@"
fi
waited=0
while [ "$(free_gb)" -lt "$min_gb" ]; do
  if [ "$waited" -ge "$wait_s" ]; then
    echo "heavy.sh: under ${min_gb} GiB free after ${wait_s}s ($(free_gb) GiB); not starting: $*" >&2
    exit 75
  fi
  [ "$waited" -eq 0 ] && echo "heavy.sh: waiting for ${min_gb} GiB free (now $(free_gb) GiB)" >&2
  sleep 15
  waited=$((waited + 15))
done
exec "$@"
