//! Raw bindings to the SWIG `CSharp_*` C exports of the game's `AkSoundEngine.bundle`.
//!
//! The bundle is the Wwise Unity integration (Wwise 2019.2, a Profile build) compiled for
//! macOS. Its 1,068 exports are the C side of the SWIG wrapper that `AkSoundEnginePINVOKE`
//! (in the game's `Assembly-CSharp.dll`) calls through P/Invoke. We call the same symbols,
//! so the declarations below follow Mono's P/Invoke marshalling rules, not Wwise's C++
//! headers:
//!
//! - C# `bool` parameters are marshalled as 4-byte `BOOL`, and the SWIG C side takes
//!   `unsigned int` (checked in the disassembly: `cmp w19, #0`). We pass `u32`.
//! - `bool` return values come straight from C++ (`AK::SoundEngine::IsInitialized` is
//!   tail-returned), so only the low byte is defined. We declare them `u8` and test `!= 0`.
//! - `[MarshalAs(LPWStr)] string` is UTF-16, NUL-terminated (Mono uses UTF-16 on every
//!   platform). The native side converts to its `char` paths. See [`wide`].
//! - `[MarshalAs(LPStr)] string` is a NUL-terminated `char*` (UTF-8 here).
//! - `UnityEngine.Vector3` is passed by value; it has the same layout as Wwise's `AkVector`
//!   (three `f32`), so [`AkVec3`] by value follows the same AAPCS64 HFA rules as the C++.
//! - `out uint` is `*mut u32`; `ulong[]` is `*const u64`.
//! - Settings structs are opaque. We only ever create them with `new_*`, reach embedded
//!   members through `*_get` (which return interior pointers), and write fields through
//!   `*_set`. We never depend on a native struct layout.
//!
//! Every function here is `unsafe`: the caller upholds the native API's contract (valid
//! pointers, engine state, threading). The safe layer is [`crate::engine`].

use std::ffi::{c_char, c_void};
use std::fmt;
use std::mem::ManuallyDrop;
use std::path::Path;

/// `AkVector` / `UnityEngine.Vector3`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AkVec3 {
    /// Right.
    pub x: f32,
    /// Up.
    pub y: f32,
    /// Forward (Wwise and Unity are both left-handed, Y up, Z forward).
    pub z: f32,
}

impl AkVec3 {
    /// Builds a vector.
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

/// Opaque native object pointer (`IntPtr` in the C# wrapper).
pub type Ptr = *mut c_void;

/// The native error logger callback (`AkLogger.ErrorLoggerInteropDelegate`).
pub type ErrorLogger = unsafe extern "C" fn(message: *const c_char);

macro_rules! ak_api {
    ($( $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) $(-> $ret:ty)? ; )*) => {
        /// Function table resolved from `AkSoundEngine.bundle`. Field names are the export
        /// names without the `CSharp_` prefix.
        #[allow(non_snake_case)]
        pub struct Api {
            /// Never unloaded: Wwise keeps threads and atexit state that point into it.
            _lib: ManuallyDrop<libloading::Library>,
            $( pub $name: unsafe extern "C" fn($($ty),*) $(-> $ret)?, )*
        }

        impl Api {
            /// Number of symbols this table resolves.
            pub const SYMBOLS: usize = [$(stringify!($name)),*].len();

            /// `dlopen`s the bundle and resolves every symbol in the table.
            ///
            /// # Errors
            ///
            /// If the library can't be loaded or a symbol is missing (e.g. a game update
            /// changed the Wwise version).
            ///
            /// # Safety
            ///
            /// Loading runs the bundle's initialisers. The caller must pass the game's
            /// `AkSoundEngine` binary, whose exports have the signatures declared here.
            #[allow(non_snake_case)]
            pub unsafe fn load(path: &Path) -> Result<Self, libloading::Error> {
                // SAFETY: the caller vouches for the library (see above).
                let lib = unsafe { libloading::Library::new(path)? };
                $(
                    // SAFETY: the declared type matches the export's SWIG signature, read
                    // from AkSoundEnginePINVOKE and checked against the disassembly for the
                    // ABI-sensitive cases (bool, Vector3, strings). The fn pointer is valid
                    // as long as `lib` stays loaded, which is forever (ManuallyDrop).
                    let $name = unsafe {
                        *lib.get::<unsafe extern "C" fn($($ty),*) $(-> $ret)?>(
                            concat!("CSharp_", stringify!($name), "\0").as_bytes(),
                        )?
                    };
                )*
                Ok(Self { _lib: ManuallyDrop::new(lib), $($name),* })
            }
        }
    };
}

