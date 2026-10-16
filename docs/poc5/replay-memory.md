# POC-5: the driven replay's memory

Written 2026-10-15 (stream O). `wr-combat replay` on the longest captures (`tb-lizard` of `poc5-tb-119`,
`-121`, `-128` and `-129`: from 2.9 million records, 46 MB gzipped and 477 MB as text, up to 57 MB gzipped)
peaked at 8.5 to 10.4 GB resident; the Mac has 32 GB, and parallel streams queue their replays
(`scripts/heavy.sh`). Now they peak at 1.7 to 2.0 GB (5x less) and run about 40% faster, with every
output byte-identical.

## Where the memory went

A counting global allocator (a temporary probe, not committed) on `poc5-tb-121/tb-lizard --run record`:

| Phase | Heap in use | Peak |
|---|---|---|
| save hydrated, content open | 276 MB | |
| trace text read (`gzip -dc` into one `String`) | 788 MB | |
| trace parsed into `Vec<serde_json::Value>`, text dropped | 3,501 MB | 4,013 MB |
| driven replay, all 2.9M records | 3,508 MB | (flat) |
| end: the oracle's T0 stream | | **6,654 MB** |

Resident memory (`/usr/bin/time -l`) was 8.5 GB: small allocations cost more than they ask for.

1. **The records** (3.2 GB, ~1.1 KB each). A record is a JSON object of 6 to 10 short fields
   (`{"seq":…,"k":"obs","t":402,"ev":"mem_v","f":"stl","u":"35fffa0a-…","v":false}`). As a
   `serde_json::Value` each object is a `BTreeMap` leaf node with room for eleven entries (about 640
   bytes whatever the field count), plus an allocation per key and per string value.
2. **The oracle stream's copies** (+3.1 GB at the end). `oracle_stream` called `without(trace, spans)`
   four times, and each call cloned every record of the trace to drop the echoed and unverified spans.
