# Wwise spike (POC-0)

Drives the game's own Wwise runtime, `AkSoundEngine.bundle` (Wwise 2019.2.15, the Unity
integration's native half), from Rust over `dlopen`, and reproduces the game's audio
lifecycle. The verdict and evidence are in [`docs/poc0/wwise.md`](../../docs/poc0/wwise.md).

This is a standalone cargo workspace (own `Cargo.lock` and `target/`), excluded from the
root workspace. The library half (`src/ffi.rs`, `src/engine.rs`, `src/settings.rs`) is the
seed of a future `crates/wr-audio`.

## Run it

Needs macOS on Apple Silicon and the game. By default it uses the oracle's APFS clone,
`builds/arm64-oracle/Wrath.app`; set `WRATH_APP` or `--app` to use another
copy (it only reads from it). It never touches the Steam install or the game's saves.

```sh
cd spikes/wwise
cargo run --release                      # silent: renders to Wwise's No_Output device
cargo run --release -- --device default  # the game's output setting: the system device (~35 s of voice)
cargo run --release -- --device system   # the "System" shareset explicitly
cargo run --release -- --cycles 10       # also repeats init -> play -> term nine more times
cargo run --release -- --no-comms        # skip InitCommunication (the game calls it)
```

Each run prints a report and writes it to `results/run-<device>.txt`. Wwise's output
capture goes to `cache/wwise-spike/captures/` (gitignored) and is analysed at the end:
audible span of the cue, left/right balance while the emitter moves, level past the
attenuation radius.

Note: the `No_Output` device renders faster than real time (2–3x), so wall-clock
durations in a silent run are short. Use `--device default` for timing.

## What it does

1. Lookups (pure Rust): `Sound.json` maps the cue's string key to its event,
   `SoundbanksInfo.xml` gives the event's bank, streamed media and duration, and the `.pck`
   lookup tables show which package holds them.
2. Startup in the game's order: error logger, `Init` with the game's serialized Mac
   settings, spatial audio, communication, base paths, language, callback serializer,
   `Init.bnk`, monitoring output, the 11 preload packages, the default listener, the 8
   preload banks.
3. Scenarios: A, the first voiced line of `MeetSeelahAnevia` on the dialogue speaker; A2,
   marker callbacks on a banter line whose media has a marker; B, the next line on an
   emitter that sweeps left to right and then recedes past 25 m; C, suspend and resume
   mid-line; D, shutdown in the game's order.

## Tests

```sh
scripts/check.sh spikes/wwise                 # from the repo root: fmt, clippy, unit tests
cargo nextest run --run-ignored only          # the real-engine integration test
```

The unit tests cover the pure parts (FNV short IDs against the game's IDs, `Sound.json`,
`SoundbanksInfo.xml`, `.pck` tables, WAV analysis). The ignored integration test loads the
real bundle, plays the cue silently and checks the callbacks.

## Files

- `src/ffi.rs`: the 158 SWIG exports we use, with the marshalling rules.
- `src/engine.rs`: the lifecycle (`SoundEngine`), callbacks, positions, capture.
- `src/settings.rs`: the game's Wwise settings and audio chunks, decoded from
  `sharedassets0.assets`.
- `src/{shortid,sound_json,bank_info,akpk,wav,proc_stats}.rs`: helpers.
- `src/main.rs`: the spike runner. `tests/engine.rs`: the integration test.
- `scripts/`: read-only Python helpers that produced `results/evidence.txt` (settings
  decode, marker scan).
- `results/`: the text reports of the runs quoted in the doc.
