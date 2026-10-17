//! The one RNG interface every consumer draws through (PLAN.md, simulation/presentation
//! contract item 3).
//!
//! The original draws dice (`RulebookEvent.Dice.D` → `UnityEngine.Random.Range(1,
//! sides + 1)`, `RulebookEvent.cs:70`), direct draws (`RuleSkillCheck`,
//! `ModifiableValueArmorClass.SelectMissReason`), animation variants, attack sequences,
//! dialogue cue selection and AI jitter all from Unity's single global generator, whose
//! state saves don't store. So:
//!
//! - **Replay** ([`Tape`]): the oracle records each `UnityEngine.Random` call individually
//!   (call site, kind, range, value, sequence number); the sim consumes them in order and
//!   **fails** on a draw from another call site or range, on a draw past the end of the tape
//!   (an extra draw), and, at [`Rng::finish`], on draws left over (a missing draw).
//! - **Play** ([`PlayRng`]): our own seeded generator, not Unity's.
//!
//! A failure doesn't panic: the generator records it, returns a deterministic fallback so
//! the caller can carry on, and the run is reported invalid (Decision 7: fail closed).
//!
//! **Call-site IDs** are WrathOracle's (`rng-sites.json`, schema
//! `wrath-oracle-rng-sites-v1`): `<assembly>|<CLR type>|<method(signature)>|#<n>@<IL
//! offset>|<Random member>`, where `n` is the 0-based ordinal of the `UnityEngine.Random`
//! call in the method's IL, e.g.
//! `Assembly-CSharp|Kingmaker.RuleSystem.RulebookEvent+Dice|D(Int32,DiceType)|#0@13|Range(int,int)`.

use serde::{Deserialize, Serialize};

/// Where a draw comes from in the original code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallSite(pub &'static str);

