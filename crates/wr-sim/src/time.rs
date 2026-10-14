//! The sim clock: `Kingmaker.Controllers.TimeController.Tick` (PLAN.md, simulation/
//! presentation contract item 1: the simulation owns time).
//!
//! Per frame the original computes
//! `DeltaTime = Math.Min(PlayerTimeScale * DebugTimeScale / 12f, justActivated ?
//! Time.unscaledDeltaTime : Time.deltaTime)`: the `/12` caps a step at 1/12 s of game time
//! (scaled), it doesn't fix the rate. Game time (`Player.GameTime`) advances by
//! `TimeSpan.FromSeconds(DeltaTime)`, which rounds to whole milliseconds, and only in the
//! `Default` and `Cutscene` game modes. In turn-based combat the player time scale is
//! multiplied for the frame by a factor the combat controller picks (waiting for UI,
//! between turns, idle, player or enemy turn), capped at 3, and restored afterwards.
//!
//! Inputs are what Unity hands the original each frame: `Time.deltaTime` (already scaled
//! by the previous frame's `Time.timeScale` and capped by `Time.maximumDeltaTime`) and
//! `Time.unscaledDeltaTime`. In replay they come from the oracle's tape; in play the host
//! supplies them.

use crate::csharp::arith::store_f32;
use crate::csharp::timespan::TimeSpan;

/// `GameModeType`, as far as the clock cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Default,
    Cutscene,
    Dialog,
    Pause,
    GlobalMap,
    /// Any other mode: game time doesn't advance.
    Other,
}

/// One frame's inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// `Time.deltaTime`.
    pub delta_time: f32,
    /// `Time.unscaledDeltaTime`.
    pub unscaled_delta_time: f32,
    pub mode: GameMode,
    /// In turn-based combat: the factor `TimeController.Tick` picks from the combat
    /// controller's state (`TimeScaleWhenIdle`, `TimeScaleBetweenTurns`, …). `None`
    /// outside turn-based combat.
    pub turn_based_scale: Option<f32>,
}

/// What a tick produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub delta_time: f32,
    pub game_delta_time: f32,
    /// The whole-millisecond step added to game time.
    pub game_time_step: TimeSpan,
    /// `Time.timeScale` set for the next frame.
    pub time_scale: f32,
    /// In turn-based combat the original then runs `CombatController.TickTime()`, which
    /// can clamp the deltas and overwrite game time. Not ported: the caller must treat a
    /// tick with this flag as unsupported (`World::tick_clock` does).
    pub needs_combat_tick_time: bool,
}

#[derive(Debug, Clone)]
pub struct SimClock {
    pub player_time_scale: f32,
    pub debug_time_scale: f32,
    ui_time_scale: f32,
    just_activated: bool,
    delta_time: f32,
    game_delta_time: f32,
    /// `RingBuffer<float>(5)` of recent deltas, newest first.
    last_deltas: std::collections::VecDeque<f32>,
    /// `Player.GameTime` (saved).
    pub game_time: TimeSpan,
    /// `Player.RealTime` (saved).
    pub real_time: TimeSpan,
}

impl SimClock {
    #[must_use]
    pub fn new(game_time: TimeSpan, real_time: TimeSpan) -> Self {
        Self {
            player_time_scale: 1.0,
            debug_time_scale: 1.0,
            ui_time_scale: 1.0,
            just_activated: false,
            delta_time: 0.0,
            game_delta_time: 0.0,
            last_deltas: std::collections::VecDeque::with_capacity(5),
            game_time,
            real_time,
        }
    }

    #[must_use]
    pub fn delta_time(&self) -> f32 {
        self.delta_time
    }

    #[must_use]
    pub fn game_delta_time(&self) -> f32 {
        self.game_delta_time
    }

    /// The frame's `GameDeltaTime` as a recording gives it (a driven replay doesn't run
    /// `TimeController.Tick`).
    pub fn set_game_delta_time(&mut self, v: f32) {
        self.game_delta_time = v;
    }

    /// The frame's `DeltaTime` as a recording gives it.
    pub fn set_delta_time(&mut self, v: f32) {
        self.delta_time = v;
    }

    /// `UiTimeScale` setter: `Mathf.Clamp01`.
    pub fn set_ui_time_scale(&mut self, v: f32) {
        self.ui_time_scale = crate::csharp::arith::clamp01(v);
    }

    fn set_delta(&mut self, v: f32) {
        self.delta_time = v;
        if self.last_deltas.len() == 5 {
            self.last_deltas.pop_back();
        }
        self.last_deltas.push_front(v);
    }

    /// `TimeController.Activate`.
    pub fn activate(&mut self) {
        self.just_activated = true;
    }

    /// `TimeController.Suspend` (also what loading does every frame).
    pub fn suspend(&mut self) {
        self.set_delta(0.0);
    }