ak_api! {
    // Version and state.
    GetMajorMinorVersion() -> u32;
    GetSubminorBuildVersion() -> u32;
    IsInitialized() -> u8;
    SetErrorLogger__SWIG_0(logger: Option<ErrorLogger>);
    SetErrorLogger__SWIG_1();

    // AkInitializationSettings: the Unity integration's aggregate of every init struct.
    new_AkInitializationSettings() -> Ptr;
    delete_AkInitializationSettings(p: Ptr);
    AkInitializationSettings_streamMgrSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_deviceSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_initSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_platformSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_musicSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_unityPlatformSpecificSettings_get(p: Ptr) -> Ptr;
    AkInitializationSettings_useAsyncOpen_set(p: Ptr, v: u32);

    AkInitSettings_uMaxNumPaths_set(p: Ptr, v: u32);
    AkInitSettings_uMaxNumPaths_get(p: Ptr) -> u32;
    AkInitSettings_uCommandQueueSize_set(p: Ptr, v: u32);
    AkInitSettings_uCommandQueueSize_get(p: Ptr) -> u32;
    AkInitSettings_uNumSamplesPerFrame_set(p: Ptr, v: u32);
    AkInitSettings_uNumSamplesPerFrame_get(p: Ptr) -> u32;
    AkInitSettings_bEnableGameSyncPreparation_set(p: Ptr, v: u32);
    AkInitSettings_uContinuousPlaybackLookAhead_set(p: Ptr, v: u32);
    AkInitSettings_uContinuousPlaybackLookAhead_get(p: Ptr) -> u32;
    AkInitSettings_uMonitorQueuePoolSize_set(p: Ptr, v: u32);
    AkInitSettings_uMonitorQueuePoolSize_get(p: Ptr) -> u32;
    AkInitSettings_uMaxHardwareTimeoutMs_set(p: Ptr, v: u32);
    AkInitSettings_uMaxHardwareTimeoutMs_get(p: Ptr) -> u32;
    AkInitSettings_bDebugOutOfRangeCheckEnabled_set(p: Ptr, v: u32);
    AkInitSettings_fDebugOutOfRangeLimit_set(p: Ptr, v: f32);
    AkInitSettings_szPluginDLLPath_set(p: Ptr, v: *const c_char);
    AkInitSettings_szPluginDLLPath_get(p: Ptr) -> *const c_char;
    AkInitSettings_settingsMainOutput_get(p: Ptr) -> Ptr;
    AkInitSettings_bUseLEngineThread_get(p: Ptr) -> u8;
    AkInitSettings_bUseSoundBankMgrThread_get(p: Ptr) -> u8;
    AkInitSettings_uBankReadBufferSize_get(p: Ptr) -> u32;

    AkOutputSettings_audioDeviceShareset_set(p: Ptr, v: u32);
    AkOutputSettings_audioDeviceShareset_get(p: Ptr) -> u32;
    AkOutputSettings_idDevice_set(p: Ptr, v: u32);
    AkOutputSettings_ePanningRule_set(p: Ptr, v: i32);
    AkOutputSettings_channelConfig_get(p: Ptr) -> Ptr;
    AkChannelConfig_SetAnonymous(p: Ptr, channels: u32);
    AkChannelConfig_SetStandard(p: Ptr, mask: u32);
    AkChannelConfig_SetAmbisonic(p: Ptr, channels: u32);
    AkChannelConfig_uNumChannels_get(p: Ptr) -> u32;
    AkChannelConfig_uChannelMask_get(p: Ptr) -> u32;
    delete_AkChannelConfig(p: Ptr);

    AkPlatformInitSettings_uSampleRate_set(p: Ptr, v: u32);
    AkPlatformInitSettings_uSampleRate_get(p: Ptr) -> u32;
    AkPlatformInitSettings_uNumRefillsInVoice_set(p: Ptr, v: u16);
    AkPlatformInitSettings_uNumRefillsInVoice_get(p: Ptr) -> u16;

    AkMusicSettings_fStreamingLookAheadRatio_set(p: Ptr, v: f32);
    AkMusicSettings_fStreamingLookAheadRatio_get(p: Ptr) -> f32;

    AkDeviceSettings_uIOMemorySize_set(p: Ptr, v: u32);
    AkDeviceSettings_uIOMemorySize_get(p: Ptr) -> u32;
    AkDeviceSettings_fTargetAutoStmBufferLength_set(p: Ptr, v: f32);
    AkDeviceSettings_fTargetAutoStmBufferLength_get(p: Ptr) -> f32;
    AkDeviceSettings_bUseStreamCache_set(p: Ptr, v: u32);
    AkDeviceSettings_uMaxCachePinnedBytes_set(p: Ptr, v: u32);
    AkDeviceSettings_uMaxCachePinnedBytes_get(p: Ptr) -> u32;
    AkDeviceSettings_uGranularity_get(p: Ptr) -> u32;
    AkDeviceSettings_uSchedulerTypeFlags_get(p: Ptr) -> u32;
    AkDeviceSettings_uMaxConcurrentIO_get(p: Ptr) -> u32;

    Init(settings: Ptr) -> i32;

    // Spatial audio.
    new_AkSpatialAudioInitSettings() -> Ptr;
    delete_AkSpatialAudioInitSettings(p: Ptr);
    AkSpatialAudioInitSettings_uMaxSoundPropagationDepth_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_uDiffractionFlags_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_fDiffractionShadowAttenFactor_set(p: Ptr, v: f32);
    AkSpatialAudioInitSettings_fDiffractionShadowDegrees_set(p: Ptr, v: f32);
    AkSpatialAudioInitSettings_fMovementThreshold_set(p: Ptr, v: f32);
    AkSpatialAudioInitSettings_uNumberOfPrimaryRays_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_uMaxReflectionOrder_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_fMaxPathLength_set(p: Ptr, v: f32);
    AkSpatialAudioInitSettings_bEnableDiffractionOnReflection_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_bEnableDirectPathDiffraction_set(p: Ptr, v: u32);
    AkSpatialAudioInitSettings_bEnableTransmission_set(p: Ptr, v: u32);
    InitSpatialAudio(settings: Ptr) -> i32;

    // Authoring-tool communication (the bundle is a Profile build, so this opens sockets).
    new_AkCommunicationSettings() -> Ptr;
    delete_AkCommunicationSettings(p: Ptr);
    AkCommunicationSettings_uPoolSize_set(p: Ptr, v: u32);
    AkCommunicationSettings_uDiscoveryBroadcastPort_set(p: Ptr, v: u16);
    AkCommunicationSettings_uCommandPort_set(p: Ptr, v: u16);
    AkCommunicationSettings_uNotificationPort_set(p: Ptr, v: u16);
    AkCommunicationSettings_commSystem_set(p: Ptr, v: i32);
    AkCommunicationSettings_bInitSystemLib_set(p: Ptr, v: u32);
    AkCommunicationSettings_szAppNetworkName_set(p: Ptr, v: *const c_char);
    InitCommunication(settings: Ptr) -> i32;

    // Paths, language, banks and file packages (the Unity integration's low-level IO).
    SetBasePath(path: *const u16) -> i32;
    AddBasePath(path: *const u16) -> i32;
    SetDecodedBankPath(path: *const u16) -> i32;
    SetCurrentLanguage(language: *const u16) -> i32;
    GetCurrentLanguage() -> *const c_char;
    LoadFilePackage(name: *const u16, out_id: *mut u32) -> i32;
    UnloadFilePackage(id: u32) -> i32;
    UnloadAllFilePackages() -> i32;
    LoadBank__SWIG_0(name: *const u16, out_id: *mut u32) -> i32;
    UnloadBank__SWIG_0(name: *const u16, mem: Ptr) -> i32;
    ClearBanks() -> i32;
    GetIDFromString__SWIG_0(name: *const u16) -> u32;

    // Callbacks: the Unity integration queues them natively; C# polls under a lock.
    AkCallbackSerializer_Init(memory: Ptr, size: u32) -> i32;
    AkCallbackSerializer_Term();
    AkCallbackSerializer_Lock() -> Ptr;
    AkCallbackSerializer_Unlock();
    AkCallbackSerializer_SetLocalOutput(error_level: u32);
    AkSerializedCallbackHeader_pPackage_get(p: Ptr) -> Ptr;
    AkSerializedCallbackHeader_pNext_get(p: Ptr) -> Ptr;
    AkSerializedCallbackHeader_eType_get(p: Ptr) -> i32;
    AkSerializedCallbackHeader_GetData(p: Ptr) -> Ptr;
    AkCallbackInfo_gameObjID_get(p: Ptr) -> u64;
    AkEventCallbackInfo_SWIGUpcast(p: Ptr) -> Ptr;
    AkEventCallbackInfo_playingID_get(p: Ptr) -> u32;
    AkEventCallbackInfo_eventID_get(p: Ptr) -> u32;
    AkMarkerCallbackInfo_SWIGUpcast(p: Ptr) -> Ptr;
    AkMarkerCallbackInfo_uIdentifier_get(p: Ptr) -> u32;
    AkMarkerCallbackInfo_uPosition_get(p: Ptr) -> u32;
    AkMarkerCallbackInfo_strLabel_get(p: Ptr) -> *const c_char;
    AkDurationCallbackInfo_SWIGUpcast(p: Ptr) -> Ptr;
    AkDurationCallbackInfo_fDuration_get(p: Ptr) -> f32;
    AkDurationCallbackInfo_fEstimatedDuration_get(p: Ptr) -> f32;
    AkDurationCallbackInfo_audioNodeID_get(p: Ptr) -> u32;
    AkDurationCallbackInfo_mediaID_get(p: Ptr) -> u32;
    AkDurationCallbackInfo_bStreaming_get(p: Ptr) -> u8;
    AkMonitoringCallbackInfo_errorCode_get(p: Ptr) -> i32;
    AkMonitoringCallbackInfo_errorLevel_get(p: Ptr) -> i32;
    AkMonitoringCallbackInfo_playingID_get(p: Ptr) -> u32;
    AkMonitoringCallbackInfo_gameObjID_get(p: Ptr) -> u64;
    AkMonitoringCallbackInfo_message_get(p: Ptr) -> *const c_char;

    // Game objects, listeners, positions, switches.
    RegisterGameObjInternal_WithName(id: u64, name: *const u16) -> i32;
    UnregisterGameObjInternal(id: u64) -> i32;
    SetObjectPosition(id: u64, position: AkVec3, front: AkVec3, top: AkVec3) -> i32;
    SetDefaultListeners(ids: *const u64, count: u32) -> i32;
    AddDefaultListener(id: u64) -> i32;
    RemoveDefaultListener(id: u64) -> i32;
    SetSwitch__SWIG_1(group: *const u16, state: *const u16, id: u64) -> i32;

    // Events.
    PostEvent__SWIG_2(event: u32, id: u64, flags: u32, callback: Ptr, cookie: Ptr) -> u32;
    PostEvent__SWIG_6(event: *const u16, id: u64, flags: u32, callback: Ptr, cookie: Ptr) -> u32;
    StopPlayingID__SWIG_2(playing: u32);
    StopAll__SWIG_1();
    GetSourcePlayPosition__SWIG_0(playing: u32, out_ms: *mut i32, extrapolate: u32) -> i32;

    // Frame, focus, output capture, monitoring, shutdown.
    RenderAudio__SWIG_1() -> i32;
    Suspend__SWIG_0(render_anyway: u32) -> i32;
    WakeupFromSuspend() -> i32;
    GetBufferTick() -> u32;
    GetSpeakerConfiguration__SWIG_1() -> Ptr;
    StartOutputCapture(file: *const c_char) -> i32;
    StopOutputCapture() -> i32;
    AddOutputCaptureMarker(text: *const c_char) -> i32;
    StartResourceMonitoring();
    StopResourceMonitoring();
    new_AkResourceMonitorDataSummary() -> Ptr;
    delete_AkResourceMonitorDataSummary(p: Ptr);
    GetResourceMonitorDataSummary(p: Ptr);
    AkResourceMonitorDataSummary_totalCPU_get(p: Ptr) -> f32;
    AkResourceMonitorDataSummary_pluginCPU_get(p: Ptr) -> f32;
    AkResourceMonitorDataSummary_physicalVoices_get(p: Ptr) -> u32;
    AkResourceMonitorDataSummary_virtualVoices_get(p: Ptr) -> u32;
    AkResourceMonitorDataSummary_nbActiveEvents_get(p: Ptr) -> u32;
    Term();
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api").field("symbols", &Self::SYMBOLS).finish_non_exhaustive()
    }
}

