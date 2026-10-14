# POC-6 render round 2: stalls, colour space, bloom (handoff)

Written 2026-10-11. Follows `first-light.md`. **[M]** measured here, **[I]** inferred. Run
outputs, shots, FLIP maps and sheets are under `fixtures/captures/poc6/round2/` (gitignored);
the pack under `cache/poc6/packs/`. Order of work: the handoff's steps 1 (stalls) and 2
(colour space), then **bloom before shadows**: BloomEnhanced was a day's work with a visible
effect at every point, while shadows turned out to be a multi-day port (spec written, not
implemented; see "Next steps").

## Result

| | First light | Round 2 |
|---|---|---|
| FLIP mean vs `oracle-2` (8 points) | 0.144 | **0.126** [M] (0.129 vs `oracle-3`, 0.127 vs `shots-3`) |
| Colour space | decode before Uber, fitted | **settled statically**: Owlcat's own Stop NaN pass decodes [I, strong] |
| Bloom | none | **BloomEnhanced**, Owlcat's shaders, the cave profile's settings [M] |
| Frame intervals ≥ 33 ms, 5-min `Fifo` route | 9 (max 1.09 s) | **0** in the final run (max 22.7 ms) [M]; see §1 for why that's partly luck |
| Renderer ready / first frame | 0.15–0.17 / 0.33–0.38 s | 0.15–0.17 / 0.34–0.40 s [M] (38 pipelines, was 34) |

## 1. Frame stalls

**Tooling [M].** `wr-game --mode bench` now writes `frames.csv`: per frame the main-world
`Update`, the render world's start, swapchain acquire (around Bevy's `prepare_windows`), our
recording and submit, and the render world's end, each with the calling thread's CPU time,
thread id and QoS class (`crates/wr-game/src/frames.rs`; collected only in bench mode). The
acquire marks are separate systems, so only their wall-time difference is meaningful.
Analysis scripts (local, gitignored): `cache/poc6/tools/stalls.py` (phases around each long
interval), `stalls2.py` (adds CPU time and counters), `side-by-side.py` (sheets). A Metal System Trace of a whole 5-minute run is 22 GB and takes xctrace longer to
finalise than to record, so traces are kept to ~10 s at one point (≈300 MB, deleted after
export).

**What the stalls were:**

1. **The display going idle [M/I].** The original game holds a `PreventUserIdleDisplaySleep`
   assertion ("disable screen saver", in `pmset -g log` for every run of it); `wr-game` held
   none. During first light's unattended bench, powerd's `delayDisplayOff` assertion timed out
   at 04:56:15, inside the run [M]; that run's 1.09 s stall and the unpaced 2.3 ms frames at
   point 7 fit a display that went idle [I]. With the display kept awake the same 5-minute
   run gave 4 and 2 intervals ≥ 33 ms (max 40.6 / 49.3 ms) instead of 9 (max 1,087 ms) [M].
   **Fix:** `wr-game` holds the same assertion in shots and bench modes (`caffeinate -d -i
   -w <pid>`, released when the process exits).
2. **Bursts of CPU starvation from outside the process [M symptoms, I cause].** The remaining
   stalls come in windows of a few seconds in which every phase slows; the recording thread's
   CPU time per frame grows ~5× (1.3 → 7–9 ms) and its wall time more, which is what a thread
   running on a slower or contended core looks like. They never sat in the acquire (max
   10–12 ms, no occluded frames). They hit runs with and without our QoS change, with and
   without artificial load, in no fixed place on the route: of six 80–160 s A/B runs, two had
   a burst (181 and 170 intervals ≥ 33 ms), one at default QoS and one at user-interactive.
   The machine was shared with other agents' builds, Codex runs, OBS and the webcam service
   (load average 5–6), so the likeliest cause is outside the process [I], not shown.
   **Not fixed.** Clean numbers need an idle machine; a short System Trace caught during a
   burst would settle it.