    /// `TimeController.Tick`.
    ///
    /// # Errors
    ///
    /// Game time overflowing `TimeSpan` (the C# throws).
    pub fn tick(&mut self, f: Frame) -> Result<Tick, crate::csharp::arith::CsError> {
        let saved_scale = self.player_time_scale;
        if let Some(num) = f.turn_based_scale {
            self.player_time_scale = store_f32(f64::from(saved_scale) * f64::from(num));
            if self.player_time_scale > 3.0 {
                self.player_time_scale = 3.0;
            }
        }
        let cap =
            store_f32(f64::from(self.player_time_scale) * f64::from(self.debug_time_scale) / 12.0);
        let frame = if self.just_activated { f.unscaled_delta_time } else { f.delta_time };
        // `Math.Min(float, float)`: NaN if either is NaN.
        let d = if cap.is_nan() || frame.is_nan() { f32::NAN } else { cap.min(frame) };
        self.set_delta(d);
        self.game_delta_time = 0.0;
        let mut step = TimeSpan::ZERO;
        if matches!(f.mode, GameMode::Default | GameMode::Cutscene) {
            self.game_delta_time = d;
            step = TimeSpan::from_seconds(f64::from(d))?;
            self.game_time = self.game_time.checked_add(step)?;
        }
        let time_scale = store_f32(
            f64::from(self.player_time_scale)
                * f64::from(self.debug_time_scale)
                * f64::from(self.ui_time_scale),
        );
        self.just_activated = false;
        if f.turn_based_scale.is_some() {
            self.player_time_scale = saved_scale;
        }
        Ok(Tick {
            delta_time: d,
            game_delta_time: self.game_delta_time,
            game_time_step: step,
            time_scale,
            needs_combat_tick_time: f.turn_based_scale.is_some(),
        })
    }

    /// `TimeController.TickRealTime`.
    ///
    /// # Errors
    ///
    /// Overflow.
    pub fn tick_real_time(&mut self, unscaled: f32) -> Result<(), crate::csharp::arith::CsError> {
        self.real_time =
            self.real_time.checked_add(TimeSpan::from_seconds(f64::from(unscaled))?)?;
        Ok(())
    }

    /// `GetDeltaTimeForFrames(n)`: the sum of the last `n` deltas (newest first).
    #[must_use]
    pub fn delta_for_frames(&self, n: usize) -> f32 {
        let mut s = 0f64;
        for d in self.last_deltas.iter().take(n) {
            s = f64::from(store_f32(s + f64::from(*d)));
        }
        store_f32(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(dt: f32, mode: GameMode) -> Frame {
        Frame { delta_time: dt, unscaled_delta_time: dt, mode, turn_based_scale: None }
    }

    #[test]
    fn delta_is_capped_at_a_twelfth_and_game_time_steps_in_milliseconds() {
        let mut c = SimClock::new(TimeSpan::ZERO, TimeSpan::ZERO);
        let t = c.tick(frame(0.5, GameMode::Default)).unwrap();
        assert_eq!(t.delta_time, store_f32(1.0 / 12.0));
        assert_eq!(t.game_time_step, TimeSpan(83 * 10_000));
        let t = c.tick(frame(1.0 / 80.0, GameMode::Default)).unwrap();
        assert_eq!(t.game_time_step, TimeSpan(13 * 10_000));
        assert_eq!(c.game_time, TimeSpan(96 * 10_000));
        // Dialog: no game time.
        let t = c.tick(frame(0.01, GameMode::Dialog)).unwrap();
        assert_eq!(t.game_delta_time, 0.0);
        assert_eq!(c.game_time, TimeSpan(96 * 10_000));
    }

    #[test]
    fn eighty_frames_of_80hz_is_not_one_second() {
        // 12.5 ms rounds to 13 ms per frame: frame rate leaks into game time.
        let mut c = SimClock::new(TimeSpan::ZERO, TimeSpan::ZERO);
        for _ in 0..80 {
            c.tick(frame(1.0 / 80.0, GameMode::Default)).unwrap();
        }
        assert_eq!(c.game_time, TimeSpan(80 * 13 * 10_000));
    }

    #[test]
    fn turn_based_scale_is_capped_and_restored() {
        let mut c = SimClock::new(TimeSpan::ZERO, TimeSpan::ZERO);
        c.player_time_scale = 2.0;
        let t = c
            .tick(Frame {
                delta_time: 1.0,
                unscaled_delta_time: 1.0,
                mode: GameMode::Default,
                turn_based_scale: Some(2.0),
            })
            .unwrap();
        assert_eq!(t.delta_time, 0.25);
        assert_eq!(t.time_scale, 3.0);
        assert_eq!(c.player_time_scale, 2.0);
    }

    #[test]
    fn just_activated_uses_unscaled_delta() {
        let mut c = SimClock::new(TimeSpan::ZERO, TimeSpan::ZERO);
        c.activate();
        let t = c
            .tick(Frame {
                delta_time: 0.0,
                unscaled_delta_time: 0.02,
                mode: GameMode::Default,
                turn_based_scale: None,
            })
            .unwrap();
        assert_eq!(t.delta_time, 0.02);
        let t = c.tick(frame(0.0, GameMode::Default)).unwrap();
        assert_eq!(t.delta_time, 0.0);
        assert_eq!(c.delta_for_frames(3), 0.02);
    }
}
