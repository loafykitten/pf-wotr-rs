//! Integration test against the game's real Wwise runtime and audio data.
//!
//! Ignored by default (it needs Ember's install). Run with:
//! `cargo nextest run --run-ignored only` (or `cargo test -- --ignored`), optionally with
//! `WRATH_APP=/path/to/Wrath.app`. It renders to the silent `No_Output` device and doesn't
//! open the profiler's sockets.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wwise_spike::engine::{Callback, EngineOptions, EnginePaths, SoundEngine};
use wwise_spike::ffi::{AkVec3, callback_type as cbt};
use wwise_spike::settings::{self, MAC};
use wwise_spike::shortid::short_id;

fn app() -> PathBuf {
    std::env::var_os("WRATH_APP").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builds/arm64-oracle/Wrath.app"),
        PathBuf::from,
    )
}

#[test]
#[ignore = "needs the game's AkSoundEngine.bundle and audio data"]
fn plays_the_seelah_anevia_cue_through_the_game_runtime() {
    let app = app();
    let banks = app.join("Contents/Resources/Data/StreamingAssets/Audio/GeneratedSoundBanks/Mac");
    let scratch = std::env::temp_dir().join(format!("wwise-spike-test-{}", std::process::id()));
    let paths = EnginePaths {
        library: app.join("Contents/PlugIns/AkSoundEngine.bundle/Contents/MacOS/AkSoundEngine"),
        plugin_dir: app.join("Contents/Plugins"),
        sound_banks: banks.clone(),
        persistent: scratch.join("persistent"),
        decoded_banks: scratch.join("DecodedBanks"),
    };
    let opts = EngineOptions { device_shareset: Some("No_Output".into()), init_comms: false };

    // SAFETY: the game's AkSoundEngine binary, whose signatures `ffi` declares.
    let mut engine = unsafe { SoundEngine::load(&paths.library) }.expect("load bundle");
    assert_eq!(engine.version().0, 2019, "Wwise major version changed: re-check the FFI");
    engine.init(&MAC, &paths, &opts).expect("init");
    assert!(engine.is_initialized());
    engine.load_packages(&banks, settings::PRELOAD_PACKAGES).expect("packages");
    engine.register(1, "Listener").unwrap();
    engine.set_default_listeners(&[1]).unwrap();
    for b in settings::PRELOAD_BANKS {
        assert_eq!(engine.load_bank(b).expect("bank"), short_id(b));
    }
    engine.register(2, "Emitter").unwrap();
    engine
        .set_position(
            2,
            AkVec3::new(0.0, 0.0, 2.0),
            AkVec3::new(0.0, 0.0, 1.0),
            AkVec3::new(0.0, 1.0, 0.0),
        )
        .unwrap();

    let id = engine.post_event("CH0_MeetSeelahAnevia_01", 2, cbt::END_OF_EVENT | cbt::DURATION, 7);
    assert_ne!(id, 0);
    let (mut duration, mut ended) = (None, false);
    let start = Instant::now();
    while !ended && start.elapsed() < Duration::from_secs(20) {
        engine.wakeup().unwrap();
        engine.poll_callbacks(|cb| match cb {
            Callback::Duration { cookie: 7, media_id, streaming, duration_ms, .. } => {
                duration = Some((media_id, streaming, duration_ms));
            }
            Callback::EndOfEvent { cookie: 7, playing_id, event_id, .. } => {
                assert_eq!((playing_id, event_id), (id, short_id("CH0_MeetSeelahAnevia_01")));
                ended = true;
            }
            _ => {}
        });
        engine.render_audio().unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(ended, "no EndOfEvent within 20 s");
    let (media, streaming, ms) = duration.expect("no Duration callback");
    assert_eq!(media, 957_188_610); // the .wem SoundbanksInfo.xml lists for this event
    assert!(streaming);
    assert!((ms - 3885.3).abs() < 1.0, "{ms}");

    engine.unregister(2).unwrap();
    engine.unregister(1).unwrap();
    engine.shutdown();
    assert!(!engine.is_initialized());
    let _ = std::fs::remove_dir_all(scratch);
}
