#!/usr/bin/env bash
# Launch a Wrath build from builds/ outside Steam's Play button (Steam must be running, unless
# WR_NO_STEAM=1: the game logs SteamAPI_Init's failure and runs on, as on booklab, the second Mac).
# usage: oracle/tools/launch.sh [--wait] [App.app]   (default: builds/arm64-oracle/Wrath.app)
#   --wait  block until this session's Player.log reports UMM finished loading mods
# UMM loads mods from the Mods/ folder next to the .app, so each build gets its own folder.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)" # the repository root
wait_for_mods=false
if [[ "${1:-}" == "--wait" ]]; then wait_for_mods=true; shift; fi
app="${1:-$root/builds/arm64-oracle/Wrath.app}"
[[ -d "$app" ]] || { echo "no app at $app" >&2; exit 1; }
app="$(cd "$app" && pwd)" # absolute: we cd into the bundle before exec'ing it
exe="$app/Contents/MacOS/Pathfinder Wrath Of The Righteous"
log="$HOME/Library/Logs/Owlcat Games/Pathfinder Wrath Of The Righteous/Player.log"
appid=1184370

[[ -x "$exe" ]] || { echo "no executable at $exe" >&2; exit 1; }
# pgrep: 0 = found, 1 = none, anything else = couldn't list processes.
running() {
  local rc=0
  pgrep -q "$@" || rc=$?
  case $rc in
    0) return 0 ;;
    1) return 1 ;;
    *) echo "pgrep failed (exit $rc); can't tell what's running" >&2; exit 1 ;;
  esac
}
if [[ "${WR_NO_STEAM:-}" != 1 ]]; then
  running steam_osx || { echo "Steam is not running; start it first (or WR_NO_STEAM=1)" >&2; exit 1; }
fi
# Match the game's process name exactly: -f would also match any shell or agent whose command
# line merely mentions the game's folder.
if running -x 'Pathfinder Wrath Of The Righteous'; then
  echo "a Wrath process is already running" >&2; exit 1
fi

# steam_api reads the app ID from steam_appid.txt in the working directory, or from
# SteamAppId; set both.
printf '%s\n' "$appid" > "$app/Contents/MacOS/steam_appid.txt"
# Always pass -logFile: the arm64 player (Unity 2020.3.48 macos_arm64 template) exits
# immediately when left to pick its default log path. Rotate the old log ourselves, as
# Unity would, so --wait can tell this session's log from the last one.
mkdir -p "$(dirname "$log")"
[[ -f "$log" ]] && mv -f "$log" "${log%.log}-prev.log"
started=$(date +%s)
cd "$app/Contents/MacOS"
SteamAppId=$appid SteamGameId=$appid nohup "$exe" -logFile "$log" </dev/null >/dev/null 2>&1 &
pid=$!
disown "$pid"
echo "launched pid $pid from $app"

$wait_for_mods || exit 0
# Unity rotates the previous session's log to Player-prev.log shortly after start, so
# only trust a Player.log written after this launch.
for _ in $(seq 1 120); do
  kill -0 "$pid" 2>/dev/null || { echo "game exited during startup" >&2; exit 1; }
  if [[ -f "$log" ]] && (( $(stat -f %B "$log") >= started )) && grep -q 'LOADED [0-9]*/[0-9]* MODS' "$log"; then
    grep -o 'LOADED [0-9]*/[0-9]* MODS' "$log"
    exit 0
  fi
  sleep 1
done
echo "timed out waiting for mods to load" >&2
exit 1
