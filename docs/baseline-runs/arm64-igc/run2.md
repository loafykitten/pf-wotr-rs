# WrathStats run 20261010T100536Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.4
- **GC:** Enabled, incremental=True

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 4.8 | 71.7 | 13.2 | 19.0 | 22.0 | 36.5 | 147.2 | 0.15 | 0.04 | 1.67 |
| cutscenes | 3.5 | 71.2 | 13.4 | 19.2 | 21.9 | 34.9 | 109.1 | 0.14 | 0.02 | 0.87 |
| dialogs | 0.1 | 76.9 | 12.8 | 18.2 | 21.7 | 35.0 | 35.0 | 0.22 | 0.00 | 0.00 |
| exploring (real-time, no combat) | 1.2 | 72.6 | 12.7 | 18.2 | 22.1 | 38.1 | 147.2 | 0.17 | 0.09 | 4.03 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 31.2 | load_save | Prologue_Kenabres | 2.0 | 1.4 | 5 | 2 | 19.0 | 22.2 | 147.2 |
| 2 | 161.6 | load_save | Prologue_Kenabres | 2.6 | 2.0 | 3 | 1 | 19.5 | 21.9 | 92.8 |
| 3 | 325.0 | area_transition | Prologue_Caves_1 | 0.2 | 0.0 | 0 | 0 | 15.0 | 17.1 | 36.2 |

## Hitches (in-game frames ≥ 50 ms): 8

- With a GC in the same frame: **0 (0%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 4 (50%)
- During another mod's reported work (WrathStatsApi): 0
- Median 64.5 ms; worst: 147.2 ms @ 31.2 s, 109.1 ms @ 87.9 s, 92.8 ms @ 221.2 s, 74.6 ms @ 31.2 s, 64.5 ms @ 161.6 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Default | no combat | 4 | 64.5 | 147.2 | 0 |
| Prologue_Kenabres | Cutscene | no combat | 3 | 92.8 | 109.1 | 0 |
| Prologue_Kenabres | EscMode | no combat | 1 | 60.8 | 60.8 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 18.0 | load_save | Prologue_Kenabres | 13.00 | 839 | 1,575.3 | 8 | None,Load |
| 149.9 | load_save | Prologue_Kenabres | 11.62 | 601 | 1,321.7 | 4 | None,Load |
| 316.2 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 8.84 | 574 | 1,329.4 | 4 | None,TeleportParty,Load |
| 328.1 | save (auto) | Prologue_Caves_1 | 0.44 | 10 | 198.3 | 1 | Save |
| 337.2 | save (manual) | Prologue_Caves_1 | 0.34 | 9 | 135.7 | 1 | Save |

Median by kind: area_transition 8.84 s (n=1), load_save 11.62 s (n=2), save 0.34 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,095 → 3,806 (max 4,344)
- mono heap: 485 → 567 (max 569)
- mono used: 463 → 488 (max 547)
- unity reserved: 1,497 → 1,571 (max 1,583)
- GC collections per minute: 1.89
