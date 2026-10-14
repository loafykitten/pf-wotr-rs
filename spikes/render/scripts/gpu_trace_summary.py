"""Summarise per-pass GPU time for one process from an xctrace Metal System Trace export.

usage: gpu_trace_summary.py INTERVALS.xml ENCODERS.xml PROCESS_PREFIX SKIP_SECONDS OUT.csv

INTERVALS.xml = table metal-gpu-intervals, ENCODERS.xml = table metal-application-encoders-list
(exported with `xctrace export --xpath ...`). Joins GPU intervals to encoder labels (wgpu
passes its pass labels through as Metal encoder labels), skips the first SKIP_SECONDS of the
process's GPU activity (loading), and writes per-label statistics:
  span   = an encoder instance's first GPU start to last GPU end (vertex/fragment/compute overlap)
  busy_* = summed interval durations per GPU channel
Frames are grouped by Metal's own frame number from the encoder list; join coverage is
reported (intervals whose encoder has no label are driver/internal work).
"""
import statistics
import sys
import xml.etree.ElementTree as ET


def rows(path):
    """Yield each <row> as a list of resolved (tag, text, fmt) per column."""
    interned = {}
    for _, el in ET.iterparse(path, events=("end",)):
        if el.tag != "row":
            continue
        out = []
        for child in el:
            ref = child.get("ref")
            if ref is not None:
                out.append(interned.get(ref, (child.tag, None, None)))
                continue
            val = (child.tag, child.text, child.get("fmt"))
            if child.get("id") is not None:
                interned[child.get("id")] = val
            # nested ids (e.g. process inside thread) must be interned too
            for sub in child.iter():
                if sub is not child and sub.get("id") is not None:
                    interned[sub.get("id")] = (sub.tag, sub.text, sub.get("fmt"))
            out.append(val)
        yield out
        el.clear()


def pct(v, q):
    """Nearest-rank percentile."""
    import math
    v = sorted(v)
    return v[min(len(v), max(1, math.ceil(q * len(v)))) - 1]


def main():
    intervals, encoders, prefix, skip, out = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4]), sys.argv[5]
    meta = {}  # encoder id -> (label, frame number)
    for r in rows(encoders):
        # columns: start, duration, thread, process, gpu, frame, cb-label, cb-label-idx,
        # encoder-label, encoder-label-idx, event-type, cmdbuffer-id, encoder-id
        if len(r) < 13 or not (r[3][2] or "").startswith(prefix):
            continue
        frame = r[5][1] if r[5][0] != "sentinel" else None
        meta[r[12][1]] = (r[8][1] or r[8][2] or "?", frame)
    inst = {}  # encoder id -> [label, frame, start, end, busy by channel]
    matched = unmatched = 0
    for r in rows(intervals):
        # columns: start, duration, channel, frame, latency, depth, label, state, uuid, color,
        # process, gpu, subtitle, iosurface, bytes, cmdbuffer-id, encoder-id, submission-id
        if len(r) < 17 or not (r[10][2] or "").startswith(prefix):
            continue
        start, dur, chan, enc = int(r[0][1]), int(r[1][1]), r[2][1], r[16][1]
        if enc in meta:
            matched += 1
        else:
            unmatched += 1
        label, frame = meta.get(enc, ("(unmatched)", None))
        rec = inst.setdefault(enc, [label, frame, start, start + dur, {}])
        rec[2] = min(rec[2], start)
        rec[3] = max(rec[3], start + dur)
        rec[4][chan] = rec[4].get(chan, 0) + dur
    if not inst:
        print("no GPU intervals for process prefix", prefix)
        return
    t0 = min(v[2] for v in inst.values())
    kept = [v for v in inst.values() if v[2] >= t0 + skip * 1e9]
    # Frames by Metal's own frame number (from the encoder list), not by timing.
    frames = {}
    for label, frame, s0, e0, busy in kept:
        if frame is None:
            continue
        f = frames.setdefault(frame, [s0, e0, 0])
        f[0] = min(f[0], s0)
        f[1] = max(f[1], e0)
        f[2] += sum(busy.values())
    n_frames = max(1, len(frames))
    by = {}
    for label, frame, s0, e0, busy in kept:
        b = by.setdefault(label, {"span": [], "busy": {}})
        b["span"].append((e0 - s0) / 1e6)
        for c, d in busy.items():
            b["busy"].setdefault(c, []).append(d / 1e6)
    chans = sorted({c for b in by.values() for c in b["busy"]})
    lines = ["label,instances,per_frame,span_mean_ms,span_p50_ms,span_p95_ms," + ",".join(f"busy_{c.lower()}_ms_per_frame" for c in chans)]
    for label, b in sorted(by.items(), key=lambda kv: -sum(sum(x) for x in kv[1]["busy"].values())):
        sp = b["span"]
        busy = [f"{sum(b['busy'].get(c, [])) / n_frames:.4f}" for c in chans]
        lines.append(f"{label},{len(sp)},{len(sp) / n_frames:.2f},{statistics.mean(sp):.4f},{pct(sp, .5):.4f},{pct(sp, .95):.4f}," + ",".join(busy))
    with open(out, "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"join: {matched} GPU intervals matched to labelled encoders, {unmatched} unmatched (driver/internal work)")
    spans = [(e0 - s0) / 1e6 for s0, e0, _ in frames.values()]
    busy = [b / 1e6 for _, _, b in frames.values()]
    if spans:
        print(f"frames={len(frames)} (by Metal frame number) GPU frame span first-start->last-end ms: mean={statistics.mean(spans):.3f} p50={pct(spans, .5):.3f} p95={pct(spans, .95):.3f} p99={pct(spans, .99):.3f} max={max(spans):.3f}")
        print(f"GPU busy per frame (sum over channels; channels overlap) ms: mean={statistics.mean(busy):.3f} p50={pct(busy, .5):.3f} p99={pct(busy, .99):.3f}")


if __name__ == "__main__":
    main()
