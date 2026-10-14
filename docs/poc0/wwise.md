# POC-0 Wwise spike: go

**Verdict: go for Decision 5.** Rust can drive the game's own Wwise runtime directly:
`AkSoundEngine.bundle` loads with `dlopen`, initialises with the game's real settings,
loads the game's packages and banks, streams a voiced line from `MeetSeelahAnevia`,
delivers end-of-event, duration and marker callbacks, pans and attenuates a moving 3D
source, suspends and resumes, and shuts down cleanly, repeatedly. Every item on Decision
5's checklist passed on Ember's M2 Max (2026-10-10). The integration needs no Unity and no
Mono: the bundle's only dependencies are system frameworks.

The open items are a listening check by Ember, and whether to keep `InitCommunication`
(see "Deviations").

Code: [`spikes/wwise/`](../../spikes/wwise/) (README there). Run reports:
[`results/run-default.txt`](../../spikes/wwise/results/run-default.txt) (the game's own
output setting, i.e. the system device; 3 init/term cycles) and
[`results/run-no-output.txt`](../../spikes/wwise/results/run-no-output.txt) (silent
device, 10 cycles); the settings decode, socket list and marker scan are in
[`results/evidence.txt`](../../spikes/wwise/results/evidence.txt). **[M]** marks something
measured (in those files unless it says "earlier runs", meaning about ten development
runs whose output wasn't kept), **[I]** something inferred from the decompiled code, the
disassembly or Wwise's documented behaviour.

## What the bundle is

- `Contents/PlugIns/AkSoundEngine.bundle`: universal binary, arm64 slice used. **Wwise
  2019.2.15 build 7667 [M]** (`GetMajorMinorVersion`/`GetSubminorBuildVersion`), a
  **Profile** build (it contains `AK::Comm` and output capture) [I, from its symbols].
  Banks are version 135 (`SoundbanksInfo.xml`, `Init.bnk` header), which matches 2019.2.
- It's the Unity integration's native half: 1,068 SWIG `CSharp_*` exports that the game's
  `AkSoundEnginePINVOKE` (in `Assembly-CSharp.dll`, not `-firstpass`) calls through
  P/Invoke. Its only imports are libc++, libSystem, AudioToolbox, AudioUnit, CoreAudio,
  CoreFoundation and Foundation [M, `otool -L`/`nm -u`]: nothing from Unity or Mono.
- All the project's effect plug-ins (Tremolo, Stereo Delay, RoomVerb, Compressor, and so
  on) are statically linked into it [M, registration symbols]. The separate
  `libAkTremolo.bundle` and `libAkStereoDelay.bundle` are never loaded [M].
- The spike resolves 158 of the 1,068 exports. The game's own code outside the Wwise
  integration uses about 40 distinct `AkSoundEngine` calls (mostly `SetRTPCValue`,
  `SetState`, `SetSwitch`, `PostEvent`, `SetMultiplePositions`) [M, grep].

## Checklist evidence

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | Initialisation as the game does it | pass | Same calls, same order, same values as `AkWwiseInitializationSettings.InitializeSoundEngine` and `AkAudioService.Initialize`; every call returned `AK_Success` [M]. Settings decoded from the game's serialized asset (below). The `default` run keeps the game's empty device shareset (ID 0); the silent run overrides it with `No_Output`. |
| 2 | Banks and packages | pass | `Init.bnk`, the 11 preload packages and the 8 preload banks all load [M]. Each bank's returned ID equals our FNV short ID [M]. The cue's bank (`Dialogs_Prologue_ENG`) and its streamed `.wem` are in `Wrath_Main_VO_Dialogues_Prologue.pck` [M, our `.pck` reader]. |
| 3 | One streamed voiced cue | pass | `MeetSeelahAnevia/Cue_0001` → key `37dd6a74…` → `Sound.json` → event `CH0_MeetSeelahAnevia_01`, posted by name on the dialogue speaker object as `LocalizedString.PlayCueVoiceOver` does. `AK_Duration` reports media 957188610, **streaming = true** [M]. End of event **3.977 s** after posting, measured at 60 Hz frame resolution (16.7 ms); `SoundbanksInfo.xml` says 3.985 s [M, `default` run]. Language `English(US)` set at init and read back [M]. |
| 4 | Callbacks | pass | Polled the way `AkCallbackManager.PostCallbacks` does (serializer lock, walk the list, unlock). Received `AK_EndOfEvent` (4 of 4 posts), `AK_Duration` (3 of 3) and `AK_Marker` (1, label and position read) [M]. Monitoring messages arrive through the same queue [M]. |
| 5 | Moving 3D source | pass | `CH0_MeetSeelahAnevia_02` (15.6 s, 25 m attenuation) on an emitter moved every frame. Stereo balance in the capture (speech windows only): **−9.4 dB** with the emitter more than 4 m left of the listener, **−0.5 dB** within 1.5 m of centre, **+9.3 dB** more than 4 m right; past 30 m every sample of all 52 windows is **exactly zero**, against −24.1 dBFS (90th percentile of speech windows) within 9 m [M]. Window-to-position mapping is approximate (see "Capture timing" below). |
| 6 | Suspend/resume, shutdown | pass | `Suspend(false)` for 1.5 s: Wwise rendered 2 more audio frames (against 70 in real time) and the play position stayed at 881 ms; after `WakeupFromSuspend` playback continued from there and ended 1.5 s late [M]. `Suspend(true)` keeps rendering [M]. Shutdown in the game's order takes 86–104 ms (70–135 ms in earlier runs); `IsInitialized` is false afterwards [M]. 10 init/term cycles in one process: resident memory within 0.2 MiB, thread count back to 1 each time [M, silent device]; with the system device CoreAudio keeps 3–4 threads of its own after `Term`, and resident memory and threads stay flat over 3 cycles [M]. |
| 7 | Proof of audio | pass | Wwise's own `StartOutputCapture` wrote the master mix (48 kHz stereo, 30 s) under `cache/` [M]. Cue A audible for 3.8 s at 100 ms resolution (expected 3.985 s); peak −10 dBFS; panning and attenuation as in row 5 [M]. Nobody has listened yet. |
| 8 | Threading, Bevy needs | notes | Below. |