3. **Our own CPU cost [M].** Recording ~889 G-buffer draws took 1.3 ms of the render thread
   per frame (p99 up to 3.1 ms). They are now pre-recorded once into a wgpu render bundle:
   1.00 ms p50, 1.2 ms p99, **pixel-identical** output (`WR_NO_BUNDLES=1` restores the old
   path). With this round's 15 extra post passes (bloom and Stop NaN) the recording is 1.40 ms p50, 2.07 ms p99.
4. **GPU [M, Metal System Trace, point 0].** Before bloom: GPU frame span p50 2.49 ms, p99
   4.59 ms; the grading LUT cost 0.50 ms of fragment time every frame. The LUT's inputs are
   constant here, so it is now **built once** (same pixels, since the inputs don't change; it
   would need re-arming if they ever changed at run time; nothing does that yet). With
   bloom: span p50 3.39 ms, p95 4.01, p99 4.31. The 14 bloom passes' own fragment work is
   only 0.145 ms; the rest of the increase is probably the fixed cost of 15 more render
   passes (bloom plus Stop NaN) [I], since other passes' times also moved between the two
   traces.

**QoS:** the render thread and Bevy's compute pool ran at default QoS (0x15); `--qos
interactive` (now the default) gives them the main thread's user-interactive class (0x21),
which Apple recommends for frame-critical threads. The A/B above couldn't show an effect
through the outside bursts; it's kept as the right default, unproven [I].

**Frame pacing at 240 Hz** (POC-0's open problem, not a stall): see §4 for the final runs.
The CPU frame (~4 ms render world, of which ~2 ms waiting for a drawable) and the GPU frame
(3.4 ms with bloom) both sit just under the 4.17 ms budget, so intervals are bimodal between
4.17 and 8.33 ms and small changes flip runs between mostly-240 and mostly-120 Hz.

## 2. Colour space: settled

gpt-6.1-sol read `Owlcat.Runtime.Visual`, the shipped data and the MSL (read-only); the key
points were checked by hand [I, strong; a frame capture would make it M]:

- The project is **Linear** (`m_ActiveColorSpace = 1`), the pipeline asset has
  `m_SupportsHDR = true` and the level's MainCamera `m_HDR = true`, so `_CameraColorRT` is
  `DefaultHDR` (RGBA16F on Metal; float formats do no sRGB conversion).
- Deferred lighting and fog **encode linear → sRGB in the shader** and write those numbers
  into the float target (fog blends in that encoded domain).
- `PostProcessPass.Render` **always starts with a Stop NaN blit** (camera colour →
  `_TempTarget`), and the shipped `Hidden/Owlcat Render Pipeline/Stop NaN` fragment
  **decodes sRGB → linear** (and zeroes NaN/Inf). Bloom and Uber read its output.
- Uber encodes to sRGB again at the end; `FinalBlit` (to the backbuffer) decodes and the sRGB
  drawable re-encodes.

So first light's fitted "decode before Uber" is exactly Stop NaN. **Change:** our WGSL
linearize pass (and `WR_LINEARIZE`) is gone; the renderer runs Owlcat's Stop NaN program.
FLIP is identical to first light's per point [M], as expected. (`first-light.md` rounded
points 0 and 3 up to 0.257 and 0.130; its JSON says 0.2565 and 0.1295.)

## 3. Bloom (BloomEnhanced)

Ported from `PostProcessPass.SetupBloomEnhanced` onto Owlcat's shipped
`Hidden/Owlcat Render Pipeline/BloomEnhanced` programs (`crates/wr-game/src/owlcat/post.rs`
`Bloom`, the post section of `Renderer::build`): prefilter at half size (pass 0, the
`ANTI_FLICKER` variant as the profile asks), `clamp((int)(log2(720) + radius − 8), 1, 16)` = 7
downsamples (pass 1, then 2) to 10×5, six upsamples (pass 3, each with its `_BaseTex`), and
Uber's `_BLOOM_HQ _HDR_GRADING` variant with `_Bloom_Params` (intensity 0.3, luminance-
normalised linear tint), `_Bloom_RGBM` 0 (RGBA16F targets) and `_Bloom_Texture` = MipUp0.
Constants from the cave profile: threshold 0.75 (gamma) → linear, soft knee 0.5, radius 6,
clamp 65472, no dirt. Lens dirt (`_BLOOM_HQ_DIRT`) would need the dirt texture imported; the
renderer logs it as skipped if a profile has dirt. A unit test pins the pyramid's shape and
constants for this profile. `WR_NO_BLOOM=1` turns it off.

**FLIP per point [M]** (vs `oracle-2` pass 1; noise floor per point 0.021–0.074):

| Point | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | mean |
|---|---|---|---|---|---|---|---|---|---|
| First light | 0.256 | 0.161 | 0.120 | 0.129 | 0.077 | 0.155 | 0.131 | 0.120 | 0.144 |
| Stop NaN (replaces our linearize) | 0.256 | 0.161 | 0.120 | 0.129 | 0.077 | 0.155 | 0.131 | 0.120 | 0.144 |
| **+ BloomEnhanced** | 0.252 | 0.134 | 0.101 | 0.116 | 0.077 | 0.145 | 0.099 | 0.086 | **0.126** |
| same, vs `oracle-3` | 0.252 | 0.136 | 0.105 | 0.117 | 0.082 | 0.147 | 0.101 | 0.088 | 0.129 |
| same, vs `shots-3` | 0.251 | 0.136 | 0.103 | 0.116 | 0.078 | 0.147 | 0.100 | 0.086 | 0.127 |

Sheets (original | first light | round 2): `fixtures/captures/poc6/round2/side-by-side-0-3.jpg`,
`side-by-side-4-7.jpg`; FLIP maps in `round2/bloom/maps-*`. By eye: the fungi and eggs now
glow like the original's; the original still has more glow at point 5, from glow particles
we don't draw [I].

## 4. Final timing runs

Full route, 8 points × 37.5 s, `Fifo` on the 240 Hz panel, OS shader cache warm, display
kept awake, the oracle lock held and no game running (checked in `pmset -g log`), other
agents' builds and Codex runs still active [M]:

| Run | p50 | p95 | p99 | max | ≥ 33 ms |
|---|---|---|---|---|---|
| First light (display idle mid-run) | 4.10 | 6.98 | 9.62 | 1,087 | 9 |
| Round 2 before bloom, timeline only (`bench-fifo-1`, `-2`) | 4.19 / 4.18 | 7.3 / 7.0 | 8.69 / 8.52 | 40.6 / 49.3 | 4 / 2 |
| **Round 2 final** (`bench-fifo-after-1`) | **7.46** | 8.86 | **12.69** | 22.7 | **0** |
| same, again (`bench-fifo-after-lat2`) | 7.25 | 8.77 | 12.68 | 46.0 | 1 |
| same, surface frame latency 3 (`bench-fifo-after-lat3`) | 7.61 | 9.17 | 12.72 | 48.0 | 3 |

In ms, intervals between main-world updates (what WrathStats measures for the original). The
original on this route: p50 12.45, p99 21.1, vsync-locked at 80 fps; the named target is
p99 ≤ 15.8 ms, met with room. **Bloom cost us 240 Hz**: before it about 83% of intervals
were one refresh (4.17 ms), after it about half are two (8.33 ms), because the GPU frame is
now 3.4 ms p50 / 4.0 ms p95 against a 4.17 ms budget. A third queued frame
(`--latency 3`) doesn't help (four 10-s A/B runs at point 0: p50 4.56–4.71 ms either way).
Stalls ≥ 33 ms: 0, 1 and 3 in the three final runs, all short bursts as in §1.

## 5. Review

GPT 6 Astra (high), read-only, over the whole diff against the decompiled C# and the packed
shaders. It confirmed the bloom pyramid (level sizes, constants, texture flow, texel sizes,
tint, RGBM), Stop NaN's placement, the LUT caching, the render bundle's depth/stencil flags
and the unsafe blocks. Findings, each checked against the code:

| Finding | Verdict | Fix |
|---|---|---|
| The timeline collected marks in every mode, growing without bound in `view` | Real | Collected only in bench mode, with capacity reserved up front |
| Acquire markers are separate systems, so their CPU-time difference isn't the acquire's | Real (the analysis only used their wall time) | Documented on the marks; no CPU claim is made for the acquire |
| `_BLOOM_HQ_DIRT` would be selected without its texture and constants | Real, latent (the cave has no dirt) | Falls back to `_BLOOM_HQ` and logs "lens dirt not imported"; the dirt variant is no longer imported |

A second Astra pass over the fixes and this handoff found no blocker and confirmed every
bloom FLIP value and aggregate, the recording timings and the final run's 0 stalls / 22.7 ms
maximum. Its findings, all fixed: two first-light table values mis-rounded (now 0.256 and
0.129, from the JSON); causal wording in §1 stronger than the evidence (now marked [I]); the
LUT doc claimed automatic invalidation that doesn't exist (now says nothing re-arms it); the
shadow-light count needed its "candidates before culling" qualifier; the timeline's buffer
reservation could overflow on a pathological `--dwell` (now bounded); a stale comment about
lens dirt.

## Next steps, in order

1. **Shadows**, from `docs/poc6/shadows-spec.md` (Sol's static reading, spot-checked): 48
   shadow-enabled, non-baked candidate lights in the cave before culling (31 spots, 17
   points; seven with zero strength), all using Owlcat's
   buffer-driven caster path. In order: import the materials' `ShadowCaster` variants
   (both `GEOMETRY_CLIP` states); a 4096² depth atlas, a comparison sampler, the 48-byte
   `_ShadowDataBuffer` and 112-byte `_ShadowMatricesBuffer` records and the 96-byte common
   buffer; MaxRects allocation in visible-light order with 128-px quantised sizes; spot and
   tetrahedral point matrices with guard angles and biases; shadow bits in
   `_LightPositionsAndPackedData.w`; per-face caster constants (not one camera-global
   buffer); depth clamp and raster slope bias in the pipeline state; then the `SHADOWS_SOFT`
   DeferredLighting variant (its bindings differ from the no-shadow one). Validate a single
   spot first. Estimate 3–5 days.
2. **Light probes** (tetrahedral SH interpolation for the 30 non-lightmapped renderers and the
   baked terrain), then reflection-probe selection rules.
3. **Post, rest:** film grain (needs the `PostProcessData` grain textures from
   `globalgamemanagers.assets`, a MonoBehaviour without type trees; `Random.value` offsets per
   frame), dithering, SMAA (if Ember's AA setting is SMAA; `CameraAntialiasingHelper` applies
   it), HBAO (active, intensity 1.59).
4. **240 Hz pacing:** drawing our final blit straight into the swapchain would drop Bevy's
   sprite and upscaling passes (~0.3 ms GPU, two passes); fewer, larger post passes would
   cut the fixed per-pass cost bloom added. Then re-measure on an idle machine.
5. **Captures of the original** (not made this round; the game was busy with the oracle
   runs): a Metal frame capture at route point 4 would confirm `_CameraColorRT`'s format,
   the Stop NaN draw (encoded in, linear out), the shadow atlas format, clear value and
   sampler compare function. Render-target dumps at points 0 and 5 remain useful.

## How to rerun

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo build --release -p wr-import -p wr-game
target/release/wr-import pack
R=fixtures/captures/poc6/round2
target/release/wr-game --windowed --out $R/bloom                     # shots
cache/tools/flip-venv/bin/python oracle/tools/compare-shots \
  fixtures/captures/poc0/oracle-2/route-pass1.json $R/bloom/route-pass1.json -o out.json --maps maps/
# timing: take builds/.oracle-run.lock first; check no game runs
# (pgrep -f 'Wrath.app/Contents/MacOS'; a bare 'Pathfinder Wrath' also matches other
# agents' shells that mention the save folder)
target/release/wr-game --mode bench --present fifo --out $R/bench    # + frames.csv
```

Options added this round: `--qos default|interactive` (default interactive), `--latency N`
(surface maximum frame latency), `WR_NO_BLOOM`, `WR_NO_BUNDLES`; removed: `WR_LINEARIZE`.
