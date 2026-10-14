#!/usr/bin/env bash
# Record the scene under Metal System Trace and summarise per-pass GPU time.
#   scripts/trace-scene.sh OUT_DIR [scene options...]
# The .trace and XML exports go to cache/poc0-render/traces/ (large, gitignored); the
# per-pass CSV goes to OUT_DIR/gpu_passes.csv.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$1"; shift
mkdir -p "$out"
traces="$here/../../cache/poc0-render/traces"
mkdir -p "$traces"
name="$(basename "$out")"
trace="$traces/$name.trace"
rm -rf "$trace"
xcrun xctrace record --template 'Metal System Trace' --output "$trace" --no-prompt \
  --time-limit 25s --launch -- "$here/target/release/render-spike" scene --seconds 6 --warmup 1 --out "$out" "$@"
xcrun xctrace export --input "$trace" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > "$traces/$name-intervals.xml"
xcrun xctrace export --input "$trace" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-application-encoders-list"]' > "$traces/$name-encoders.xml"
# Skip the load: summarise only GPU work after the first 4 s of the process's GPU activity.
python3 -I "$here/scripts/gpu_trace_summary.py" "$traces/$name-intervals.xml" "$traces/$name-encoders.xml" \
  render-spike "${SKIP_SECONDS:-4}" "$out/gpu_passes.csv" | tee "$out/gpu_passes.txt"
