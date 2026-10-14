//! `UnitAttack`'s class code (POC-5 round 9, slice B; `docs/poc5/commands.md`), run as lifecycle
//! code when the recording has WrathOracle's `attacks` observer: `Init`, `InitAttacks`
//! (`CreateFullAttack`, `CreateSingleAttack`, `EnumerateAttacks`), `OnStart`,
//! `TryStartNextAttack`, `OnTick` (the iteration delay), `UpdateTarget` (with its retarget),
//! `OnAction`, `ProvideNextAttack` and `TriggerAttackRule`.
//!
//! The rules it triggers are the sim's ([`Inputs::attacks_count`], [`Inputs::attack_rule`]: the
//! recorded roots are their checks). What it reads outside the sim is an input, in the order the
//! game read it: `IsUnitCloseEnough`, `IsUnitInFiveFeetRange` (positions, corpulences, the
//! weapon's range), the UI's attack mode (`GetEnabledFullAttack`), the move action's realtime
//! timestamp, the animation handles (`ConfigureAnimations`' list: the order a sequence leaves,
//! each handle's presence, `IsSequential`, the skip test; a handle's `IsFinished` and
//! `IsReleased`; the weapon's animation style) and the group memory's enemies with their order by
//! distance.

use super::{
    Inputs, Of, R, Target, cmd, cmd_mut, current_turn, desync, executor, ignores_cooldown,
    interrupt, res, turn_based, ty, unsupported,
};
use crate::combat::counts::AttacksCount;
use crate::combat::weapon::{self, Hand};
use crate::unit_state::{cond, has_condition};
use crate::world::{EntityIdx, World};

/// `MechanicsFeatureType.Pounce`, `HasRend`.
const POUNCE: i64 = 11;
const HAS_REND: i64 = 46;
/// `WeaponAnimationStyle.Bow`.
const BOW: i64 = 9;
/// `CombatController.MaxDelayBetweenIterativeAttacks` (a field initialised to 1, never set).
const MAX_DELAY_BETWEEN_ITERATIVE_ATTACKS: f32 = 1.0;

/// An attack's slot: a hand of the current set or an additional limb (by index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Primary,
    Secondary,
    Limb(usize),
}

/// An `AttackHandInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attack {
    pub slot: Slot,
    pub penalty: i32,
    /// The slot's item reference as `InitAttacks` recorded it (a limb's: the game's id).
    pub item: String,
    /// What `ConfigureAnimations` left (inputs): a handle, `IsSequential`, and `OnAction`'s skip
    /// test (`handle == null || Variant < 0 && AnimationClipWrapper == null`).
    pub handle: bool,
    pub seq: bool,
    pub skip: bool,
    /// `IsHit` (its rule's roll).
    pub is_hit: bool,
}

/// The flags its creator set: `IsSingleAttack`, `ForceFullAttack`, `IsCharge`, `OnlyForInspect`,
/// an `OverrideAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub single: bool,
    pub force_full: bool,
    pub charge: bool,
    pub inspect: bool,
    pub override_action: bool,
}

impl Flags {
    /// WrathOracle's five 0/1 characters.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let [single, force_full, charge, inspect, override_action] = super::flags::<5>(s)?;
        Some(Self { single, force_full, charge, inspect, override_action })
    }
}

/// A `UnitAttack`'s own state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub flags: Flags,
    /// `m_AllAttacks` (`None` before `InitAttacks`).
    pub list: Option<Vec<Attack>>,
    /// The entries whose handle `OnTick` read released (`IsReleased` only becomes true: once read so,
    /// so in every later read, until `InitAttacks` builds another list).
    pub released: Vec<usize>,
    /// `m_AttackIndex`.
    pub index: usize,
    /// `m_HasStartedAttack`.
    pub started_attack: bool,
    /// `m_ClearAnimationOnFinish`.
    pub clear_on_finish: bool,
    /// `m_AnimationsDuration`.
    pub duration: f32,
    /// `IsAttackFull`.
    pub full: bool,
    /// `m_IsUnconsciousAtStartOfCommand`.
    pub unconscious_at_start: bool,
    /// `LastAttackRule.AttackNumber`.
    pub last_number: Option<i32>,
    /// `UpdateTarget` assigned `Target` (a new `TargetWrapper`, whose recorded id the next check
    /// gives).
    pub new_wrapper: bool,
    /// The current handle has been seen finished (`IsFinished` only becomes true, and the handle
    /// stays until the class code starts another or clears it: every later row has it finished).
    pub finished_handle: bool,
    /// The current handle has been seen acted (`IsActed` only becomes true, like `IsFinished`).
    pub acted_handle: bool,
    /// `OnTick`'s read of the handle it kept, when nothing after the read could change it (no
    /// interrupt): the row after `OnTick` has it so.
    pub finished_read: Option<bool>,
}

/// The class state as WrathOracle's `ua` row has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: usize,
    pub started_attack: bool,
    pub clear_on_finish: bool,
    pub full: bool,
    pub unconscious_at_start: bool,
    pub duration: f32,
    pub last_number: Option<i32>,
    pub len: Option<usize>,
}