3. **The text** (0.5 GB, more while `gzip`'s output grew) during the load.

The driver's own indexes, the tape, the sim's event log and the projections were small.

After the change, the same probe: 276 MB hydrated, 1,369 MB with the trace loaded (peak 1,402 MB
while parsing), 1,380 MB peak at the end; 1.7 GB resident.

## What changed

- `combat/oracle/rv.rs`: the trace's records are [`Rv`]s, a compact immutable JSON value. An object is
  one boxed slice of (key, value) pairs sorted by key; keys are interned for the process (the trace
  vocabulary is a few hundred names: one pointer each), strings per trace (unit ids, event names,
  callers repeat); numbers stay `serde_json::Number`s. `Rv` reads like `serde_json::Value`: indexing
  (a missing key or index reads `Null`), `get`, `as_*`, `is_*`, comparisons with strings, numbers,
  booleans and `Value`s, `Display`, `Debug` and `Serialize` all give what the `Value` would (objects
  in key order, the last of duplicate keys kept, as the default `serde_json::Map` does). It is parsed
  by a `DeserializeSeed` that makes numbers as `serde_json`'s own `Value` visitor does. A unit test
  checks parsing, equality, printing and reads against `serde_json` on edge cases.
- `Trace::load` reads `gzip -dc`'s output a line at a time (the text is never held whole).
- `oracle_stream` borrows the records it keeps (`without` is an iterator; `project_records` takes one).
- Readers of trace records take `&Rv` instead of `&serde_json::Value`; the code is otherwise unchanged
  (`r["k"] == "obs"`, `r["t"].as_i64()`, `match v { Rv::Null => …, Rv::String(s) => … }`). The rule-span
  checker in `replay/commands.rs` (`span_ok` and its helpers) and `draw_ok` keep working on
  `serde_json::Value`s: they build and compare JSON, so their callers convert the span (`Rv::to_json`).
- Tests that damage a recording edit it as JSON: `trace.edited(|recs: &mut Vec<Value>| …)` (or
  `to_json`/`from_json`), as before on the clone.

## Measurements

Release build, `wr-combat replay <fixture> --run <run>`, each run alone (`scripts/heavy.sh`),
`/usr/bin/time -l`; before = `poc` at `61bdde0`, after = this change rebased onto it.

| Capture | Trace (gz) | Peak before | Peak after | Ratio | Time before | Time after |
|---|---|---|---|---|---|---|
| `poc5-tb-96/tb-hold` | 3 MB | 1.05 GB | 0.52 GB | 2.0x | 1.7 s | 1.0 s |
| `poc5-tb-95/tb-focus` | 3 MB | 1.19 GB | 0.54 GB | 2.2x | 1.9 s | 1.2 s |
| `poc5-tb-95/tb-nearest` | 3 MB | 1.20 GB | 0.55 GB | 2.2x | 2.0 s | 1.2 s |
| `poc5-tb-109/tb-nearest` | 5 MB | 1.39 GB | 0.59 GB | 2.4x | 2.1 s | 1.2 s |
| `poc5-tb-109/tb-focus` | 5 MB | 1.49 GB | 0.61 GB | 2.5x | 2.3 s | 1.3 s |
| `poc5-tb-113/tb-nearest` | 8 MB | 1.81 GB | 0.65 GB | 2.8x | 3.1 s | 1.8 s |
| `poc5-tb-125/tb-nearest` | 9 MB | 1.96 GB | 0.70 GB | 2.8x | 3.6 s | 2.3 s |
| `poc5-tb-113/tb-focus` | 9 MB | 1.98 GB | 0.68 GB | 2.9x | 3.5 s | 1.9 s |
| `poc5-tb-112/tb-nearest` | 10 MB | 2.12 GB | 0.70 GB | 3.0x | 3.6 s | 2.0 s |
| `poc5-tb-115/tb-focus` | 10 MB | 2.16 GB | 0.73 GB | 2.9x | 4.2 s | 2.5 s |
| `poc5-tb-126/tb-focus` | 10 MB | 2.17 GB | 0.73 GB | 3.0x | 4.2 s | 2.5 s |
| `poc5-tb-122/tb-focus` | 11 MB | 2.37 GB | 0.76 GB | 3.1x | 4.5 s | 2.9 s |
| `poc5-tb-114/tb-nearest` | 12 MB | 2.44 GB | 0.75 GB | 3.2x | 4.5 s | 2.5 s |
| `poc5-tb-117/tb-solo` | 12 MB | 2.45 GB | 0.78 GB | 3.1x | 4.6 s | 2.9 s |
| `poc5-tb-115/tb-nearest` | 12 MB | 2.49 GB | 0.78 GB | 3.2x | 4.8 s | 2.9 s |
| `poc5-tb-120/tb-cast` | 12 MB | 2.48 GB | 0.78 GB | 3.2x | 4.9 s | 3.1 s |
| `poc5-tb-111/tb-focus` | 12 MB | 2.45 GB | 0.75 GB | 3.3x | 4.0 s | 2.3 s |
| `poc5-tb-122/tb-nearest` | 13 MB | 2.57 GB | 0.79 GB | 3.2x | 5.0 s | 3.1 s |
| `poc5-tb-116/tb-hold` | 13 MB | 2.57 GB | 0.79 GB | 3.2x | 5.0 s | 3.1 s |
| `poc5-tb-126/tb-nearest` | 13 MB | 2.67 GB | 0.81 GB | 3.3x | 5.3 s | 3.1 s |
| `poc5-tb-111/tb-nearest` | 14 MB | 2.70 GB | 0.79 GB | 3.4x | 4.6 s | 2.5 s |
| `poc5-tb-127/tb-tie` | 14 MB | 2.78 GB | 0.82 GB | 3.4x | 5.8 s | 3.3 s |
| `poc5-tb-114/tb-focus` | 14 MB | 2.79 GB | 0.81 GB | 3.4x | 5.3 s | 2.9 s |
| `poc5-tb-112/tb-focus` | 16 MB | 3.00 GB | 0.84 GB | 3.6x | 5.2 s | 2.9 s |
| `poc5-tb-125/tb-focus` | 16 MB | 3.13 GB | 0.88 GB | 3.6x | 6.1 s | 3.8 s |
| `poc5-tb-118/tb-chain` | 20 MB | 4.08 GB | 1.04 GB | 3.9x | 8.4 s | 5.2 s |
| `poc5-tb-123/tb-chain` | 23 MB | 4.41 GB | 1.09 GB | 4.1x | 9.0 s | 5.6 s |
| `poc5-tb-124/tb-chain` | 23 MB | 4.51 GB | 1.10 GB | 4.1x | 9.5 s | 5.7 s |
| `poc5-tb-121/tb-lizard` | 46 MB | 8.53 GB | 1.72 GB | 5.0x | 18.8 s | 11.8 s |
| `poc5-tb-119/tb-lizard` | 47 MB | 9.06 GB | 1.81 GB | 5.0x | 17.4 s | 10.7 s |
| `poc5-tb-128/tb-lizard` | 51 MB | 9.20 GB | 1.82 GB | 5.1x | 20.9 s | 12.5 s |
| `poc5-tb-129/tb-lizard` | 57 MB | 10.35 GB | 1.99 GB | 5.2x | 25.1 s | 14.2 s |

Peak = the largest of the fixture's three runs (record, replay-1, replay-2), resident set size in
10^9 bytes; time = their mean wall clock (other streams' builds ran meanwhile, so times are noisy).
All 96 runs together: 633 s before, 378 s after.

Every run's stdout (with the run time masked) and output files (`*.oracle.t0.jsonl`, `*.sim.t0.jsonl`,
`*.state-end.json`, `*.compare-end.json`) are byte-identical before and after on all 96 runs of
`poc5-tb-95`, `-96`, `-109` and `-111` to `-129` at `61bdde0`. The same held at each earlier base the
branch was rebased over (`73abee2`: 90 of 90; `5e12bf2`: 90 of 90; `a8658c3`: 12 of 12), and at
`b8c901c` (stream R's area parts) on the 27 runs of `-96`, `-109`, `-118`, `-119`, `-121`, `-127`,
`-128` and `-129`.

A replay's floor is now about 0.5 GB resident (the content pack, the hydrated save, the nav and scene
caches); a record adds about 0.4 KB (it was 1.1 KB, and twice that at the end), so the ratio grows
with the capture. The POC-5 capture suites (`poc5`, `poc5_agent`, `poc5_log`, `poc5_spells`,
`poc5_ties`, run ignored) pass; their largest test, which edits a long recording as JSON, peaked at
11.7 GB (another stream's run of the same test before this change: 19.8 GB).

## Further room

An `Rv` is 24 bytes and an object entry 32. Splitting `Number` into its three kinds and making strings,
arrays and objects thin pointers would make them 16 and 24 (about a quarter less for the records), at
the cost of an extra indirection; not needed for now.
