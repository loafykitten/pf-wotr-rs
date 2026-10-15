#!/usr/bin/env bash
# scripts/regress.sh <label> <bindir> [outroot]: outputs in <outroot or $WR_REGRESS_OUT>/out-<label>; WR_CACHE picks the cache: hydrate/compare saves, dialogue fixtures and Manual_18 with one build.
set -uo pipefail
label=$1; bin=$2
R="$(cd "$(dirname "$0")/.." && pwd)"
S=${WR_REGRESS_OUT:-$R/cache/regress}; mkdir -p "$S"
O=${3:-$S}/out-$label; rm -rf "$O"; mkdir -p "$O"
export WR_CACHE=${WR_CACHE:-$R/cache}
K=$WR_CACHE
C=$R/fixtures/captures/oracle
cd "$R" || exit 2
hyd() { # save oracle-dump
  "$bin/wr-sim" hydrate "$1" > "$O/hydrate-$1.log" 2>&1
  "$bin/wr-sim" compare "$K/sim/$1.state-after-load.json" "$2" > "$O/compare-$1.log" 2>&1
  cp "$K/sim/compare.json" "$O/compare-$1.json" 2>/dev/null
  for f in "$K/sim/$1".*; do cp "$f" "$O/"; done
}
hyd Manual_13 "$C/state-manual13-after-load.json"
hyd Manual_19 "$C/manual19-dump-2/state-manual19-after-load.json"
gunzip -c "$C/state-manual10-after-load.json.gz" > "$S/state-manual10.json"
hyd Manual_10 "$S/state-manual10.json"
for cap in poc4-seelah-7 poc4-horgus-11 cert-24 poc4-camelia-5; do
  for fx in "$C/$cap"/fixtures/*/; do
    fx=$(basename "$fx"); [ "$fx" = attack ] || [ "$fx" = dice ] && continue
    for t in "$C/$cap/fixtures/$fx"/*.trace.jsonl.gz; do
      run=$(basename "$t" .trace.jsonl.gz)
      extra=(); [ "$fx" = dialog-checks ] && extra=(--oracle-inputs)
      "$bin/wr-sim" dialog "$fx" --oracle "$C/$cap/fixtures/$fx" --run "$run" --out "$O/dialog/$fx-$run" ${extra[@]+"${extra[@]}"} > "$O/dialog-$fx-$run.log" 2>&1
    done
  done
done
if [ -x "$bin/wr-combat" ]; then
  "$bin/wr-combat" hydrate Manual_18 > "$O/combat-hydrate.log" 2>&1
  "$bin/wr-combat" compare "$K/poc5/Manual_18.state-after-load.json" "$C/manual18-dump-1/state-manual18-after-load.json" > "$O/combat-compare.log" 2>&1
  for f in "$K/poc5"/Manual_18.*; do cp "$f" "$O/combat-$(basename "$f")"; done
  cp "$K/poc5/compare.json" "$O/combat-compare.json"
fi
echo done "$O"
