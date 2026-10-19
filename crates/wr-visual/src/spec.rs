//! Building the spec from a hydrated world.

use std::rc::Rc;

use anyhow::{Context, Result};
use wr_content::Guid;
use wr_content::node::Node;
use wr_sim::content::{Blueprint, Content, View};
use wr_sim::world::{EntityIdx, World};
use wr_view::{EquipmentView, Gender, SCHEMA, UnitView, UnitsSpec};

/// `Kingmaker.Blueprints.Race.Human`, the race an unset or unknown race falls back to.
const RACE_HUMAN: i32 = 0;

/// A `WeakResourceLink`'s asset id (a plain id string, a legacy `{guid}` or a link object).
fn weak_id(n: &Node) -> Option<String> {
    let id: &str = match n {
        Node::WeakLink(Some(s)) | Node::WeakLinkLegacy(s, _) | Node::Str(s) => s,
        Node::Object(o) => View(o).str("AssetId")?,
        _ => return None,
    };
    Some(id.to_owned()).filter(|s| !s.is_empty())
}

fn weak_ids(v: View<'_>, field: &str) -> Vec<String> {
    v.seq(field).iter().filter_map(weak_id).collect()
}

/// `KingmakerEquipmentEntity.GetLinks(gender, race)`.
#[must_use]
pub fn kee_links(kee: &Blueprint, gender: Gender, race: i32) -> Vec<String> {
    let v = kee.view();
    let (male, female) = ("m_MaleArray", "m_FemaleArray");
    if v.bool("m_RaceDependent")
        && let Some(two) =
            usize::try_from(race).ok().and_then(|r| v.objs("m_RaceDependentArrays").get(r).copied())
    {
        let t = View(two);
        let (m, f) = (weak_ids(t, "MaleArray"), weak_ids(t, "FemaleArray"));
        if !m.is_empty() || !f.is_empty() {
            return if gender == Gender::Male { m } else { f };
        }
    }
    weak_ids(v, if gender == Gender::Male { male } else { female })
}

/// The links of a `KingmakerEquipmentEntity` reference field.
fn kee_field(
    content: &Content,
    v: View<'_>,
    field: &str,
    gender: Gender,
    race: i32,
) -> Vec<String> {
    v.guid(field)
        .and_then(|g| content.get(g))
        .map(|k| kee_links(&k, gender, race))
        .unwrap_or_default()
}

/// `BlueprintCharacterClass.GetClothesLinks`.
fn class_clothes(content: &Content, class: &Blueprint, gender: Gender, race: i32) -> Vec<String> {
    let v = class.view();
    let mut out = Vec::new();
    for g in v.guids("m_EquipmentEntities") {
        if let Some(k) = content.get(g) {
            out.extend(kee_links(&k, gender, race));
        }
    }
    out.extend(weak_ids(
        v,
        if gender == Gender::Male { "MaleEquipmentEntities" } else { "FemaleEquipmentEntities" },
    ));
    out
}

/// `BlueprintRace.RaceId` of a unit's race.
fn race_of(w: &World, u: EntityIdx, content: &Content) -> i32 {
    w.unit(u)
        .and_then(|u| u.progression.race)
        .and_then(|g| content.get(g))
        .map_or(RACE_HUMAN, |b| b.view().i32("RaceId"))
}

/// The doll a unit's save carries (`UnitPartDollData.Default`), from the save's JSON.
#[derive(Debug, Clone, Default)]
struct Doll {
    gender: Option<Gender>,
    race_preset: Option<String>,
    ee_ids: Vec<String>,
    ramps: Vec<(String, i32)>,
    secondary_ramps: Vec<(String, i32)>,
    clothes_primary: i32,
    clothes_secondary: i32,
}

fn doll_of(d: &serde_json::Value) -> Doll {
    let strs = |k: &str| -> Vec<String> {
        d[k].as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    let map = |k: &str| -> Vec<(String, i32)> {
        d[k].as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), i32::try_from(v.as_i64()?).ok()?)))
                    .collect()
            })
            .unwrap_or_default()
    };
    Doll {
        gender: d["Gender"]
            .as_str()
            .map(|g| if g == "Female" { Gender::Female } else { Gender::Male }),
        race_preset: d["RacePreset"].as_str().map(str::to_owned).filter(|s| !s.is_empty()),
        ee_ids: strs("EquipmentEntityIds"),
        ramps: map("EntityRampIdices"),
        secondary_ramps: map("EntitySecondaryRampIdices"),
        clothes_primary: d["ClothesPrimaryIndex"].as_i64().map_or(-1, |v| v as i32),
        clothes_secondary: d["ClothesSecondaryIndex"].as_i64().map_or(-1, |v| v as i32),
    }
}

