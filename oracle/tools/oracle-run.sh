#!/usr/bin/env bash
# One unattended WrathOracle run on the oracle build (builds/arm64-oracle): set WrathPerf's configuration, write
# WrathOracle's one-shot plan, launch, wait for the game to quit, then keep the oracle's
# outputs plus the WrathStats session and the game's logs in OUT_DIR. WrathPerf's
# settings are restored afterwards, and the plan is removed even if the game never ran.
# usage: oracle/tools/oracle-run.sh [options] PRESET OUT_DIR
#   PRESET: oracle  identity, scene dump, nav queries, route with screenshots, reload,
#                   route with screenshots again (functional; about 4 min)
#           bench   matched-workload baseline: load (process-cold), route with 37.5 s
#                   dwell per point (5 min), reload (warm), route again, 2 more reloads,
#                   a timed manual save into a WR_Bench slot that is deleted again
#                   (about 13 min)
#           shots   route with screenshots only (noise floor; about 2 min)
#           check   identity, dumps, nav, a timed save (functional smoke test; about 2 min)
#           plan    WrathOracle 0.2: the steps and options of --plan-file (a JSON object with
#                   "steps" and any plan keys, e.g. "fidelity", "fixedDelta", "area",
#                   "faults", "blueprintList"); see mods/WrathOracle/Driver.cs and
#                   oracle/tools/oracle-fixtures/ for the fixture plans
#   --config game|best   game: WrathPerf native DXT off, cleanup wait 100 frames (the
#                        game's own behaviour); best: native DXT on, 10 frames (default)
#   --save NAME          save name prefix (default Manual_13_Underground_Caves_of_Kenabres)
#   --label TEXT         recorded in run.json
#   --plan-file FILE     for PRESET plan
#   --deadline SECONDS   for PRESET plan (default 1800)
#   --allow-unhooked N   fixture windows may have up to N unhooked RNG steps (default 0)
# Steam must be running (unless WR_NO_STEAM=1, see launch.sh); the game must not be. Never touches the
# Steam install.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)" # the repository root
# The oracle runs on its own build: an APFS clone of Ember's arm64 build (made with pf-wotr-silicon)
# whose Mods/ holds only our own mods, so no gameplay mod (ToyBox, BuffIt2TheLimit) can change what
# the oracle measures (Ember, 2026-10-11; see builds/arm64-oracle/BUILD_INFO). oracle/mods builds
# into it; WrathPerf's binary came with the clone (its source is pf-wotr-silicon's).
build="$root/builds/arm64-oracle"
oracle_mods="WrathBench WrathOracle WrathPerf WrathStats"
data="$HOME/Library/Application Support/Owlcat Games/Pathfinder Wrath Of The Righteous"
player_log="$HOME/Library/Logs/Owlcat Games/Pathfinder Wrath Of The Righteous/Player.log"
config=best save=Manual_13_Underground_Caves_of_Kenabres label="" plan_file="" deadline_opt=1800 allow_unhooked=0
# The extraction spike's outputs, if present: the oracle evaluates its zone probes and
# runs its nav queries too (WrathOracle plan "inventory" / "navQueries").
import_dir="$root/cache/import/prologue_caves_1"

while [[ $# -gt 2 ]]; do
  case "$1" in
    --config) config="$2" ;;
    --save) save="$2" ;;
    --label) label="$2" ;;
    --plan-file) plan_file="$2" ;;
    --deadline) deadline_opt="$2" ;;
    --allow-unhooked) allow_unhooked="$2" ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift 2
