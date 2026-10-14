//! A safe(ish) layer over [`crate::ffi`] that reproduces the game's Wwise lifecycle.
//!
//! The game drives Wwise from Unity's main thread in this order (decompiled
//! `AkAudioService.Initialize`, `AkWwiseInitializationSettings.InitializeSoundEngine`,
//! `AkSoundEngineController.LateUpdate`, `AkWwiseInitializationSettings.TerminateSoundEngine`):
//!
//! 1. `SetErrorLogger` (static constructor of `AkLogger`), `IsInitialized`;
//! 2. `Init(AkInitializationSettings)`: inside the bundle this runs `MemoryMgr::Init`,
//!    `StreamMgr::Create`, the blocking low-level IO hook with file-package support,
//!    `SoundEngine::Init` and `MusicEngine::Init` (read from the disassembly);
//! 3. `InitSpatialAudio`, `InitCommunication`;
//! 4. `SetBasePath`, `AddBasePath(persistentDataPath)`, `SetDecodedBankPath` +
//!    `AddBasePath(decoded)`, `SetCurrentLanguage`;
//! 5. `AkCallbackSerializer::Init` with a 4 KiB buffer, `LoadBank("Init.bnk")`;
//! 6. `SetLocalOutput(ErrorLevel_All)` (monitoring messages through the serializer);
//! 7. `AddBasePath(<banks>/Packages)`, `LoadFilePackage` for the preload chunk, listener
//!    registration, `LoadBank` for the preload banks;
//! 8. every frame (`LateUpdate`): positions, `WakeupFromSuspend`, poll callbacks,
//!    `RenderAudio`;
//! 9. shutdown: `StopAll`, `ClearBanks`, `RenderAudio`, drain callbacks, `Term`, drain,
//!    `AkCallbackSerializer::Term`, free the buffer.
//!
//! [`SoundEngine`] owns the loaded bundle and that state. It is `!Send` and `!Sync` (it
//! holds raw pointers): keep it on the thread that owns the frame loop, as the game does.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::ffi::{self, AkResult, AkVec3, Api, Ptr, callback_type, wide};
use crate::settings::{ChannelConfigType, GameSettings};
use crate::shortid::short_id;

/// A Wwise game object ID (the game uses `GameObject.GetInstanceID()`).
pub type GameObjectId = u64;

/// Messages from the native error logger. It can be called from Wwise's threads, so it
/// only appends to this buffer; [`SoundEngine::take_native_errors`] drains it.
static NATIVE_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Wwise is a process-wide singleton (global managers, one serializer buffer, one error
/// logger). At most one [`SoundEngine`] may exist at a time; this flag enforces it.
static OWNED: AtomicBool = AtomicBool::new(false);

/// The process-wide ownership token, released when the engine is dropped (after its
/// shutdown has run).
#[derive(Debug)]
struct Ownership;

impl Ownership {
    fn acquire() -> Result<Self> {
        ensure!(
            OWNED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            "another SoundEngine already owns Wwise in this process"
        );
        Ok(Self)
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        OWNED.store(false, Ordering::Release);
    }
}

unsafe extern "C" fn error_logger(message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: Wwise passes a NUL-terminated C string valid for the duration of the call.
    let msg = unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned();
    if let Ok(mut v) = NATIVE_ERRORS.lock() {
        v.push(msg);
    }
}

fn cstr(p: *const c_char) -> Option<String> {
    // SAFETY: callers pass pointers the native API documents as NUL-terminated strings that
    // stay valid while the serializer lock is held (or static, for the language).
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn check(what: &str, r: i32) -> Result<()> {
    let r = AkResult(r);
    ensure!(r.is_success(), "{what} failed: {r}");
    Ok(())
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().with_context(|| format!("non-UTF-8 path {}", p.display()))
}

/// Where the engine finds and puts things.
#[derive(Debug, Clone)]
pub struct EnginePaths {
    /// `AkSoundEngine.bundle/Contents/MacOS/AkSoundEngine`.
    pub library: PathBuf,
    /// Plugin folder passed as `szPluginDLLPath` (`Application.dataPath + "/Plugins/"`; on
    /// macOS `Wrath.app/Contents/Plugins/`, which resolves to `PlugIns/` on a
    /// case-insensitive volume). Holds `libAkTremolo` and `libAkStereoDelay`.
    pub plugin_dir: PathBuf,
    /// Sound bank base path (`StreamingAssets/Audio/GeneratedSoundBanks/Mac`).
    pub sound_banks: PathBuf,
    /// The game adds `Application.persistentDataPath` as a base path. We use a folder we
    /// own, so nothing ever looks in or writes to the game's save folder.
    pub persistent: PathBuf,
    /// `SetDecodedBankPath`. The game uses `<sound_banks>/DecodedBanks`; we use a folder we
    /// own (the game never decodes: `LoadBankSync` passes `decodeBank: false`).
    pub decoded_banks: PathBuf,
}

/// Choices the game doesn't make (or that we deliberately change).
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Audio device shareset name; `None` = the game's setting (empty: system default).
    /// `Some("No_Output")` renders without a device (silent, for unattended runs).
    pub device_shareset: Option<String>,
    /// Call `InitCommunication` like the game does (opens the Wwise profiler's sockets).
    pub init_comms: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self { device_shareset: None, init_comms: true }
    }
}

/// Values read back from the native settings structs (defaults, then the game's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsProbe {
    /// `(field, value)` pairs.
    pub fields: Vec<(&'static str, String)>,
}

/// Byte offsets of the embedded structs inside `AkInitializationSettings`, from the SWIG
/// member getters. A fingerprint of the native layout: if a game update changes it, we
/// notice (we never rely on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutProbe {
    /// `streamMgrSettings`.
    pub stream_mgr: isize,
    /// `deviceSettings`.
    pub device: isize,
    /// `initSettings`.
    pub init: isize,
    /// `platformSettings`.
    pub platform: isize,
    /// `musicSettings`.
    pub music: isize,
    /// `unityPlatformSpecificSettings`.
    pub unity: isize,
    /// `initSettings.settingsMainOutput` relative to `initSettings`.
    pub main_output_in_init: isize,
}

