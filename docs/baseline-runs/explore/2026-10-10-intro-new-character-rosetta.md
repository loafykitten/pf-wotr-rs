# WrathStats run 20261010T075144Z

- **Build:** game 2.7.0y.782, Unity 2020.3.48f1, process X64 (Rosetta), `~/Developer/Wrath-Mac/builds/x64-modded/Wrath.app/Contents`
- **Machine:** Mac14,13 (Apple M2 Max), Apple M2 Max via Metal, macOS 27.0
- **Display:** 2560x1440 (FullScreenWindow), vsync 1, target fps -1, quality 'High'
- **Mods:** BuffIt2TheLimit 1.21.3, 0ToyBox0 1.7.27, WrathStats 0.1.1
- **GC:** Enabled, incremental=False

## Frame times (ms)

| context | min | fps | p50 | p95 | p99 | p99.9 | max | % >33 | % >50 | hitches/min (≥50) |
|---|---|---|---|---|---|---|---|---|---|---|
| all in-game | 8.7 | 55.6 | 16.7 | 23.3 | 35.8 | 174.4 | 1,261.0 | 1.78 | 0.48 | 16.06 |
| exploring (real-time, no combat) | 7.8 | 55.7 | 16.7 | 23.8 | 35.6 | 167.4 | 1,261.0 | 1.78 | 0.47 | 15.56 |
| real-time combat | 0.2 | 51.2 | 16.6 | 20.9 | 73.9 | 414.2 | 414.2 | 2.25 | 1.45 | 44.51 |
| turn-based combat | 0.6 | 56.8 | 16.7 | 21.4 | 39.4 | 67.7 | 263.4 | 1.64 | 0.39 | 13.14 |

## Hitches (in-game frames ≥ 50 ms): 139

- With a GC in the same frame: **5 (4%)**
- Within 15 s after a load (likely warm-up: shaders, assets): 19 (14%)
- During another mod's reported work (WrathStatsApi): 0
- Median 85.5 ms; worst: 1,261.0 ms @ 254.9 s, 534.6 ms @ 255.5 s, 449.9 ms @ 549.3 s, 433.3 ms @ 501.4 s, 414.2 ms @ 743.9 s

| area | mode | fight | hitches | median ms | max ms | with GC |
|---|---|---|---|---|---|---|
| Prologue_Kenabres | Cutscene | no combat | 80 | 97.9 | 1,261.0 | 2 |
| Prologue_Caves_1 | Default | no combat | 16 | 67.0 | 357.8 | 1 |
| Prologue_Kenabres | Dialog | no combat | 8 | 98.7 | 352.6 | 2 |
| Prologue_Caves_1 | FullScreenUi | no combat | 8 | 57.3 | 347.7 | 0 |
| Prologue_Caves_1 | Default | combat | 8 | 101.3 | 414.2 | 0 |
| Prologue_Caves_1 | Default | turn-based | 8 | 58.7 | 263.4 | 0 |
| Prologue_Kenabres | Default | no combat | 5 | 54.6 | 120.8 | 0 |
| Prologue_Caves_1 | Dialog | no combat | 4 | 55.5 | 105.0 | 0 |
| Prologue_Caves_1 | Pause | no combat | 1 | 57.4 | 57.4 | 0 |
| Prologue_Caves_1 | Pause | combat | 1 | 74.0 | 74.0 | 0 |

## Loads and saves: 7

| t (s) | what | area | seconds | frames | max frame ms | GCs | tags |
|---|---|---|---|---|---|---|---|
| 233.7 | unknown |  | 0.32 | 4 | 325.0 | 0 | None |
| 234.7 | area_transition | Prologue_Kenabres | 18.94 | 415 | 2,265.3 | 3 | None,Load |
| 383.0 | save (auto) | Prologue_Kenabres | 1.75 | 8 | 1,067.9 | 1 | Save |
| 563.8 | area_transition | Prologue_Kenabres → Prologue_Caves_1 | 13.53 | 374 | 1,721.2 | 2 | None,TeleportParty,Load |
| 587.4 | save (auto) | Prologue_Caves_1 | 0.73 | 10 | 327.5 | 1 | Save |
| 594.7 | save (manual) | Prologue_Caves_1 | 0.71 | 9 | 338.6 | 1 | Save |
| 600.5 | load_save | Prologue_Caves_1 | 12.57 | 376 | 1,669.6 | 2 | None,Load |

Median by kind: area_transition 13.53 s (n=2), load_save 12.57 s (n=1), save 0.73 s (n=3), unknown 0.32 s (n=1)

## Memory (MB, in-game seconds: first → last, max)

- resident: 4,057 → 4,344 (max 4,345)
- mono heap: 1,503 → 1,533 (max 1,535)
- mono used: 1,348 → 1,302 (max 1,501)
- unity reserved: 1,538 → 1,553 (max 1,618)
- GC collections per minute: 0.59

## Markers: 15

- 56.2 s:  / None
- 260.6 s: Prologue_Kenabres / Cutscene
- 268.5 s: Prologue_Kenabres / Cutscene
- 321.2 s: Prologue_Kenabres / Dialog
- 388.3 s: Prologue_Kenabres / Default
- 446.6 s: Prologue_Kenabres / Cutscene
- 458.9 s: Prologue_Kenabres / Cutscene
- 470.7 s: Prologue_Kenabres / Cutscene
- 481.0 s: Prologue_Kenabres / Cutscene
- 486.2 s: Prologue_Kenabres / Cutscene
- 499.4 s: Prologue_Kenabres / Cutscene
- 508.5 s: Prologue_Kenabres / Cutscene
- 550.1 s: Prologue_Kenabres / Cutscene
- 659.9 s: Prologue_Caves_1 / Default
- 744.6 s: Prologue_Caves_1 / Default

## Notes

- Exploratory run, not the Phase 3 benchmark protocol: new character through the
  prologue (Kenabres cutscenes including Deskari, Prologue_Caves_1 fights), one manual
  save and one reload. WrathStats 0.1.1, x64 build under Rosetta.
- **Ember's impressions:** most hitches were felt during cutscenes, especially while spell
  animations played. The big Deskari cutscene had jagged fps but was solid between
  hitches. Dialogues were smooth. Saving and loading felt possibly faster than on their
  old Windows PC.
- **Markers match measurements:** 14 of the 15 Ctrl+Shift+M markers have recorded hitches
  0.1–1.5 s before them (the 15th was pressed at the main menu, which isn't measured).
- **Reading:** steady state is a locked 60 fps (vsync; p50 16.7 ms, p95 about 23 ms).
  Hitches are spiky one-offs (median 86 ms), concentrated in VFX-heavy cutscenes, and
  only 4% coincide with a GC (0.6 GCs per minute), so GC is not the main cause. This
  points to first-use costs: Metal shader/pipeline compilation, asset loading, and/or
  Mono JIT output going through Rosetta translation. A replay of the same cutscene in one
  session separates warm-up costs (they vanish the second time) from steady rendering
  cost; the arm64 build (Phase 4) removes the Rosetta part.
- The 0.32 s "unknown" load at 233.7 s plus the "area_transition" right after is the new
  game starting. Same-frame reasons now rank (new_game beats the area_transition it
  triggers) as of WrathStats 0.1.2.
