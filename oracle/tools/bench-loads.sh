#!/usr/bin/env bash
# Load benchmark on the oracle build: set WrathPerf's settings, have WrathBench load a save
# from the main menu and reload it N times, wait for the game to quit, then keep the
# WrathStats session and the game's logs under fixtures/captures/runs-raw/loads/<label>/.
# usage: oracle/tools/bench-loads.sh [options] LABEL
#   --reloads N        reloads after the first load (default 3)
#   --native on|off    WrathPerf native DXT compressor (default on)
#   --verify on|off    also check every compression against the game's (default off)
#   --wait-frames N    frames before each cleanup pass; 100 is the game's (default 10)
#   --save NAME        save name prefix (default Manual_14_WS_Bench_Deskari)
#   --shots            screenshot at the end of each settle (portraits, equipment)
# Steam must be running. Takes about 30 s plus 20 s per load.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)" # the repository root
build="$root/builds/arm64-oracle"
data="$HOME/Library/Application Support/Owlcat Games/Pathfinder Wrath Of The Righteous"
player_log="$HOME/Library/Logs/Owlcat Games/Pathfinder Wrath Of The Righteous/Player.log"
reloads=3 native=true verify=false frames=10 save=Manual_14_WS_Bench_Deskari shots=false

onoff() {
  case "$1" in
    on) echo true ;;
    off) echo false ;;
    *) echo "expected on or off, got '$1'" >&2; exit 2 ;;
  esac
}
while [[ $# -gt 1 ]]; do
  if [[ "$1" == --shots ]]; then shots=true; shift; continue; fi
  case "$1" in
    --reloads) reloads="$2" ;;
    --native) native="$(onoff "$2")" ;;
    --verify) verify="$(onoff "$2")" ;;
    --wait-frames) frames="$2" ;;
    --save) save="$2" ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift 2
done
[[ $# -eq 1 ]] || { sed -n '2,12p' "$0" >&2; exit 2; }
label="$1"
[[ "$reloads" =~ ^[0-9]+$ && "$frames" =~ ^[0-9]+$ ]] || { echo "--reloads and --wait-frames take numbers" >&2; exit 2; }
out="$root/fixtures/captures/runs-raw/loads/$label"
[[ -e "$out" ]] && { echo "$out already exists" >&2; exit 1; }
[[ -d "$build/Mods/WrathPerf" && -d "$build/Mods/WrathBench" ]] || { echo "WrathPerf and WrathBench must be installed in $build/Mods" >&2; exit 1; }

cat > "$build/Mods/WrathPerf/Settings.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<Settings xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <NativeDxt>$native</NativeDxt>
  <VerifyDxt>$verify</VerifyDxt>
  <CleanupWaitFrames>$frames</CleanupWaitFrames>
</Settings>
EOF
plan="$data/WrathBench/plan.json"
[[ -e "$plan" ]] && { echo "$plan already exists (an unfinished run?); remove it first" >&2; exit 1; }
mkdir -p "$data/WrathBench"
rm -rf "$data/WrathBench/shots"
marker="$(mktemp)"
# If the game never picks the plan up, don't leave it for the next normal launch.
trap 'rm -f "$marker" "$plan"' EXIT
printf '{"save":"%s","reloads":%d,"settle":8,"quit":true,"screenshots":%s}\n' "$save" "$reloads" "$shots" > "$plan"

"$root/oracle/tools/launch.sh" --wait "$build/Wrath.app"
# 30 s to the menu, then about 20 s per load; allow plenty.
deadline=$(( $(date +%s) + 120 + 60 * (reloads + 1) ))
while pgrep -qx 'Pathfinder Wrath Of The Righteous'; do
  if (( $(date +%s) > deadline )); then echo "the game is still running after the deadline; not killing it" >&2; exit 1; fi
  sleep 2
done
if [[ -f "$plan" ]]; then echo "WrathBench never picked up the plan" >&2; exit 1; fi

session="$(find "$data/WrathStats" -mindepth 1 -maxdepth 1 -type d -newer "$marker" | sort | tail -1)"
[[ -n "$session" ]] || { echo "no new WrathStats session" >&2; exit 1; }
mkdir -p "$out"
cp -R "$session" "$out/session"
cp "$data/GameLogFull.txt" "$out/"
cp "$player_log" "$out/"
cp "$build/Mods/WrathPerf/Settings.xml" "$out/WrathPerf-Settings.xml"
if [[ -d "$data/WrathBench/shots" ]]; then mv "$data/WrathBench/shots" "$out/shots"; fi

echo "== $label: native=$native verify=$verify wait-frames=$frames reloads=$reloads"
python3 -I - "$out" <<'PY'
import json, re, sys
out = sys.argv[1]
for line in open(f"{out}/session/events.jsonl"):
    e = json.loads(line)
    if e["kind"] == "load":
        print(f"  load {e['ms'] / 1000:6.2f} s  ({e['reason']}, {e['frames']} frames)")
    elif e["kind"] == "note" and ("bench:" in e["text"] or "wrathperf" in e["text"]):
        print("  " + e["text"])
stages = {}
for m in re.finditer(r"Loading process finished '<?(\w+)>?[^']*' duration: ([\d.]+)s", open(f"{out}/GameLogFull.txt", errors="replace").read()):
    stages.setdefault(m.group(1), []).append(float(m.group(2)))
for name in ("UnloadUnusedAssetsCoroutine", "LoadAreaCoroutine", "AwaitTextureCompression"):
    if name in stages:
        print(f"  {name}: " + " ".join(f"{v:.2f}" for v in stages[name]))
PY
echo "kept in $out"
# A crash or manual quit mid-run still leaves a session; only a finished run counts.
python3 -I - "$out/session/events.jsonl" "$reloads" <<'PY' || { echo "INCOMPLETE run (kept for diagnosis)" >&2; exit 1; }
import json, sys
events = [json.loads(l) for l in open(sys.argv[1])]
loads = sum(e["kind"] == "load" and e.get("reason") == "load_save" for e in events)
done = any(e["kind"] == "note" and e["text"] == "bench: bench done" for e in events)
sys.exit(0 if done and loads == int(sys.argv[2]) + 1 else 1)
PY
