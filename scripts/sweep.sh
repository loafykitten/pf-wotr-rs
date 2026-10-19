#!/usr/bin/env bash
# scripts/sweep.sh [OUTDIR] [CAPTURE...]: the POC-5 sweep: the driven replay (`wr-combat replay`, release build) of
# every run of every fixture of the current captures, one after another under scripts/heavy.sh. Each run's output goes
# to OUTDIR/<capture>.<fixture>.<run>.out (default: cache/sweep/<timestamp>); prints one line per run with its
# `valid=` line, then the totals. The captures default to the sweep's list (PLAN.md, "Where things stand"); name
# others (e.g. poc5-tb-140) to sweep only those. The save is the one the capture recorded (run.json's save_file).
#   WR_SWEEP_BIN   the wr-combat binary (default: $CARGO_TARGET_DIR or target/, release)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
caps="$root/fixtures/captures/oracle"
out=${1:-"$root/cache/sweep/$(date +%Y%m%d-%H%M%S)"}
shift || true
list=("$@")
if [ ${#list[@]} -eq 0 ]; then
  for c in 95 96 109 111 112 113 114 115 116 117 120 122 123 124 125 126 127 129 131 132 133 134 135 136 140; do
    list+=("poc5-tb-$c")
  done
fi
bin=${WR_SWEEP_BIN:-"${CARGO_TARGET_DIR:-$root/target}/release/wr-combat"}
[ -x "$bin" ] || { echo "no $bin: cargo build --release -p wr-combat" >&2; exit 1; }
mkdir -p "$out"

run_all() {
  local valid=0 invalid=0
  for c in "${list[@]}"; do
    local save
    save=$(basename "$(jq -r .save_file "$caps/$c/run.json")" .zks)
    for f in "$caps/$c"/fixtures/*/; do
      f=${f%/}
      for t in "$f"/*.trace.jsonl.gz; do
        local r o v
        r=$(basename "$t" .trace.jsonl.gz)
        o="$out/$c.$(basename "$f").$r.out"
        "$bin" replay "$f" --save "$save" --run "$r" >"$o" 2>&1 || true
        v=$(grep -m1 '^valid=' "$o" || echo "valid=? (no summary; see $o)")
        echo "$c $(basename "$f") $r: $v"
        case $v in valid=true*) valid=$((valid + 1)) ;; *) invalid=$((invalid + 1)) ;; esac
      done
    done
  done
  echo "sweep: $((valid + invalid)) runs, $valid valid, $invalid not; outputs in $out"
}

if [ "${WR_SWEEP_INNER:-}" = 1 ]; then
  run_all
else
  WR_SWEEP_INNER=1 exec "$root/scripts/heavy.sh" "$0" "$out" "${list[@]}"
fi