/// Known call sites.
pub mod sites {
    use super::CallSite;
    /// `RulebookEvent.Dice.D(int, DiceType)`: `Random.Range(1, dice.Sides() + 1)`.
    pub const DICE_D: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.RuleSystem.RulebookEvent+Dice|D(Int32,DiceType)|#0@13|Range(int,int)",
    );
    /// `ModifiableValueArmorClass.SelectMissReason`: `Random.Range(0, num)`.
    pub const AC_SELECT_MISS_REASON: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.EntitySystem.Stats.ModifiableValueArmorClass|SelectMissReason(Boolean,Boolean)|#0@40|Range(int,int)",
    );
    /// `RuleSkillCheck.OnTrigger`'s direct draw.
    pub const RULE_SKILL_CHECK: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.RuleSystem.Rules.RuleSkillCheck|OnTrigger(RulebookEventContext)|#0@197|Range(int,int)",
    );
    /// `CueSelection.Select()` with the `Random` strategy: `Random.Range(0, count)`.
    pub const CUE_SELECTION: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.DialogSystem.CueSelection|Select()|#0@119|Range(int,int)",
    );
    /// `CharacterSelection.SelectUnit` with the `Random` type: `Random.Range(0, party)`.
    pub const CHARACTER_SELECTION: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.DialogSystem.CharacterSelection|SelectUnit(BlueprintAnswer,UnitEntityData,Boolean)|#0@100|Range(int,int)",
    );

    /// `Projectile.CalculateMissTarget` (no `FallsOnMiss`): `num2 = MaxRange +
    /// Random.Range(-1, 1)` when the projectile's range is below its miss raycast distance.
    pub const MISS_TARGET_RANGE: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.Controllers.Projectiles.Projectile|CalculateMissTarget()|#2@289|Range(int,int)",
    );
    /// The deviation angle: `Random.Range(MinProjectileMissDeviation, MaxProjectileMissDeviation)`.
    pub const MISS_TARGET_DEVIATION: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.Controllers.Projectiles.Projectile|CalculateMissTarget()|#3@351|Range(float,float)",
    );
    /// Its side: `Random.value < 0.5f`.
    pub const MISS_TARGET_SIDE: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.Controllers.Projectiles.Projectile|CalculateMissTarget()|#4@356|get_value",
    );

    /// `UnitEntityData.DropLootToGround`: the dropped loot's yaw, `Random.Range(0f, 360f)`.
    pub const DROP_LOOT_ROTATION: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.EntitySystem.Entities.UnitEntityData|DropLootToGround(Boolean,Nullable`1,Boolean)|#0@237|Range(float,float)",
    );

    /// `UnitMovementAgent.SlowDown`: `m_SlowDownTime = Random.Range(0.2f, 0.3f)`.
    pub const AGENT_SLOW_DOWN: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.View.UnitMovementAgent|SlowDown(Single&)|#0@211|Range(float,float)",
    );

    /// `UnitCommand.TickApproaching`: `NextApproachTime = GameTime + Random.Range(0.2f, 0.6f)`.
    pub const TICK_APPROACHING: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.UnitLogic.Commands.Base.UnitCommand|TickApproaching()|#0@1147|Range(float,float)",
    );

    /// `AiBrainController.SelectAction`: `NextCommandTime += 0.3f + Random.Range(0f, 0.1f)`.
    pub const AI_NEXT_COMMAND_TIME: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.AI.AiBrainController|SelectAction(UnitEntityData,DecisionContext,AIDebugScope)|#0@417|Range(float,float)",
    );

    /// `CutscenePlayerGateData.SelectTrackToStart` of a `RandomTrack` gate: `Random.Range(0,
    /// tracks)` (made only from a recording's cutscene progress: `elements::cutscene`).
    pub const GATE_SELECT_TRACK: CallSite = CallSite(
        "Assembly-CSharp|Kingmaker.AreaLogic.Cutscenes.CutscenePlayerGateData|SelectTrackToStart()|#0@164|Range(int,int)",
    );

    /// Every site the sim draws from, for filtering an oracle tape to the T0 stream.
    pub const ALL: [CallSite; 10] = [
        DICE_D,
        AC_SELECT_MISS_REASON,
        RULE_SKILL_CHECK,
        CUE_SELECTION,
        CHARACTER_SELECTION,
        MISS_TARGET_RANGE,
        MISS_TARGET_DEVIATION,
        MISS_TARGET_SIDE,
        DROP_LOOT_ROTATION,
        GATE_SELECT_TRACK,
    ];
}

/// What was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DrawKind {
    /// `Random.Range(int min, int max)`: `[min, max)`, or `min` when `max <= min`.
    RangeInt { min: i32, max: i32 },
    /// `Random.Range(float min, float max)`: `[min, max]`.
    RangeFloat { min: f32, max: f32 },
    /// `Random.value`: `[0, 1]`.
    Value,
    /// `Random.insideUnitCircle`: a point in the unit disc.
    InsideUnitCircle,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DrawValue {
    Int(i32),
    Float(f32),
    /// A vector draw's components (`insideUnitCircle`'s `x`, `y`).
    Vec2([f32; 2]),
}

/// One recorded draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    pub seq: u64,
    pub site: String,
    pub kind: DrawKind,
    pub value: DrawValue,
}

/// Why a replay failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RngFailure {
    /// The sim drew past the end of the tape.
    ExtraDraw { seq: u64, site: String },
    /// The sim finished with recorded draws left over.
    MissingDraws { from_seq: u64, count: usize },
    /// The next recorded draw came from another call site.
    SiteMismatch { seq: u64, expected: String, actual: String },
    /// Same call site, different kind or range.
    KindMismatch { seq: u64, site: String },
    /// The recorded value is outside the requested range.
    ValueOutOfRange { seq: u64, site: String },
}

/// The interface every consumer draws through.
pub trait Rng {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32;
    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32;
    fn value(&mut self, site: CallSite) -> f32;
    /// `Random.insideUnitCircle`.
    fn inside_unit_circle(&mut self, site: CallSite) -> [f32; 2];
    /// Every draw made so far, in order (a play run's log replays as a tape).
    fn log(&self) -> &[Draw];
    /// Failures so far (always empty for [`PlayRng`]).
    fn failures(&self) -> &[RngFailure];
    /// Ends the run: a tape with draws left over records [`RngFailure::MissingDraws`].
    fn finish(&mut self);
}