done
[[ $# -eq 2 ]] || { sed -n '2,20p' "$0" >&2; exit 2; }
preset="$1"
out="$2"
[[ "$out" = /* ]] || { echo "OUT_DIR must be absolute" >&2; exit 2; }
[[ -e "$out" ]] && { echo "$out already exists" >&2; exit 1; }
case "$config" in
  game) native=false frames=100 ;;
  best) native=true frames=10 ;;
  *) echo "--config is game or best" >&2; exit 2 ;;
esac
[[ -d "$build/Mods/WrathOracle" && -d "$build/Mods/WrathPerf" && -d "$build/Mods/WrathStats" ]] ||
  { echo "WrathOracle, WrathPerf and WrathStats must be installed in $build/Mods" >&2; exit 1; }
actual_mods="$(cd "$build/Mods" && ls -1 | sort | tr '\n' ' ' | sed 's/ $//')"
[[ "$actual_mods" == "$oracle_mods" ]] ||
  { echo "the oracle build's Mods/ must hold exactly: $oracle_mods (found: $actual_mods)" >&2; exit 1; }
for p in render-spike wr-game; do
  if pgrep -qx "$p"; then echo "$p is running; not launching the game" >&2; exit 1; fi
done

case "$preset" in
  oracle)
    steps='["load","settle:10","mem:after-load","identity","dump","nav","route","reload","settle:10","route","mem:end","quit"]'
    route='{"mode":"shots","settle":5,"fow":"off","hideUi":true}'
    deadline_s=900 ;;
  check)
    steps='["load","settle:5","identity","dump","nav","save","quit"]'
    route='{"mode":"shots"}'
    deadline_s=600 ;;
  shots)
    steps='["load","settle:10","identity","route","quit"]'
    route='{"mode":"shots","settle":5,"fow":"off","hideUi":true}'
    deadline_s=600 ;;
  bench)
    steps='["load","settle:10","mem:after-load","identity","route","mem:after-route1","reload","settle:10","mem:after-reload","route","mem:after-route2","reload","settle:5","reload","settle:10","mem:before-save","save","mem:after-save","quit"]'
    route='{"mode":"bench","dwell":37.5,"warmupFrames":15,"fow":"off","hideUi":true}'
    deadline_s=1800 ;;
  plan)
    [[ -f "$plan_file" ]] || { echo "--plan-file FILE is required for PRESET plan" >&2; exit 2; }
    steps="$(python3 -I -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["steps"]))' "$plan_file")"
    route='{"mode":"shots"}'
    deadline_s=$deadline_opt ;;
  *) echo "PRESET is oracle, bench, shots, check or plan" >&2; exit 2 ;;
esac

# One oracle run at a time: the plan, WrathPerf's settings and the game are shared.
lock="$root/builds/.oracle-run.lock" # one lock for the game and GPU (wr-game benches take it too)
mkdir "$lock" 2>/dev/null || { echo "another oracle run holds $lock (remove it if stale)" >&2; exit 1; }
settings="$build/Mods/WrathPerf/Settings.xml"
plan="$data/WrathOracle/plan.json"
[[ -e "$plan" ]] && { rmdir "$lock"; echo "$plan already exists (an unfinished run?); remove it first" >&2; exit 1; }
backup="$(mktemp)"
cp "$settings" "$backup"
marker="$(mktemp)"
restore() {
  rm -f "$plan" "$marker"
  cp "$backup" "$settings" && rm -f "$backup"
  rmdir "$lock" 2>/dev/null || true
}
trap restore EXIT

cat > "$settings" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<Settings xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <NativeDxt>$native</NativeDxt>
  <VerifyDxt>false</VerifyDxt>
  <CleanupWaitFrames>$frames</CleanupWaitFrames>
</Settings>
EOF
mkdir -p "$data/WrathOracle"
python3 -I - "$plan" "$save" "$out" "$label" "$steps" "$route" "$import_dir" "$plan_file" "$root" <<'PY'
import json, os, sys
path, save, out, label, steps, route, imp, extra, root = sys.argv[1:]
plan = {"save": save, "out": out, "label": label, "steps": json.loads(steps),
        "route": json.loads(route), "hierarchy": True, "saveSlot": "WR_Bench"}
if extra:
    for k, v in json.load(open(extra)).items():
        if k not in ("steps", "out", "label"):
            plan[k] = v
# A plan file's paths may be relative to the repository root.
if isinstance(plan.get("blueprintList"), str) and not os.path.isabs(plan["blueprintList"]):
    plan["blueprintList"] = os.path.join(root, plan["blueprintList"])
for key, name in (("inventory", "inventory.json"), ("navQueries", "nav.json")):
    if os.path.isfile(os.path.join(imp, name)):
        plan[key] = os.path.join(imp, name)
json.dump(plan, open(path, "w"))
PY

"$root/oracle/tools/launch.sh" --wait "$build/Wrath.app"
deadline=$(( $(date +%s) + deadline_s ))
# pgrep: 0 = running, 1 = exited, anything else = couldn't tell (keep waiting).
while true; do
  rc=0
  pgrep -qx 'Pathfinder Wrath Of The Righteous' || rc=$?
  (( rc == 1 )) && break
  if (( $(date +%s) > deadline )); then echo "the game is still running after ${deadline_s}s; not killing it" >&2; exit 1; fi
  sleep 2
done
if [[ -f "$plan" ]]; then echo "WrathOracle never picked up the plan" >&2; exit 1; fi
mkdir -p "$out"
session="$(find "$data/WrathStats" -mindepth 1 -maxdepth 1 -type d -newer "$marker" | sort | tail -1)"
[[ -n "$session" ]] && cp -R "$session" "$out/wrathstats"
cp "$data/GameLogFull.txt" "$out/" 2>/dev/null || true
cp "$player_log" "$out/" 2>/dev/null || true
cp "$data/general_settings.json" "$out/" 2>/dev/null || true
cp "$settings" "$out/WrathPerf-Settings.xml"
# Bench slots are deleted in game; report any that survived (none expected).
leftover="$(find "$data/Saved Games" -maxdepth 1 -name 'Manual_*_WR_*.zks' -newer "$marker" 2>/dev/null || true)"
[[ -n "$leftover" ]] && echo "WARNING: bench save slot(s) left behind: $leftover" >&2
echo "== $preset ($config) -> $out"
[[ -f "$out/run.json" ]] || { echo "no run.json: the oracle never wrote results" >&2; exit 1; }
python3 -I - "$out/run.json" "$allow_unhooked" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
allow_unhooked = int(sys.argv[2])
ok = True
print("  aborted:", r["aborted"], " errors:", r["errors"])
h = r["health"]
print(f"  health: written={h['records_written']} failed={h['records_failed']} dropped={h['records_dropped']}")
for s in r["steps"]:
    if "load" in s:
        l = s["load"]
        print(f"  {s['step']}: call->idle {l['call_to_idle_s']:.3f} s (busy {l['busy_to_idle_s']:.3f} s) area={l['area']}")
    elif "save" in s:
        v = s["save"]
        total = v.get("total_s")
        print(f"  save: total {total if total is None else round(total, 3)} s (routine {v.get('routine_end_s', -1):.3f}, commit {v.get('commit_end_s', -1):.3f}), valid={v.get('valid')} deleted={v.get('deleted')}")
        ok = ok and bool(v.get("valid")) and bool(v.get("deleted"))
# Fixture windows: each must be clean; unhooked RNG steps only up to --allow-unhooked.
# (Replay tape counters can be T1-only; oracle/tools/oracle-trace-diff classifies them.)
for fh in r.get("fixture_health", []):
    bad = {k: v for k, v in fh.items() if k not in ("name", "mode", "file", "rng_unhooked_steps") and v}
    if (fh.get("rng_unhooked_steps") or 0) > allow_unhooked:
        bad["rng_unhooked_steps"] = fh["rng_unhooked_steps"]
    print(f"  fixture {fh['mode']} {fh['name']}: " + ("clean" if not bad else f"PROBLEMS {bad}") + f", unhooked RNG steps {fh.get('rng_unhooked_steps')}")
    ok = ok and not bad
# A finished run ends with its quit step, without errors or failed/dropped records.
finished = bool(r["steps"]) and r["steps"][-1]["step"] == "quit"
ok = ok and finished and not r["aborted"] and not r["errors"] and h["records_failed"] == 0 and h["records_dropped"] == 0
if not ok:
    print(f"  INCOMPLETE OR FAILED RUN (finished={finished})", file=sys.stderr)
sys.exit(0 if ok else 1)
PY
