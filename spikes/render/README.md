# POC-0 rendering spike

Bevy 0.20.0 / wgpu 30.0.1 on Metal (M2 Max). Findings are written up in
[`docs/poc0/rendering.md`](../../docs/poc0/rendering.md); raw numbers are in `results/`.

This is a standalone Cargo workspace (own `Cargo.lock` and `target/`). The root workspace
excludes it.

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cd spikes/render
scripts/fetch-assets.sh          # Bevy's Fox.glb into assets/ (gitignored)
cargo build --release
```

`../../scripts/check.sh spikes/render` runs fmt, clippy (`-D warnings`) and nextest.

## Experiments

All commands are `target/release/render-spike <command> [options]`. Each exits non-zero when
its checks fail (`metalfx` currently does: frame interpolation doesn't pass its check).

| Command | What it shows | Output |
|---|---|---|
| `features [--out FILE]` | wgpu features and limits, raw `MTLDevice` counter-sampling points, BC support, MetalFX support | `results/features.txt` |
| `scene [opts]` | Skinned foxes on Bevy, a load that waits for every pipeline, presentation measured at the display, GPU frame time | `--out DIR` (summary.txt + CSVs) |
| `owlcat [--vs F --fs F]` | A real Owlcat/Lit vertex+fragment pair (game MSL, unmodified) drawn through wgpu MSL passthrough into Owlcat's 6-target G-buffer with a BC1 base map, checked numerically | report to stdout / `--report FILE`; PNGs to `cache/poc0-render/out/` |
| `owlcat-bench --pass P [--limit N --threads T --salt S]` | Pipeline creation cost for every variant of an Owlcat/Lit pass | `--out DIR` |
| `archive build\|load [--salt S]` | Persisting an Owlcat pipeline across launches with `MTLBinaryArchive` (raw Metal) | stdout / `--out FILE` |
| `metalfx [--frames N]` | MetalFX spatial, temporal and frame interpolation encoded into wgpu command buffers on wgpu textures | `--out FILE` |

### `scene` options

- `--count N` characters (default 150); `--parts P` skinned meshes per character, each with its
  own skeleton instance (emulates body + equipment layers; default 1).
- `--present fifo|immediate|auto-vsync|auto-novsync|mailbox` (default `fifo`);
  `--latency N` = `desired_maximum_frame_latency` (default 2).
- `--seconds S` measured, `--warmup S` after the load (defaults 20 / 2).
- `--refresh-hz HZ` display refresh for the vsync histogram (default 240).
- `--no-pipelined` disables Bevy's pipelined rendering (main and render world in the same frame).
- `--no-gate` shows the scene immediately instead of waiting for pipelines (to see first-use
  hitches). `--windowed` (default is borderless fullscreen on the primary display, the 240 Hz
  AW2725DF on Ember's Mac). `--no-variants` drops the material-variant grid; `--no-shadows`.

Esc quits. Results: `summary.txt` (percentiles), `frame_starts.csv`, `presented.csv`
(`presentedTime` per drawable, 0 = dropped), `gpu_frames.csv`, `pipelines.csv`.

How it measures:

- **Displayed frames**: `-[CAMetalLayer nextDrawable]` is swizzled process-wide (wgpu keeps the
  drawable private) to attach `addPresentedHandler` to every drawable, which reports
  `presentedTime` on the host clock.
- **GPU frame time**: two compute passes with `timestamp_writes` (each dispatches one no-op
  thread; Metal doesn't sample empty encoders) bracket the frame in Bevy's root `RenderGraph`
  schedule, resolved only after the end command buffer completes (Metal returns 0 if you resolve
  in the same command buffer). Cross-checked with `MTLCommandBuffer` GPU start/end times.
- **Pipelines**: the render world calls `PipelineCache::process_queue()` itself, timed, before
  Bevy's own call (on macOS Bevy builds pipelines synchronously).

### Owlcat shader inputs (game data, never committed)

`owlcat`, `owlcat-bench` and `archive` read MSL dumped from Ember's install into `cache/`:

```sh
cd ../..   # repo root
python3 -m venv cache/unitypy-venv && cache/unitypy-venv/bin/pip install UnityPy
B="$HOME/Library/Application Support/Steam/steamapps/common/Pathfinder Second Adventure/Wrath.app/Bundles"
SHADERS="Owlcat/Lit" DUMP_ALL=1 cache/unitypy-venv/bin/python -I \
  spikes/render/scripts/extract_msl.py cache/poc0-render/shaders/msl-all "$B/shaders"
```

`scripts/extract_msl.py` reads the game's bundles read-only and writes only under the output
directory. Without `DUMP_ALL` it writes one variant per pass and stage plus a full `index.tsv`.

`--salt S` appends a comment to every MSL source so macOS's Metal shader cache (keyed by
source) misses: a repeatable cold-compile measurement without deleting the cache.

### Per-pass GPU timing in Bevy

wgpu on Apple Silicon has `TIMESTAMP_QUERY` but not `TIMESTAMP_QUERY_INSIDE_ENCODERS` or
`_INSIDE_PASSES`, so Bevy's `RenderDiagnosticsPlugin` only produces CPU spans. For per-pass GPU
time use Metal System Trace:

```sh
scripts/trace-scene.sh results/trace-150   # records ~6 s with xctrace and summarises GPU intervals per encoder label
```
