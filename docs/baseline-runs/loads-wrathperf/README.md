# WrathPerf 0.1.0: native DXT compressor and shorter cleanup waits

Phase 8, shortlist items #1 and #2 ([`docs/phase8-shortlist.md`](../../docs/phase8-shortlist.md)).
Native arm64 build with incremental GC, all of Ember's mods, WrathPerf 0.1.0. Automated
with `tools/bench-loads.sh` (WrathBench: load `Manual_14_WS_Bench_Deskari` from the main
menu, then reload it N times, settling 8 s after each load). Recorded 2026-10-10 by Claude.
Sessions and game logs are in `runs/raw/loads/<label>/`.

## Load times

| Run (`runs/raw/loads/`) | Native compressor | Cleanup wait | First load | Reloads |
|---|---|---|---|---|
| `base-1` (the game's behaviour) | off | 100 frames | 12.60 s | 11.88 / 11.46 / 11.37 s |
| `native-1` | **on** | 100 frames | 9.08 s | 8.46 / 8.22 / 8.35 s |
| `wait10-1` | off | **10 frames** | 11.60 s | 9.78 / 9.62 / 9.72 s |
| `both-1` | **on** | **10 frames** | **7.62 s** | **6.44 / 6.40 / 6.56 s** |
| `soak-base` (15 reloads) | off | 100 frames | 12.39 s | 11.36–11.51 s |
| `soak-both` (15 reloads) | **on** | **10 frames** | **7.42 s** | **6.35–6.92 s** (6.5 s typical) |

**Each reload is about 5 s faster (11.5 → 6.5 s, −43%); the first load 12.5 → 7.5 s.**
The two changes add up: about 3.2 s from the compressor and 1.8 s from the waits.

Owlcat's own stage timings per reload (`GameLogFull.txt`, "Loading process finished"):

| Stage | Game | WrathPerf |
|---|---|---|
| `AwaitTextureCompression` (atlas build + DXT) | 3.9 s | 0.71 s |
| 2 × `UnloadUnusedAssetsCoroutine` | 1.4 + 1.3 s | 0.5 + 0.4 s |
| `LoadAreaCoroutine` (unchanged) | 3.1–3.2 s | 3.1–3.2 s |

DXT compression CPU per reload (WrathBench's probe on `Request.Compress`, 180 calls):
**10.5–10.6 s → 0.89–0.90 s** (11.8× less); the slowest single call 440 → 42 ms. So
atlas rebuilds in play (equipment changes, new characters) get the same speed-up.

## Correctness

- **Byte-identical output.** `native/wrathdxt/test/verify.sh` runs the game's own
  `DXTCompressor.dll` on the game's own arm64 Mono (outside the game) next to
  libwrathdxt: lookup and float tables bit-identical; about 9.3 M fuzzed 4×4 blocks over
  all four modes, DXT1 and DXT5, 0 mismatches; 88 k blocks through the game's exact entry
  point; and 132 constructed blocks covering every input where Mono's float rounding could
  matter. Benchmark: a 2048² atlas takes 600 ms in the game's code, 55 ms native.
- **In game (`verify-1`, verify mode on):** all 1,110 real atlas compressions over 6 loads
  were also run through the game's compressor and compared: **0 mismatches**.
- WrathPerf also checks the tables and a test image against the game's compressor at
  every startup, and uses the game's compressor if anything is off.

## Risk checks for the shorter wait

- **Memory over 15 reloads** (sampled at the end of each settle): the same path in both
  runs, step for step. Resident 4.38 → 4.58 GB (game) vs 4.38 → 4.59 GB (WrathPerf);
  Unity allocated 1.19 → 1.26 GB and Mono heap 0.54 → 0.58 GB in both. The small growth is
  the game's own.
- **Logs** over the 15-reload soaks: the same error-like lines in `GameLogFull.txt` and
  `Player.log`, the same counts (2,308 vs 2,309, 121 vs 121). The only extra line is the
  game's own "Failing to quit for 0/20" race at quit (`Game.HandleQuit`), which also
  appears in `native-1` with the 100-frame wait. No new Wwise/audio errors.
- **Portraits and equipment** (`shots-base` vs `shots-both`, screenshots after each load):
  the party portrait is shown, and the character's armour, colours and weapon look the
  same in both.
- **Not checked by machine:** audio dropouts by ear, and area transitions (only save loads
  were benchmarked). Both are for normal play.
