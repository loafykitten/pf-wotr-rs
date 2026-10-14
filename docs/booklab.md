# booklab: the second Mac

Set up 2026-10-12 (tenth session). Ember's second Mac, dedicated to this project: it runs the game for correctness
work in parallel with the main Mac (Ember's Mac Studio, M2 Max), so recordings and GPU work no longer queue behind each
other on one game/GPU lock.

## The machine

- `ssh booklab` (an entry in `~/.ssh/config` on the main Mac, with its own key). LAN copies
  run at about 70 MB/s.
- Apple M1 (8 CPU cores, 8-core GPU, Metal 4), 16 GB, macOS 27.0, built-in 2560×1600 display, ~750 GB free.
- Ember keeps it logged in (console session), awake (a caffeine app) and plugged in. Xcode 27 (`metal`, `xctrace`),
  Homebrew, system `python3`, `jq`, `rsync`. No Rust or .NET: build on the main Mac (same arm64 macOS 27) and copy.
- Steam is installed (it needed Rosetta), logged in and running, as on the main Mac; the game connects to it.
  `~/.zshenv` there puts Homebrew on `PATH` (ssh commands read it).

**Role.** Correctness only: oracle recordings, and POC-6's render comparisons against the original (screenshots,
Metal frame captures of the original), each against the original *on booklab*. **Never timing:** the performance targets
and benchmarks belong to the main Mac, which is what Ember plays on (Ember, 2026-10-12: the M1 isn't a target).

## What's there

- `~/Developer/Wrath-Rust`: a git checkout that accepts pushes into its working tree
  (`receive.denyCurrentBranch updateInstead`). From the main Mac: `git push booklab poc main` (remote `booklab`).
  Don't commit on booklab; bring results back with `rsync`.
- `builds/arm64-oracle`: an `rsync` copy of the main Mac's (48 GB, with the four oracle mods). After rebuilding the
  oracle mods on the main Mac: `rsync -a builds/arm64-oracle/Mods/ booklab:Developer/Wrath-Rust/builds/arm64-oracle/Mods/`
  (with the game closed on booklab).
- `cache/import` and `fixtures/saves` (copied; gitignored), and the game's user data: `~/Library/Application
  Support/Owlcat Games/Pathfinder Wrath Of The Righteous` (saves, `general_settings.json`) and the Unity preferences
  (`defaults export` / `import` of `unity.Owlcat Games.Pathfinder Wrath Of The Righteous`).
- macOS asked a few permission questions on the game's first launch; Ember allowed them.

## Running things

- Steam must be running, as on the main Mac (`oracle/tools/launch.sh` checks; `WR_NO_STEAM=1` skips the check, and
  the game then runs on after `SteamAPI_Init()` fails, but nothing is certified that way). Launched from ssh, the game
  opens on booklab's display. A load takes about 10.6 s there (4.7 s on the M2 Max).
- Recordings: `ssh booklab 'cd ~/Developer/Wrath-Rust && scripts/oracle-record.sh LABEL SAVE PLAN [options]'`, then
  `rsync -a booklab:Developer/Wrath-Rust/fixtures/captures/oracle/LABEL fixtures/captures/oracle/`. Give booklab's
  captures a `booklab-` prefix so labels never collide; each machine has its own `builds/.oracle-run.lock`.
- Binaries: `cargo build --release` on the main Mac, then copy `target/release/<bin>` to booklab.
- Screenshots of the desktop over ssh fail (ssh has no Screen Recording permission); the game's own captures, our
  renderer's read-backs and Metal frame captures don't need it. Ask Ember before anything needs it.

## Certification

**Certified for fights (2026-10-13): `booklab-tb-1`** (`poc5-tb.plan.json` on Manual_18, with Steam running): both
fights, record and two replays, every replay T0-equal to its record tick for tick (`oracle-trace-diff --t0`,
`t0_max_tick_shift` 0), and the sim validates all six runs (T0-equal, end state 1.0000, nothing reported; 1,001 and
1,764 turn controller ticks, 6 and 10 turn ends computed). A copy is on the main Mac
(`fixtures/captures/oracle/booklab-tb-1`). Its fights differ from the main Mac's captures, as every capture's do.

**Known issue: a reload right after a dialogue fixture stalls on booklab.** `certification.plan.json` (its first
fixture is `dialog-checks`) records that fixture normally (498 ticks, as `cert-20`), then the next `reload` never
starts loading: `Game.LoadGame` is called and the game stops requesting frames (CPU under 1%, every thread idle, the
main thread in `NSApplication`'s event loop, the display link waiting). Ruled out: Steam (stalls with it running),
the screen resolution (stalls at booklab's native one too; the game's surface is 2880×1800 either way), focus (the
game is frontmost), display sleep (held awake), memory (no swap, 82% free); Wwise's `CAkOutputMgr::SuspendedThread`
is there at the main menu too. Plain reloads work (`load` then three `reload`s), and so does a reload after the `dice`
fixture: it takes a dialogue first. Samples: `/tmp/reload-hang2.txt` on booklab (until a reboot), and the watchdog's in
`fixtures/captures/runs-raw/startup-hang/` there. **Until it's fixed, run dialogue plans (POC-4,
`certification.plan.json`) on the main Mac.** Next to try: a dialogue fixture without the voiced lines, a dialogue
that's closed before the reload, the main Mac's `Player.log` against booklab's around the dialogue's end.
