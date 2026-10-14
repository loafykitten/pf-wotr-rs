# The oracle

WrathOracle and its tools record the original game, so the simulation can be tested against
it. This code moved here from `~/Developer/Wrath-Mac` on 2026-10-12. Wrath-Mac is now
pf-wotr-silicon (public: the native arm64 build tooling and WrathPerf), and pf-wotr-rs no
longer needs it. Commit hashes in older docs ("Wrath-Mac `8fa7cc6`") are in Wrath-Mac's
private history, which is archived on its local branch `archive/private-history` (never
pushed).

| Path | What |
|---|---|
| `mods/WrathOracle/` | The oracle mod (UMM/Harmony): plans, fixtures, traces, RNG tape, replay pins, state dumps. |
| `mods/WrathStats/` | Frame and load-time recorder; the oracle's runs carry its sessions. |
| `mods/WrathBench/` | Load benchmark driver (`tools/bench-loads.sh`). |
| `tools/oracle-run.sh` | One unattended run on the oracle build. For anything unattended, use `scripts/oracle-record.sh`, which wraps it with the hang watchdog and retries. |
| `tools/oracle-trace-diff` (+ `-selftest`), `oracle-state-diff`, `oracle-obs-diff`, `oracle-rng-table` | Comparing and checking captures. |
| `tools/oracle-fixtures/` | Fixture specs, `make-plan`, and the generated plans. |
| `tools/launch.sh`, `metal-cache.sh`, `bench-loads.sh`, `oracle-bench-batch.sh`, `summarize-*`, `compare-shots` | Launching, benchmarks, summaries and screenshot comparison. |

Local, gitignored:

| Path | What |
|---|---|
| `builds/arm64-oracle/` | The game build the oracle runs on. It is an APFS clone of Ember's native arm64 build, made with pf-wotr-silicon's `tools/build-arm64-bundle.sh`. `Mods/` holds exactly WrathBench, WrathOracle, WrathPerf and WrathStats as real folders, with no gameplay mods. WrathPerf's binary came with the clone; its source is in pf-wotr-silicon. |
| `builds/.oracle-run.lock` | The one lock for the game and the GPU. `oracle-run.sh` takes it, and so does anything timing-sensitive (wr-game benches). |
| `decompiled/` | The decompiled game code, a reading reference. Never commit it. |
| `fixtures/saves/` | Ember's fixture saves, read-only copies (`wr_paths::fixture_saves()`). |
| `fixtures/captures/oracle/` | Captures (`scripts/oracle-record.sh LABEL SAVE PLAN`). |
| `fixtures/captures/runs-raw/` | Wrath-Mac's raw benchmark runs, plus the hang samples from `oracle-record.sh`. |

Build the mods with the game closed. Each build installs into `builds/arm64-oracle/Mods`:

```sh
dotnet build -c Release oracle/mods/WrathOracle/WrathOracle.csproj
```

`docs/poc1/oracle.md` has the method and `docs/poc5/tb-replay.md` the turn-based replay
work. `docs/baseline-runs/` holds Wrath-Mac's benchmark reports.

To rebuild `builds/arm64-oracle` after a game update:

1. Build a fresh native build with pf-wotr-silicon.
2. `cp -cRp` its `Wrath.app` here.
3. Copy WrathPerf's installed folder into `Mods/`.
4. Rebuild the three mods above.
5. Write `BUILD_INFO`.
6. Re-record anything that compares against the old build.
