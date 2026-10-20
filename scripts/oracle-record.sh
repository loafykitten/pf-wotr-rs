#!/bin/bash
# scripts/oracle-record.sh LABEL SAVE PLAN [oracle-run.sh options...]: one WrathOracle plan run
# into fixtures/captures/oracle/LABEL, through a startup watchdog. The game sometimes deadlocks on
# its loading screen before WrathOracle's runner starts; if "[WrathOracle] run start: LABEL" isn't
# in Player.log within 300 s of launch, the game is killed (a sample and the log kept in
# fixtures/captures/runs-raw/startup-hang), the empty capture dropped and the run retried (3 tries). The same
# deadlock (main thread on a dispatch semaphore in UnityPlayer) can strike mid-run (poc5-tb-28, after an
# hour idle): a game under 3% CPU for 3 minutes after the run started counts as hung too.
# Waits for the game/GPU lock, refuses to start with the screen locked, and keeps the display
# awake (caffeinate) while it runs, since game runs hang on a locked screen.
# e.g. scripts/oracle-record.sh poc5-tb-22 Manual_18 oracle/tools/oracle-fixtures/poc5-tb.plan.json --deadline 3600 --allow-unhooked 5
set -u
[ $# -ge 3 ] || { sed -n '2,9p' "$0" >&2; exit 2; }
REPO="$(cd "$(dirname "$0")/.." && pwd)"
CAP=$REPO/fixtures/captures/oracle
LOCK=$REPO/builds/.oracle-run.lock
PLOG="$HOME/Library/Logs/Owlcat Games/Pathfinder Wrath Of The Righteous/Player.log"
HANGS=$REPO/fixtures/captures/runs-raw/startup-hang
cd "$REPO" || exit 2
caffeinate -dimu -w $$ &
wait_free() { until [ ! -e "$LOCK" ] && ! pgrep -qx 'Pathfinder Wrath Of The Righteous' && ! pgrep -qx wr-game; do sleep 15; done; }
locked() { ioreg -n Root -d1 -a | grep -q CGSSessionScreenIsLocked; }
attempt() { # label save plan extra...
  local label=$1 save=$2 plan=$3; shift 3
  oracle/tools/oracle-run.sh --save "$save" --label "$label" --plan-file "$plan" "$@" plan "$CAP/$label" > "$CAP/$label.log" 2>&1 &
  local orp=$! t0=0 low=0 hung=0
  while kill -0 $orp 2>/dev/null; do
    if [ $t0 -eq 0 ] && grep -q "launched pid" "$CAP/$label.log"; then t0=$(date +%s); fi
    local why=""
    if [ $t0 -gt 0 ] && ! grep -q "run start: $label" "$PLOG" 2>/dev/null && (( $(date +%s) - t0 > 300 )); then
      why="startup hang"
    elif [ $t0 -gt 0 ] && grep -q "run start: $label" "$PLOG" 2>/dev/null; then
      local gp0 cpu; gp0=$(pgrep -x 'Pathfinder Wrath Of The Righteous')
      cpu=$( [ -n "$gp0" ] && ps -o %cpu= -p "$gp0" | awk '{print int($1)}' )
      if [ -n "$cpu" ] && [ "$cpu" -lt 3 ]; then
        [ $low -eq 0 ] && low=$(date +%s)
        (( $(date +%s) - low > 180 )) && why="mid-run hang (under 3% CPU for 3 minutes)"
      else
        low=0
      fi
    fi
    if [ -n "$why" ]; then
      local gp; gp=$(pgrep -x 'Pathfinder Wrath Of The Righteous')
      echo "$label: $why (pid $gp), killing"
      mkdir -p "$HANGS"; [ -n "$gp" ] && sample "$gp" 3 -file "$HANGS/sample-$label-$(date +%H%M%S).txt" >/dev/null 2>&1
      cp "$PLOG" "$HANGS/Player-$label-$(date +%H%M%S).log"
      [ -n "$gp" ] && kill -TERM "$gp"
      t0=-1 hung=1
    fi
    sleep 5
  done
  wait $orp; local rc=$?
  # A killed run can still write run.json; it is a failed try either way.
  [ $hung -eq 0 ] && [ -f "$CAP/$label/run.json" ] && { echo "$label: exit $rc at $(date +%T)"; return 0; }
  # oracle-run.sh can give up (e.g. "timed out waiting for mods to load") with the game it launched
  # still hung: the next try would wait for it forever. Only this try's own game is killed.
  local mine; mine=$(sed -n 's/^launched pid \([0-9][0-9]*\) .*/\1/p' "$CAP/$label.log" | head -1)
  echo "$label: no run.json (exit $rc)"; rm -rf "${CAP:?}/${label:?}"; mv "$CAP/$label.log" "$CAP/$label.failed-$(date +%H%M%S).log"
  if [ -n "$mine" ] && [ "$(ps -o comm= -p "$mine" 2>/dev/null)" != "" ] && ps -o command= -p "$mine" | grep -q arm64-oracle; then
    echo "$label: the failed try's game (pid $mine) is still running, killing"; kill -TERM "$mine"
  fi
  return 1
}
# Each replay's start state against its recording's (oracle-state-diff): a replay's window can open with a
# unit ticked one real-time frame more or less before it (docs/poc5/tb-replay.md, "a replay's window can start
# one frame apart"), which nothing else reports. One line per replay, also in the capture's start-states.txt.
start_states() {
  local out="$CAP/$1/start-states.txt" f t r s n d line
  : >"$out"
  for f in "$CAP/$1"/fixtures/*/; do
    # (Every replay the fixture has a trace of; a missing dump is a failure.)
    for t in "$f"replay-*.trace.jsonl.gz; do
      [ -e "$t" ] || continue
      r=$(basename "$t" .trace.jsonl.gz)
      s="${f}state-start-$r.json"
      n="$(basename "$f") $r"
      if [ ! -e "$s" ] || [ ! -e "${f}state-start-record.json" ]; then
        echo "$n: start-state comparison FAILED: a start-state dump is missing" | tee -a "$out"
        continue
      fi
      # (Exit 1 is "not equal"; its JSON says why. No JSON, or an invalid comparison, is a failure.)
      d=$("$REPO/oracle/tools/oracle-state-diff" "${f}state-start-record.json" "$s" --max 4 2>&1 || true)
      line=$([ -n "$d" ] && jq -er --arg n "$n" '
        if .valid != true then "\($n): start-state comparison FAILED: \(.problems | tostring)"
        elif .equal then "\($n): start state equal"
        else "\($n): start state differs (\(.differences)): \(.first | join("; "))" end' <<<"$d" 2>/dev/null) ||
        line="$n: start-state comparison FAILED: $(head -c 300 <<<"$d" | tr '\n' ' ')"
      echo "$line" | tee -a "$out"
    done
  done
}
label=$1
for try in 1 2 3; do
  wait_free
  if locked; then echo "$label: screen locked, not starting"; exit 1; fi
  echo "$label: try $try start $(date +%T)"
  attempt "$@" && { start_states "$label"; exit 0; }
  sleep 20
done
echo "$label: gave up"; exit 1
