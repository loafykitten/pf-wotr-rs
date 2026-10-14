# How the recent "rewrites" were actually done

Research for PLAN.md, 2026-10-10. The examples Ember cited (Skate 3, Modern Warfare 2,
Minecraft) look alike from the outside, but they use three different methods. Which method
fits Wrath is the first design decision.

## 1. Static recompilation: Skate 3, Modern Warfare 2 (Xbox 360)

- **Skate 3 Recomp** (github.com/mchughalex/skate3recomp) and **MW2 recompiled**
  (github.com/PaulCombal/mw2-recompiled) translate the game's PowerPC machine code to C++
  ahead of time (XenonRecomp). The game's own logic runs unchanged, by construction. The
  human work is the *runtime*: kernel imports, threads, memory, the GPU command stream,
  audio and input.
- MW2's runtime doc (docs/runtime.md) shows where that effort goes: emulating exact
  console behaviour the title depends on (64 KB-aligned physical allocations, real thread
  IDs for render locks, a 50 MHz timebase), plus hooks on individual guest functions.
- Skate 3 Recomp 2.0 (July 2026) replaced the emulated GPU with a **native renderer**
  (D3D12/Vulkan, MoltenVK on macOS), using "exact runtime ports of the game's own
  material shading". On macOS: **~25 → ~250 fps**. It falls back to the emulated
  renderer on failure.
- Both are recent and AI-assisted: MW2's commits are co-authored by Claude Opus 5.5, and
  it went from first commit (27 Sep 2026) to v0.4.0 in two weeks. The community's main
  criticism is maintainability: "the actual native renderer code is such a giant
  unreadable mess that it's practically unmaintainable".
- Neither ships game code or data. Both need the player's own copy.

**Lesson:** the fast successes keep game logic exact *by construction* and spend people
on the platform layer. They then replace the slowest platform piece (the GPU path) with
a native one, and that's where the big speed-ups come from.

## 2. Clean-room reimplementation: Minecraft servers in Rust

- FerrumC, Valence (built on Bevy's ECS), Pumpkin and others reimplement the Minecraft
  *server* from the protocol and observed behaviour. Reported gains are large (one talk:
  27 MB RAM and 1.5% CPU vs 2 GB and 24% for the Java server).
- Client-side Rust reimplementations (e.g. Leafish) have stayed incomplete. The server
  is the tractable half because it's logic and data; the client is presentation.

**Lesson:** logic-and-data reimplementations in Rust succeed and are much faster. The
presentation side is where such projects stall.

## 3. Engine reimplementations of CRPGs: GemRB, xoreos, OpenMW

- GemRB (Infinity Engine: Baldur's Gate, Planescape), xoreos (BioWare Aurora: NWN,
  KotOR) and OpenMW (Morrowind) reimplement the engine and load the original data files.
  This is the closest analogue to a Wrath rewrite.
- They show the legal model (require the user's own data) and the long tail: all three
  have been in development for well over a decade, mostly on engine and presentation
  behaviour, not on reading data formats.

**Lesson:** reading the data is the easy part; the decade goes into matching behaviour.
OpenMW does compare against the original game, so having a reference isn't our edge.
Our edge is instrumentation: Harmony hooks on decompiled code let us record rule traces,
dice and state from inside the running game, in far more detail than comparing from the
outside.

## What this means for Wrath

Wrath is C# IL, not machine code. A recompilation-style port would mean either keeping
.NET and reimplementing the whole UnityEngine API surface underneath (enormous, and the
result isn't Rust), or mechanically translating IL to Rust (unidiomatic, and it would
need a GC and reflection runtime). Neither fits the goal.

What does carry over:

1. **Verified equivalence over a stated test domain, instead of exact by construction.**
   We translate the logic by hand and with LLMs, using the decompiled source as one part
   of the specification, and test differentially against the instrumented game. That's
   how Wrath-Mac's native DXT compressor was shown byte-identical
   (`native/wrathdxt/test/verify.sh` there), but a recompilation's runtime isn't exact
   automatically either, and neither is ours.
2. **Reuse native components the user already has where that's better than rewriting.**
   The game ships an arm64 Wwise runtime with a C API. Like MW2's launcher using "the
   decoder the game links", we may be able to call it instead of reimplementing Wwise.
   Whether that's licensed is a separate question (see PLAN.md, Ground rules).
3. **Put the speed-up where Wrath loses time.** Skate 3's ~10x came from replacing an
   *emulated* GPU; Wrath-Mac already renders natively on Metal, so the renderer alone
   won't repeat that. Wrath's measured losses are loading, GC, first-use pipeline
   creation and asset handling.
4. **Guard against unmaintainable AI-written code:** independent adversarial review
   (GPT 6 Astra) of every change, coverage manifests, and fail-closed handling of
   unimplemented behaviour.
