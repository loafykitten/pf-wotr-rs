//! `UnitEntityView.Corpulence` (decompiled, not copied: `Kingmaker.View/UnitEntityView.cs`,
//! `OverrideSizeScale.cs`, `Kingmaker.UnitLogic/{UnitProgressionData,UnitDescriptor,UnitState}.cs`,
//! `Kingmaker.UnitLogic.FactLogic/AddUnitScale.cs`, `Kingmaker.Blueprints.Root/
//! MountModelSizeSetting.cs`; docs/poc5/corpulence.md), computed from the unit's view prefab and
//! its size instead of taken from a recording's `corp` reads.
//!
//! `UnitEntityData.Corpulence` is `View ? View.Corpulence : 0.5f`, and the view's is the saddled
//! unit's `View.Corpulence` for a rider, else `Mathf.Max(m_Corpulence * GetSizeScale(), 0.5f)`
//! with `m_Corpulence` the prefab's serialized field (`wr-import unit-prefabs`).
//! `GetSizeScale()` is 1 when `DisableSizeScaling` (a polymorph's view, `RecreateView`'s), else a
//! per-view cache of `CalculateSizeScale()`, which reads `OriginalSize`, `State.Size`,
//! `Progression.CurrentScalePercent`, `m_RacialPetScaleCoeff` (`Start`: the master's race
//! coefficient of `BlueprintRoot.MountModelSizing`, 1 without a master) and the prefab's
//! `OverrideSizeScale`.
//!
//! The cache (`m_CachedSizeScale`) is cleared by `ResetSizeScale()` only: `UnitDescriptor.
//! UpdateSizeModifiers()` (every write of `State.Size`, `AddUnitScale`'s turning on and off),
//! `UnitProgressionData.SetRace`, and the view's attach, detach and `OnAreaDidLoad`. It is filled
//! at the next `GetSizeScale()` (the view's per-frame `Update` calls it), so a value is the
//! inputs' as of the first call after the last reset: an input changed without a reset leaves
//! it stale. The sim's inputs all change right before a reset (`set_state_size`, `AddUnitScale`,
//! `set_race`) or before the view exists; the cache is modelled the same, filled when first
//! asked, so a change without a reset would stay stale here too.
//!
//! What the sim can't establish is unknown, with a [`Cause`] counted by the replay: never a guess.

use std::collections::{HashMap, HashSet};

use crate::stats::types::Size;
use crate::world::{EntityIdx, World};

/// `Mathf.Max(.., 0.5f)` in `UnitEntityView.Corpulence`, and `UnitEntityData.Corpulence` without
/// a view.
pub const MIN_CORPULENCE: f32 = 0.5;

/// `UnitEntityView.ResultReduceSizeScaleStep` without an `OverrideSizeScale`.
pub const DEFAULT_SCALE_STEP: f32 = 0.66;

/// Why a unit's corpulence isn't known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
    /// No such unit in the sim (not made yet, or never).
    Unit,
    /// The unit rides: its view answers with the saddled unit's, which the sim doesn't track.
    Rider,
    /// A polymorph is active: the view is the polymorph's prefab, with `DisableSizeScaling`.
    Polymorph,
    /// The unit has a doll (`UnitPartDollData`) the sim didn't read from a save: its view is
    /// built from the doll's prefab, which one isn't known.
    Doll,
    /// The unit's view may be the optimization dummy (`UpdateHasDummyView`: unknown, or an area
    /// with parts, which suppresses units).
    DummyView,
    /// No prefab: the unit has no view prefab (the blueprint names none).
    NoPrefab,
    /// The prefab table (`cache/sim/unit-prefabs.json`, schema v2) or the doll prefabs
    /// (`cache/sim/doll-prefabs.json`) weren't built.
    NoTable,
    /// The prefab isn't in the table, or it has no single `m_Corpulence`.
    PrefabUnlisted,
    /// The prefab's `OverrideSizeScale` is unknown (several, or without its fields).
    Override,
    /// The unit's size, its original size or its part (`UnitPartSizeModifier` from a save) isn't known.
    Size,
    /// A pet whose master's race has a `MountModelSizing` coefficient other than 1 (`Start` reads
    /// it once; whether `Master` was set by then isn't known), or the table can't be read.
    PetCoefficient,
}

