# WrathStats run 20261010T093334Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.2
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 4.8 | 71.3 | 13.1 | 19.4 | 22.0 | 48.2 | 128.1 | 0.19 | 0.10 | 4.17 |
| cutscenes | 3.5 | 70.9 | 13.3 | 19.7 | 22.0 | 43.5 | 100.4 | 0.17 | 0.09 | 3.75 |
| dialogs | 0.1 | 76.1 | 12.7 | 18.6 | 21.5 | 42.1 | 42.1 | 0.44 | 0.00 | 0.00 |
| exploring (real-time, no combat) | 1.2 | 72.1 | 12.7 | 18.0 | 22.0 | 54.0 | 128.1 | 0.21 | 0.13 | 5.74 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 29.4 | load_save | Prologue_Kenabres | 2.1 | 1.4 | 15 | 10 | 19.6 | 22.3 | 128.1 |
| 2 | 164.2 | load_save | Prologue_Kenabres | 2.5 | 2.0 | 5 | 3 | 19.6 | 21.9 | 100.4 |
| 3 | 323.8 | area_transition | Prologue_Caves_1 | 0.2 | 0.0 | 0 | 0 | 15.8 | 17.2 | 42.1 |

## Hitches (in-game frames ≥ 50 ms): 20

- With a GC in the same frame: **13 (65%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 4 (20%)
- During another mod's reported work (WrathStatsApi): 0
- Median 92.5 ms; worst: 128.1 ms @ 29.4 s, 100.4 ms @ 234.8 s, 98.7 ms @ 293.2 s, 98.0 ms @ 78.3 s, 97.4 ms @ 89.5 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 13 | 93.2 | 100.4 | 11 |
| Prologue_Kenabres | Default | no combat | 6 | 70.0 | 128.1 | 2 |
| Prologue_Kenabres | EscMode | no combat | 1 | 52.3 | 52.3 | 0 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 17.1 | load_save | Prologue_Kenabres | 12.19 | 828 | 1,343.7 | 8 | None,Load |
| 152.7 | load_save | Prologue_Kenabres | 11.51 | 602 | 1,274.5 | 3 | None,Load |
| 315.0 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 8.77 | 574 | 1,262.0 | 3 | None,TeleportParty,Load |
| 326.9 | save (auto) | Prologue_Caves_1 | 0.42 | 10 | 185.8 | 1 | Save |
| 335.5 | save (manual) | Prologue_Caves_1 | 0.33 | 9 | 123.0 | 1 | Save |

Median by kind: area_transition 8.77 s (n=1), load_save 11.51 s (n=2), save 0.33 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,053 → 3,906 (max 4,292)
- mono heap: 443 → 555 (max 555)
- mono used: 424 → 478 (max 520)
- unity reserved: 1,504 → 1,541 (max 1,624)
- GC collections per minute: 2.74

_No session-end note: the game probably crashed or is still running._