impl Row {
    /// `index|hasStartedAttack clearOnFinish IsAttackFull unconsciousAtStart|duration|last|len`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('|').collect();
        let [index, f, duration, last, len] = p[..] else { return None };
        let [started_attack, clear_on_finish, full, unconscious_at_start] = super::flags::<4>(f)?;
        let opt = |x: &str| -> Option<Option<i64>> {
            if x == "-" { Some(None) } else { x.parse().ok().map(Some) }
        };
        Some(Self {
            index: index.parse().ok()?,
            started_attack,
            clear_on_finish,
            full,
            unconscious_at_start,
            duration: super::finite(duration)?,
            last_number: opt(last)?.map(i32::try_from).transpose().ok()?,
            len: opt(len)?.map(usize::try_from).transpose().ok()?,
        })
    }

    #[must_use]
    pub fn of(s: &State) -> Self {
        Self {
            index: s.index,
            started_attack: s.started_attack,
            clear_on_finish: s.clear_on_finish,
            full: s.full,
            unconscious_at_start: s.unconscious_at_start,
            duration: s.duration,
            last_number: s.last_number,
            len: s.list.as_ref().map(Vec::len),
        }
    }

    pub(super) fn same(&self, o: &Self) -> bool {
        self.duration.to_bits() == o.duration.to_bits() && self == o
    }
}

/// One attack of `ua_atk`'s list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `None`: a slot that is neither hand nor a limb.
    pub slot: Option<Slot>,
    /// The slot's fields: its item reference's id (empty without one), `m_Active`, `m_IsItemFake`.
    pub item: String,
    pub active: bool,
    pub fake: bool,
    pub penalty: i32,
    pub handle: bool,
    pub seq: bool,
    pub skip: bool,
}

impl Entry {
    /// `slot,item,active,fake,penalty,flags`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split(',').collect();
        let [slot, item, active, fake, penalty, f] = p[..] else { return None };
        let [active] = super::flags::<1>(active)?;
        let [fake] = super::flags::<1>(fake)?;
        let slot = match slot {
            "p" => Some(Slot::Primary),
            "s" => Some(Slot::Secondary),
            "?" => None,
            l => Some(Slot::Limb(l.strip_prefix('l')?.parse().ok()?)),
        };
        let [handle, seq, skip] = super::flags::<3>(f)?;
        // Without a handle the skip test holds, and no sequence takes it.
        if !handle && (!skip || seq) {
            return None;
        }
        Some(Self {
            slot,
            item: item.to_owned(),
            active,
            fake,
            penalty: penalty.parse().ok()?,
            handle,
            seq,
            skip,
        })
    }
}

/// `ua_atk`: what `InitAttacks` left.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub flags: Flags,
    pub full: bool,
    /// `ConfigureAnimations` ran (the view has an animation manager).
    pub configured: bool,
    pub duration: f32,
    /// `None`: `m_AllAttacks` is null.
    pub attacks: Option<Vec<Entry>>,
}

/// The fields `TriggerAttackRule` gives its `RuleAttackWithWeapon`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    /// `attack.Weapon` (the slot's weapon now).
    pub weapon: EntityIdx,
    pub slot: Slot,
    /// The slot's item reference `InitAttacks` recorded.
    pub item: String,
    pub penalty: i32,
    pub is_rend: bool,
    pub is_first_attack: bool,
    pub is_full_attack: bool,
    pub is_charge: bool,
    pub attack_number: i32,
    pub attacks_count: i32,
    /// An attack of opportunity's (`IsAttackOfOpportunity`, with its `ForceFlatFooted`): no
    /// `WeaponSlot`, triggered by `UnitAttackOfOpportunity.OnAction`.
    pub opportunity: Option<bool>,
}

/// What the attack-count rules answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Counts {
    /// `RuleCalculateAttacksCount`: the primary and secondary hands.
    Hands(AttacksCount, AttacksCount),
    /// `RuleCalculateAttacksCountLimbs`: per counted limb, in the body's order.
    Limbs(Vec<AttacksCount>),
}

/// Whether the sim runs this command's class code.
#[must_use]
pub fn ported(w: &World, c: &super::Cmd) -> bool {
    w.combat.commands.attacks && c.class == super::class::ATTACK
}

fn st(w: &World, c: u32) -> R<&State> {
    match &cmd(w, c)?.ua {
        Some(s) => Ok(s),
        None => desync(format!("command {c} has no UnitAttack state")),
    }
}

fn st_mut(w: &mut World, c: u32) -> R<&mut State> {
    match &mut cmd_mut(w, c)?.ua {
        Some(s) => Ok(s),
        None => desync(format!("command {c} has no UnitAttack state")),
    }
}

fn list(w: &World, c: u32) -> R<&Vec<Attack>> {
    match &st(w, c)?.list {
        Some(l) => Ok(l),
        // `m_AllAttacks.Count` on null.
        None => desync(format!("command {c}'s attack list is null")),
    }
}

