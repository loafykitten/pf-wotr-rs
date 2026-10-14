# Rosetta baseline: Deskari cutscene benchmark

Phase 3 baseline for the stock x86_64 player running under Rosetta
(`builds/x64-modded`, game 2.7.0y.782, Unity 2020.3.48f1, Apple M2 Max, macOS 27.0,
2560x1440 at 60 Hz, vsync on, quality "High"). Mods: ToyBox 1.7.27, Buff It 2 The Limit
1.21.3 (with the Mac shader bundle), WrathStats 0.2.1. Recorded 2026-10-10 by Ember,
following the protocol in `PLAN.md` (Phase 3).

Each run is a fresh launch:
- load the bench save "WS Bench Deskari", then 20 s idle;
- grab the drink, which triggers the Deskari cutscene (**pass 1**: first play in this
  process);
- reload, 20 s idle, play it again through the follow-up dialogue and short cutscene
  (**pass 2**: warm replay);
- the area transition to Prologue_Caves_1, a save, quit.

**Run 1 started with macOS's Metal shader cache cleared** (`tools/metal-cache.sh clear`);
runs 2 and 3 kept it.

| Run | Summary | Session |
|---|---|---|
| 1, cold shader cache | [run1-cold.md](run1-cold.md) | `runs/raw/20261010T081726Z` |
| 2 | [run2.md](run2.md) | `runs/raw/20261010T082613Z` |
| 3 | [run3.md](run3.md) | `runs/raw/20261010T083356Z` |

Comparisons: [run 1 vs run 2](compare-run1cold-run2.md), [run 2 vs run 3](compare-run2-run3.md).

## Results

Hitch = an in-game frame ≥ 50 ms.

| | Run 1 (cold cache) | Run 2 | Run 3 | **Baseline (runs 2–3)** |
|---|---|---|---|---|
| Pass 1 hitches (in cutscenes) | 98 (85) | 37 (26) | 36 (25) | **36–37** |
| Pass 1 p99 / max (ms) | 68 / 1,393 | 36 / 516 | 36 / 521 | **36 / ~520** |
| Pass 2 hitches (in cutscenes) | 25 (20) | 10 (7) | 9 (5) | **9–10** |
| Pass 2 p99 / max (ms) | 33 / 451 | 33 / 122 | 34 / 230 | **33 / 120–230** |
| Hitches with a GC in the frame | 11 | 12 | 12 | **12 per run** |
| Cutscene frames: p95 / p99 / p99.9 (ms) | 30 / 45 / 344 | 29 / 35 / 111 | 26 / 34 / 100 | **~28 / 34 / ~105** |
| All in-game: fps / p50 (ms) | 55.0 / 16.7 | 57.0 / 16.7 | 57.0 / 16.7 | **57 / 16.7** |
| Load save, first in process (s) | 23.3 | 17.9 | 18.3 | **~18** |
| Load save, second (s) | 14.8 | 12.6 | 12.7 | **12.6** |
| Area transition → Prologue_Caves_1 (s) | 13.9 | 11.6 | 11.7 | **11.6** |
| Manual save (s) | 0.68 | 0.46 | 0.46 | **0.46** |
| WrathStats' own cost (µs/frame, mean) | 7.9 | 7.5 | 7.6 | |

## What it means

- **Steady state is fine:** a vsync-locked 60 fps (p50 16.7 ms) everywhere, including
  cutscenes. The problem is spikes, not throughput.
- **Most stutter is a first-time cost:**
  - **Metal shader compilation is the biggest share.** With macOS's shader cache
    empty, pass 1 had 98 hitches (worst frame 1.4 s); with it kept, 36–37. macOS
    persists compiled shaders per app, so this part is paid once per new effect.
    That's what Ember felt in the first playthrough.
  - **In-process warm-up is the second share.** Even with the OS cache warm, a fresh
    process hitches about 4x more on first play (36–37) than on an immediate replay
    (9–10): asset loading, Unity creating GPU objects, and Mono JIT compiling, whose
    x86 output Rosetta then has to translate. The arm64 player (Phase 4) removes the
    Rosetta part of this, which is the main thing to look for in its numbers.
- **GC is a separate, constant source:** about 12 GC-coincident hitches per run,
  cold or warm (25% of hitches once the warm-up ones are gone). Unity's incremental GC
  is off in this build, which is the Phase 7 lead.
- **Loads:** the first save load in a process costs about 5 s more than later ones
  (18 vs 12.6 s), and a cold shader cache adds about 5 s more. Saves take about 0.5 s.
- **Reproducibility:** runs 2 and 3 agree within a few percent on every metric, so a
  difference of more than about 10% between builds is real.

## Notes

- Pass 2 runs a little longer than pass 1 (2.5–3.3 vs 2.2–2.5 in-game minutes). It
  includes the follow-up dialogue and a short cutscene that must finish before saving
  is allowed. Compare pass to pass across runs, not pass 1 against pass 2 within a run.
- The original Metal cache (from the first playthrough) is stashed in
  `builds/metal-cache-stash/20261010T081708Z/`. The live cache was rebuilt by run 1 and
  now covers the bench content. Restoring is unnecessary.
- Next: repeat this protocol unchanged on the arm64 build (Phase 4), in
  `runs/arm64/`, and compare with `tools/summarize-run --compare`.