impl Cause {
    pub const ALL: [Cause; 11] = [
        Cause::Unit,
        Cause::Rider,
        Cause::Polymorph,
        Cause::Doll,
        Cause::DummyView,
        Cause::NoPrefab,
        Cause::NoTable,
        Cause::PrefabUnlisted,
        Cause::Override,
        Cause::Size,
        Cause::PetCoefficient,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Cause::Unit => "unit",
            Cause::Rider => "rider",
            Cause::Polymorph => "polymorph",
            Cause::Doll => "doll",
            Cause::DummyView => "dummy view",
            Cause::NoPrefab => "no prefab",
            Cause::NoTable => "no prefab table",
            Cause::PrefabUnlisted => "prefab unlisted",
            Cause::Override => "size scale override",
            Cause::Size => "size",
            Cause::PetCoefficient => "pet coefficient",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// `CalculateSizeScale`'s inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleInputs {
    /// `Descriptor.OriginalSize` and `Descriptor.State.Size`.
    pub original: Size,
    pub size: Size,
    /// `Progression.CurrentScalePercent`.
    pub percent: f32,
    /// `ResultReduceSizeScaleStep`.
    pub step: f32,
    /// The `OverrideSizeScale`'s `m_MaxScale`, if the view has one.
    pub max_scale: Option<f32>,
    /// `m_RacialPetScaleCoeff`.
    pub pet_coeff: f32,
}

/// `UnitEntityView.CalculateSizeScale(size)` for the unit's own state.
#[must_use]
pub fn calculate_size_scale(i: &ScaleInputs) -> f32 {
    let mut num = 1.0_f32;
    if i.original == i.size {
        if i.percent > 0.01 {
            num += i.percent;
        }
        return num * i.pet_coeff;
    }
    let diff = i.size as i32 - i.original as i32;
    for _ in 0..diff.unsigned_abs() {
        num = if diff < 0 { num * i.step } else { num / i.step };
    }
    if i.percent > 0.01 {
        let scale = 1.0_f32 + i.percent;
        num *= scale;
    }
    num *= i.pet_coeff;
    if let Some(max) = i.max_scale {
        // (`OverrideSizeScale.GetFinalScale`: only on this path.)
        num = num.min(max);
    }
    num
}

/// `Mathf.Max(m_Corpulence * sizeScale, 0.5f)`.
#[must_use]
pub fn view_corpulence(m_corpulence: f32, size_scale: f32) -> f32 {
    let c = m_corpulence * size_scale;
    if c > MIN_CORPULENCE { c } else { MIN_CORPULENCE }
}

/// The sim's view state that `Corpulence` depends on, per unit.
#[derive(Debug, Default)]
pub struct Corpulence {
    /// `UnitProgressionData.CurrentScalePercent` (0 until `AddUnitScale` sets it; not saved).
    pub scale_percent: HashMap<EntityIdx, f32>,
    /// `UnitEntityView.m_CachedSizeScale`, as filled by the sim.
    cache: HashMap<EntityIdx, f32>,
    /// Units whose view was created again since they were spawned or loaded
    /// (`UnitEntityData.CreateView`: `CustomPrefabGuid` else the blueprint's prefab).
    view_created: HashSet<EntityIdx>,
    /// Every event that could change a unit's corpulence from here (a reset of the size scale, a
    /// view replaced, a scale percent set), in order: the replay's check doesn't compare a read
    /// with the state of an entry an event happened in.
    pub events: Vec<EntityIdx>,
    /// Fills of the cache (statistics).
    pub fills: usize,
}

/// `UnitEntityView.ResetSizeScale()`.
pub fn reset_size_scale(w: &mut World, u: EntityIdx) {
    let c = &mut w.combat.corpulence;
    c.cache.remove(&u);
    c.events.push(u);
}

/// The unit's view was replaced (a polymorph started or ended, the optimization dummy swapped in
/// or out): a view made by `CreateView` has a fresh cache.
pub fn view_replaced(w: &mut World, u: EntityIdx) {
    let c = &mut w.combat.corpulence;
    c.cache.remove(&u);
    c.view_created.insert(u);
    c.events.push(u);
}

/// `UnitProgressionData.UpdateScalePercent(p)`.
pub fn update_scale_percent(w: &mut World, u: EntityIdx, p: f32) {
    let cur = w.combat.corpulence.scale_percent.get(&u).copied().unwrap_or(0.0);
    if (p - cur).abs() > 0.005 {
        let c = &mut w.combat.corpulence;
        c.scale_percent.insert(u, p);
        c.events.push(u);
    }
}

/// `UnitProgressionData.ResetScalePercent()`.
pub fn reset_scale_percent(w: &mut World, u: EntityIdx) {
    let c = &mut w.combat.corpulence;
    c.scale_percent.insert(u, 0.0);
    c.events.push(u);
}

fn has_part(w: &World, u: EntityIdx, clr: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == clr)
}

/// `GetActivePolymorph()` is some: an active buff with a `Polymorph` component.
fn polymorph_active(w: &World, u: EntityIdx) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        let f = w.fact(*f);
        f.kind == crate::world::FactKind::Buff
            && f.is_active
            && f.bp.as_ref().is_some_and(|b| crate::unit_facts::has_component(b, "Polymorph"))
    })
}

/// Where the unit's view comes from.
enum Source {
    /// A unit prefab, by asset GUID.
    Prefab(String),
    /// The doll prefab of its gender (`true` female).
    Doll(bool),
}