/// `PlannedAttack`.
fn planned(w: &World, c: u32) -> R<Option<Attack>> {
    let s = st(w, c)?;
    let l = list(w, c)?;
    Ok(if !l.is_empty() && s.index < l.len() { Some(l[s.index].clone()) } else { None })
}

/// `Target` (`base.Target?.Unit`): the target unit, which must resolve.
fn target(w: &World, c: u32) -> R<EntityIdx> {
    let x = cmd(w, c)?;
    match x.s.target.unit().and_then(|id| w.by_id.get(id).copied()).filter(|e| w.unit(*e).is_some())
    {
        Some(t) => Ok(t),
        None => desync(format!(
            "command {c}'s target doesn't resolve to a unit (Target.Descriptor on null)"
        )),
    }
}

fn set_target(w: &mut World, c: u32, t: EntityIdx) -> R<()> {
    let id = w.entity(t).unique_id.clone();
    let x = cmd_mut(w, c)?;
    x.s.target = Target::Unit(id);
    x.s.target_ref = None;
    if let Some(s) = &mut x.ua {
        s.new_wrapper = true;
    }
    Ok(())
}

fn uid(w: &World, u: EntityIdx) -> Option<String> {
    Some(w.entity(u).unique_id.clone())
}

fn is_unconscious(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.life_state == 1)
}

fn has_part(w: &World, u: EntityIdx, clr: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == clr)
}

/// A slot's `MaybeWeapon`, and `HasWeapon` (a hand: a weapon; a limb: an item and a weapon).
pub(super) fn slot_weapon(w: &mut World, u: EntityIdx, s: Slot) -> (Option<EntityIdx>, bool) {
    match s {
        Slot::Primary | Slot::Secondary => {
            let h = if s == Slot::Primary { Hand::Primary } else { Hand::Secondary };
            let wp = weapon::hand_weapon(w, u, h);
            (wp, wp.is_some())
        }
        Slot::Limb(i) => {
            let wp = weapon::limb_weapons(w, u).get(i).copied().flatten();
            let item = w.unit(u).and_then(|x| x.body.limbs.get(i)).and_then(|l| l.item);
            (wp, wp.is_some() && item.is_some())
        }
    }
}

// ---- Init and the attack list ----

/// `Init(executor)`: `base.Init`, `m_AnimationsDuration = 6`, `InitAttacks()`, the target's
/// unconsciousness (`ApproachRadius` only feeds `IsUnitCloseEnough`, an input).
pub fn init<I: Inputs>(w: &mut World, inp: &mut I, c: u32, owner: Option<String>) -> R<()> {
    cmd_mut(w, c)?.s.executor = owner;
    st_mut(w, c)?.duration = 6.0;
    init_attacks(w, inp, c)?;
    let t = target(w, c)?;
    st_mut(w, c)?.unconscious_at_start = is_unconscious(w, t);
    Ok(())
}

/// `IsFullAttackRestrictedBecauseOfMoveAction`: computed in turn-based combat but for the UI's
/// attack mode (`efa`); outside it, `m_LastUsageOfMoveActionTime` (realtime) decides, and the
/// recorded value (`farm`) is taken.
fn full_attack_restricted_by_move<I: Inputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<bool> {
    let id = uid(w, u);
    let computed = if turn_based(w) && super::super::is_in_combat(w, u) {
        Some(if crate::combat::cooldowns::used_one_move_action(w, u) {
            true
        } else if is_current_unit(w, u)? {
            // `TBM.EnabledFullAttack`: `CurrentTurn?.GetEnabledFullAttack(unit) ?? false`.
            !inp.leaf(w, "efa", Of::Unit(id.clone()))?
        } else {
            false
        })
    } else {
        None
    };
    let v = inp.leaf(w, "farm", Of::Unit(id))?;
    if computed.is_some_and(|x| x != v) {
        return desync(format!(
            "IsFullAttackRestrictedBecauseOfMoveAction: the sim's {computed:?}, the recording's {v}"
        ));
    }
    Ok(v)
}

/// `UnitEntityData.IsCurrentUnit()` (`CombatController.Rider`; mounts aren't modelled).
fn is_current_unit(w: &mut World, u: EntityIdx) -> R<bool> {
    let turn = current_turn(w);
    if [Some(u), turn]
        .into_iter()
        .flatten()
        .any(|x| has_part(w, x, "UnitPartRider") || has_part(w, x, "UnitPartSaddled"))
    {
        return unsupported(
            w,
            "UnitAttack: IsCurrentUnit of a mounted unit (CombatController.Mount)",
        );
    }
    Ok(turn == Some(u))
}

