//! The game's Wwise initialisation settings.
//!
//! Values come from the serialized `AkWwiseInitializationSettings` platform object named
//! `Mac` (a MonoBehaviour-script asset, path ID 1200 in `Data/sharedassets0.assets` of the
//! 2026-10-10 install), decoded field by field in the order the decompiled
//! `AkCommonUserSettings`/`AkCommonAdvancedSettings`/`AkCommonCommSettings` declare them.
//! The decode consumed the object exactly (2,564 bytes). They match Wwise's Unity defaults
//! except that `m_InitializeSystemComms` is `false` and `m_NetworkName` is `"\0"`.
//!
//! The package and bank chunks come from the `AudioFilePackagesSettings` asset (path ID
//! 1197 in the same file).
//!
//! How each field reaches the engine is `AkCommonPlatformSettings.AkInitializationSettings`
//! and the `CopyTo` methods; [`crate::engine`] applies them in the same order.

/// `AkCommonOutputSettings.ChannelConfiguration.ChannelConfigType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelConfigType {
    /// Channel mask 0, channel count set directly (0 = let the device decide).
    Anonymous,
    /// Standard speaker mask.
    Standard,
    /// Ambisonic order.
    Ambisonic,
}

/// `AkCommonUserSettings` (the fields that reach native settings).
#[derive(Debug, Clone, PartialEq)]
pub struct UserSettings {
    /// `m_BasePath`, relative to `StreamingAssets` (backslashes as serialized).
    pub base_path: &'static str,
    /// `m_StartupLanguage`.
    pub startup_language: &'static str,
    /// `m_EngineLogging` (only feeds `AkCallbackManager.InitializationSettings`).
    pub engine_logging: bool,
    /// `m_MaximumNumberOfPositioningPaths` -> `AkInitSettings.uMaxNumPaths`.
    pub max_positioning_paths: u32,
    /// `m_CommandQueueSize` -> `uCommandQueueSize`.
    pub command_queue_size: u32,
    /// `m_SamplesPerFrame` -> `uNumSamplesPerFrame`.
    pub samples_per_frame: u32,
    /// `m_MainOutputSettings.m_AudioDeviceShareset` (empty = default device).
    pub audio_device_shareset: &'static str,
    /// `m_MainOutputSettings.m_DeviceID`.
    pub device_id: u32,
    /// `m_MainOutputSettings.m_PanningRule` (0 = speakers, 1 = headphones).
    pub panning_rule: i32,
    /// `m_MainOutputSettings.m_ChannelConfig.m_ChannelConfigType`.
    pub channel_config_type: ChannelConfigType,
    /// `m_ChannelConfig.m_ChannelMask`.
    pub channel_mask: u32,
    /// `m_ChannelConfig.m_NumberOfChannels`.
    pub number_of_channels: u32,
    /// `m_StreamingLookAheadRatio` -> `AkMusicSettings.fStreamingLookAheadRatio`.
    pub streaming_look_ahead_ratio: f32,
    /// `m_SampleRate` -> `AkPlatformInitSettings.uSampleRate`.
    pub sample_rate: u32,
    /// `m_NumberOfRefillsInVoice` -> `uNumRefillsInVoice`.
    pub refills_in_voice: u16,
    /// `m_SpatialAudioSettings`.
    pub spatial: SpatialUserSettings,
}

/// `AkCommonUserSettings.SpatialAudioSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialUserSettings {
    /// `m_MaxSoundPropagationDepth`.
    pub max_sound_propagation_depth: u32,
    /// `m_DiffractionFlags` (serialized as -1: all flags).
    pub diffraction_flags: i32,
    /// `m_MovementThreshold`.
    pub movement_threshold: f32,
    /// `m_NumberOfPrimaryRays`.
    pub number_of_primary_rays: u32,
    /// `m_MaxReflectionOrder`.
    pub max_reflection_order: u32,
    /// `m_MaxPathLength`.
    pub max_path_length: f32,
    /// `m_EnableDiffractionOnReflections`.
    pub enable_diffraction_on_reflections: bool,
    /// `m_EnableDirectPathDiffraction`.
    pub enable_direct_path_diffraction: bool,
    /// `m_EnableTransmission`.
    pub enable_transmission: bool,
}