/// Every unit's doll in the save's documents, by `UniqueId`.
fn save_dolls(path: &std::path::Path) -> Result<std::collections::HashMap<String, Doll>> {
    fn walk(v: &serde_json::Value, out: &mut std::collections::HashMap<String, Doll>) {
        match v {
            serde_json::Value::Object(m) => {
                let is_unit = m.get("$type").and_then(|t| t.as_str()).is_some_and(|t| {
                    t.starts_with("Kingmaker.EntitySystem.Entities.UnitEntityData,")
                });
                if is_unit && let Some(id) = m.get("UniqueId").and_then(|i| i.as_str()) {
                    let mut doll = None;
                    find_part(v, &mut doll);
                    if let Some(d) = doll {
                        out.insert(id.to_owned(), doll_of(d));
                    }
                }
                for x in m.values() {
                    walk(x, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    fn find_part<'a>(v: &'a serde_json::Value, out: &mut Option<&'a serde_json::Value>) {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("$type")
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t.starts_with("Kingmaker.UnitLogic.Parts.UnitPartDollData,"))
                    && m.get("Default").is_some_and(serde_json::Value::is_object)
                {
                    *out = m.get("Default");
                    return;
                }
                for x in m.values() {
                    find_part(x, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| find_part(x, out)),
            _ => {}
        }
    }
    let mut out = std::collections::HashMap::new();
    for e in wr_save::zks::read(path)? {
        if e.name.ends_with(".json")
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&e.bytes)
        {
            walk(&v, &mut out);
        }
    }
    Ok(out)
}

fn push(
    out: &mut Vec<EquipmentView>,
    ee: String,
    source: &str,
    primary: i32,
    secondary: i32,
    preset: i32,
) {
    // `Character.AddEquipmentEntity` ignores an entity the character already has. A class entry
    // is kept either way: the game still sets its clothes ramps on the entity that is there.
    if source.starts_with("class:") || !out.iter().any(|e| e.ee == ee) {
        out.push(EquipmentView { ee, source: source.to_owned(), primary, secondary, preset });
    }
}

/// The equipment entities a unit's view carries beyond its prefab's, in the game's order.
fn equipment(
    w: &World,
    content: &Content,
    u: EntityIdx,
    gender: Gender,
    race: i32,
    doll: Option<&Doll>,
) -> Result<Vec<EquipmentView>> {
    let mut out = Vec::new();
    if let Some(d) = doll {
        // `DollData.CreateUnitView`: the race preset's skin, the generator's clothes, then
        // the chosen entities, then their ramps.
        let ramp = |list: &[(String, i32)], ee: &str| {
            list.iter().find(|(k, _)| k == ee).map_or(-1, |(_, v)| *v)
        };
        if let Some(p) = d.race_preset.as_deref().and_then(Guid::parse).and_then(|g| content.get(g))
        {
            let preset_race = p.view().i32("RaceId");
            for ee in kee_field(content, p.view(), "m_Skin", gender, preset_race) {
                push(&mut out, ee, "skin", -1, -1, -1);
            }
        }
        // `CharGen.MaleClothes` / `FemaleClothes` come from the root blueprint.
        if let Some(root) = content.get(wr_sim::unit::BLUEPRINT_ROOT)
            && let Some(cg) = root.view().obj("CharGen")
        {
            let field = if gender == Gender::Male { "MaleClothes" } else { "FemaleClothes" };
            for ee in weak_ids(cg, field) {
                push(&mut out, ee, "doll-clothes", -1, -1, -1);
            }
        }
        for ee in &d.ee_ids {
            push(&mut out, ee.clone(), "doll", -1, -1, -1);
        }
        for e in &mut out {
            e.primary = ramp(&d.ramps, &e.ee);
            e.secondary = ramp(&d.secondary_ramps, &e.ee);
        }
    }
    let unit = w.unit(u).context("unit")?;
    // `UpdateBodyEquipmentModel`: every slot's item, unless the slot hides it.
    for slot in unit.body.slots.iter().filter(|s| !s.name.starts_with("QuickSlot")) {
        let Some(item) = slot.item.and_then(|i| w.item(i)) else { continue };
        let Some(ibp) = item.blueprint.and_then(|g| content.get(g)) else { continue };
        // `ItemEntity.EquipmentEntity`: the visual source item's, else the item's own.
        let source_bp = item.visual_source.and_then(|g| content.get(g));
        let kee = source_bp
            .as_ref()
            .and_then(|b| b.view().guid("m_EquipmentEntity"))
            .or_else(|| ibp.view().guid("m_EquipmentEntity"))
            .and_then(|g| content.get(g));
        let Some(kee) = kee else { continue };
        let preset = source_bp.as_ref().map_or_else(
            || ibp.view().i32("m_ForcedRampColorPresetIndex"),
            |b| b.view().i32("m_ForcedRampColorPresetIndex"),
        );
        for ee in kee_links(&kee, gender, race) {
            push(&mut out, ee, &format!("item:{}", ibp.name), -1, -1, preset);
        }
    }
    // `UpdateClassEquipment`: the class with the most levels that has entities.
    if unit.is_main_character || doll.is_some() {
        let mut best: Option<(i32, Rc<Blueprint>)> = None;
        for c in &unit.progression.classes {
            if let Some(b) = content.get(c.class) {
                let v = b.view();
                let has = !v.seq("MaleEquipmentEntities").is_empty()
                    || !v.seq("FemaleEquipmentEntities").is_empty();
                if has && best.as_ref().is_none_or(|(l, _)| c.level > *l) {
                    best = Some((c.level, b));
                }
            }
        }
        if let Some((_, class)) = best {
            let (p, s) = doll.map_or((-1, -1), |d| (d.clothes_primary, d.clothes_secondary));
            for ee in class_clothes(content, &class, gender, race) {
                push(&mut out, ee, &format!("class:{}", class.name), p, s, -1);
            }
        }
    }
    Ok(out)
}

/// The spec of a save: every unit that is in the game, with its view.
///
/// # Errors
///
/// A save that doesn't read or hydrate, or content that doesn't open.
pub fn build(save: &str) -> Result<UnitsSpec> {
    let path = wr_save::report::find_save(save)?;
    let s = wr_sim::hydrate::Save::read(&path)?;
    let content = Rc::new(Content::open_default()?);
    let registry = Rc::new(wr_sim::components::registry());
    let (w, report) = wr_sim::hydrate::hydrate(
        content.clone(),
        registry,
        &s,
        &wr_sim::hydrate::Options::default(),
    )?;
    let dolls = save_dolls(&path)?;
    let mut units = Vec::new();
    for i in 0..w.entities.len() {
        let e = EntityIdx(u32::try_from(i)?);
        let Some(u) = w.unit(e) else { continue };
        let ent = w.entity(e);
        if !ent.is_in_game {
            continue;
        }
        let Some(bp) = u.blueprint.and_then(|g| content.get(g)) else { continue };
        let Some(prefab) =
            u.custom_prefab_guid.clone().or_else(|| wr_sim::scene_spawn::blueprint_prefab(&bp))
        else {
            continue;
        };
        let gender = if wr_sim::text::gender(&w, e) == wr_text::Gender::Female {
            Gender::Female
        } else {
            Gender::Male
        };
        let race = race_of(&w, e, &content);
        let doll = dolls.get(&ent.unique_id);
        let gender = doll.and_then(|d| d.gender).unwrap_or(gender);
        units.push(UnitView {
            id: ent.unique_id.clone(),
            blueprint: bp.guid.to_string(),
            name: bp.name.clone(),
            prefab,
            position: u.position,
            orientation: u.orientation,
            gender,
            life_state: u.life_state,
            prone: u.prone_active,
            race,
            replace_base_equipment: doll.is_some(),
            equipment: equipment(&w, &content, e, gender, race, doll)?,
        });
    }
    Ok(UnitsSpec {
        schema: SCHEMA.to_owned(),
        save: wr_save::report::short_name(&path),
        area: report.area.unwrap_or_default(),
        units,
    })
}