/// `InitAttacks()`.
fn init_attacks<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    let u = executor(w, c)?;
    let f = st(w, c)?.flags;
    if f.charge {
        return unsupported(w, "UnitAttack: a charge (pounce, UnitPartChargeWeapon)");
    }
    if f.inspect {
        return unsupported(w, "UnitAttack: OnlyForInspect run through the lifecycle");
    }
    // `(ForceFullAttack | (IsCharge && Pounce && ...)) || (!IsSingleAttack &&
    // !IsFullAttackRestricted(IsFullAttackRestrictedBecauseOfMoveAction) && !Staggered && (!IsCharge
    // || Pounce))`: the argument is evaluated before the call.
    let full = f.force_full || {
        !f.single && {
            let arg = full_attack_restricted_by_move(w, inp, u)?;
            !is_full_attack_restricted(w, c, u, arg)? && !has_condition(w, u, cond::STAGGERED)
        }
    };
    let _ = POUNCE;
    let attacks =
        if full { create_full_attack(w, inp, c, u)? } else { create_single_attack(w, inp, c, u)? };
    let rec = inp.attack_list(w, c)?;
    if rec.flags != f {
        return desync(format!(
            "command {c}'s flags changed before InitAttacks ({f:?}, recorded {:?})",
            rec.flags
        ));
    }
    if rec.full != full {
        return desync(format!(
            "command {c}'s IsAttackFull: the sim's {full}, the recording's {}",
            rec.full
        ));
    }
    let Some(entries) = rec.attacks else {
        return desync(format!(
            "command {c}: InitAttacks left a null list (the sim has {attacks:?})"
        ));
    };
    // (`m_AllAttacks.Empty()`: null or no attack.)
    let Some(attacks) = attacks.filter(|a| !a.is_empty()) else {
        return unsupported(
            w,
            "UnitAttack.InitAttacks: no attack (\"Executor can't make any attack\")",
        );
    };
    // `ConfigureAnimations` may reorder the list (a sequence): the recorded list must be the sim's,
    // in some order.
    if entries.len() != attacks.len() {
        return desync(format!(
            "command {c}'s attacks: the sim's {attacks:?}, the recording's {entries:?}"
        ));
    }
    // Only a sequence reorders the list: `ConfigureAnimations` adopts its working copy when a
    // sequence's `TryToReorder` succeeds, marking that sequence's attacks, the list's first,
    // sequential (the copy may hold swaps of sequences that failed before: any order, then). Without
    // one the list keeps the sim's order.
    let k = entries.iter().take_while(|e| e.seq).count();
    if entries.iter().skip(k).any(|e| e.seq)
        || (k > 0 && !rec.configured)
        || (k == 0
            && entries
                .iter()
                .map(|e| e.slot.map(|s| (s, e.penalty)))
                .ne(attacks.iter().copied().map(Some)))
    {
        return desync(format!(
            "command {c}'s attacks: the sim's {attacks:?} (a sequence of {k}), the recording's {entries:?}"
        ));
    }
    let mut left: Vec<Option<(Slot, i32)>> = attacks.iter().copied().map(Some).collect();
    let mut out = Vec::with_capacity(entries.len());
    for e in &entries {
        let Some(slot) = e.slot else {
            return desync(format!(
                "command {c}: an attack of a slot the sim doesn't know ({e:?})"
            ));
        };
        let Some(k) = left.iter().position(|x| *x == Some((slot, e.penalty))) else {
            return desync(format!(
                "command {c}'s attacks: the sim's {attacks:?}, the recording's {entries:?}"
            ));
        };
        left[k] = None;
        if !slot_matches(w, u, slot, e) {
            return desync(format!("command {c}: the slot of attack {e:?} differs from the sim's"));
        }
        out.push(Attack {
            slot,
            penalty: e.penalty,
            item: e.item.clone(),
            handle: e.handle,
            seq: e.seq,
            skip: e.skip,
            is_hit: false,
        });
    }
    // `m_AnimationsDuration`: `ConfigureAnimations` (when it ran: the view has an animation manager,
    // an input) sets it from the standard cooldown for a command ignoring its cooldown. Without it
    // no attack has a handle.
    if !rec.configured && out.iter().any(|a| a.handle || a.seq) {
        return desync(format!("command {c}: animation handles without ConfigureAnimations"));
    }
    let x = cmd(w, c)?.clone();
    let expected = if rec.configured && ignores_cooldown(w, &x) && x.s.ty == ty::STANDARD {
        3.0_f32.max(w.unit(u).map_or(0.0, |x| x.combat.cooldown.standard_action))
    } else {
        st(w, c)?.duration
    };
    if expected.to_bits() != rec.duration.to_bits() {
        return desync(format!(
            "command {c}'s m_AnimationsDuration: the sim's {expected}, the recording's {}",
            rec.duration
        ));
    }
    let s = st_mut(w, c)?;
    s.full = full;
    s.duration = rec.duration;
    s.list = Some(out);
    s.released.clear();
    Ok(())
}

/// The recorded slot is the sim's: the same item (a limb's items get fresh ids at load: an item,
/// then) and active flag, not a fake item.
fn slot_matches(w: &World, u: EntityIdx, slot: Slot, e: &Entry) -> bool {
    let x = SlotFields { slot: e.slot, item: e.item.clone(), active: e.active, fake: e.fake };
    slot_holds(w, u, slot, &x)
}

/// A slot as WrathOracle writes it: `slot,item,active,fake` (the slot `p`, `s`, `l<i>` or `?`;
/// its item reference's id, empty without one; `m_Active`; `m_IsItemFake`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotFields {
    pub slot: Option<Slot>,
    pub item: String,
    pub active: bool,
    pub fake: bool,
}

