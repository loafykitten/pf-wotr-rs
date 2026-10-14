# POC-0 baseline: the original game on Wrath-Rust's matched workload

For Wrath-Rust POC-0 (`~/Developer/Wrath-Rust/docs/poc0/baseline.md` has the full
definition and the reading). Native arm64 build with incremental GC and all of Ember's
mods, plus WrathOracle 0.1.0 driving the run. Automated with
`tools/oracle-bench-batch.sh` (which runs `tools/oracle-run.sh --config game|best bench`
under a contention policy); recorded 2026-10-10/11 by Claude.
Raw runs (oracle outputs, WrathStats sessions, logs) are in `runs/raw/baseline/<label>/`.

**The workload:** Ember's `Manual_13_Underground_Caves_of_Kenabres` save, loaded from the
main menu; the `Caves_1` Arbiter instruction's eight camera points with a 37.5 s dwell
each (5 min, live game, UI hidden, fog of war off), twice (after the first load and after
a reload); three warm reloads in all; a timed manual save into a `WR_Bench` slot that is
deleted again. Ember's settings: 2560×1440 full-screen window, VSync on at 240 Hz, High.

**Configurations:** `game` = WrathPerf native DXT off, cleanup wait 100 frames (the game's
own behaviour); `best` = native DXT on, 10 frames (as in `soak-both`).

| Run (`runs/raw/baseline/`) | Summary |
|---|---|
| `game-1`, `game-2`, `game-3` (OS-warm) | [game.md](game.md) |
| `best-1`, `best-2`, `best-3` (OS-warm) | [best.md](best.md) |
| `game-oscold`, `best-oscold` (Metal shader cache cleared first, restored after) | [oscold.md](oscold.md) |
| `contended/` | runs discarded because another process (mostly other agents' `rustc` builds) used CPU during the run; kept for reference only |

## Results (median, min–max over 3 runs)

| | game | best |
|---|---|---|
| First load in a fresh process | 10.27 s (9.94–10.80) | **6.00 s** (5.95–6.20) |
| Warm reload (9 per config) | 9.07 s (8.86–9.32) | **5.00 s** (4.70–5.20) |
| OS-cold first load (1 run) | 12.24 s | 7.77 s |
| Route fps, p50 / p95 / p99 | 80.7, 12.45 / 17.0 / 21.1 ms | the same |
| Route max frame (pass 1 / pass 2) | 27 (26–42) / 22 (22–23) ms | 26 (23–26) / 27 (27–28) ms |
| Route frames ≥ 33 / ≥ 50 ms (10 min per run) | 0–1 / 0 | 0 / 0 |
| Shader cache cleared, route pass 1 frames ≥ 50 ms (1 run) | 6 (max 355 ms) | 7 (max 371 ms) |
| Manual save, call to save routine finished | 0.465 s (0.456–0.473) | 0.469 s (0.460–0.490) |
| … `.zks` completely written | 0.430 s (0.423–0.440) | 0.433 s (0.427–0.450) |
| Frames during the save ≥ 50 ms / longest | 3 of 9 / 239–253 ms | 3 of 9 / 240–248 ms |
| Resident after first load / end | 3.50 / 3.65 GB | 3.50 / 3.64 GB |
| Mono heap after first load / end | 0.46–0.47 / 0.49–0.50 GB | the same |

## What it means

- **Frame pacing is identical in both configurations and very steady:** frame intervals
  cluster at multiples of 4.17 ms (VSync at 240 Hz, inferred): p50 at three steps
  (12.5 ms, 80 fps), p99 at five (21 ms). With a warm shader cache the 10 minutes of route
  per run have at most one frame ≥ 33 ms. The remaining stalls come after a shader-cache
  clear (consistent with shader compilation, not instrumented) and during the save (three
  frames ≥ 50 ms, one ~250 ms).
- **WrathPerf's load gains hold on this area:** 4 s off every load (−45%), the same
  margin as on the Deskari save.
- **Contention matters a lot:** runs with other agents compiling Rust at the same time
  showed hundreds of hitches and 30 s reloads. `contended/` holds them; none count.
- The original nearly meets the Wrath-Rust target "zero hitches ≥ 33 ms on the route
  after load" when its shader cache is warm (0–1 per run). The rewrite has to match that from a cold
  start to be better.