/// `AkCommonAdvancedSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvancedSettings {
    /// `m_IOMemorySize` -> `AkDeviceSettings.uIOMemorySize`.
    pub io_memory_size: u32,
    /// `m_TargetAutoStreamBufferLengthMs` -> `fTargetAutoStmBufferLength`.
    pub target_auto_stream_buffer_length_ms: f32,
    /// `m_UseStreamCache` -> `bUseStreamCache`.
    pub use_stream_cache: bool,
    /// `m_MaximumPinnedBytesInCache` -> `uMaxCachePinnedBytes`.
    pub maximum_pinned_bytes_in_cache: u32,
    /// `m_EnableGameSyncPreparation` -> `AkInitSettings.bEnableGameSyncPreparation`.
    pub enable_game_sync_preparation: bool,
    /// `m_ContinuousPlaybackLookAhead` -> `uContinuousPlaybackLookAhead`.
    pub continuous_playback_look_ahead: u32,
    /// `m_MonitorQueuePoolSize` -> `uMonitorQueuePoolSize`.
    pub monitor_queue_pool_size: u32,
    /// `m_MaximumHardwareTimeoutMs` -> `uMaxHardwareTimeoutMs`.
    pub maximum_hardware_timeout_ms: u32,
    /// `m_DebugOutOfRangeCheckEnabled`.
    pub debug_out_of_range_check_enabled: bool,
    /// `m_DebugOutOfRangeLimit`.
    pub debug_out_of_range_limit: f32,
    /// `m_SpatialAudioSettings.m_DiffractionShadowAttenuationFactor`.
    pub diffraction_shadow_attenuation_factor: f32,
    /// `m_SpatialAudioSettings.m_DiffractionShadowDegrees`.
    pub diffraction_shadow_degrees: f32,
    /// `m_RenderDuringFocusLoss`: `Suspend(renderAnyway)` on focus loss.
    pub render_during_focus_loss: bool,
    /// `m_SoundBankPersistentDataPath` (empty: banks come from `StreamingAssets`).
    pub sound_bank_persistent_data_path: &'static str,
    /// `m_UseAsyncOpen` -> `AkInitializationSettings.useAsyncOpen`.
    pub use_async_open: bool,
}

/// `AkCommonCommSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommSettings {
    /// `m_PoolSize`.
    pub pool_size: u32,
    /// `m_DiscoveryBroadcastPort`.
    pub discovery_broadcast_port: u16,
    /// `m_CommandPort` (0 = ephemeral).
    pub command_port: u16,
    /// `m_NotificationPort` (0 = ephemeral).
    pub notification_port: u16,
    /// `m_InitializeSystemComms`.
    pub initialize_system_comms: bool,
    /// `m_NetworkName`. Serialized as a single NUL character, which marshals to an empty
    /// C string; the C# substitutes `Application.productName` only for a null or empty
    /// managed string, so the native side receives "".
    pub network_name: &'static str,
    /// `m_commSystem` (0 = socket).
    pub comm_system: i32,
}

/// Everything the game feeds Wwise at startup on macOS.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSettings {
    /// User settings.
    pub user: UserSettings,
    /// Advanced settings.
    pub advanced: AdvancedSettings,
    /// Communication settings.
    pub comms: CommSettings,
    /// `AkCallbackManager.InitializationSettings.DefaultBufferSize`.
    pub callback_buffer_size: u32,
}

