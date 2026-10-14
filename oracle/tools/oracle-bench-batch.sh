#!/usr/bin/env bash
# Baseline bench runs (tools/oracle-run.sh bench) with contention control, into
# fixtures/captures/runs-raw/baseline/<label>. usage: oracle/tools/oracle-bench-batch.sh config:label[:cold]...
#   e.g. oracle/tools/oracle-bench-batch.sh game:game-1 best:best-1 game:game-oscold:cold
# One instance at a time (fixtures/captures/runs-raw/baseline/.batch.lock); run it detached (nohup ... &).
# Before each run: wait until the machine has been quiet for 60 s (no process outside
# the fixed background set above 150% CPU, no compiler or tool above 20%, no spike or
# AssetRipper). During the run: sample every 5 s (ps %cpu, a decaying average, so short
# bursts between samples can be missed); if any hog appears, move the
# run to contended/ and try again (up to 4 attempts per label).
set -uo pipefail
mac="$(cd "$(dirname "$0")/../.." && pwd)" # the repository root
raw="$mac/fixtures/captures/runs-raw/baseline"
mkdir -p "$raw/contended"
bg_re='^(UVCAssistant|OBS|WindowServer|kernel_task|Pathfinder Wrath Of The Righteous|top|ps|awk)$'
comp_re='^(rustc|cargo|clippy-driver|clang|ld|cc|swift-frontend|wr-.*|wr_.*|AssetRipper.*|render-spike|dotnet)$'
hogs() { # "name:cpu" for compilers/tools over 20%, or any other non-background process over 150%
  ps -axo pcpu=,comm= | while read -r cpu path; do
    name="${path##*/}"
    [[ "$name" =~ $bg_re ]] && continue
    if [[ "$name" =~ $comp_re ]]; then lim=20; else lim=150; fi
    awk -v c="$cpu" -v l="$lim" 'BEGIN{exit !(c > l)}' && echo "$name:$cpu"
  done
}
# pgrep: 0 found, 1 none; anything else counts as blocked (can't tell).
running() { local rc=0; pgrep -q "$@" || rc=$?; (( rc != 1 )); }
blocked() {
  running -x render-spike || running -x "Pathfinder Wrath Of The Righteous" || running -x cargo || running -x rustc || running -f 'AssetRipper'
}
wait_quiet() {
  local quiet_since=-1 now
  while true; do
    now=$(date +%s)
    if blocked || [[ -n "$(hogs)" ]]; then quiet_since=-1
    elif (( quiet_since < 0 )); then quiet_since=$now
    elif (( now - quiet_since >= 60 )); then return 0
    fi
    sleep 5
  done
}
# Validate every spec before doing anything.
for spec in "$@"; do
  IFS=: read -r config label cold extra <<<"$spec"
  [[ "$config" == game || "$config" == best ]] || { echo "bad config in '$spec'" >&2; exit 2; }
  [[ "$label" =~ ^[A-Za-z0-9_-]+$ ]] || { echo "bad label in '$spec'" >&2; exit 2; }
  [[ -z "$cold" || "$cold" == cold ]] && [[ -z "$extra" ]] || { echo "bad spec '$spec'" >&2; exit 2; }
done
batch_lock="$raw/.batch.lock"
mkdir "$batch_lock" 2>/dev/null || { echo "another batch holds $batch_lock (remove it if stale)" >&2; exit 1; }
trap 'rmdir "$batch_lock" 2>/dev/null' EXIT
exhausted=()
for spec in "$@"; do
  IFS=: read -r config label cold <<<"$spec"
  done_ok=false
  for attempt in 1 2 3 4; do
    out="$raw/$label"
    if [[ -e "$out" ]]; then
      echo "$out exists; not touching it (move it away to rerun $label)" >&2
      done_ok=true; break
    fi
    echo "$(date +%T) waiting for quiet before $label (attempt $attempt)"
    wait_quiet
    if [[ "$cold" == cold ]]; then
      clear_out="$("$mac/oracle/tools/metal-cache.sh" clear)" || { echo "cache clear failed; stopping" >&2; exit 1; }
      echo "$clear_out"
      [[ "$clear_out" == moved* ]] || { echo "cache was already cold or not stashed; refusing a cold run" >&2; exit 1; }
    fi
    log="$(mktemp)"
    ( while true; do h="$(hogs)"; [[ -n "$h" ]] && echo "$(date +%T) $h" >> "$log"; sleep 5; done ) &
    sampler=$!
    echo "$(date +%T) start $label ($config ${cold:-warm})"
    "$mac/oracle/tools/oracle-run.sh" --config "$config" --label "$label" bench "$out" > "$log.out" 2>&1
    rc=$?
    kill "$sampler" 2>/dev/null
    if [[ "$cold" == cold ]]; then
      until ! running -x "Pathfinder Wrath Of The Righteous"; do sleep 5; done
      "$mac/oracle/tools/metal-cache.sh" restore || { echo "cache restore FAILED: restore by hand (tools/metal-cache.sh restore)" >&2; exit 1; }
    fi
    tail -10 "$log.out"
    if [[ -d "$out" ]]; then cp "$log" "$out/contention.txt"; fi
    if [[ -s "$log" ]]; then
      echo "$(date +%T) $label CONTENDED: $(sort -u "$log" | head -5 | tr '\n' ' ')"
      [[ -d "$out" ]] && mv "$out" "$raw/contended/$label-$(date +%H%M%S)"
    elif (( rc != 0 )) || [[ ! -f "$out/run.json" ]]; then
      echo "$(date +%T) $label FAILED rc=$rc"
      [[ -d "$out" ]] && mv "$out" "$raw/contended/$label-failed-$(date +%H%M%S)"
    else
      echo "$(date +%T) end $label clean"
      rm -f "$log" "$log.out"
      done_ok=true
      break
    fi
    rm -f "$log" "$log.out"
  done
  $done_ok || exhausted+=("$label")
done
if (( ${#exhausted[@]} )); then echo "NOT DONE (all attempts contended or failed): ${exhausted[*]}"; exit 1; fi
echo ALL-DONE
