# WrathStats run 20261010T082613Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process X64 (Rosetta), `~/Developer/Wrath-Mac/builds/x64-modded/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.1
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 5.5 | 57.0 | 16.7 | 20.5 | 34.0 | 109.1 | 515.9 | 1.31 | 0.25 | 8.72 |
| cutscenes | 3.5 | 56.2 | 16.7 | 29.3 | 34.6 | 111.4 | 172.0 | 1.75 | 0.28 | 9.46 |
| dialogs | 0.2 | 57.7 | 16.7 | 19.1 | 33.0 | 130.7 | 130.7 | 0.93 | 0.31 | 10.73 |
| exploring (real-time, no combat) | 1.8 | 58.5 | 16.6 | 18.4 | 28.1 | 85.6 | 515.9 | 0.53 | 0.20 | 7.10 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 54.5 | load_save | Prologue_Kenabres | 2.5 | 1.5 | 37 | 26 | 21.4 | 36.3 | 515.9 |
| 2 | 214.1 | load_save | Prologue_Kenabres | 2.8 | 2.0 | 10 | 7 | 20.3 | 33.4 | 121.9 |
| 3 | 393.9 | area_transition | Prologue_Caves_1 | 0.2 | 0.0 | 1 | 0 | 19.2 | 29.7 | 70.1 |

## Hitches (in-game frames ≥ 50 ms): 48

- With a GC in the same frame: **12 (25%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 7 (15%)
- During another mod's reported work (WrathStatsApi): 0
- Median 85.6 ms; worst: 515.9 ms @ 54.5 s, 177.7 ms @ 54.7 s, 172.0 ms @ 122.3 s, 148.4 ms @ 115.4 s, 130.7 ms @ 180.8 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 33 | 95.1 | 172.0 | 9 |
| Prologue_Kenabres | Default | no combat | 11 | 85.6 | 515.9 | 3 |
| Prologue_Kenabres | Dialog | no combat | 1 | 130.7 | 130.7 | 0 |
| Prologue_Kenabres | EscMode | no combat | 1 | 95.5 | 95.5 | 0 |
| Prologue_Kenabres | FullScreenUi | no combat | 1 | 53.0 | 53.0 | 0 |
| Prologue_Caves_1 | Dialog | no combat | 1 | 70.1 | 70.1 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 36.1 | load_save | Prologue_Kenabres | 17.92 | 427 | 2,393.7 | 9 | None,Load |
| 201.4 | load_save | Prologue_Kenabres | 12.63 | 389 | 1,862.1 | 3 | None,Load |
| 382.2 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 11.58 | 373 | 1,832.2 | 3 | None,TeleportParty,Load |
| 396.3 | save (auto) | Prologue_Caves_1 | 0.78 | 10 | 371.1 | 1 | Save |
| 405.2 | save (manual) | Prologue_Caves_1 | 0.46 | 9 | 165.1 | 1 | Save |

Median by kind: area_transition 11.58 s (n=1), load_save 12.63 s (n=2), save 0.46 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,102 → 3,977 (max 4,368)
- mono heap: 433 → 529 (max 529)
- mono used: 418 → 477 (max 512)
- unity reserved: 1,504 → 1,563 (max 1,563)
- GC collections per minute: 2.21
