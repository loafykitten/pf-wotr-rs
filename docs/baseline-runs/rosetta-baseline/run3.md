# WrathStats run 20261010T083356Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process X64 (Rosetta), `~/Developer/Wrath-Mac/builds/x64-modded/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.1
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 5.0 | 57.0 | 16.7 | 20.5 | 34.2 | 99.5 | 520.9 | 1.35 | 0.27 | 9.38 |
| cutscenes | 3.5 | 56.6 | 16.7 | 26.3 | 34.4 | 99.5 | 230.4 | 1.54 | 0.25 | 8.58 |
| dialogs | 0.1 | 56.0 | 16.7 | 21.0 | 41.1 | 138.1 | 138.1 | 1.95 | 0.65 | 21.87 |
| exploring (real-time, no combat) | 1.4 | 58.1 | 16.6 | 18.5 | 30.6 | 99.4 | 520.9 | 0.81 | 0.29 | 10.16 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 42.3 | load_save | Prologue_Kenabres | 2.2 | 1.5 | 36 | 25 | 20.5 | 35.6 | 520.9 |
| 2 | 187.1 | load_save | Prologue_Kenabres | 2.5 | 2.0 | 9 | 5 | 21.1 | 33.6 | 230.4 |
| 3 | 350.8 | area_transition | Prologue_Caves_1 | 0.3 | 0.0 | 2 | 0 | 18.7 | 29.1 | 64.7 |

## Hitches (in-game frames ≥ 50 ms): 47

- With a GC in the same frame: **12 (26%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 11 (23%)
- During another mod's reported work (WrathStatsApi): 0
- Median 76.1 ms; worst: 520.9 ms @ 42.3 s, 230.4 ms @ 242.8 s, 169.7 ms @ 42.5 s, 160.3 ms @ 109.7 s, 145.4 ms @ 138.1 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 30 | 76.1 | 230.4 | 8 |
| Prologue_Kenabres | Default | no combat | 11 | 73.0 | 520.9 | 3 |
| Prologue_Kenabres | Dialog | no combat | 2 | 119.7 | 138.1 | 1 |
| Prologue_Kenabres | EscMode | no combat | 1 | 88.8 | 88.8 | 0 |
| Prologue_Kenabres | FullScreenUi | no combat | 1 | 52.5 | 52.5 | 0 |
| Prologue_Caves_1 | Dialog | no combat | 1 | 64.7 | 64.7 | 0 |
| Prologue_Caves_1 | FullScreenUi | no combat | 1 | 53.4 | 53.4 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 23.5 | load_save | Prologue_Kenabres | 18.27 | 427 | 2,531.2 | 9 | None,Load |
| 174.3 | load_save | Prologue_Kenabres | 12.67 | 391 | 1,844.7 | 3 | None,Load |
| 339.0 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 11.69 | 375 | 1,851.2 | 3 | None,TeleportParty,Load |
| 353.9 | save (auto) | Prologue_Caves_1 | 0.76 | 10 | 338.9 | 1 | Save |
| 364.3 | save (manual) | Prologue_Caves_1 | 0.46 | 9 | 159.6 | 1 | Save |

Median by kind: area_transition 11.69 s (n=1), load_save 12.67 s (n=2), save 0.46 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,056 → 3,920 (max 4,299)
- mono heap: 433 → 529 (max 529)
- mono used: 418 → 474 (max 498)
- unity reserved: 1,504 → 1,555 (max 1,596)
- GC collections per minute: 2.24
