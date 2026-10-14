# WrathStats run 20261010T081726Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process X64 (Rosetta), `~/Developer/Wrath-Mac/builds/x64-modded/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.1
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 5.9 | 55.0 | 16.7 | 20.6 | 35.5 | 314.3 | 1,393.3 | 1.75 | 0.65 | 21.30 |
| cutscenes | 3.7 | 53.6 | 16.8 | 30.2 | 44.6 | 343.7 | 718.9 | 2.46 | 0.89 | 28.68 |
| dialogs | 0.3 | 56.8 | 16.6 | 19.4 | 33.2 | 344.2 | 344.2 | 0.95 | 0.32 | 10.84 |
| exploring (real-time, no combat) | 2.0 | 57.4 | 16.6 | 18.5 | 29.6 | 100.1 | 1,393.3 | 0.65 | 0.26 | 9.11 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 52.0 | load_save | Prologue_Kenabres | 2.4 | 1.6 | 98 | 85 | 30.2 | 68.2 | 1,393.3 |
| 2 | 207.4 | load_save | Prologue_Kenabres | 3.3 | 2.0 | 25 | 20 | 19.4 | 33.4 | 450.6 |
| 3 | 417.1 | area_transition | Prologue_Caves_1 | 0.3 | 0.0 | 3 | 0 | 18.8 | 28.8 | 89.6 |

## Hitches (in-game frames ≥ 50 ms): 126

- With a GC in the same frame: **11 (9%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 14 (11%)
- During another mod's reported work (WrathStatsApi): 0
- Median 94.7 ms; worst: 1,393.2 ms @ 53.4 s, 718.9 ms @ 108.8 s, 578.9 ms @ 52.0 s, 450.6 ms @ 388.6 s, 448.8 ms @ 164.8 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 105 | 99.7 | 718.9 | 8 |
| Prologue_Kenabres | Default | no combat | 13 | 100.1 | 1,393.2 | 2 |
| Prologue_Kenabres | EscMode | no combat | 3 | 91.3 | 94.7 | 0 |
| Prologue_Kenabres | Dialog | no combat | 2 | 137.2 | 344.2 | 1 |
| Prologue_Caves_1 | FullScreenUi | no combat | 2 | 50.2 | 54.0 | 0 |
| Prologue_Caves_1 | Dialog | no combat | 1 | 89.6 | 89.6 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 28.2 | load_save | Prologue_Kenabres | 23.26 | 425 | 3,188.8 | 6 | None,Load |
| 192.5 | load_save | Prologue_Kenabres | 14.82 | 392 | 1,681.3 | 3 | None,Load |
| 403.1 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 13.87 | 373 | 1,666.5 | 3 | None,TeleportParty,Load |
| 421.6 | save (auto) | Prologue_Caves_1 | 0.99 | 10 | 362.1 | 1 | Save |
| 433.2 | save (manual) | Prologue_Caves_1 | 0.68 | 9 | 324.1 | 1 | Save |

Median by kind: area_transition 13.87 s (n=1), load_save 14.82 s (n=2), save 0.68 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,271 → 3,839 (max 4,323)
- mono heap: 1,391 → 1,503 (max 1,503)
- mono used: 1,388 → 1,434 (max 1,486)
- unity reserved: 1,497 → 1,539 (max 1,605)
- GC collections per minute: 1.89