/// The `Mac` platform object as serialized in the game.
pub const MAC: GameSettings = GameSettings {
    user: UserSettings {
        base_path: "Audio\\GeneratedSoundBanks",
        startup_language: "English(US)",
        engine_logging: true,
        max_positioning_paths: 255,
        command_queue_size: 262_144,
        samples_per_frame: 1024,
        audio_device_shareset: "",
        device_id: 0,
        panning_rule: 0,
        channel_config_type: ChannelConfigType::Anonymous,
        channel_mask: 0,
        number_of_channels: 0,
        streaming_look_ahead_ratio: 1.0,
        sample_rate: 48_000,
        refills_in_voice: 4,
        spatial: SpatialUserSettings {
            max_sound_propagation_depth: 8,
            diffraction_flags: -1,
            movement_threshold: 1.0,
            number_of_primary_rays: 100,
            max_reflection_order: 1,
            max_path_length: 10_000.0,
            enable_diffraction_on_reflections: true,
            enable_direct_path_diffraction: true,
            enable_transmission: true,
        },
    },
    advanced: AdvancedSettings {
        io_memory_size: 2_097_152,
        target_auto_stream_buffer_length_ms: 380.0,
        use_stream_cache: false,
        maximum_pinned_bytes_in_cache: u32::MAX,
        enable_game_sync_preparation: false,
        continuous_playback_look_ahead: 1,
        monitor_queue_pool_size: 65_536,
        maximum_hardware_timeout_ms: 1000,
        debug_out_of_range_check_enabled: false,
        debug_out_of_range_limit: 16.0,
        diffraction_shadow_attenuation_factor: 1.0,
        diffraction_shadow_degrees: 30.0,
        render_during_focus_loss: false,
        sound_bank_persistent_data_path: "",
        use_async_open: false,
    },
    comms: CommSettings {
        pool_size: 262_144,
        discovery_broadcast_port: 24_024,
        command_port: 0,
        notification_port: 0,
        initialize_system_comms: false,
        network_name: "",
        comm_system: 0,
    },
    callback_buffer_size: 4096,
};

/// `AudioFilePackagesSettings.AudioChunk.Preload`: file packages loaded at audio startup,
/// in order (`m_PackageMapping[0]`).
pub const PRELOAD_PACKAGES: &[&str] = &[
    "Wrath_Main_Default.pck",
    "Wrath_Main_Music.pck",
    "Wrath_Main_VO_PC.pck",
    "Wrath_Main_Creatures.pck",
    "Wrath_Main_VO_Companions.pck",
    "Wrath_Main_Narrative.pck",
    "Wrath_Main_VFX.pck",
    "Wrath_Main_Ambs_Prologue.pck",
    "Wrath_Main_VO_Dialogues_Prologue.pck",
    "Wrath_Main_VO_Dialogues_Other.pck",
    "Wrath_Main_VO_Dialogues_PartyBanter.pck",
];

/// `AudioChunk.Preload` banks, loaded synchronously after the packages
/// (`m_BankMapping[0]`).
pub const PRELOAD_BANKS: &[&str] = &[
    "Main",
    "VFX",
    "Interface",
    "Music",
    "Tech",
    "Dialogs_ENG",
    "Dialogs_Prologue_ENG",
    "Dialogs_PartyBanter_ENG",
];

/// `AudioChunk.MainGame` packages (loaded when leaving the prologue).
pub const MAIN_GAME_PACKAGES: &[&str] = &[
    "Wrath_Main_Ambs_CH1.pck",
    "Wrath_Main_Ambs_CH2.pck",
    "Wrath_Main_Ambs_CH3.pck",
    "Wrath_Main_Ambs_CH4.pck",
    "Wrath_Main_Ambs_CH5.pck",
    "Wrath_Main_Ambs_CH6.pck",
    "Wrath_Main_VO_Dialogues_CH1.pck",
    "Wrath_Main_VO_Dialogues_CH2.pck",
    "Wrath_Main_VO_Dialogues_CH3.pck",
    "Wrath_Main_VO_Dialogues_CH4.pck",
    "Wrath_Main_VO_Dialogues_CH5.pck",
    "Wrath_Main_VO_Dialogues_CH6.pck",
];

/// `AudioChunk.MainGame` banks.
pub const MAIN_GAME_BANKS: &[&str] = &[
    "Dialogs_CH1_ENG",
    "Dialogs_CH2_ENG",
    "Dialogs_CH3_ENG",
    "Dialogs_CH4_ENG",
    "Dialogs_CH5_ENG",
    "Dialogs_CH6_ENG",
];

/// `AudioChunk.DLC1`..`DLC6`: each loads `Wrath_DLC<n>_VO_Dialogues.pck` and
/// `Wrath_DLC<n>_Other.pck`, then the bank `Dialogs_DLC<n>_ENG`.
pub const DLC_CHUNKS: u8 = 6;

/// Packages and bank of DLC chunk `n` (1-based).
#[must_use]
pub fn dlc_chunk(n: u8) -> ([String; 2], String) {
    (
        [format!("Wrath_DLC{n}_VO_Dialogues.pck"), format!("Wrath_DLC{n}_Other.pck")],
        format!("Dialogs_DLC{n}_ENG"),
    )
}
