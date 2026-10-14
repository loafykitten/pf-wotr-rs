//! `UnitAttackOfOpportunity`'s class code (POC-5 slice C; `docs/poc5/commands.md`), run as
//! lifecycle code when the recording has WrathOracle's `approach` observer: `Init` (the threat
//! hand), `OnRun` (the base one, then the animation's gate), `OnStart` and `OnTick` (the base
//! ones), `OnAction` (its attack rule, the sim's: the recorded root is its check) and its answers
//! to the lifecycle.
//!
//! Inputs: `ShouldBeInterrupted` (`CanAttack`: conditions and the attack-type restriction, a
//! recorded leaf), the view's animation manager and whether it can run an idle action. Past that
//! gate `OnRun` is presentation (the attack's animation handle, `LookAt`).

use super::attack::{Slot, Spec};
use super::{Inputs, Of, R, cmd, cmd_mut, executor, interrupt, on_run_base, res, unsupported};
use crate::combat::weapon::{self, Hand, Weapon};
use crate::world::{EntityIdx, World};

/// `MechanicsFeatureType.SnapShot`.
const SNAP_SHOT: i64 = 56;

/// An attack of opportunity's own state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// `ForceFlatFooted`, `m_ShouldTurnToTarget` (its creator's; `uc_new`'s `aoo`).
    pub force_flat_footed: bool,
    pub turn: bool,
    /// `Hand` (`GetThreatHand` in `Init`), with its item reference's id as recorded, and the
    /// hands set it was of (a hand is the current set's slot object).
    pub hand: Option<(Slot, String)>,
    pub hand_set: Option<i32>,
    /// A limb's item at `Init` (the slot object `Hand` holds keeps its own: an item removed or
    /// granted again since isn't followed by the limb's index).
    pub limb_item: Option<EntityIdx>,
}

impl State {
    /// `ForceFlatFooted m_ShouldTurnToTarget`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let [force_flat_footed, turn] = super::flags::<2>(s)?;
        Some(Self { force_flat_footed, turn, hand: None, hand_set: None, limb_item: None })
    }
}

/// Whether the sim runs this command's class code.
#[must_use]
pub fn ported(w: &World, c: &super::Cmd) -> bool {
    w.combat.commands.approach && c.class == super::class::AOO
}

fn st(w: &World, c: u32) -> R<&State> {
    match &cmd(w, c)?.aoo {
        Some(s) => Ok(s),
        None => super::desync(format!("command {c} has no attack of opportunity state")),
    }
}

/// `IsThreatHand(unit, hand)`: a weapon, melee (or any with snap shot), unarmed only with
/// improved unarmed strike.
fn is_threat_hand(w: &mut World, u: EntityIdx, s: Slot) -> bool {
    let (wp, has) = super::attack::slot_weapon(w, u, s);
    let Some(x) = wp.filter(|_| has).and_then(|i| Weapon::of(w, i)) else { return false };
    if !(x.is_melee() || weapon::has_feature(w, u, SNAP_SHOT)) {
        return false;
    }
    !x.is_unarmed() || weapon::has_feature(w, u, weapon::IMPROVED_UNARMED_STRIKE)
}

/// `UnitHelper.GetThreatHand(unit)`: the primary hand, the secondary unless the primary's weapon
/// is two-handed, then the additional limbs in order.
pub fn threat_hand(w: &mut World, u: EntityIdx) -> Option<Slot> {
    if is_threat_hand(w, u, Slot::Primary) {
        return Some(Slot::Primary);
    }
    let two_handed = weapon::hand_weapon(w, u, Hand::Primary)
        .and_then(|i| Weapon::of(w, i))
        .is_some_and(|x| x.is_two_handed());
    if !two_handed && is_threat_hand(w, u, Slot::Secondary) {
        return Some(Slot::Secondary);
    }
    let limbs = w.unit(u).map_or(0, |x| x.body.limbs.len());
    (0..limbs).map(Slot::Limb).find(|s| is_threat_hand(w, u, *s))
}

/// A limb slot's item (`None` for a hand, an empty limb or none).
fn limb_item(w: &World, u: EntityIdx, s: Option<Slot>) -> Option<EntityIdx> {
    let Some(Slot::Limb(i)) = s else { return None };
    w.unit(u).and_then(|x| x.body.limbs.get(i)).and_then(|l| l.item)
}

