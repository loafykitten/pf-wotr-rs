# WrathStats run 20261010T091757Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.2
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 5.6 | 69.8 | 13.0 | 20.3 | 22.8 | 144.5 | 1,251.3 | 0.47 | 0.32 | 13.36 |
| cutscenes | 4.2 | 68.5 | 13.3 | 20.7 | 23.1 | 151.4 | 532.9 | 0.55 | 0.39 | 15.85 |
| dialogs | 0.2 | 73.5 | 12.7 | 19.1 | 21.8 | 286.1 | 286.1 | 0.33 | 0.22 | 9.72 |
| exploring (real-time, no combat) | 1.2 | 73.2 | 12.6 | 17.9 | 22.1 | 57.2 | 1,251.3 | 0.24 | 0.13 | 5.64 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 42.6 | load_save | Prologue_Kenabres | 2.6 | 2.1 | 71 | 64 | 20.9 | 29.5 | 1,251.3 |
| 2 | 211.4 | area_transition | Prologue_Caves_1 | 0.1 | 0.0 | 1 | 0 | 18.0 | 24.6 | 57.8 |
| 3 | 227.7 | load_save | Prologue_Kenabres | 2.5 | 2.0 | 3 | 2 | 20.2 | 21.9 | 289.7 |
| 4 | 389.3 | area_transition | Prologue_Caves_1 | 0.4 | 0.0 | 0 | 0 | 16.2 | 17.1 | 45.4 |

## Hitches (in-game frames ≥ 50 ms): 75

- With a GC in the same frame: **3 (4%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 8 (11%)
- During another mod's reported work (WrathStatsApi): 0
- Median 127.9 ms; worst: 1,251.3 ms @ 43.9 s, 532.9 ms @ 91.3 s, 429.3 ms @ 186.2 s, 391.7 ms @ 144.5 s, 355.2 ms @ 141.6 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 66 | 120.6 | 532.9 | 2 |
| Prologue_Kenabres | Default | no combat | 7 | 127.9 | 1,251.3 | 0 |
| Prologue_Kenabres | Dialog | no combat | 1 | 286.1 | 286.1 | 1 |
| Prologue_Caves_1 | Dialog | no combat | 1 | 57.8 | 57.8 | 0 |

## Loads and saves: 6

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 25.9 | load_save | Prologue_Kenabres | 16.47 | 842 | 2,215.1 | 2 | None,Load |
| 200.5 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 10.76 | 576 | 1,269.6 | 2 | None,TeleportParty,Load |
| 215.2 | load_save | Prologue_Caves_1 → Prologue_Kenabres | 12.37 | 806 | 1,231.2 | 2 | None,Load |
| 379.1 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 10.12 | 587 | 1,244.7 | 2 | None,TeleportParty,Load |
| 393.3 | save (auto) | Prologue_Caves_1 | 0.61 | 10 | 285.8 | 1 | Save |
| 409.0 | save (manual) | Prologue_Caves_1 | 0.53 | 9 | 280.7 | 1 | Save |

Median by kind: area_transition 10.12 s (n=2), load_save 12.37 s (n=2), save 0.53 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 5,106 → 4,891 (max 5,382)
- mono heap: 1,343 → 1,274 (max 1,343)
- mono used: 1,148 → 1,116 (max 1,332)
- unity reserved: 1,485 → 1,538 (max 1,573)
- GC collections per minute: 0.54

_No session-end note: the game probably crashed or is still running._