## The startup sequence

What the game does on macOS, reproduced call for call by `SoundEngine::load`/`init`,
`load_packages` and `load_bank` (all on one thread, the game's main thread):

1. `SetErrorLogger(cb)` (static constructor of `AkLogger`), then `IsInitialized()`.
2. `new_AkInitializationSettings()`, fill it (values below), `Init(settings)`. Inside the
   bundle this runs `MemoryMgr::Init` (default memory settings), `StreamMgr::Create`, the
   Unity integration's blocking low-level IO hook with file-package support
   (`CAkDefaultIOHookBlocking`), `SoundEngine::Init` and `MusicEngine::Init` [I,
   disassembly]. On failure the game calls `Term()`.
3. `InitSpatialAudio(settings)`; `InitCommunication(settings)` (result ignored).
4. `SetBasePath(<StreamingAssets>/Audio/GeneratedSoundBanks/Mac)`,
   `AddBasePath(Application.persistentDataPath)`,
   `SetDecodedBankPath(<base>/DecodedBanks)` and `AddBasePath` of the same,
   `SetCurrentLanguage("English(US)")`.
5. `AkCallbackSerializer::Init(buffer, 4096)` (a 4 KiB buffer the caller owns until
   shutdown), then `LoadBank("Init.bnk")`.
6. `AkCallbackSerializer::SetLocalOutput(ErrorLevel_All)`: engine log messages are queued
   with the callbacks.
7. `AddBasePath(<base>/Packages)`, then `LoadFilePackage` for each preload package:
   `Wrath_Main_Default`, `_Music`, `_VO_PC`, `_Creatures`, `_VO_Companions`, `_Narrative`,
   `_VFX`, `_Ambs_Prologue`, `_VO_Dialogues_Prologue`, `_VO_Dialogues_Other`,
   `_VO_Dialogues_PartyBanter` (`.pck`).
8. Register the listener object, set its position, `SetDefaultListeners([listener])`.
9. `LoadBank` (synchronous, by name) for the preload banks: `Main`, `VFX`, `Interface`,
   `Music`, `Tech`, `Dialogs_ENG`, `Dialogs_Prologue_ENG`, `Dialogs_PartyBanter_ENG`.
10. Every frame, at the end of `LateUpdate`: object positions, `WakeupFromSuspend()`,
    poll callbacks, `RenderAudio()`, then deferred `UnregisterGameObj`s.
11. Shutdown (`TerminateSoundEngine`): `StopAll`, `ClearBanks`, `RenderAudio`, poll until
    five empty polls (10 ms apart), `Term`, poll once more, `AkCallbackSerializer::Term`,
    free the buffer.

Leaving the prologue loads the `MainGame` chunk (12 more packages, 6 more banks), and each
DLC has its own chunk (two packages and one bank); all are in `settings.rs`.

### The game's settings

The game's values come from the serialized `AkWwiseInitializationSettings` platform object
`Mac` in `Data/sharedassets0.assets`, which we decoded field by field in declaration order
(the decode consumed the object's 2,564 bytes exactly) [M, `scripts/decode_settings.py`,
output in `evidence.txt`]. The package and bank chunks come
from the `AudioFilePackagesSettings` asset in the same file [M]. They're Wwise's Unity
defaults except where marked:

| Setting | Value | vs the default created by `new_AkInitializationSettings` [M] |
|---|---|---|
| Samples per frame / sample rate / refills | 1024 / 48,000 Hz / 4 | same |
| Command queue / max positioning paths | 256 KiB / 255 | same |
| Monitor queue pool | 64 KiB | **default 1 MiB** |
| Output device | default system device, speaker panning, channels from the device | same (the device reported 2 channels, mask 0x3 [M]) |
| Plug-in path | `Wrath.app/Contents/Plugins/` (`Application.dataPath + "/Plugins/"`) | default null |
| IO memory / auto-stream buffer / pinned cache | 2 MiB / 380 ms / unlimited | same |
| Async open / stream cache / game-sync preparation | off / off / off | same |
| Spatial audio | 8 portals deep, all diffraction flags, 100 rays, reflection order 1, transmission on | Unity defaults |
| Comms | pool 256 KiB, discovery port 24024, `InitializeSystemComms` **false**, network name "" | |
| Render during focus loss | false | |
| Callback buffer / engine logging | 4096 bytes / on | |

`settings.rs` holds them as constants; `engine.rs` applies them in the order
`AkCommonPlatformSettings.AkInitializationSettings` does.

## How the FFI works, and its quirks

- **No native struct layouts.** All settings structs are created with the SWIG
  constructors, reached through the SWIG member getters (which return interior pointers)
  and written through the SWIG setters, exactly as the C# does. The getters also give a
  layout fingerprint (`deviceSettings` at +8, `initSettings` at +80, `platformSettings` at
  +200, `musicSettings` at +304) [M], which a future check can compare after game updates.
- **Mono's marshalling, not C++'s.** C# `bool` arguments are 4-byte (the SWIG side takes
  `unsigned int`; checked in the disassembly). `bool` returns come straight from C++, so
  only the low byte is defined; we read a `u8`. `LPWStr` strings are UTF-16 (Mono uses
  UTF-16 on every platform), `LPStr` strings are 8-bit. `Vector3` by value is three `f32`
  (an AAPCS64 homogeneous aggregate); the C++ side takes Wwise's identical `AkVector`.
