# Native arm64 + incremental GC: Deskari cutscene benchmark

Phase 7, patch 1. The same native build as [`../arm64/`](../arm64/README.md), with one
change: `gc-max-time-slice=3` in `Data/boot.config`, which the Unity 2020.3 player turns
into Mono's incremental GC. WrathStats 0.2.4 confirms `GarbageCollector.isIncremental =
true` with a 3 ms slice. Three OS-warm runs (no shader-cache clearing), same protocol.
Recorded 2026-10-10 by Ember.

| Run | Summary | Session |
|---|---|---|
| 1 | [run1.md](run1.md) | `runs/raw/20261010T095851Z` |
| 2 | [run2.md](run2.md) | `runs/raw/20261010T100536Z` |
| 3 | [run3.md](run3.md) | `runs/raw/20261010T101208Z` |

Comparison: [native run 2 vs incremental-GC run 2](compare-arm64-run2-vs-igc-run2.md).

## Results

| | Native (arm64 runs 2–3) | **+ incremental GC (runs 1–3)** |
|---|---|---|
| Hitches per run (≥ 50 ms) | 18–20 | **8, 8, 8** |
| of which with a GC in the frame | 12–13 | **0, 0, 0** |
| Pass 1 / pass 2 hitches | 13–15 / 5 | 5–6 / 2–3 |
| Cutscene hitches per minute | 3.5–3.8 | 0.6–1.2 |
| All in-game fps / p50 / p99 | 71.3–71.6 / 13.0 / 22 ms | 71.7–72.0 / 13.0–13.2 / 22 ms |
| Load save, first / later (runs 2–3) | 12.2–12.3 / 11.5–12.0 s | 12.4–13.0 / 11.6–11.7 s |
| Area transition | 8.7–8.8 s | 8.8 s (run 1: 10.5 s) |
| Manual save | 0.33 s | 0.34 s (run 1: 0.53 s) |

## What it means

- **Incremental GC removes every GC-coincident hitch** and halves the total, with no
  measurable cost to steady frame rate or frame-time percentiles.
- **Loads and saves are unchanged** within noise in runs 2 and 3; at most the first save
  load in a process is 0.2–0.8 s slower. Run 1 was slower across all loads and saves (+1.5–1.8 s);
  run 3 confirmed that was noise, likely other activity on the Mac at the time.
- It's now **on by default** in `tools/build-arm64-bundle.sh` (`--no-incremental-gc` turns
  it off).
- **What's left:** about 8 hitches per run, 5–6 of them on first play (warm-up: Metal
  pipeline creation, asset loads, JIT) and 2–3 on replay. Under 1% of frames exceed 33 ms.