impl SlotFields {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split(',').collect();
        let [slot, item, active, fake] = p[..] else { return None };
        let [active] = super::flags::<1>(active)?;
        let [fake] = super::flags::<1>(fake)?;
        let slot = match slot {
            "p" => Some(Slot::Primary),
            "s" => Some(Slot::Secondary),
            "?" => None,
            l => Some(Slot::Limb(l.strip_prefix('l')?.parse().ok()?)),
        };
        Some(Self { slot, item: item.to_owned(), active, fake })
    }
}

/// The recorded slot fields are the sim's slot's (see [`slot_matches`]).
#[must_use]
pub fn slot_holds(w: &World, u: EntityIdx, slot: Slot, e: &SlotFields) -> bool {
    let Some((item, active)) = slot_fields(w, u, slot) else { return false };
    let item_ok = match (slot, item) {
        (Slot::Limb(_), i) => i.is_some() != e.item.is_empty(),
        (_, None) => e.item.is_empty(),
        (_, Some(i)) => w.entity(i).unique_id == e.item,
    };
    item_ok && active == e.active && !e.fake
}

/// A slot's item and `m_Active` (`None`: the sim has no such slot).
#[must_use]
pub fn slot_fields(w: &World, u: EntityIdx, slot: Slot) -> Option<(Option<EntityIdx>, bool)> {
    let body = &w.unit(u)?.body;
    match slot {
        Slot::Limb(i) => body.limbs.get(i).map(|l| (l.item, l.active)),
        Slot::Primary | Slot::Secondary => {
            let n = if slot == Slot::Primary { "PrimaryHand" } else { "SecondaryHand" };
            let name = format!("HandsEquipmentSets[{}].{n}", body.current_hand_set);
            body.slots.iter().find(|s| s.name == name).map(|s| (s.item, s.active))
        }
    }
}

/// `IsFullAttackRestricted(isFullAttackRestrictedBecauseOfMoveAction)`.
fn is_full_attack_restricted(w: &mut World, c: u32, u: EntityIdx, arg: bool) -> R<bool> {
    if turn_based(w) && super::super::is_in_combat(w, u) {
        if crate::combat::cooldowns::is_move_action_restricted(w, u) {
            return Ok(true);
        }
        let x = cmd(w, c)?.clone();
        if ignores_cooldown(w, &x) {
            return Ok(false);
        }
    }
    Ok(arg)
}

/// `EnumerateAttacks(AttacksCount)`: the main attacks, then the penalized ones (`5 * j`, `4 * j`
/// with flawless attacks).
fn penalties(a: AttacksCount) -> Vec<i32> {
    let main = (a.additional + a.haste).max(0);
    let mut v = vec![0; usize::try_from(main).unwrap_or(0)];
    let step = if a.flawless { 4 } else { 5 };
    v.extend((1..=a.penalized.max(0)).map(|j| step * j));
    v
}

/// The counted limbs (the order `RuleCalculateAttacksCountLimbs` takes them): a limb with an item
/// and a weapon.
#[must_use]
pub fn counted_limbs(w: &World, u: EntityIdx) -> Vec<usize> {
    let items: Vec<bool> = w
        .unit(u)
        .map(|x| x.body.limbs.iter().map(|l| l.item.is_some()).collect())
        .unwrap_or_default();
    weapon::limb_weapons(w, u)
        .into_iter()
        .enumerate()
        .filter(|(i, wp)| wp.is_some() && items.get(*i).copied().unwrap_or(false))
        .map(|(i, _)| i)
        .collect()
}

/// `CreateFullAttack()` (`EnumerateFullAttack`: the hands interleaved, then the limbs; a charge's
/// weapon first isn't modelled).
fn create_full_attack<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    c: u32,
    u: EntityIdx,
) -> R<Option<Vec<(Slot, i32)>>> {
    let Counts::Hands(p, s) = inp.attacks_count(w, c, u, false)? else {
        return desync("RuleCalculateAttacksCount answered as the limbs' rule");
    };
    let (mut pi, mut si) = (penalties(p).into_iter(), penalties(s).into_iter());
    let mut out = Vec::new();
    loop {
        let mut any = false;
        if let Some(x) = pi.next() {
            out.push((Slot::Primary, x));
            any = true;
        }
        if let Some(x) = si.next() {
            out.push((Slot::Secondary, x));
            any = true;
        }
        if !any {
            break;
        }
    }
    let limbs = counted_limbs(w, u);
    let Counts::Limbs(counts) = inp.attacks_count(w, c, u, true)? else {
        return desync("RuleCalculateAttacksCountLimbs answered as the hands' rule");
    };
    if counts.len() != limbs.len() {
        return desync(format!(
            "RuleCalculateAttacksCountLimbs: {} slots, the sim counts {limbs:?}",
            counts.len()
        ));
    }
    for (i, n) in limbs.into_iter().zip(counts) {
        out.extend(penalties(n).into_iter().map(|x| (Slot::Limb(i), x)));
    }
    // `new AttackHandInfo(hand, penalty)` throws for a slot without a weapon.
    for (slot, _) in &out {
        if !slot_weapon(w, u, *slot).1 {
            return unsupported(
                w,
                "UnitAttack.CreateFullAttack: an attack of a slot without a weapon",
            );
        }
    }
    Ok(Some(out))
}