/// A callback drained from the native serializer.
#[derive(Debug, Clone, PartialEq)]
pub enum Callback {
    /// `AK_EndOfEvent`.
    EndOfEvent {
        /// Cookie passed to `post_event`.
        cookie: usize,
        /// Playing ID.
        playing_id: u32,
        /// Event ID.
        event_id: u32,
        /// Game object.
        game_object: GameObjectId,
    },
    /// `AK_Marker`.
    Marker {
        /// Cookie.
        cookie: usize,
        /// Playing ID.
        playing_id: u32,
        /// Marker ID.
        identifier: u32,
        /// Position in the source, samples.
        position: u32,
        /// Label.
        label: Option<String>,
    },
    /// `AK_Duration`.
    Duration {
        /// Cookie.
        cookie: usize,
        /// Playing ID.
        playing_id: u32,
        /// Duration, ms.
        duration_ms: f32,
        /// Estimated duration (with pitch), ms.
        estimated_ms: f32,
        /// Sound node.
        audio_node_id: u32,
        /// Media ID (the `.wem`).
        media_id: u32,
        /// Streamed from disk.
        streaming: bool,
    },
    /// `AK_Monitoring`: an engine log message.
    Monitoring {
        /// `AkMonitorErrorCode`.
        code: i32,
        /// 1 = message, 2 = error.
        level: i32,
        /// Playing ID, if any.
        playing_id: u32,
        /// Game object, if any.
        game_object: GameObjectId,
        /// Text.
        message: Option<String>,
    },
    /// Anything else (bank callbacks, music, …), by type.
    Other {
        /// `AkCallbackType`.
        kind: i32,
        /// Cookie / package pointer.
        cookie: usize,
    },
}

/// Wwise's own resource monitor (`AK::SoundEngine::GetResourceMonitorDataSummary`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceSummary {
    /// Audio thread CPU, percent of one core.
    pub total_cpu: f32,
    /// Of which plug-ins.
    pub plugin_cpu: f32,
    /// Physical voices.
    pub physical_voices: u32,
    /// Virtual voices.
    pub virtual_voices: u32,
    /// Active events.
    pub active_events: u32,
}

/// Callback serializer memory: allocated here, written by Wwise's threads, read only
/// through the native accessors while the serializer lock is held.
#[derive(Debug)]
struct CallbackBuffer {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl CallbackBuffer {
    fn new(size: u32) -> Result<Self> {
        let layout = Layout::from_size_align(size as usize, 16)?;
        ensure!(layout.size() > 0, "callback buffer size must be non-zero");
        // SAFETY: the layout has non-zero size (checked above).
        let raw = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).context("allocating the callback buffer")?;
        Ok(Self { ptr, layout })
    }
}

impl Drop for CallbackBuffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout; dropped only after
        // `AkCallbackSerializer_Term` (see `SoundEngine::shutdown`).
        unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
    }
}

/// Timings of the startup sequence.
#[derive(Debug, Clone, Copy, Default)]
pub struct InitTimings {
    /// `dlopen` + symbol resolution.
    pub load: Duration,
    /// `Init` through `LoadBank("Init.bnk")` (the game's `InitializeSoundEngine`).
    pub engine: Duration,
    /// The preload file packages.
    pub packages: Duration,
    /// The preload banks.
    pub banks: Duration,
}

/// The game's Wwise runtime, loaded and (once [`SoundEngine::init`] succeeds) running.
#[derive(Debug)]
pub struct SoundEngine {
    api: Api,
    callbacks: Option<CallbackBuffer>,
    initialized: bool,
    /// Timings.
    pub timings: InitTimings,
    /// Wwise's defaults as created by `new_AkInitializationSettings`.
    pub defaults: SettingsProbe,
    /// The values after applying the game's settings.
    pub applied: SettingsProbe,
    /// Native layout fingerprint.
    pub layout: Option<LayoutProbe>,
    /// Results of the non-fatal init calls, for the report.
    pub init_log: Vec<(String, AkResult)>,
    /// Dropped last, after `Drop::drop` has shut the engine down.
    _owner: Ownership,
}

impl SoundEngine {
    /// Loads the bundle and installs the error logger (the game's `AkLogger` does this in
    /// its static constructor, before `Init`).
    ///
    /// # Errors
    ///
    /// If another engine exists in this process, or the library or one of its symbols
    /// can't be loaded.
    ///
    /// # Safety
    ///
    /// `library` must be the game's `AkSoundEngine` binary (Wwise 2019.2.15 Unity
    /// integration, the version these declarations were checked against). Matching symbol
    /// names don't prove matching signatures; a different build could have different ones.
    pub unsafe fn load(library: &Path) -> Result<Self> {
        let owner = Ownership::acquire()?;
        let t = Instant::now();
        // SAFETY: forwarded from this function's contract.
        let api = unsafe { Api::load(library) }
            .with_context(|| format!("loading {}", library.display()))?;
        // SAFETY: `error_logger` matches `ErrorLoggerInteropDelegate` (one `char*`), is
        // thread-safe, and stays valid forever (a plain function).
        unsafe { (api.SetErrorLogger__SWIG_0)(Some(error_logger)) };
        Ok(Self {
            api,
            callbacks: None,
            initialized: false,
            timings: InitTimings { load: t.elapsed(), ..InitTimings::default() },
            defaults: SettingsProbe::default(),
            applied: SettingsProbe::default(),
            layout: None,
            init_log: Vec::new(),
            _owner: owner,
        })
    }

    /// Fails unless `init` succeeded and the engine is still running. Several natives
    /// (e.g. `LoadFilePackage`) dereference global managers without checking.
    fn running(&self) -> Result<()> {
        ensure!(self.initialized && self.is_initialized(), "sound engine not initialized");
        Ok(())
    }