- **Callbacks are a sentinel.** `PostEvent` with flags passes the callback pointer `1`
  (any non-null value); the SWIG wrapper substitutes the serializer's own function
  (disassembly) [I], and the cookie comes back as the package pointer [M]. Nothing native
  ever calls into Rust except the error logger.
- **Base paths are most-recent-first, and always prepended.** The IO hook searches base
  paths newest first and builds `<base>/<name>` even for absolute names, so
  `StartOutputCapture("/abs/path.wav")` fails [M]. The spike adds a cache folder as the
  newest base path and captures by relative name.

## Capture timing

The analysis maps capture time to scenario time with Wwise's buffer tick (1024 samples
per tick), taking the tick read right after `StartOutputCapture` as sample zero. Both the
capture start and position updates are queued commands applied on a later audio buffer,
so the mapping is good to a few 21 ms buffers, not exact [I]. The windows it classifies
are 100–500 ms and the emitter moves 2.7 m/s, so this doesn't change the verdicts, but
exact attribution would need capture markers (`AddOutputCaptureMarker`) read back from
the WAV. The runner exits non-zero if any capture check fails or lacks evidence.

## Cost [M]

First the two kept runs (`default` / silent), then the range over earlier runs (debug and
release builds, both devices) where it's wider.

| | Kept runs | Earlier runs | Notes |
|---|---|---|---|
| `dlopen` + symbol resolution | 6.2 / 7.0 ms | 5–23 ms | |
| `Init` through `Init.bnk` | 0.9 / 1.4 ms | 0.7–2.3 ms | |
| 11 preload packages | 0.9 / 1.6 ms | 0.7–1.2 ms | header tables only |
| 8 preload banks | 44.6 / 55.5 ms | 37–57 ms | 117 MiB of bank data; `VFX` (76.8 MiB) and `Main` (33.3 MiB) dominate |
| Sum of those engine calls | 52.6 / 65.5 ms | 44–83 ms | excludes our `Sound.json`/XML lookups (~80–160 ms), which a game wouldn't do at startup |
| Process resident memory | 52.0 before `dlopen` → 64.6 after `Init` → 186.9 after banks, peak 198.1 while playing (MiB) | similar | so Wwise adds about **135–146 MiB** to the process, almost all bank data |
| Process CPU while playing one voice | 1.0% / 1.1% of one core | 0.9–1.5% | Wwise's own audio-thread estimate 1.1% / 2.1% (one earlier spike to 21.6%) |
| Main-thread work per 60 Hz frame | p50 5 / 6 µs, p99 63 / 159 µs, worst 0.27 / 0.59 ms | worst 0.1–7.7 ms | cause of the occasional slow frame not identified |
| Threads | +5 at `Init` (4 at Mach priority 47, 1 at 31), +1 CoreAudio IO (priority 97) | same | two priority-47 threads do the work: 50–105 ms of CPU each over 25 s of playback |