/// `CreateSingleAttack()`: the primary hand, else the secondary, by the counts' main attacks; else
/// the first limb with a weapon.
fn create_single_attack<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    c: u32,
    u: EntityIdx,
) -> R<Option<Vec<(Slot, i32)>>> {
    let Counts::Hands(p, s) = inp.attacks_count(w, c, u, false)? else {
        return desync("RuleCalculateAttacksCount answered as the limbs' rule");
    };
    let can = |w: &mut World, slot: Slot| -> bool {
        if !slot_weapon(w, u, slot).1 {
            return false;
        }
        match slot {
            Slot::Primary => p.additional + p.haste > 0,
            Slot::Secondary => s.additional + s.haste > 0,
            Slot::Limb(_) => true,
        }
    };
    if can(w, Slot::Primary) {
        return Ok(Some(vec![(Slot::Primary, 0)]));
    }
    if can(w, Slot::Secondary) {
        return Ok(Some(vec![(Slot::Secondary, 0)]));
    }
    let n = w.unit(u).map_or(0, |x| x.body.limbs.len());
    for i in 0..n {
        if can(w, Slot::Limb(i)) {
            return Ok(Some(vec![(Slot::Limb(i), 0)]));
        }
    }
    Ok(None)
}

// ---- start, tick, act ----

/// `OnStart()`: `base.OnStart()` (no `TriggerAnimation`), `InitAttacks()`, the first attack.
pub fn on_start<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    init_attacks(w, inp, c)?;
    if !try_start_next_attack(w, inp, c, true)? {
        interrupt(w, inp, c, true)?;
    }
    st_mut(w, c)?.started_attack = true;
    Ok(())
}

/// `TryStartNextAttack(first)`.
fn try_start_next_attack<I: Inputs>(w: &mut World, inp: &mut I, c: u32, first: bool) -> R<bool> {
    let u = executor(w, c)?;
    if cmd(w, c)?.s.ty == ty::STANDARD && has_condition(w, u, cond::CANT_USE_STANDARD_ACTIONS) {
        return Ok(false);
    }
    if !first {
        if planned(w, c)?.is_none() {
            return Ok(false);
        }
        if !update_target(w, inp, c)? {
            return Ok(false);
        }
    }
    let Some(p) = planned(w, c)? else { return Ok(false) };
    // `IsValid`: `Hand.HasWeapon`.
    if !slot_weapon(w, u, p.slot).1 {
        return Ok(false);
    }
    let x = cmd_mut(w, c)?;
    if p.handle && !x.s.skip_animation {
        // The handle's rend, charge and distance are presentation; `StartAnimation(handle)`.
        x.s.animation = true;
        x.s.has_animation = true;
        x.s.act_handled = false;
    } else {
        // `ClearAnimation()`, `ScheduleAct()`.
        x.s.animation = false;
        x.s.act_handled = false;
        x.s.pretend_act_time = x.s.time_since_start + 1.0;
    }
    // A new handle's progress comes with the next check.
    x.s.anim_acted = None;
    x.s.anim_finished = None;
    let s = st_mut(w, c)?;
    s.clear_on_finish = false;
    s.finished_handle = false;
    s.acted_handle = false;
    Ok(true)
}

/// `ModifyDelay(delay)`.
fn modify_delay(w: &World, delay: f32) -> f32 {
    if turn_based(w) { delay.min(MAX_DELAY_BETWEEN_ITERATIVE_ATTACKS) } else { delay }
}

/// `OnTick()`.
pub fn on_tick<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    let s = st(w, c)?.clone();
    let l = list(w, c)?.clone();
    let since = cmd(w, c)?.s.time_since_start;
    #[allow(clippy::cast_precision_loss)]
    let mut num = modify_delay(w, s.duration / l.len() as f32) * s.index as f32;
    if let Some(p) = planned(w, c)? {
        // `plannedAttack.Weapon?.GetAnimationStyle()` (`Weapon` throws without one).
        let u = executor(w, c)?;
        if slot_weapon(w, u, p.slot).0.is_none() {
            return unsupported(w, "UnitAttack.OnTick: the planned attack's slot has no weapon");
        }
        if inp.leaf_int(w, "style", c)? == BOW {
            num = since;
        }
    }
    if s.index >= 1 && s.index < l.len() && l[s.index].seq && l[s.index - 1].seq {
        let prev = s.index - 1;
        let released = inp.leaf(w, "arel", Of::Cmd(c))?;
        if !released && s.released.contains(&prev) {
            return desync(format!(
                "command {c}'s attack {prev}'s handle, read released, unreleased in a later OnTick"
            ));
        }
        if released {
            let st = st_mut(w, c)?;
            if !st.released.contains(&prev) {
                st.released.push(prev);
            }
            num = since;
        }
    }
    if !s.started_attack && since >= num {
        st_mut(w, c)?.started_attack = true;
        if !try_start_next_attack(w, inp, c, false)? {
            interrupt(w, inp, c, true)?;
        }
    }
    if cmd(w, c)?.s.animation {
        let finished = inp.leaf(w, "afin", Of::Cmd(c))?;
        if finished && st(w, c)?.clear_on_finish {
            // `ClearAnimation()`.
            let x = cmd_mut(w, c)?;
            x.s.animation = false;
            x.s.act_handled = false;
            x.s.anim_acted = None;
            x.s.anim_finished = None;
            let s = st_mut(w, c)?;
            s.finished_handle = false;
            s.acted_handle = false;
        } else {
            let s = st_mut(w, c)?;
            s.finished_handle |= finished;
            s.finished_read = Some(finished);
        }
    }
    if planned(w, c)?.is_some() && !update_target(w, inp, c)? {
        // (An interrupt releases the handle: its state after it isn't the read's.)
        st_mut(w, c)?.finished_read = None;
        interrupt(w, inp, c, true)?;
    }
    Ok(())
}

