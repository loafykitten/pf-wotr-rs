# WrathStats run 20261010T095851Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.4
- **GC:** Enabled, incremental=True

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 4.8 | 72.0 | 13.1 | 19.1 | 21.9 | 35.6 | 130.8 | 0.13 | 0.04 | 1.66 |
| cutscenes | 3.5 | 71.3 | 13.4 | 19.3 | 21.9 | 33.6 | 103.1 | 0.10 | 0.01 | 0.57 |
| dialogs | 0.1 | 76.3 | 12.7 | 18.2 | 21.8 | 38.4 | 38.4 | 0.19 | 0.00 | 0.00 |
| exploring (real-time, no combat) | 1.2 | 73.7 | 12.7 | 18.0 | 21.9 | 60.7 | 130.8 | 0.20 | 0.11 | 4.92 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 78.8 | load_save | Prologue_Kenabres | 2.0 | 1.4 | 6 | 1 | 19.2 | 21.9 | 130.8 |
| 2 | 209.4 | load_save | Prologue_Kenabres | 2.5 | 2.0 | 2 | 1 | 19.5 | 22.0 | 89.9 |
| 3 | 370.8 | area_transition | Prologue_Caves_1 | 0.4 | 0.1 | 0 | 0 | 14.5 | 17.0 | 30.6 |

## Hitches (in-game frames ≥ 50 ms): 8

- With a GC in the same frame: **0 (0%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 5 (62%)
- During another mod's reported work (WrathStatsApi): 0
- Median 72.8 ms; worst: 130.8 ms @ 78.8 s, 103.1 ms @ 132.7 s, 98.0 ms @ 79.0 s, 89.9 ms @ 264.6 s, 72.8 ms @ 209.4 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Default | no combat | 5 | 72.8 | 130.8 | 0 |
| Prologue_Kenabres | Cutscene | no combat | 2 | 89.9 | 103.1 | 0 |
| Prologue_Kenabres | EscMode | no combat | 1 | 60.7 | 60.7 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 64.9 | load_save | Prologue_Kenabres | 13.73 | 842 | 1,345.2 | 4 | None,Load |
| 196.0 | load_save | Prologue_Kenabres | 13.37 | 598 | 1,278.7 | 3 | None,Load |
| 360.3 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 10.49 | 579 | 1,302.1 | 2 | None,TeleportParty,Load |
| 376.5 | save (auto) | Prologue_Caves_1 | 0.63 | 10 | 290.6 | 1 | Save |
| 388.4 | save (manual) | Prologue_Caves_1 | 0.53 | 9 | 276.5 | 1 | Save |

Median by kind: area_transition 10.49 s (n=1), load_save 13.37 s (n=2), save 0.53 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,491 → 4,507 (max 4,828)
- mono heap: 1,285 → 1,285 (max 1,285)
- mono used: 1,027 → 1,046 (max 1,212)
- unity reserved: 1,498 → 1,586 (max 1,620)
- GC collections per minute: 0.63
