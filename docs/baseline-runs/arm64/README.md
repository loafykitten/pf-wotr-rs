# Native arm64: Deskari cutscene benchmark

Phase 4 results for `builds/arm64` (the game's data and managed code on Unity 2020.3.48f1's
native arm64 Mono player, Harmony 2.4.2, the Steam compatibility shim), compared with the
Rosetta baseline in [`../rosetta-baseline/`](../rosetta-baseline/README.md). Same machine,
mods and protocol. WrathStats 0.2.2 reports `process_arch: arm64`, `proc_translated: 0`.
Recorded 2026-10-10 by Ember.

| Run | Summary | Session |
|---|---|---|
| 1, cold shader cache | [run1-cold.md](run1-cold.md) | `runs/raw/20261010T091757Z` |
| 2 | [run2.md](run2.md) | `runs/raw/20261010T092710Z` |
| 3 | [run3.md](run3.md) | `runs/raw/20261010T093334Z` |

Comparisons: [cold vs cold](compare-rosetta-cold-vs-arm64-cold.md),
[Rosetta run 2 vs arm64 run 2](compare-rosetta-run2-vs-arm64-run2.md).

## Results

Hitch = an in-game frame ≥ 50 ms. Baselines are runs 2–3 of each build (normal launches).

| | Rosetta | **Native arm64** | Change |
|---|---|---|---|
| Pass 1 hitches (first play in a process) | 36–37 | **13–15** | about −60% |
| Pass 1 worst frame | ~520 ms | **~130 ms** | −75% |
| Pass 2 hitches (replay) | 9–10 | **5** | about −50% |
| Hitches with a GC in the frame | 12 | 12–13 | unchanged |
| Cold shader cache, pass 1 hitches | 98 | 71 | −28% |
| Cold shader cache, pass 2 hitches | 25 | 3 | −88% |
| Load save, first in process | ~18 s | **~12.2 s** | −32% |
| Load save, later | 12.6 s | **~11.8 s** | −6% |
| Area transition | 11.6 s | **8.7 s** | −25% |
| Manual save | 0.46 s | **0.33 s** | −28% |
| Median frame (all in-game) | 16.7 ms (60 fps cap) | 13.0 ms (~71–77 fps) | see note |
| p99 frame (all in-game) | 34 ms | **22 ms** | −35% |

**Frame-rate note:** the native player sees the display as 2560x1440 at **240 Hz**
(Ember's monitor is high refresh); the Rosetta player reported 60 Hz. With vsync on, the
Rosetta build was locked to 60 fps while the native build runs uncapped at about 71–77
fps. Frame-time percentiles are therefore not directly comparable, though the native
p99 is still far lower. Hitch counts and load times are.

## What it means

- **The native build is clearly better on everything measured.** First-play stutter
  (about −60%), the worst spikes (−75%), loads (−6 to −32%), saves (−28%), and a higher
  frame rate. This matches Ember's impression: "the 60fps feels like a smoother 60",
  "almost hitchless" on the replay, loads and saves "slightly quicker".
- **Rosetta was the "in-process warm-up" cost.** Under Rosetta every newly compiled code
  path (Mono JIT output) also had to be translated; native removes that, which is why
  first-play hitches and first-in-process loads improved most.
- **What remains is mostly GC.** About 12 GC-coincident hitches per run under both builds,
  now 65–67% of the remaining hitches. Unity's incremental GC is off in this build,
  which is the obvious Phase 7 target. Metal shader compilation on a cold cache is the
  other remaining one-time cost (macOS caches it afterwards).
- **Reproducibility:** native runs 2 and 3 agree closely (13 vs 15, and 5 vs 5 hitches;
  loads within 0.5 s).

## Notes

- In run 1, pass 1 also included the follow-up dialogue and a trip to Prologue_Caves_1
  before reloading (no save). That run's passes table shows it as an extra pass.
- The native player's quit path force-kills the process (`Game.HandleQuit`'s "Force
  quit" thread) before WrathStats 0.2.2 finished writing, so these three sessions lack
  the session-end note and up to about 1 s of data after the final save. WrathStats
  0.2.3 ends recording in a `Game.HandleQuit` prefix (verified). Nothing measured above is
  affected.
- Run 1 started with macOS's Metal cache cleared again (`tools/metal-cache.sh clear`;
  stash `builds/metal-cache-stash/20261010T091745Z`).