/// `IsUnitInFiveFeetRange(unit)`.
fn five<I: Inputs>(w: &mut World, inp: &mut I, c: u32, u: EntityIdx) -> R<bool> {
    let id = uid(w, u);
    inp.leaf(w, "five", Of::CmdUnit(c, id))
}

/// `UpdateTarget()`.
fn update_target<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<bool> {
    if planned(w, c)?.is_none() {
        return Ok(false);
    }
    let original = target(w, c)?;
    let tb = turn_based(w);
    let unc_start = st(w, c)?.unconscious_at_start;
    let mut retarget = crate::combat::units::is_dead(w, original)
        || (is_unconscious(w, original) && !unc_start)
        || !inp.leaf(w, "close", Of::Cmd(c))?;
    if tb {
        retarget |= !five(w, inp, c, original)?;
    }
    if !retarget {
        return Ok(true);
    }
    let u = executor(w, c)?;
    // `Executor.Group.Memory.Enemies.Select(Unit).NotNull().Where(!Sleeping).Where(five).ToList()`,
    // sorted by distance (the recorded order).
    let enemies = inp.leaf_ids(w, "enemies", c)?;
    // (The memory keys its units by id: each once.)
    let mut ids: Vec<&Option<String>> = enemies.iter().collect();
    ids.sort();
    if ids.windows(2).any(|p| p[0] == p[1]) {
        return desync(format!("command {c}'s recorded enemies list a unit twice: {enemies:?}"));
    }
    let mut near = Vec::new();
    for e in enemies {
        let Some(id) = e else {
            return unsupported(w, "UnitAttack.UpdateTarget: a null entry in the memory's enemies");
        };
        // `NotNull` drops a reference that doesn't resolve; the sim can't tell that from a unit it
        // lacks: reported.
        let Some(x) = w.by_id.get(&id).copied().filter(|x| w.unit(*x).is_some()) else {
            return unsupported(
                w,
                "UnitAttack.UpdateTarget: an enemy in the memory the sim doesn't have",
            );
        };
        if has_condition(w, x, crate::combat::units::condition::SLEEPING) {
            continue;
        }
        if five(w, inp, c, x)? {
            near.push(id);
        }
    }
    let sorted = inp.leaf_ids(w, "sorted", c)?;
    let mut a: Vec<Option<String>> = near.iter().cloned().map(Some).collect();
    let mut b = sorted.clone();
    a.sort();
    b.sort();
    if a != b {
        return desync(format!(
            "UpdateTarget: the sim's candidates {near:?}, the recording sorts {sorted:?}"
        ));
    }
    let mut found = false;
    for id in sorted.into_iter().flatten() {
        let Some(x) = w.by_id.get(&id).copied() else { continue };
        if crate::combat::units::is_conscious(w, x)
            && !inp.leaf(w, "untgt", Of::Pair(uid(w, u), Some(id.clone())))?
        {
            set_target(w, c, x)?;
            if inp.leaf(w, "close", Of::Cmd(c))? || (!tb && five(w, inp, c, x)?) {
                found = true;
                break;
            }
        }
    }
    if !found && is_unconscious(w, original) {
        set_target(w, c, original)?;
        found = inp.leaf(w, "close", Of::Cmd(c))? || (!tb && five(w, inp, c, original)?);
    }
    if !found {
        set_target(w, c, original)?;
        if !is_unconscious(w, original)
            || (!inp.leaf(w, "close", Of::Cmd(c))? && (tb || !five(w, inp, c, original)?))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `OnAction()`: the attack, and the following ones without an animation; `Success` once every
/// attack is made.
pub fn on_action<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<u8> {
    let u = executor(w, c)?;
    // `MakeSaveCheckForAttack`.
    if has_part(w, u, "UnitPartSaveThrowForAttackOrSpell") {
        return unsupported(
            w,
            "UnitAttack: a save for attacks (UnitPartSaveThrowForAttackOrSpell)",
        );
    }
    provide_next_attack(w, inp, c)?;
    loop {
        let s = st(w, c)?;
        let l = list(w, c)?;
        if !(s.index < l.len() && l[s.index].skip) {
            break;
        }
        // `m_AllAttacks[m_AttackIndex].AnimationHandle?.Release()`: a skipped entry's handle.
        let before = s.index;
        if l[before].handle {
            let st = st_mut(w, c)?;
            if !st.released.contains(&before) {
                st.released.push(before);
            }
        }
        // A dead target makes no attack and doesn't advance: the game loops forever.
        provide_next_attack(w, inp, c)?;
        if st(w, c)?.index == before {
            return unsupported(
                w,
                "UnitAttack.OnAction: skipped attacks on a dead target (an endless loop)",
            );
        }
    }
    let n = list(w, c)?.len();
    let s = st_mut(w, c)?;
    s.started_attack = false;
    s.clear_on_finish = s.index < n;
    Ok(if s.clear_on_finish { res::NONE } else { res::SUCCESS })
}

/// `ProvideNextAttack()`.
fn provide_next_attack<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    let t = target(w, c)?;
    if crate::combat::units::is_dead(w, t) {
        return Ok(());
    }
    if st(w, c)?.flags.override_action {
        return unsupported(w, "UnitAttack: an OverrideAction");
    }
    let Some(p) = planned(w, c)? else {
        // `TriggerAttackRule(null)`.
        return desync(format!("command {c}: an attack without a planned attack"));
    };
    trigger_attack_rule(w, inp, c, t, &p)?;
    st_mut(w, c)?.index += 1;
    Ok(())
}

/// `IsRend(attack)`.
fn is_rend(w: &mut World, c: u32, u: EntityIdx, a: &Attack) -> R<bool> {
    if !weapon::has_feature(w, u, HAS_REND) || a.slot != Slot::Secondary {
        return Ok(false);
    }
    let Some(wp) = slot_weapon(w, u, a.slot).0 else { return Ok(false) };
    if weapon::Weapon::of(w, wp).is_none_or(|x| x.is_ranged()) {
        return Ok(false);
    }
    let s = st(w, c)?;
    let l = list(w, c)?;
    // `LastExecutedAttack`.
    let last = (s.index >= 1 && s.index <= l.len()).then(|| &l[s.index - 1]);
    Ok(last.is_some_and(|x| x.is_hit && x.slot == Slot::Primary))
}

/// `TriggerAttackRule(attack)`.
fn trigger_attack_rule<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    c: u32,
    t: EntityIdx,
    a: &Attack,
) -> R<()> {
    let u = executor(w, c)?;
    let Some(weapon) = slot_weapon(w, u, a.slot).0 else {
        return unsupported(w, "UnitAttack.TriggerAttackRule: the attack's slot has no weapon");
    };
    let rend = is_rend(w, c, u, a)?;
    let s = st(w, c)?.clone();
    let n = list(w, c)?.len();
    let spec = Spec {
        initiator: u,
        target: t,
        weapon,
        slot: a.slot,
        item: a.item.clone(),
        penalty: a.penalty,
        is_rend: rend,
        is_first_attack: s.last_number.is_none(),
        is_full_attack: s.full,
        is_charge: s.flags.charge,
        attack_number: s.last_number.map_or(0, |x| x + 1),
        attacks_count: i32::try_from(n).unwrap_or(i32::MAX),
        opportunity: None,
    };
    let hit = inp.attack_rule(w, c, &spec)?;
    let s = st_mut(w, c)?;
    s.last_number = Some(spec.attack_number);
    let i = s.index;
    if let Some(x) = s.list.as_mut().and_then(|l| l.get_mut(i)) {
        x.is_hit = hit;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn penalties_follow_the_counts() {
        let a = AttacksCount { additional: 1, penalized: 2, haste: 1, ..AttacksCount::default() };
        assert_eq!(penalties(a), vec![0, 0, 5, 10]);
        let f =
            AttacksCount { additional: 1, penalized: 2, flawless: true, ..AttacksCount::default() };
        assert_eq!(penalties(f), vec![0, 4, 8]);
    }

    #[test]
    fn rows_and_entries_parse() {
        let r = Row::parse("1|1010|6|0|2").expect("row");
        assert_eq!(r.index, 1);
        assert!(r.started_attack && !r.clear_on_finish && r.full && !r.unconscious_at_start);
        assert_eq!(r.last_number, Some(0));
        assert_eq!(r.len, Some(2));
        assert_eq!(Row::parse("0|0000|6|-|-").expect("row").last_number, None);
        assert!(Row::parse("0|000|6|-|-").is_none());
        assert!(Row::parse("0|0000|NaN|-|-").is_none());
        let e = Entry::parse("l2,abc,1,0,5,101").expect("entry");
        assert_eq!(e.slot, Some(Slot::Limb(2)));
        assert!(e.active && !e.fake);
        assert_eq!((e.penalty, e.handle, e.seq, e.skip), (5, true, false, true));
        assert!(Entry::parse("x,abc,1,0,5,101").is_none());
        assert!(Entry::parse("p,abc,1,0,5,10").is_none());
        assert_eq!(
            Flags::parse("10001").expect("flags"),
            Flags { single: true, override_action: true, ..Flags::default() }
        );
    }
}