/// A point of the closed unit disc (`insideUnitCircle`'s range), finite.
#[must_use]
pub fn in_unit_disc(v: [f32; 2]) -> bool {
    let [x, y] = v.map(f64::from);
    x.is_finite() && y.is_finite() && x * x + y * y <= 1.0
}

/// Replays a recorded tape.
#[derive(Debug, Clone)]
pub struct Tape {
    draws: Vec<Draw>,
    cursor: usize,
    log: Vec<Draw>,
    failures: Vec<RngFailure>,
}

impl Tape {
    #[must_use]
    pub fn new(draws: Vec<Draw>) -> Self {
        Self { draws, cursor: 0, log: Vec::new(), failures: Vec::new() }
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.draws.len() - self.cursor
    }

    fn next(&mut self, site: CallSite, kind: DrawKind) -> Option<DrawValue> {
        let seq = self.log.len() as u64;
        let Some(d) = self.draws.get(self.cursor) else {
            self.failures.push(RngFailure::ExtraDraw { seq, site: site.0.into() });
            return None;
        };
        if d.site != site.0 {
            self.failures.push(RngFailure::SiteMismatch {
                seq,
                expected: d.site.clone(),
                actual: site.0.into(),
            });
            return None;
        }
        if d.kind != kind {
            self.failures.push(RngFailure::KindMismatch { seq, site: site.0.into() });
            return None;
        }
        let ok = match (kind, d.value) {
            (DrawKind::RangeInt { min, max }, DrawValue::Int(v)) => {
                if max <= min {
                    v == min
                } else {
                    (min..max).contains(&v)
                }
            }
            (DrawKind::RangeFloat { min, max }, DrawValue::Float(v)) => (min..=max).contains(&v),
            (DrawKind::Value, DrawValue::Float(v)) => (0.0..=1.0).contains(&v),
            (DrawKind::InsideUnitCircle, DrawValue::Vec2(v)) => in_unit_disc(v),
            _ => false,
        };
        if !ok {
            self.failures.push(RngFailure::ValueOutOfRange { seq, site: site.0.into() });
            return None;
        }
        self.cursor += 1;
        let v = d.value;
        self.log.push(Draw { seq, site: site.0.into(), kind, value: v });
        Some(v)
    }
}

impl Rng for Tape {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32 {
        match self.next(site, DrawKind::RangeInt { min, max }) {
            Some(DrawValue::Int(v)) => v,
            _ => min,
        }
    }

    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32 {
        match self.next(site, DrawKind::RangeFloat { min, max }) {
            Some(DrawValue::Float(v)) => v,
            _ => min,
        }
    }

    fn value(&mut self, site: CallSite) -> f32 {
        match self.next(site, DrawKind::Value) {
            Some(DrawValue::Float(v)) => v,
            _ => 0.0,
        }
    }

    fn inside_unit_circle(&mut self, site: CallSite) -> [f32; 2] {
        match self.next(site, DrawKind::InsideUnitCircle) {
            Some(DrawValue::Vec2(v)) => v,
            _ => [0.0; 2],
        }
    }

    fn log(&self) -> &[Draw] {
        &self.log
    }

    fn failures(&self) -> &[RngFailure] {
        &self.failures
    }

    fn finish(&mut self) {
        let left = self.remaining();
        if left > 0 {
            self.failures
                .push(RngFailure::MissingDraws { from_seq: self.log.len() as u64, count: left });
            self.cursor = self.draws.len();
        }
    }
}

/// Our own generator for play: PCG-XSH-RR 64/32, seeded. Not Unity's algorithm, and it
/// doesn't need to be: saves don't store Unity's RNG state, so play can't reproduce the
/// original's rolls anyway.
#[derive(Debug, Clone)]
pub struct PlayRng {
    state: u64,
    inc: u64,
    log: Vec<Draw>,
}

