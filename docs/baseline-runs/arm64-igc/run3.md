# WrathStats run 20261010T101208Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.4
- **GC:** Enabled, incremental=True

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 4.9 | 71.9 | 13.0 | 19.2 | 22.0 | 37.1 | 139.8 | 0.12 | 0.04 | 1.64 |
| cutscenes | 3.5 | 71.1 | 13.3 | 19.6 | 22.0 | 38.5 | 120.0 | 0.13 | 0.03 | 1.15 |
| dialogs | 0.1 | 79.1 | 12.5 | 18.3 | 21.7 | 37.1 | 37.1 | 0.16 | 0.00 | 0.00 |
| exploring (real-time, no combat) | 1.3 | 73.4 | 12.7 | 17.9 | 22.0 | 33.7 | 139.8 | 0.11 | 0.07 | 3.16 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 26.3 | load_save | Prologue_Kenabres | 2.0 | 1.4 | 6 | 3 | 19.2 | 22.2 | 139.8 |
| 2 | 158.0 | load_save | Prologue_Kenabres | 2.5 | 2.0 | 2 | 1 | 20.0 | 21.9 | 97.2 |
| 3 | 318.7 | area_transition | Prologue_Caves_1 | 0.3 | 0.0 | 0 | 0 | 14.6 | 17.2 | 31.6 |

## Hitches (in-game frames ≥ 50 ms): 8

- With a GC in the same frame: **0 (0%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 3 (38%)
- During another mod's reported work (WrathStatsApi): 0
- Median 77.2 ms; worst: 139.8 ms @ 26.3 s, 120.0 ms @ 84.5 s, 104.8 ms @ 89.4 s, 97.2 ms @ 214.7 s, 77.2 ms @ 26.4 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 4 | 97.2 | 120.0 | 0 |
| Prologue_Kenabres | Default | no combat | 3 | 77.2 | 139.8 | 0 |
| Prologue_Kenabres | EscMode | no combat | 1 | 58.4 | 58.4 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 13.7 | load_save | Prologue_Kenabres | 12.44 | 825 | 1,348.4 | 8 | None,Load |
| 146.3 | load_save | Prologue_Kenabres | 11.68 | 602 | 1,342.5 | 4 | None,Load |
| 309.9 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 8.76 | 574 | 1,302.2 | 4 | None,TeleportParty,Load |
| 323.8 | save (auto) | Prologue_Caves_1 | 0.43 | 10 | 194.8 | 1 | Save |
| 335.7 | save (manual) | Prologue_Caves_1 | 0.34 | 9 | 134.3 | 1 | Save |

Median by kind: area_transition 8.76 s (n=1), load_save 11.68 s (n=2), save 0.34 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,105 → 3,777 (max 4,328)
- mono heap: 501 → 569 (max 569)
- mono used: 457 → 488 (max 545)
- unity reserved: 1,506 → 1,555 (max 1,596)
- GC collections per minute: 1.87