/// UTF-16, NUL-terminated: what Mono passes for `[MarshalAs(UnmanagedType.LPWStr)]`.
#[must_use]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `AKRESULT`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AkResult(pub i32);

impl AkResult {
    /// `AK_Success`.
    pub const SUCCESS: Self = Self(1);

    /// Whether this is `AK_Success`.
    #[must_use]
    pub fn is_success(self) -> bool {
        self == Self::SUCCESS
    }

    /// The enum name from the game's `AKRESULT.cs`, if known.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        Some(match self.0 {
            0 => "AK_NotImplemented",
            1 => "AK_Success",
            2 => "AK_Fail",
            3 => "AK_PartialSuccess",
            4 => "AK_NotCompatible",
            5 => "AK_AlreadyConnected",
            7 => "AK_InvalidFile",
            14 => "AK_InvalidID",
            15 => "AK_IDNotFound",
            22 => "AK_InvalidLanguage",
            31 => "AK_InvalidParameter",
            36 => "AK_PathNotFound",
            52 => "AK_InsufficientMemory",
            54 => "AK_UnknownBankID",
            56 => "AK_BankReadError",
            64 => "AK_WrongBankVersion",
            66 => "AK_FileNotFound",
            67 => "AK_DeviceNotReady",
            69 => "AK_BankAlreadyLoaded",
            75 => "AK_StreamMgrNotInitialized",
            77 => "AK_Busy",
            84 => "AK_DLLCannotLoad",
            85 => "AK_DLLPathNotFound",
            88 => "AK_PluginNotRegistered",
            92 => "AK_InitBankNotLoaded",
            93 => "AK_DeviceNotFound",
            94 => "AK_PlayingIDNotFound",
            99 => "AK_ResourceInUse",
            _ => return None,
        })
    }
}

