#!/usr/bin/env bash
# scripts/scene-cache.sh <area name> <area guid>: build cache/sim/scenes/<guid>.json, the area's
# scene documents (script zones with shapes, map objects' components, cutscene players, unit
# groups), from the game install via the importer. Known areas:
#   Prologue_Caves_1     bad465e5404604346bdf71ef50b3299a
#   Prologue_Neathholm   61fcf2a352daa394ebae399b1348ba62
# The sim loads the file when it exists; without it an area's scene entities are reported
# unsupported (the run is not valid), never approximated. The unit prefab table
# (cache/sim/unit-prefabs.json: each unit prefab's IsInGameBySettings, which every unit built
# from a view starts with) is built once per install with `wr-import unit-prefabs`, here too
# when it is missing. The area's nav file (cache/sim/nav/<guid>.wrnav: its navigation graphs in
# wr-nav's format) is written alongside.
set -euo pipefail
name=${1:?area name, e.g. Prologue_Neathholm}
guid=${2:?area guid}
R=$(cd "$(dirname "$0")/.." && pwd)
export WR_CACHE=${WR_CACHE:-$R/cache}
cd "$R"
lower=$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')
# The dependency list names the bundles in lower case (`prologue_neathholm_*.scenes`).
cargo run --release -q -p wr-import -- import --area "$lower" --no-census
cargo run --release -q -p wr-import -- nav-pack --area "$lower" --guid "$guid"
[ -f "$WR_CACHE/sim/unit-prefabs.json" ] || cargo run --release -q -p wr-import -- unit-prefabs
cargo run --release -q -p wr-sim --bin wr-sim -- scene-import \
  "$WR_CACHE/import/$lower/inventory.json" --area "$guid"
