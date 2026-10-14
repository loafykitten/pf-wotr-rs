#!/usr/bin/env bash
# Fetch Bevy's sample Fox.glb (CC0 model from the glTF sample models, as shipped in the Bevy
# repo at the v0.20.0 tag) into spikes/render/assets/ (gitignored).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
dest="$here/assets/models/animated/Fox.glb"
mkdir -p "$(dirname "$dest")"
if [[ ! -s "$dest" ]]; then
  curl -fsSL -o "$dest" "https://raw.githubusercontent.com/bevyengine/bevy/v0.20.0/assets/models/animated/Fox.glb"
fi
ls -l "$dest"