For scale, the original uses 4.4–4.6 GB resident in total. How much of that is Wwise
inside Unity isn't measured; with the same runtime and banks it's probably of the same
order as here, but voices, streaming and the scene's banks differ [I].

## Threading [M/I]

- The game makes every Wwise call from Unity's main thread [I, decompiled]. Wwise's API
  is thread-safe for event, position and game-sync calls (they go into the command queue)
  [I, Wwise docs], but init, term, bank and package management and callback polling should
  stay on one owning thread, as in the game.
- Rendering happens on Wwise's own thread (`bUseLEngineThread = true` [M]) fed by the
  CoreAudio IO thread. `RenderAudio()` only submits the frame's queued commands, so a
  slow or irregular frame rate delays commands but doesn't starve audio. The game calls it
  once per frame at the end of `LateUpdate`; positions take effect on the next audio buffer
  (1024 samples = 21.3 ms).
- Banks load synchronously on the calling thread here (as `LoadBankSync` does in the
  game); 37–57 ms for the preload chunk, so production should load bank chunks off the
  frame (Wwise's async `LoadBank` with a bank callback, or a worker thread).
- The error logger can be called from any Wwise thread; the spike's only appends to a
  mutex-protected buffer.
- Wwise is a process-wide singleton, so `SoundEngine::load` takes a process-wide
  ownership token (a second engine is refused), and lifecycle-dependent calls refuse to
  run unless `init` succeeded (some natives, like `LoadFilePackage`, don't check). It
  also holds raw pointers, so it's `!Send`/`!Sync`: in Bevy it's a non-send resource, or
  the owner of a command channel. Callbacks are copied out under the serializer lock and
  handed to the caller after unlocking.
- The game also calls `WakeupFromSuspend()` every frame [I, `AkAudioService.OnLateUpdate`],
  so a suspend only lasts while the frame loop is stopped (an unfocused player that doesn't
  run in the background). We should mirror the effect (silence on focus loss, if Ember wants
  it), not the per-frame call.
- Dialogue voice end (`VoiceOverStatus.IsEnded`) drives only UI (barks, book events) [I,
  grep], so audio timing doesn't reach the T0 rules.

## A `wr-audio` crate for Bevy (sketch)

- **`ffi`**: generate the full function table from `AkSoundEnginePINVOKE.cs` at build
  time of a codegen tool (the signatures are mechanical; bool/string rules as above),
  instead of hand-writing them. Keep the bundle loaded for the process lifetime.
- **`Engine`** (owner): init/term in the game's order, packages and banks with reference
  counts (`SoundPackagesManager`, `SoundBanksManager`, chunk loading by game phase), async
  bank loads, output capture for tests.
- **Bevy plugin**, in `PostUpdate` after transform propagation:
  - `Emitter` component holding a stable Wwise game-object ID (from our durable entity
    ID, never from Bevy's `Entity`); register on `Added`, `SetObjectPosition` on
    `Changed<GlobalTransform>`, unregister after `RenderAudio` for despawned emitters (the
    game defers unregistration the same way, so a sound posted this frame isn't cut);
  - `Listener` component (the camera rig) feeding `SetDefaultListeners` and the dialogue
    speaker object at the listener;
  - one system that polls callbacks into Bevy messages (`VoiceEnded`, `Marker`, engine
    log), then calls `RenderAudio` last;
  - window focus messages → suspend/wakeup; app exit → the shutdown sequence.
- **Coordinates**: Wwise and the game are left-handed (+Z forward, +Y up); Bevy is
  right-handed (−Z forward). Convert at one boundary.
- **Game-side behaviour** is the bigger job: `SoundState` (music, time-of-day and combat
  states, RTPC volumes from the options menu), unit and animation-event sounds, area
  ambience and zones (`AudioZone`, `AudioObject`, triggers, `EtudeConnectedAudio`), UI
  sounds, barks and dialogue VO with gender, title and race switches. That's
  `Kingmaker.Sound` (2.1k lines) and `Kingmaker.Visual.Sound` (2.4k lines) plus about 90
  call sites, and it needs the scene audio objects from each area's `audio` scene bundle,
  which the extraction pipeline has to import.

## Risks

1. **Version drift on game updates.** A patch that changes the Wwise version or the SWIG
   wrapper could change signatures silently: a missing symbol fails loudly at load, but a
   changed argument list wouldn't. Mitigation: pin the expected version (the integration
   test asserts 2019) and the layout fingerprint, hash the bundle, and regenerate the table
   from the new `AkSoundEnginePINVOKE.cs` when either changes.
2. **ABI assumptions.** The bool, string and `Vector3` rules are checked in the
   disassembly for the functions the spike uses, not for all 1,068. A generated table
   inherits the C# declarations, which Mono already proves against this binary in the
   shipping game. Struct layouts are never assumed.
3. **Callback buffer capacity.** The serializer buffer is 4 KiB, as in the game. What
   happens when it fills (drops or blocks) isn't measured; busy scenes post many
   callbacks. Mitigation: poll every frame (as here), measure in POC-6, raise the size if
   needed (it's our choice, not a fidelity rule).
4. **The No_Output device isn't real time.** It renders about 2–3× faster than real time
   [M], so unattended tests that check timing must use the system device, or audio time
   (buffer ticks) instead of wall time, as the spike's analysis does.
5. **Profile build.** The game ships Wwise's Profile build, with its profiling overhead
   and an open profiler port when `InitCommunication` runs. That's the same as the game;
   we can't switch to a Release build without Audiokinetic's SDK (licensing: per title).
6. **Licensing.** We reuse the game's Wwise runtime unchanged, for personal use only.
   Audiokinetic licenses Wwise per title (PLAN.md ground rules); nothing here changes
   that, and Ember's decision stands.
7. **Not yet heard.** The capture proves a non-silent, correctly panned mix of the right
   length; it doesn't prove it sounds identical to the game (reverb sends, bus effects,
   states and RTPCs the game sets at runtime aren't set here).

## Deviations from the game

- `persistentDataPath` and the decoded-bank folder point at `cache/wwise-spike/` instead
  of the game's save folder and `<banks>/DecodedBanks`, so nothing writes into the game's
  folders. The game never decodes banks (`LoadBankSync` passes `decodeBank: false`) and no
  banks live in its persistent folder, so behaviour is the same [I].
- The spike adds one extra base path (its capture folder), newest first, for the output
  capture. It holds no banks.
- `InitCommunication` runs as in the game, which opens the Wwise profiler's ports on all
  interfaces: UDP 24024 (discovery) and two ephemeral TCP listeners [M, `lsof` in
  `evidence.txt`]. `wr-audio` should make it a developer option, off by default (an
  improvement Ember may want labelled as such).
- The silent run overrides the output device with `No_Output`; the `default` run uses
  the game's setting.
- `SetSwitch` for title and gender is set as the game does (Baron, Female); the race
  switch is skipped because no save is loaded. These steer PC lines, not companion VO [I].

## Integration budget

Effort for a production `wr-audio`, in focused agent days (Claude implementing, Astra
reviewing), with what the spike already provides:

| Piece | Estimate | Notes |
|---|---|---|
| Generated FFI table + safe engine (init/term, banks, packages, async loads, callbacks, capture) | 3–4 days | the spike's `ffi`/`engine` are the template |
| Bevy plugin (emitters, listener, polling, focus, exit, coordinate conversion) | 2–3 days | |
| Voice-over and dialogue hookup (Sound.json, switches, end callbacks for UI) | 1–2 days | |
| Game-side audio behaviour (`SoundState`, music, ambience, zones, triggers, unit and animation sounds, UI sounds, options RTPCs) | 2–4 weeks | ports ~4.5k lines plus ~90 call sites; depends on scene import of audio objects |
| Tests: capture-based regression checks against the original (same scene, same route) | 3–5 days | needs the oracle to capture the original's mix |

So the runtime part is about **1.5–2 weeks**; full audio parity with the game is about
**4–6 weeks**, dominated by game-side behaviour and its scene data, not by Wwise.

## Needed from Ember

- **A listening check**: `cd spikes/wwise && cargo run --release -- --device default`
  plays Anevia's first line, a short Penta banter song, then Seelah's 15-second line moving
  from left to right and fading into the distance, then Anevia's line again with a 1.5 s
  gap. Does it sound like the game? (The captures are also in
  `cache/wwise-spike/captures/`.)
- Whether the Wwise profiler connection (`InitCommunication`) should stay available as a
  developer option.