/// The prefab the unit's view was instantiated from: the one it was spawned with in this run,
/// else `UnitEntityData.CreateView` (a doll's, `HasDummyView`'s dummy, `Descriptor.
/// CustomPrefabGuid`, else `Blueprint.Prefab`).
fn view_source(w: &World, u: EntityIdx) -> Result<Source, Cause> {
    let x = w.unit(u).ok_or(Cause::Unit)?;
    if has_part(w, u, "UnitPartRider") {
        return Err(Cause::Rider);
    }
    if polymorph_active(w, u) {
        return Err(Cause::Polymorph);
    }
    // (Suppression, which makes the dummy view, exists only in areas with parts.)
    if crate::scene::area_has_parts(w) {
        return Err(Cause::DummyView);
    }
    let created = w.combat.corpulence.view_created.contains(&u);
    // `HasDummyView` as far as the sim knows it (saved; flipped by `UpdateHasDummyView`).
    let dummy = w.combat.vision.dummy.get(&u).copied().unwrap_or(Some(false));
    if dummy.is_none() {
        return Err(Cause::DummyView);
    }
    if !created && let Some(p) = x.view_prefab.clone() {
        return Ok(Source::Prefab(p));
    }
    if has_part(w, u, "UnitPartDollData") {
        // (`dollData?.RacePreset != null`; else the unit's own prefab.)
        if !x.doll_hydrated {
            return Err(Cause::Doll);
        }
        if let Some(female) = x.doll_gender_female {
            return Ok(Source::Doll(female));
        }
    }
    if dummy == Some(true) {
        return crate::scene_spawn::dummy_prefab(w).map(Source::Prefab).ok_or(Cause::NoPrefab);
    }
    if let Some(c) = x.custom_prefab_guid.clone().filter(|c| !c.is_empty()) {
        return Ok(Source::Prefab(c));
    }
    x.blueprint
        .and_then(|g| w.content.get(g))
        .and_then(|b| crate::scene_spawn::blueprint_prefab(&b))
        .map(Source::Prefab)
        .ok_or(Cause::NoPrefab)
}

/// `Descriptor.OriginalSize`: the race's size, else the blueprint's (read from the content, as
/// a race set after the unit was built moves it; the unit's stored size when the content has
/// neither).
fn original_size(w: &World, u: EntityIdx) -> Option<Size> {
    let x = w.unit(u)?;
    let race = x.progression.race.and_then(|g| w.content.get(g)).map(|b| b.view().int("Size"));
    let bp = x.blueprint.and_then(|g| w.content.get(g)).map(|b| b.view().int("Size"));
    race.or(bp).map_or(x.original_size, Size::from_i64)
}

/// `m_RacialPetScaleCoeff`: 1 without a master; with one, `MountModelSizing.GetCoeff` of the
/// master's race, known only when no entry of the table differs from 1 (the field is set once,
/// in `Start`).
fn pet_coefficient(w: &World, u: EntityIdx) -> Result<f32, Cause> {
    if w.unit(u).ok_or(Cause::Unit)?.master.is_none() {
        return Ok(1.0);
    }
    let root = w.content.get(crate::unit::BLUEPRINT_ROOT).ok_or(Cause::PetCoefficient)?;
    let sizing = root.view().obj("MountModelSizing").ok_or(Cause::PetCoefficient)?;
    let all_one =
        sizing.objs("Entries").iter().all(|e| crate::content::View(e).f32("Coefficient") == 1.0);
    if all_one { Ok(1.0) } else { Err(Cause::PetCoefficient) }
}

/// `UnitEntityData.Corpulence` of unit `u`: `View.Corpulence`, from the unit's prefab and size.
///
/// # Errors
///
/// The [`Cause`] the sim can't establish it by.
pub fn corpulence(w: &mut World, u: EntityIdx) -> Result<f32, Cause> {
    let prefab = match view_source(w, u)? {
        Source::Prefab(guid) => {
            let table = w.scene.unit_prefabs.as_ref().ok_or(Cause::NoTable)?;
            table.get(&guid).ok_or(Cause::PrefabUnlisted)?.clone()
        }
        Source::Doll(female) => {
            let dolls = w.scene.doll_prefabs.as_ref().ok_or(Cause::NoTable)?;
            if female { dolls.female.clone() } else { dolls.male.clone() }
        }
    };
    let m_corpulence = prefab.corpulence.ok_or(Cause::PrefabUnlisted)?;
    if let Some(&s) = w.combat.corpulence.cache.get(&u) {
        return Ok(view_corpulence(m_corpulence, s));
    }
    let (step, max_scale) = match prefab.override_present {
        Some(false) => (DEFAULT_SCALE_STEP, None),
        Some(true) => match (prefab.scale_step, prefab.max_scale) {
            (Some(s), Some(m)) => (s, Some(m)),
            _ => return Err(Cause::Override),
        },
        None => return Err(Cause::Override),
    };
    let x = w.unit(u).ok_or(Cause::Unit)?;
    if x.size_part_from_save {
        return Err(Cause::Size);
    }
    let original = original_size(w, u).ok_or(Cause::Size)?;
    let size = x.state_size.unwrap_or(original);
    let inputs = ScaleInputs {
        original,
        size,
        percent: w.combat.corpulence.scale_percent.get(&u).copied().unwrap_or(0.0),
        step,
        max_scale,
        pet_coeff: pet_coefficient(w, u)?,
    };
    let scale = calculate_size_scale(&inputs);
    let c = &mut w.combat.corpulence;
    c.cache.insert(u, scale);
    c.fills += 1;
    Ok(view_corpulence(m_corpulence, scale))
}

#[cfg(test)]
mod tests;