impl fmt::Debug for AkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "{n}"),
            None => write!(f, "AKRESULT({})", self.0),
        }
    }
}

impl fmt::Display for AkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// `AkCallbackType` flags and values (game's `AkCallbackType.cs`).
pub mod callback_type {
    /// End of the event (all its actions and sounds finished).
    pub const END_OF_EVENT: u32 = 0x1;
    /// A cue marker embedded in a sound.
    pub const MARKER: u32 = 0x4;
    /// A sound's duration became known.
    pub const DURATION: u32 = 0x8;
    /// Allow `GetSourcePlayPosition` on the playing ID.
    pub const ENABLE_GET_SOURCE_PLAY_POSITION: u32 = 0x10_0000;
    /// Monitoring (log) message, routed through the serializer by `SetLocalOutput`.
    pub const MONITORING: u32 = 0x2000_0000;
    /// Bank load/unload completion.
    pub const BANK: u32 = 0x4000_0000;
    /// Audio interruption (mobile).
    pub const AUDIO_INTERRUPTION: u32 = 0x2200_0000;
    /// Background music change (consoles).
    pub const AUDIO_SOURCE_CHANGE: u32 = 0x2300_0000;
}

/// `AkMonitorErrorLevel::ErrorLevel_All`.
pub const ERROR_LEVEL_ALL: u32 = 3;

/// `AK_INVALID_GAME_OBJECT`.
pub const INVALID_GAME_OBJECT: u64 = u64::MAX;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated_utf16() {
        assert_eq!(wide("Ab"), vec![0x41, 0x62, 0]);
        assert_eq!(wide("é"), vec![0xE9, 0]);
    }

    #[test]
    fn ak_result_names() {
        assert!(AkResult(1).is_success());
        assert_eq!(format!("{}", AkResult(66)), "AK_FileNotFound");
        assert_eq!(format!("{}", AkResult(1234)), "AKRESULT(1234)");
    }
}
