# WrathStats run 20261010T092710Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process arm64 (native), `~/Developer/Wrath-Mac/builds/arm64/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.2.2
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 4.9 | 71.6 | 13.0 | 19.5 | 21.9 | 41.4 | 132.8 | 0.16 | 0.09 | 3.67 |
| cutscenes | 3.5 | 70.8 | 13.3 | 20.0 | 22.0 | 41.8 | 116.2 | 0.17 | 0.08 | 3.46 |
| dialogs | 0.3 | 78.4 | 12.6 | 17.6 | 21.4 | 34.1 | 99.7 | 0.16 | 0.08 | 3.86 |
| exploring (real-time, no combat) | 1.2 | 72.7 | 12.7 | 18.1 | 21.9 | 38.5 | 132.8 | 0.14 | 0.10 | 4.23 |
| real-time combat | – | – | – | – | – | – | – | – | – | – |
| turn-based combat | – | – | – | – | – | – | – | – | – | – |

## Passes (in-game stretches between loads)

| # | starts (s) | after | area | in-game min | cutscene min | hitches | in cutscenes | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 30.4 | load_save | Prologue_Kenabres | 2.0 | 1.4 | 13 | 9 | 19.5 | 22.1 | 132.8 |
| 2 | 162.3 | load_save | Prologue_Kenabres | 2.6 | 2.0 | 5 | 3 | 20.4 | 21.9 | 106.3 |
| 3 | 326.0 | area_transition | Prologue_Caves_1 | 0.3 | 0.0 | 0 | 0 | 14.6 | 17.0 | 30.4 |

## Hitches (in-game frames ≥ 50 ms): 18

- With a GC in the same frame: **12 (67%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 4 (22%)
- During another mod's reported work (WrathStatsApi): 0
- Median 93.1 ms; worst: 132.8 ms @ 30.4 s, 116.2 ms @ 108.7 s, 112.0 ms @ 83.9 s, 106.3 ms @ 242.2 s, 99.7 ms @ 281.9 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 12 | 94.5 | 116.2 | 10 |
| Prologue_Kenabres | Default | no combat | 4 | 70.7 | 132.8 | 1 |
| Prologue_Kenabres | EscMode | no combat | 1 | 57.2 | 57.2 | 0 |
| Prologue_Kenabres | Dialog | no combat | 1 | 99.7 | 99.7 | 1 |

## Loads and saves: 5

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 18.0 | load_save | Prologue_Kenabres | 12.29 | 839 | 1,399.0 | 7 | None,Load |
| 150.3 | load_save | Prologue_Kenabres | 11.99 | 600 | 1,395.1 | 3 | None,Load |
| 317.3 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 8.67 | 574 | 1,245.2 | 3 | None,TeleportParty,Load |
| 333.6 | save (auto) | Prologue_Caves_1 | 0.42 | 10 | 191.3 | 1 | Save |
| 342.8 | save (manual) | Prologue_Caves_1 | 0.33 | 9 | 129.8 | 1 | Save |

Median by kind: area_transition 8.67 s (n=1), load_save 11.99 s (n=2), save 0.33 s (n=2)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,052 → 3,926 (max 4,276)
- mono heap: 443 → 555 (max 555)
- mono used: 442 → 479 (max 522)
- unity reserved: 1,499 → 1,571 (max 1,605)
- GC collections per minute: 2.47

_No session-end note: the game probably crashed or is still running._