impl PlayRng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut r = Self { state: 0, inc: (seed << 1) | 1, log: Vec::new() };
        r.step();
        r.state = r.state.wrapping_add(seed);
        r.step();
        r
    }

    fn step(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(self.inc);
        #[allow(clippy::cast_possible_truncation)]
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[0, n)` without modulo bias.
    fn below(&mut self, n: u32) -> u32 {
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.step();
            if r >= threshold {
                return r % n;
            }
        }
    }

    fn unit(&mut self) -> f32 {
        // 24 random bits mapped to [0, 1].
        #[allow(clippy::cast_precision_loss)]
        let x = (self.step() >> 8) as f32 / ((1u32 << 24) - 1) as f32;
        x
    }

    fn record(&mut self, site: CallSite, kind: DrawKind, value: DrawValue) {
        let seq = self.log.len() as u64;
        self.log.push(Draw { seq, site: site.0.into(), kind, value });
    }
}

impl Rng for PlayRng {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32 {
        let v = if max <= min {
            min
        } else {
            let span = u32::try_from(i64::from(max) - i64::from(min)).unwrap_or(u32::MAX);
            let off = self.below(span);
            i32::try_from(i64::from(min) + i64::from(off)).unwrap_or(min)
        };
        self.record(site, DrawKind::RangeInt { min, max }, DrawValue::Int(v));
        v
    }

    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32 {
        let t = self.unit();
        let v = crate::csharp::arith::store_f32(
            f64::from(min) + (f64::from(max) - f64::from(min)) * f64::from(t),
        );
        self.record(site, DrawKind::RangeFloat { min, max }, DrawValue::Float(v));
        v
    }

    fn value(&mut self, site: CallSite) -> f32 {
        let v = self.unit();
        self.record(site, DrawKind::Value, DrawValue::Float(v));
        v
    }

    fn inside_unit_circle(&mut self, site: CallSite) -> [f32; 2] {
        // Rejection from the square (not Unity's algorithm: play needn't reproduce it).
        let v = loop {
            let v = [self.unit() * 2.0 - 1.0, self.unit() * 2.0 - 1.0];
            if in_unit_disc(v) {
                break v;
            }
        };
        self.record(site, DrawKind::InsideUnitCircle, DrawValue::Vec2(v));
        v
    }

    fn log(&self) -> &[Draw] {
        &self.log
    }

    fn failures(&self) -> &[RngFailure] {
        &[]
    }

    fn finish(&mut self) {}
}

/// `RulebookEvent.Dice.D(rolls, dice)`: the sum of `rolls` draws of `Range(1, sides + 1)`,
/// each a separate draw from the same call site.
pub fn dice(rng: &mut dyn Rng, rolls: i32, sides: i32) -> i32 {
    let mut sum = 0i32;
    let mut n = rolls;
    while n > 0 {
        n -= 1;
        sum = crate::csharp::arith::add(sum, rng.range_int(sites::DICE_D, 1, sides + 1));
    }
    sum
}

/// An [`Rng`] that counts its draws into a shared cell (the trace's
/// [`crate::events::EventLog::draws`]), so records and draws can be put back in one order.
pub struct Counted {
    pub inner: Box<dyn Rng>,
    pub n: std::rc::Rc<std::cell::Cell<usize>>,
}

impl std::fmt::Debug for Counted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Counted").field("n", &self.n.get()).finish_non_exhaustive()
    }
}

impl Counted {
    fn sync(&self) {
        self.n.set(self.inner.log().len());
    }
}

impl Rng for Counted {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32 {
        let v = self.inner.range_int(site, min, max);
        self.sync();
        v
    }

    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32 {
        let v = self.inner.range_float(site, min, max);
        self.sync();
        v
    }

    fn value(&mut self, site: CallSite) -> f32 {
        let v = self.inner.value(site);
        self.sync();
        v
    }

    fn inside_unit_circle(&mut self, site: CallSite) -> [f32; 2] {
        let v = self.inner.inside_unit_circle(site);
        self.sync();
        v
    }

    fn log(&self) -> &[Draw] {
        self.inner.log()
    }

    fn failures(&self) -> &[RngFailure] {
        self.inner.failures()
    }

    fn finish(&mut self) {
        self.inner.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(seq: u64, v: i32, max: i32) -> Draw {
        Draw {
            seq,
            site: sites::DICE_D.0.into(),
            kind: DrawKind::RangeInt { min: 1, max },
            value: DrawValue::Int(v),
        }
    }

    #[test]
    fn tape_replays_in_order() {
        let mut t = Tape::new(vec![d(0, 3, 7), d(1, 6, 7)]);
        assert_eq!(dice(&mut t, 2, 6), 9);
        t.finish();
        assert!(t.failures().is_empty());
        assert_eq!(t.log().len(), 2);
    }

    #[test]
    fn tape_fails_on_extra_draw() {
        let mut t = Tape::new(vec![d(0, 3, 7)]);
        let _ = dice(&mut t, 2, 6);
        assert!(matches!(t.failures()[0], RngFailure::ExtraDraw { seq: 1, .. }));
    }

    #[test]
    fn tape_fails_on_missing_draw() {
        let mut t = Tape::new(vec![d(0, 3, 7), d(1, 4, 7)]);
        let _ = dice(&mut t, 1, 6);
        t.finish();
        assert_eq!(t.failures(), &[RngFailure::MissingDraws { from_seq: 1, count: 1 }]);
    }

    #[test]
    fn tape_fails_on_other_site_or_range() {
        let mut t = Tape::new(vec![d(0, 3, 7)]);
        let _ = t.range_int(sites::AC_SELECT_MISS_REASON, 1, 7);
        assert!(matches!(t.failures()[0], RngFailure::SiteMismatch { .. }));
        let mut t = Tape::new(vec![d(0, 3, 7)]);
        let _ = dice(&mut t, 1, 8);
        assert!(matches!(t.failures()[0], RngFailure::KindMismatch { .. }));
        let mut t = Tape::new(vec![d(0, 9, 7)]);
        let _ = dice(&mut t, 1, 6);
        assert!(matches!(t.failures()[0], RngFailure::ValueOutOfRange { .. }));
    }

    #[test]
    fn inside_unit_circle_replays_and_checks_its_range() {
        const SITE: CallSite = CallSite("roll");
        let mut p = PlayRng::new(7);
        let v: Vec<[f32; 2]> = (0..20).map(|_| p.inside_unit_circle(SITE)).collect();
        assert!(v.iter().all(|x| in_unit_disc(*x)));
        let mut t = Tape::new(p.log().to_vec());
        let w: Vec<[f32; 2]> = (0..20).map(|_| t.inside_unit_circle(SITE)).collect();
        t.finish();
        assert_eq!(v, w);
        assert!(t.failures().is_empty());
        let out = Draw {
            seq: 0,
            site: SITE.0.into(),
            kind: DrawKind::InsideUnitCircle,
            value: DrawValue::Vec2([0.8, 0.8]),
        };
        let mut t = Tape::new(vec![out]);
        let _ = t.inside_unit_circle(SITE);
        assert!(matches!(t.failures()[0], RngFailure::ValueOutOfRange { .. }));
        let mut t = Tape::new(vec![d(0, 3, 7)]);
        let _ = t.inside_unit_circle(sites::DICE_D);
        assert!(matches!(t.failures()[0], RngFailure::KindMismatch { .. }));
    }

    #[test]
    fn play_log_replays_as_tape() {
        let mut p = PlayRng::new(42);
        let a: Vec<i32> = (0..50).map(|_| dice(&mut p, 1, 20)).collect();
        assert!(a.iter().all(|v| (1..=20).contains(v)));
        assert_eq!(p.range_int(sites::DICE_D, 5, 5), 5);
        let mut t = Tape::new(p.log().to_vec());
        let b: Vec<i32> = (0..50).map(|_| dice(&mut t, 1, 20)).collect();
        assert_eq!(t.range_int(sites::DICE_D, 5, 5), 5);
        t.finish();
        assert_eq!(a, b);
        assert!(t.failures().is_empty());
        // Deterministic per seed.
        let mut q = PlayRng::new(42);
        let c: Vec<i32> = (0..50).map(|_| dice(&mut q, 1, 20)).collect();
        assert_eq!(a, c);
    }
}