/// `Init(executor)`: the base one, then the threat hand; without one the command is interrupted.
pub fn init<I: Inputs>(w: &mut World, inp: &mut I, c: u32, owner: Option<String>) -> R<()> {
    cmd_mut(w, c)?.s.executor = owner;
    let u = executor(w, c)?;
    let hand = threat_hand(w, u);
    let item = inp.threat(w, c, u, hand)?;
    let hand = hand.zip(item);
    let set = w.unit(u).map(|x| x.body.current_hand_set);
    let limb_item = limb_item(w, u, hand.as_ref().map(|h| h.0));
    if let Some(s) = &mut cmd_mut(w, c)?.aoo {
        s.hand = hand.clone();
        s.hand_set = set;
        s.limb_item = limb_item;
    }
    if hand.is_none() {
        // "can't make attack of opportunity: has no threat hand".
        interrupt(w, inp, c, true)?;
    }
    Ok(())
}

/// `OnRun()`: the base one, then (presentation) the attack's animation when the view's manager
/// can run an idle action, which reads the executor's mount first.
pub fn on_run<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    on_run_base(w, inp, c)?;
    if !inp.leaf(w, "anim", Of::Cmd(c))? || !inp.leaf(w, "idle", Of::Cmd(c))? {
        return Ok(());
    }
    let u = executor(w, c)?;
    let uid = w.entity(u).unique_id.clone();
    if inp.saddled(w, Some(&uid))?.is_some() {
        return unsupported(w, "UnitAttackOfOpportunity.OnRun: a mounted executor");
    }
    Ok(())
}

/// `OnAction()`: unless the target is dead, `RuleAttackWithWeapon(executor, target, Hand.Weapon,
/// 0)` with `IsAttackOfOpportunity` and `ForceFlatFooted`; then `Success`.
pub fn on_action<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<u8> {
    let u = executor(w, c)?;
    let Some(t) = super::target_in_world(w, c)?.and_then(|id| w.by_id.get(&id).copied()) else {
        return unsupported(w, "UnitAttackOfOpportunity.OnAction: a target that doesn't resolve");
    };
    if super::super::units::is_dead(w, t) {
        return Ok(res::SUCCESS);
    }
    let s = st(w, c)?.clone();
    let Some((slot, item)) = s.hand else {
        return unsupported(w, "UnitAttackOfOpportunity.OnAction without a threat hand");
    };
    if matches!(slot, Slot::Limb(_)) && limb_item(w, u, Some(slot)) != s.limb_item {
        return unsupported(
            w,
            "UnitAttackOfOpportunity.OnAction: the threat limb's item changed since Init",
        );
    }
    if !matches!(slot, Slot::Limb(_)) && w.unit(u).map(|x| x.body.current_hand_set) != s.hand_set {
        return unsupported(
            w,
            "UnitAttackOfOpportunity.OnAction: the hands set changed since Init",
        );
    }
    // `Hand.Weapon` (`MaybeWeapon`; throws without one).
    let (Some(weapon), _) = super::attack::slot_weapon(w, u, slot) else {
        return unsupported(w, "UnitAttackOfOpportunity.OnAction: a hand without a weapon");
    };
    let spec = Spec {
        initiator: u,
        target: t,
        weapon,
        slot,
        item,
        penalty: 0,
        is_rend: false,
        is_first_attack: false,
        is_full_attack: false,
        is_charge: false,
        attack_number: 0,
        attacks_count: 0,
        opportunity: Some(s.force_flat_footed),
    };
    inp.attack_rule(w, c, &spec)?;
    Ok(res::SUCCESS)
}

/// Its answer to a lifecycle getter `f` (`None`: not one it answers; `ShouldBeInterrupted` is a
/// recorded leaf).
pub fn answer(w: &World, c: u32, f: &str) -> R<Option<bool>> {
    let x = cmd(w, c)?;
    Ok(match f {
        "stt" => Some(st(w, c)?.turn),
        "sbi" => None,
        _ => super::base_answer(x, f),
    })
}