    /// The raw function table, for experiments not wrapped here.
    #[must_use]
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// Wwise SDK version as `(major, minor, sub-minor, build)`.
    #[must_use]
    pub fn version(&self) -> (u32, u32, u32, u32) {
        // SAFETY: pure getters, valid before Init.
        let (mm, sb) =
            unsafe { ((self.api.GetMajorMinorVersion)(), (self.api.GetSubminorBuildVersion)()) };
        (mm >> 16, mm & 0xFFFF, sb >> 16, sb & 0xFFFF)
    }

    /// `AK::SoundEngine::IsInitialized`.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        // SAFETY: valid in any state.
        unsafe { (self.api.IsInitialized)() != 0 }
    }

    /// Messages the native error logger received since the last call.
    #[must_use]
    pub fn take_native_errors() -> Vec<String> {
        NATIVE_ERRORS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    fn probe(&self, s: Ptr) -> SettingsProbe {
        let a = &self.api;
        // SAFETY: `s` is a live AkInitializationSettings; every getter takes the interior
        // pointer returned by the matching SWIG member getter.
        unsafe {
            let init = (a.AkInitializationSettings_initSettings_get)(s);
            let dev = (a.AkInitializationSettings_deviceSettings_get)(s);
            let plat = (a.AkInitializationSettings_platformSettings_get)(s);
            let music = (a.AkInitializationSettings_musicSettings_get)(s);
            let out = (a.AkInitSettings_settingsMainOutput_get)(init);
            let ch = (a.AkOutputSettings_channelConfig_get)(out);
            let fields = vec![
                ("init.uMaxNumPaths", (a.AkInitSettings_uMaxNumPaths_get)(init).to_string()),
                (
                    "init.uCommandQueueSize",
                    (a.AkInitSettings_uCommandQueueSize_get)(init).to_string(),
                ),
                (
                    "init.uNumSamplesPerFrame",
                    (a.AkInitSettings_uNumSamplesPerFrame_get)(init).to_string(),
                ),
                (
                    "init.uContinuousPlaybackLookAhead",
                    (a.AkInitSettings_uContinuousPlaybackLookAhead_get)(init).to_string(),
                ),
                (
                    "init.uMonitorQueuePoolSize",
                    (a.AkInitSettings_uMonitorQueuePoolSize_get)(init).to_string(),
                ),
                (
                    "init.uMaxHardwareTimeoutMs",
                    (a.AkInitSettings_uMaxHardwareTimeoutMs_get)(init).to_string(),
                ),
                (
                    "init.uBankReadBufferSize",
                    (a.AkInitSettings_uBankReadBufferSize_get)(init).to_string(),
                ),
                (
                    "init.bUseLEngineThread",
                    ((a.AkInitSettings_bUseLEngineThread_get)(init) != 0).to_string(),
                ),
                (
                    "init.bUseSoundBankMgrThread",
                    ((a.AkInitSettings_bUseSoundBankMgrThread_get)(init) != 0).to_string(),
                ),
                (
                    "init.szPluginDLLPath",
                    cstr((a.AkInitSettings_szPluginDLLPath_get)(init))
                        .unwrap_or_else(|| "(null)".into()),
                ),
                (
                    "init.settingsMainOutput.audioDeviceShareset",
                    (a.AkOutputSettings_audioDeviceShareset_get)(out).to_string(),
                ),
                (
                    "init.settingsMainOutput.channelConfig.uNumChannels",
                    (a.AkChannelConfig_uNumChannels_get)(ch).to_string(),
                ),
                (
                    "platform.uSampleRate",
                    (a.AkPlatformInitSettings_uSampleRate_get)(plat).to_string(),
                ),
                (
                    "platform.uNumRefillsInVoice",
                    (a.AkPlatformInitSettings_uNumRefillsInVoice_get)(plat).to_string(),
                ),
                ("device.uIOMemorySize", (a.AkDeviceSettings_uIOMemorySize_get)(dev).to_string()),
                (
                    "device.fTargetAutoStmBufferLength",
                    (a.AkDeviceSettings_fTargetAutoStmBufferLength_get)(dev).to_string(),
                ),
                (
                    "device.uMaxCachePinnedBytes",
                    (a.AkDeviceSettings_uMaxCachePinnedBytes_get)(dev).to_string(),
                ),
                ("device.uGranularity", (a.AkDeviceSettings_uGranularity_get)(dev).to_string()),
                (
                    "device.uSchedulerTypeFlags",
                    (a.AkDeviceSettings_uSchedulerTypeFlags_get)(dev).to_string(),
                ),
                (
                    "device.uMaxConcurrentIO",
                    (a.AkDeviceSettings_uMaxConcurrentIO_get)(dev).to_string(),
                ),
                (
                    "music.fStreamingLookAheadRatio",
                    (a.AkMusicSettings_fStreamingLookAheadRatio_get)(music).to_string(),
                ),
            ];
            SettingsProbe { fields }
        }
    }

    /// Applies the game's settings to a fresh `AkInitializationSettings`, in the order of
    /// `AkCommonPlatformSettings.AkInitializationSettings` (user `CopyTo`s, then advanced).
    fn apply_settings(
        &mut self,
        s: Ptr,
        g: &GameSettings,
        paths: &EnginePaths,
        opts: &EngineOptions,
    ) -> Result<()> {
        let mut plugin_dir = path_str(&paths.plugin_dir)?.to_owned();
        if !plugin_dir.ends_with('/') {
            plugin_dir.push('/');
        }
        let plugin_path = CString::new(plugin_dir)?;
        let shareset = opts.device_shareset.as_deref().unwrap_or(g.user.audio_device_shareset);
        let shareset_id = if shareset.is_empty() { 0 } else { short_id(shareset) };
        let a = &self.api;
        let (u, adv) = (&g.user, &g.advanced);
        // SAFETY: `s` is a live AkInitializationSettings from `new_AkInitializationSettings`;
        // every setter receives the interior pointer from the matching member getter. The
        // `szPluginDLLPath` setter copies the string (`new[]` + `strcpy`, disassembly), so
        // `plugin_path` only has to outlive the call.
        unsafe {
            let init = (a.AkInitializationSettings_initSettings_get)(s);
            let dev = (a.AkInitializationSettings_deviceSettings_get)(s);
            let plat = (a.AkInitializationSettings_platformSettings_get)(s);
            let music = (a.AkInitializationSettings_musicSettings_get)(s);
            // userSettings.CopyTo(deviceSettings), CopyTo(streamMgrSettings): no-ops.
            // userSettings.CopyTo(initSettings):
            (a.AkInitSettings_uMaxNumPaths_set)(init, u.max_positioning_paths);
            (a.AkInitSettings_uCommandQueueSize_set)(init, u.command_queue_size);
            (a.AkInitSettings_uNumSamplesPerFrame_set)(init, u.samples_per_frame);
            let out = (a.AkInitSettings_settingsMainOutput_get)(init);
            (a.AkOutputSettings_audioDeviceShareset_set)(out, shareset_id);
            (a.AkOutputSettings_idDevice_set)(out, u.device_id);
            (a.AkOutputSettings_ePanningRule_set)(out, u.panning_rule);
            let ch = (a.AkOutputSettings_channelConfig_get)(out);
            match u.channel_config_type {
                ChannelConfigType::Anonymous => {
                    (a.AkChannelConfig_SetAnonymous)(ch, u.number_of_channels)
                }
                ChannelConfigType::Standard => (a.AkChannelConfig_SetStandard)(ch, u.channel_mask),
                ChannelConfigType::Ambisonic => {
                    (a.AkChannelConfig_SetAmbisonic)(ch, u.number_of_channels)
                }
            }
            (a.AkInitSettings_szPluginDLLPath_set)(init, plugin_path.as_ptr());
            // userSettings.CopyTo(platformSettings), CopyTo(musicSettings):
            (a.AkPlatformInitSettings_uSampleRate_set)(plat, u.sample_rate);
            (a.AkPlatformInitSettings_uNumRefillsInVoice_set)(plat, u.refills_in_voice);
            (a.AkMusicSettings_fStreamingLookAheadRatio_set)(music, u.streaming_look_ahead_ratio);
            // advancedSettings.CopyTo(deviceSettings):
            (a.AkDeviceSettings_uIOMemorySize_set)(dev, adv.io_memory_size);
            (a.AkDeviceSettings_fTargetAutoStmBufferLength_set)(
                dev,
                adv.target_auto_stream_buffer_length_ms,
            );
            (a.AkDeviceSettings_bUseStreamCache_set)(dev, u32::from(adv.use_stream_cache));
            (a.AkDeviceSettings_uMaxCachePinnedBytes_set)(dev, adv.maximum_pinned_bytes_in_cache);
            // advancedSettings.CopyTo(initSettings):
            (a.AkInitSettings_bEnableGameSyncPreparation_set)(
                init,
                u32::from(adv.enable_game_sync_preparation),
            );
            (a.AkInitSettings_uContinuousPlaybackLookAhead_set)(
                init,
                adv.continuous_playback_look_ahead,
            );
            (a.AkInitSettings_uMonitorQueuePoolSize_set)(init, adv.monitor_queue_pool_size);
            (a.AkInitSettings_uMaxHardwareTimeoutMs_set)(init, adv.maximum_hardware_timeout_ms);
            (a.AkInitSettings_bDebugOutOfRangeCheckEnabled_set)(
                init,
                u32::from(adv.debug_out_of_range_check_enabled),
            );
            (a.AkInitSettings_fDebugOutOfRangeLimit_set)(init, adv.debug_out_of_range_limit);
            // advancedSettings.CopyTo(platformSettings / unityPlatformSpecificSettings): no-ops.
            (a.AkInitializationSettings_useAsyncOpen_set)(s, u32::from(adv.use_async_open));
        }
        drop(plugin_path);
        Ok(())
    }

    fn layout_probe(&self, s: Ptr) -> LayoutProbe {
        let a = &self.api;
        let off = |p: Ptr, base: Ptr| (p as isize) - (base as isize);
        // SAFETY: member getters on a live settings object return interior pointers.
        unsafe {
            let init = (a.AkInitializationSettings_initSettings_get)(s);
            LayoutProbe {
                stream_mgr: off((a.AkInitializationSettings_streamMgrSettings_get)(s), s),
                device: off((a.AkInitializationSettings_deviceSettings_get)(s), s),
                init: off(init, s),
                platform: off((a.AkInitializationSettings_platformSettings_get)(s), s),
                music: off((a.AkInitializationSettings_musicSettings_get)(s), s),
                unity: off((a.AkInitializationSettings_unityPlatformSpecificSettings_get)(s), s),
                main_output_in_init: off((a.AkInitSettings_settingsMainOutput_get)(init), init),
            }
        }
    }

    /// The game's `AkWwiseInitializationSettings.InitializeSoundEngine` (steps 1–5 above),
    /// plus `SetLocalOutput(ErrorLevel_All)` from `AkAudioService.Initialize`.
    ///
    /// # Errors
    ///
    /// If `Init`, `SetBasePath` or `Init.bnk` fail (the game terminates the engine in the
    /// first two cases and logs the third; we terminate in all three).
    pub fn init(
        &mut self,
        g: &GameSettings,
        paths: &EnginePaths,
        opts: &EngineOptions,
    ) -> Result<()> {
        ensure!(!self.is_initialized(), "sound engine already initialized");
        for dir in [&paths.persistent, &paths.decoded_banks] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let t = Instant::now();
        // SAFETY: plain constructor; freed below with the matching destructor.
        let s = unsafe { (self.api.new_AkInitializationSettings)() };
        ensure!(!s.is_null(), "new_AkInitializationSettings returned null");
        self.defaults = self.probe(s);
        self.layout = Some(self.layout_probe(s));
        let applied = self.apply_settings(s, g, paths, opts);
        self.applied = self.probe(s);
        // SAFETY: `s` is fully initialised; Init copies what it needs (the C# frees its
        // settings object right after, through the SWIG finalizer).
        let r = applied.map(|()| unsafe { (self.api.Init)(s) });
        // SAFETY: created above, not used after this.
        unsafe { (self.api.delete_AkInitializationSettings)(s) };
        let r = r?;
        if let Err(e) = check("AkSoundEngine.Init", r) {
            // SAFETY: the game calls Term after a failed Init; Term tolerates partial init.
            unsafe { (self.api.Term)() };
            return Err(e);
        }
        self.initialized = true;
        if let Err(e) = self.init_rest(g, paths, opts) {
            self.shutdown();
            return Err(e);
        }
        self.timings.engine = t.elapsed();
        Ok(())
    }

    fn init_rest(
        &mut self,
        g: &GameSettings,
        paths: &EnginePaths,
        opts: &EngineOptions,
    ) -> Result<()> {
        let a = &self.api;
        let sp = &g.user.spatial;
        // SAFETY: settings objects are created, filled through their own setters, passed to
        // the init call (which copies them) and deleted.
        let spatial = unsafe {
            let s = (a.new_AkSpatialAudioInitSettings)();
            ensure!(!s.is_null(), "new_AkSpatialAudioInitSettings returned null");
            (a.AkSpatialAudioInitSettings_uMaxSoundPropagationDepth_set)(
                s,
                sp.max_sound_propagation_depth,
            );
            (a.AkSpatialAudioInitSettings_uDiffractionFlags_set)(s, sp.diffraction_flags as u32);
            (a.AkSpatialAudioInitSettings_fMovementThreshold_set)(s, sp.movement_threshold);
            (a.AkSpatialAudioInitSettings_uNumberOfPrimaryRays_set)(s, sp.number_of_primary_rays);
            (a.AkSpatialAudioInitSettings_uMaxReflectionOrder_set)(s, sp.max_reflection_order);
            (a.AkSpatialAudioInitSettings_fMaxPathLength_set)(s, sp.max_path_length);
            (a.AkSpatialAudioInitSettings_bEnableDiffractionOnReflection_set)(
                s,
                u32::from(sp.enable_diffraction_on_reflections),
            );
            (a.AkSpatialAudioInitSettings_bEnableDirectPathDiffraction_set)(
                s,
                u32::from(sp.enable_direct_path_diffraction),
            );
            (a.AkSpatialAudioInitSettings_bEnableTransmission_set)(
                s,
                u32::from(sp.enable_transmission),
            );
            (a.AkSpatialAudioInitSettings_fDiffractionShadowAttenFactor_set)(
                s,
                g.advanced.diffraction_shadow_attenuation_factor,
            );
            (a.AkSpatialAudioInitSettings_fDiffractionShadowDegrees_set)(
                s,
                g.advanced.diffraction_shadow_degrees,
            );
            let r = (a.InitSpatialAudio)(s);
            (a.delete_AkSpatialAudioInitSettings)(s);
            AkResult(r)
        };
        self.init_log.push(("InitSpatialAudio".into(), spatial));

        if opts.init_comms {
            let c = &g.comms;
            let name = CString::new(c.network_name)?;
            // SAFETY: as above; `name` outlives the call.
            let comms = unsafe {
                let s = (a.new_AkCommunicationSettings)();
                ensure!(!s.is_null(), "new_AkCommunicationSettings returned null");
                (a.AkCommunicationSettings_uPoolSize_set)(s, c.pool_size);
                (a.AkCommunicationSettings_uDiscoveryBroadcastPort_set)(
                    s,
                    c.discovery_broadcast_port,
                );
                (a.AkCommunicationSettings_uCommandPort_set)(s, c.command_port);
                (a.AkCommunicationSettings_uNotificationPort_set)(s, c.notification_port);
                (a.AkCommunicationSettings_bInitSystemLib_set)(
                    s,
                    u32::from(c.initialize_system_comms),
                );
                (a.AkCommunicationSettings_commSystem_set)(s, c.comm_system);
                (a.AkCommunicationSettings_szAppNetworkName_set)(s, name.as_ptr());
                let r = (a.InitCommunication)(s);
                (a.delete_AkCommunicationSettings)(s);
                AkResult(r)
            };
            self.init_log.push(("InitCommunication".into(), comms));
        }

        // Base paths, in the game's order.
        let base = path_str(&paths.sound_banks)?;
        // SAFETY: NUL-terminated UTF-16 strings that outlive each call.
        unsafe {
            check("SetBasePath", (a.SetBasePath)(wide(base).as_ptr()))?;
            if paths.persistent != paths.sound_banks {
                let r = AkResult((a.AddBasePath)(wide(path_str(&paths.persistent)?).as_ptr()));
                self.init_log.push(("AddBasePath(persistent)".into(), r));
            }
            let decoded = wide(path_str(&paths.decoded_banks)?);
            self.init_log.push((
                "SetDecodedBankPath".into(),
                AkResult((a.SetDecodedBankPath)(decoded.as_ptr())),
            ));
            self.init_log
                .push(("AddBasePath(decoded)".into(), AkResult((a.AddBasePath)(decoded.as_ptr()))));
            let lang = AkResult((a.SetCurrentLanguage)(wide(g.user.startup_language).as_ptr()));
            self.init_log.push((format!("SetCurrentLanguage({})", g.user.startup_language), lang));
        }

        // AkCallbackManager.Init: the serializer gets memory we own until Term.
        let buf = CallbackBuffer::new(g.callback_buffer_size)?;
        // SAFETY: `buf` is a live allocation of `size` bytes, kept in `self.callbacks` until
        // after AkCallbackSerializer_Term.
        let r = unsafe {
            (a.AkCallbackSerializer_Init)(buf.ptr.as_ptr().cast(), g.callback_buffer_size)
        };
        self.callbacks = Some(buf);
        check("AkCallbackSerializer.Init", r)?;

        // AkBankManager.LoadInitBank.
        let mut id = 0u32;
        // SAFETY: valid string; `id` is a valid out pointer.
        check("LoadBank(Init.bnk)", unsafe {
            (a.LoadBank__SWIG_0)(wide("Init.bnk").as_ptr(), &raw mut id)
        })?;
        // AkCallbackManager.SetMonitoringCallback(ErrorLevel_All, OnAkLog).
        // SAFETY: plain setter.
        unsafe { (a.AkCallbackSerializer_SetLocalOutput)(ffi::ERROR_LEVEL_ALL) };
        Ok(())
    }

    /// `AudioFilePackagesSettings.LoadPackagesChunk`: adds `<banks>/Packages` as a base path
    /// and loads each package.
    ///
    /// # Errors
    ///
    /// If a package fails to load (the game ignores the result; we report it).
    pub fn load_packages(
        &mut self,
        sound_banks: &Path,
        names: &[&str],
    ) -> Result<Vec<(String, u32)>> {
        self.running()?;
        let t = Instant::now();
        let dir = sound_banks.join("Packages");
        // SAFETY: valid string.
        let r = unsafe { (self.api.AddBasePath)(wide(path_str(&dir)?).as_ptr()) };
        self.init_log.push(("AddBasePath(Packages)".into(), AkResult(r)));
        let mut out = Vec::new();
        for name in names {
            let mut id = 0u32;
            // SAFETY: valid string and out pointer.
            let r = unsafe { (self.api.LoadFilePackage)(wide(name).as_ptr(), &raw mut id) };
            check(&format!("LoadFilePackage({name})"), r)?;
            out.push(((*name).to_owned(), id));
        }
        self.timings.packages += t.elapsed();
        Ok(out)
    }

    /// `AkBankManager.BankHandle.DoLoadBank` (synchronous `LoadBank` by name).
    ///
    /// # Errors
    ///
    /// If the bank fails to load.
    pub fn load_bank(&mut self, name: &str) -> Result<u32> {
        self.running()?;
        let t = Instant::now();
        let mut id = 0u32;
        // SAFETY: valid string and out pointer.
        let r = unsafe { (self.api.LoadBank__SWIG_0)(wide(name).as_ptr(), &raw mut id) };
        self.timings.banks += t.elapsed();
        check(&format!("LoadBank({name})"), r)?;
        Ok(id)
    }

    /// `RegisterGameObjInternal_WithName`.
    ///
    /// # Errors
    ///
    /// If registration fails.
    pub fn register(&self, id: GameObjectId, name: &str) -> Result<()> {
        self.running()?;
        // SAFETY: valid string.
        check("RegisterGameObj", unsafe {
            (self.api.RegisterGameObjInternal_WithName)(id, wide(name).as_ptr())
        })
    }

    /// `UnregisterGameObjInternal`.
    ///
    /// # Errors
    ///
    /// If the object wasn't registered.
    pub fn unregister(&self, id: GameObjectId) -> Result<()> {
        self.running()?;
        // SAFETY: by-value call.
        check("UnregisterGameObj", unsafe { (self.api.UnregisterGameObjInternal)(id) })
    }

    /// `SetObjectPosition(id, transform.position, transform.forward, transform.up)`.
    ///
    /// # Errors
    ///
    /// If Wwise rejects it (e.g. non-orthonormal orientation).
    pub fn set_position(
        &self,
        id: GameObjectId,
        position: AkVec3,
        front: AkVec3,
        top: AkVec3,
    ) -> Result<()> {
        self.running()?;
        // SAFETY: by-value call; AkVec3 matches AkVector / Vector3 (three f32).
        check("SetObjectPosition", unsafe {
            (self.api.SetObjectPosition)(id, position, front, top)
        })
    }

    /// `SetDefaultListeners` (what `DefaultListener.Register` does).
    ///
    /// # Errors
    ///
    /// If Wwise rejects it.
    pub fn set_default_listeners(&self, ids: &[GameObjectId]) -> Result<()> {
        self.running()?;
        let n = u32::try_from(ids.len())?;
        // SAFETY: pointer and count describe a live slice.
        check("SetDefaultListeners", unsafe { (self.api.SetDefaultListeners)(ids.as_ptr(), n) })
    }

    /// `SetSwitch(group, state, object)` by name.
    ///
    /// # Errors
    ///
    /// If Wwise rejects it.
    pub fn set_switch(&self, group: &str, state: &str, id: GameObjectId) -> Result<()> {
        self.running()?;
        // SAFETY: valid strings.
        check("SetSwitch", unsafe {
            (self.api.SetSwitch__SWIG_1)(wide(group).as_ptr(), wide(state).as_ptr(), id)
        })
    }

    /// Posts an event by name, as `SoundEventsManager.PostEvent(name, go, flags, cb, cookie)`
    /// does. With non-zero `flags`, callbacks are queued by the native serializer (the
    /// callback pointer is only a non-null sentinel) and come back from
    /// [`Self::poll_callbacks`] with `cookie`. Returns the playing ID (0 = failure).
    #[must_use]
    pub fn post_event(&self, name: &str, id: GameObjectId, flags: u32, cookie: usize) -> u32 {
        if self.running().is_err() {
            return 0;
        }
        let (cb, ck): (Ptr, Ptr) = if flags != 0 {
            // The C# passes (IntPtr)1 when it wants callbacks; the SWIG wrapper substitutes
            // AkCallbackSerializer's function (checked in the disassembly).
            (ptr::without_provenance_mut(1), ptr::without_provenance_mut(cookie))
        } else {
            (ptr::null_mut(), ptr::null_mut())
        };
        // SAFETY: valid string; the callback pointer is never called by us or dereferenced
        // by the wrapper (it's replaced when non-null).
        unsafe { (self.api.PostEvent__SWIG_6)(wide(name).as_ptr(), id, flags, cb, ck) }
    }

    /// `StopPlayingID`.
    pub fn stop_playing(&self, playing_id: u32) {
        // SAFETY: by-value call.
        unsafe { (self.api.StopPlayingID__SWIG_2)(playing_id) };
    }

    /// `GetSourcePlayPosition` (needs `AK_EnableGetSourcePlayPosition` at post time), ms.
    /// With `extrapolate`, Wwise adds the wall-clock time since the last audio frame.
    #[must_use]
    pub fn source_play_position(&self, playing_id: u32, extrapolate: bool) -> Option<i32> {
        let mut ms = 0i32;
        // SAFETY: valid out pointer.
        let r = unsafe {
            (self.api.GetSourcePlayPosition__SWIG_0)(
                playing_id,
                &raw mut ms,
                u32::from(extrapolate),
            )
        };
        AkResult(r).is_success().then_some(ms)
    }

    /// Drains queued callbacks, like `AkCallbackManager.PostCallbacks`, and hands them to
    /// `f` after releasing the serializer lock (so `f` may call back into the engine, and a
    /// panic in `f` can't leave Wwise's threads blocked). Returns how many.
    pub fn poll_callbacks(&self, mut f: impl FnMut(Callback)) -> usize {
        if self.callbacks.is_none() {
            return 0;
        }
        let a = &self.api;
        let mut drained = Vec::new();
        // SAFETY: Lock returns the head of a list the native side keeps valid until Unlock;
        // every accessor gets a header or data pointer from that list, `decode` copies
        // strings out, nothing between Lock and Unlock can panic except allocation failure
        // (which aborts), and Unlock is always called.
        unsafe {
            let mut h = (a.AkCallbackSerializer_Lock)();
            while !h.is_null() {
                let package = (a.AkSerializedCallbackHeader_pPackage_get)(h) as usize;
                let kind = (a.AkSerializedCallbackHeader_eType_get)(h);
                let data = (a.AkSerializedCallbackHeader_GetData)(h);
                drained.push(Self::decode(a, kind, package, data));
                h = (a.AkSerializedCallbackHeader_pNext_get)(h);
            }
            (a.AkCallbackSerializer_Unlock)();
        }
        let n = drained.len();
        drained.into_iter().for_each(&mut f);
        n
    }

    /// # Safety
    ///
    /// `data` must be the payload of a serialized callback of type `kind`, under the lock.
    unsafe fn decode(a: &Api, kind: i32, cookie: usize, data: Ptr) -> Callback {
        let k = kind as u32;
        // SAFETY: per the contract, `data` points to the struct matching `kind`; the SWIG
        // upcasts give the base-class pointers the base getters expect.
        unsafe {
            match k {
                callback_type::END_OF_EVENT => {
                    let base = (a.AkEventCallbackInfo_SWIGUpcast)(data);
                    Callback::EndOfEvent {
                        cookie,
                        playing_id: (a.AkEventCallbackInfo_playingID_get)(data),
                        event_id: (a.AkEventCallbackInfo_eventID_get)(data),
                        game_object: (a.AkCallbackInfo_gameObjID_get)(base),
                    }
                }
                callback_type::MARKER => {
                    let ev = (a.AkMarkerCallbackInfo_SWIGUpcast)(data);
                    Callback::Marker {
                        cookie,
                        playing_id: (a.AkEventCallbackInfo_playingID_get)(ev),
                        identifier: (a.AkMarkerCallbackInfo_uIdentifier_get)(data),
                        position: (a.AkMarkerCallbackInfo_uPosition_get)(data),
                        label: cstr((a.AkMarkerCallbackInfo_strLabel_get)(data)),
                    }
                }
                callback_type::DURATION => {
                    let ev = (a.AkDurationCallbackInfo_SWIGUpcast)(data);
                    Callback::Duration {
                        cookie,
                        playing_id: (a.AkEventCallbackInfo_playingID_get)(ev),
                        duration_ms: (a.AkDurationCallbackInfo_fDuration_get)(data),
                        estimated_ms: (a.AkDurationCallbackInfo_fEstimatedDuration_get)(data),
                        audio_node_id: (a.AkDurationCallbackInfo_audioNodeID_get)(data),
                        media_id: (a.AkDurationCallbackInfo_mediaID_get)(data),
                        streaming: (a.AkDurationCallbackInfo_bStreaming_get)(data) != 0,
                    }
                }
                callback_type::MONITORING => Callback::Monitoring {
                    code: (a.AkMonitoringCallbackInfo_errorCode_get)(data),
                    level: (a.AkMonitoringCallbackInfo_errorLevel_get)(data),
                    playing_id: (a.AkMonitoringCallbackInfo_playingID_get)(data),
                    game_object: (a.AkMonitoringCallbackInfo_gameObjID_get)(data),
                    message: cstr((a.AkMonitoringCallbackInfo_message_get)(data)),
                },
                _ => Callback::Other { kind, cookie },
            }
        }
    }

    /// `RenderAudio()` (end of the game's `LateUpdate`).
    ///
    /// # Errors
    ///
    /// If Wwise returns an error.
    pub fn render_audio(&self) -> Result<()> {
        self.running()?;
        // SAFETY: valid after Init; returns an error code otherwise.
        check("RenderAudio", unsafe { (self.api.RenderAudio__SWIG_1)() })
    }

    /// `Suspend(renderAnyway)`, as on focus loss or app pause.
    ///
    /// # Errors
    ///
    /// If Wwise returns an error.
    pub fn suspend(&self, render_anyway: bool) -> Result<()> {
        self.running()?;
        // SAFETY: by-value call.
        check("Suspend", unsafe { (self.api.Suspend__SWIG_0)(u32::from(render_anyway)) })
    }

    /// `WakeupFromSuspend()`. The game also calls this every frame in `OnLateUpdate`.
    ///
    /// # Errors
    ///
    /// If Wwise returns an error.
    pub fn wakeup(&self) -> Result<()> {
        self.running()?;
        // SAFETY: no arguments.
        check("WakeupFromSuspend", unsafe { (self.api.WakeupFromSuspend)() })
    }

    /// `GetBufferTick()`: audio frames rendered since init (advances only while rendering).
    #[must_use]
    pub fn buffer_tick(&self) -> u32 {
        // SAFETY: no arguments.
        unsafe { (self.api.GetBufferTick)() }
    }

    /// Output channel count of the main device (`GetSpeakerConfiguration`).
    #[must_use]
    pub fn output_channels(&self) -> Option<(u32, u32)> {
        // SAFETY: the wrapper returns a heap AkChannelConfig (operator new) that we read and
        // free with the matching SWIG destructor.
        unsafe {
            let p = (self.api.GetSpeakerConfiguration__SWIG_1)();
            if p.is_null() {
                return None;
            }
            let v = (
                (self.api.AkChannelConfig_uNumChannels_get)(p),
                (self.api.AkChannelConfig_uChannelMask_get)(p),
            );
            (self.api.delete_AkChannelConfig)(p);
            Some(v)
        }
    }

    /// `AddBasePath(dir)`. The low-level IO searches base paths most-recent-first and opens
    /// files for writing (output captures) in the first one that accepts them; it always
    /// prepends a base path, so absolute file names don't work.
    ///
    /// # Errors
    ///
    /// If Wwise refuses.
    pub fn add_base_path(&self, dir: &Path) -> Result<()> {
        self.running()?;
        // SAFETY: valid string.
        check("AddBasePath", unsafe { (self.api.AddBasePath)(wide(path_str(dir)?).as_ptr()) })
    }

    /// `StartOutputCapture(file)`: `file` is relative to the most recently added base path
    /// (see [`Self::add_base_path`]).
    ///
    /// # Errors
    ///
    /// If Wwise refuses.
    pub fn start_output_capture(&self, file: &str) -> Result<()> {
        self.running()?;
        let f = CString::new(file)?;
        // SAFETY: valid C string; Wwise copies the name.
        check("StartOutputCapture", unsafe { (self.api.StartOutputCapture)(f.as_ptr()) })
    }

    /// `AddOutputCaptureMarker(text)`.
    ///
    /// # Errors
    ///
    /// If Wwise refuses.
    pub fn add_output_capture_marker(&self, text: &str) -> Result<()> {
        self.running()?;
        let t = CString::new(text)?;
        // SAFETY: valid C string.
        check("AddOutputCaptureMarker", unsafe { (self.api.AddOutputCaptureMarker)(t.as_ptr()) })
    }

    /// `StopOutputCapture()`.
    ///
    /// # Errors
    ///
    /// If Wwise refuses.
    pub fn stop_output_capture(&self) -> Result<()> {
        self.running()?;
        // SAFETY: no arguments.
        check("StopOutputCapture", unsafe { (self.api.StopOutputCapture)() })
    }

    /// Starts Wwise's resource monitor.
    pub fn start_resource_monitoring(&self) {
        // SAFETY: no arguments.
        unsafe { (self.api.StartResourceMonitoring)() };
    }

    /// Reads Wwise's resource monitor summary.
    #[must_use]
    pub fn resource_summary(&self) -> Option<ResourceSummary> {
        let a = &self.api;
        // SAFETY: a SWIG-allocated summary, filled by the engine, read, then freed.
        unsafe {
            let p = (a.new_AkResourceMonitorDataSummary)();
            if p.is_null() {
                return None;
            }
            (a.GetResourceMonitorDataSummary)(p);
            let s = ResourceSummary {
                total_cpu: (a.AkResourceMonitorDataSummary_totalCPU_get)(p),
                plugin_cpu: (a.AkResourceMonitorDataSummary_pluginCPU_get)(p),
                physical_voices: (a.AkResourceMonitorDataSummary_physicalVoices_get)(p),
                virtual_voices: (a.AkResourceMonitorDataSummary_virtualVoices_get)(p),
                active_events: (a.AkResourceMonitorDataSummary_nbActiveEvents_get)(p),
            };
            (a.delete_AkResourceMonitorDataSummary)(p);
            Some(s)
        }
    }

    /// The language Wwise reports.
    #[must_use]
    pub fn current_language(&self) -> Option<String> {
        // SAFETY: returns a pointer to the stream manager's language string (or null).
        cstr(unsafe { (self.api.GetCurrentLanguage)() })
    }

    /// `GetIDFromString`, to cross-check [`short_id`].
    #[must_use]
    pub fn id_from_string(&self, name: &str) -> u32 {
        // SAFETY: valid string.
        unsafe { (self.api.GetIDFromString__SWIG_0)(wide(name).as_ptr()) }
    }

    /// The game's `TerminateSoundEngine`, then the serializer teardown. Returns the
    /// callbacks drained during shutdown. Idempotent.
    pub fn shutdown(&mut self) -> Vec<Callback> {
        let mut drained = Vec::new();
        if !self.initialized {
            return drained;
        }
        let a = &self.api;
        if self.is_initialized() {
            // SAFETY: engine initialised; these take no pointers.
            unsafe {
                (a.StopAll__SWIG_1)();
                (a.ClearBanks)();
                (a.RenderAudio__SWIG_1)();
            }
            // The game's loop: sleep 10 ms per empty poll until five empty polls, 1 ms each
            // iteration.
            let mut empty = 0;
            while empty < 5 {
                if self.poll_callbacks(|c| drained.push(c)) == 0 {
                    std::thread::sleep(Duration::from_millis(10));
                    empty += 1;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            // SAFETY: engine initialised; Term stops Wwise's threads.
            unsafe { (a.Term)() };
        }
        self.poll_callbacks(|c| drained.push(c));
        if self.callbacks.is_some() {
            // SAFETY: Term has run; the serializer stops using the buffer here, after which
            // we free it (by dropping it below).
            unsafe { (a.AkCallbackSerializer_Term)() };
            self.callbacks = None;
        }
        self.initialized = false;
        drained
    }
}

impl Drop for SoundEngine {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Fails unless running on Apple Silicon macOS (the bundle's arm64 slice).
///
/// # Errors
///
/// On other platforms.
pub fn require_platform() -> Result<()> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        bail!("this spike needs macOS on arm64");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Ownership;

    #[test]
    fn only_one_owner_at_a_time() {
        let first = Ownership::acquire().unwrap();
        assert!(Ownership::acquire().is_err());
        drop(first);
        assert!(Ownership::acquire().is_ok());
    }
}
