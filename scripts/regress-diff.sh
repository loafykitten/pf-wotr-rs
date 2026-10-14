#!/usr/bin/env bash
# regress-diff.sh <out-dir> [baseline-dir]: compare a regress.sh output with the baseline
# (a previous run of regress.sh): sheet match rates and per-unit counts, Manual_18, and every dialogue
# run's log (timings stripped). Prints nothing for a section that is unchanged.
S=${WR_REGRESS_OUT:-"$(cd "$(dirname "$0")/.." && pwd)/cache/regress"}
new=$1; base=${2:?usage: regress-diff.sh <out-dir> <baseline-out-dir>}
for f in "$base"/compare-*.log "$base"/combat-compare.log; do
  b=$(basename "$f")
  d=$(diff <(rg -v 'details:' "$f") <(rg -v 'details:' "$new/$b" 2>&1))
  [ -n "$d" ] && { echo "### $b"; echo "$d" | cut -c1-300; }
done
for f in "$base"/dialog-*.log; do
  b=$(basename "$f")
  d=$(diff <(sed -E 's/run [0-9.]+ ms//; s#/out-[^/]+/#/OUT/#g; s#baseline-[0-9a-f]+#OUT#g' "$f") <(sed -E 's/run [0-9.]+ ms//; s#/out-[^/]+/#/OUT/#g' "$new/$b" 2>&1))
  [ -n "$d" ] && { echo "### $b"; echo "$d" | cut -c1-400; }
done
echo "regress-diff done"
