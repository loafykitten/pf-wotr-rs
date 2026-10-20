//! Characters: a unit body composed from equipment entities, as Owlcat's `Character` does at run
//! time (`Character.UpdateCharacter`, `BuildMesh`, `MergeOverlays`), with the merged mesh kept unposed
//! and skinned to the rig at run time.
//!
//! The `Character` component names a rig (`AnimatorPrefab`, a skeleton hierarchy in `techart`), a
//! `Skeleton` (per-bone scale and offset: the race and build), and the equipment entities the
//! prefab carries. Equipment entities (EEs) are scriptable objects of body parts: a skinned mesh
//! prefab per part, a material, and the part's texture layers. The character picks, per body part
//! type, the part of the highest EE layer that no other EE hides, merges those meshes into one
//! skinned mesh (their UVs already address a fixed 2048-pixel atlas layout), and composes three
//! atlases (diffuse, normal, masks) from every overlay part's textures, painting colour ramps
//! into the grey-scale ones first. The unit is one renderer with the atlas material.
//!
//! Not done in stage 1: outfit parts (capes, shields, backpacks, props attached to bones), the
//! race preset's skeleton override for doll units, weapons in hand, PBD cloth, and the LOD
//! atlases (the original starts a character at 256 pixels and refines it as it nears the camera).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use glam::Mat4;
use wr_assets::{
    AssetId, Channel as MeshChannel, Mesh as PackMesh, SkinBinding, SubMesh, TexDimension,
    TexFormat, Texture, VertexStream,
};
use wr_view::UnitView;

use super::anim::{AnimCache, BuiltRig, bound_poses, build_rig, or_rest_pose};
use super::atlas::{self, Atlas, Rect};
use super::image::{self, Image};
use super::skin;
use super::{UnitPrefab, bindposes, placement, push_renderer_for};
use crate::area::ObjRef;
use crate::catalog::Locations;
use crate::pack::{Builder, SceneIndex, mesh as pack_mesh, skip};
use crate::unity::serialized::{SerializedFile, class_id};
use crate::unity::value::{PPtr, Value};

/// Unity `BodyPartType` flags the composition tests.
mod ty {
    pub const BELT: i64 = 1;
    pub const BELT_BAG: i64 = 2;
    pub const BROWS: i64 = 4;
    pub const CHEST_BAG: i64 = 8;
    pub const CUFFS: i64 = 0x10;
    pub const CUTS: i64 = 0x20;
    pub const EYES: i64 = 0x40;
    pub const ELBOW_COPS: i64 = 0x80;
    pub const FEET: i64 = 0x100;
    pub const FOREARMS: i64 = 0x200;
    pub const HANDS: i64 = 0x400;
    pub const HEAD: i64 = 0x800;
    pub const HELMET: i64 = 0x1000;
    pub const HORNS: i64 = 0x2000;
    pub const KNEE_COPS: i64 = 0x4000;
    pub const LASHES: i64 = 0x8000;
    pub const LOWER_LEGS: i64 = 0x10000;
    pub const MASK_BOTTOM: i64 = 0x20000;
    pub const MASK_GOGGLES: i64 = 0x40000;
    pub const RING_LEFT: i64 = 0x80000;
    pub const RING_RIGHT: i64 = 0x100000;
    pub const SKIRT: i64 = 0x200000;
    pub const SPAULDERS: i64 = 0x400000;
    pub const TAIL: i64 = 0x800000;
    pub const THIGH_BAG: i64 = 0x1000000;
    pub const TORSO: i64 = 0x2000000;
    pub const UPPER_ARMS: i64 = 0x4000000;
    pub const UPPER_LEGS: i64 = 0x8000000;
    pub const HEAD_TOP: i64 = 0x1000_0000;
    pub const HEAD_BOTTOM: i64 = 0x2000_0000;
    pub const EARS: i64 = 0x4000_0000;
    pub const NECK: i64 = 0x8000_0000;
    pub const TEETH: i64 = 0x1_0000_0000;
    pub const MASK: i64 = 0x2_0000_0000;
    pub const HAIR: i64 = 0x8_0000_0000;
    pub const HEAD_BAND: i64 = 0x10_0000_0000;
    pub const NECK_TORSO: i64 = 0x20_0000_0000;
    pub const NOSE: i64 = 0x40_0000_0000;
}

/// `CharacterAtlas.MapRect`: where a body part type's textures sit in the 2048-pixel layout.
#[must_use]
pub fn map_rect(t: i64) -> Rect {
    use ty::*;
    match t {
        BELT => (0.0, 673.0, 685.0, 380.0),
        BELT_BAG => (1295.0, 318.0, 394.0, 200.0),
        BROWS => (1295.0, 193.0, 265.0, 125.0),
        CHEST_BAG => (1295.0, 803.0, 170.0, 330.0),
        CUFFS => (1690.0, 443.0, 358.0, 245.0),
        CUTS => (925.0, 0.0, 165.0, 378.0),
        EYES => (1090.0, 283.0, 205.0, 95.0),
        ELBOW_COPS => (1295.0, 518.0, 395.0, 285.0),
        FEET => (310.0, 1053.0, 375.0, 315.0),
        FOREARMS => (1465.0, 1798.0, 583.0, 250.0),
        HANDS => (1690.0, 688.0, 358.0, 375.0),
        HEAD | HEAD_BOTTOM | EARS | NECK | TEETH | HEAD_TOP | NOSE => (0.0, 1628.0, 685.0, 420.0),
        HELMET | HAIR => (1465.0, 1060.0, 584.0, 512.0),
        HORNS => (925.0, 378.0, 370.0, 295.0),
        KNEE_COPS => (0.0, 1053.0, 310.0, 315.0),
        LASHES => (1090.0, 238.0, 125.0, 45.0),
        LOWER_LEGS => (1465.0, 1573.0, 583.0, 225.0),
        MASK_BOTTOM => (1690.0, 0.0, 358.0, 318.0),
        MASK_GOGGLES => (1690.0, 318.0, 357.0, 124.0),
        RING_LEFT => (1560.0, 256.0, 130.0, 63.0),
        RING_RIGHT => (1560.0, 193.0, 130.0, 63.0),
        SKIRT => (0.0, 98.0, 925.0, 575.0),
        SPAULDERS => (685.0, 673.0, 610.0, 460.0),
        TAIL => (0.0, 0.0, 925.0, 98.0),
        THIGH_BAG => (1465.0, 803.0, 225.0, 260.0),
        TORSO | NECK_TORSO => (685.0, 1628.0, 780.0, 420.0),
        UPPER_ARMS => (0.0, 1368.0, 685.0, 260.0),
        UPPER_LEGS => (685.0, 1133.0, 780.0, 495.0),
        HEAD_BAND => (1371.0, 50.0, 318.0, 143.0),
        MASK => (1690.0, 0.0, 357.0, 442.0),
        _ => (0.0, 0.0, 0.0, 0.0),
    }
}

/// A texture's identity (serialized file, path id).
type TexId = (String, i64);

fn tex_id(o: &ObjRef) -> TexId {
    (o.file.name.clone(), o.info.path_id)
}

/// `CharacterTextureDescription`.
#[derive(Debug, Clone)]
struct TexDesc {
    active: Option<ObjRef>,
    diffuse: Option<ObjRef>,
    normal: Option<ObjRef>,
    mask: Option<ObjRef>,
    ramp_shadow: Option<ObjRef>,
    default_mask1: Option<ObjRef>,
    default_mask2: Option<ObjRef>,
    use_ramp1: bool,
    use_ramp2: bool,
    use_default1: bool,
    use_default2: bool,
    /// `UseShadowMask`: the texture bakes a shadow mask into the atlas (not done in stage 1).
    use_shadow: bool,
    dont_blend_alpha: bool,
    /// `Ramps.PrimaryRamp` and `Ramps.SecondaryRamp`.
    primary_ramp: Option<ObjRef>,
    secondary_ramp: Option<ObjRef>,
    channel: i64,
}

/// `BodyPart`.
#[derive(Debug, Clone)]
struct Part {
    ty: i64,
    /// The renderer prefab's GameObject.
    prefab: Option<ObjRef>,
    material: Option<ObjRef>,
    textures: Vec<TexDesc>,
    /// Has a renderer prefab and a material (`SkinnedRenderer != null && Material != null` as far as
    /// the selection can tell).
    visible: bool,
    /// `Textures[0].ActiveTexture`, the identity overlays are compared by.
    tex0: Option<TexId>,
    /// Every texture layer has its source texture (`AddBodyParts` skips the part otherwise).
    complete: bool,
}

/// `EquipmentEntity`.
#[derive(Debug)]
struct Ee {
    guid: String,
    name: String,
    layer: i32,
    hide: i64,
    show_lower: bool,
    primary_ramps: Vec<Option<ObjRef>>,
    /// `PrimaryRamps.Count` (what `AddEquipmentEntity` tests).
    primary_ramp_count: usize,
    secondary_ramps: Vec<Option<ObjRef>>,
    special_primary: Vec<Option<ObjRef>>,
    presets: Vec<(i32, i32)>,
    parts: Vec<Part>,
}

fn pptr_obj(
    b: &Builder<'_>,
    from: &SerializedFile<'_>,
    v: &Value,
    key: &str,
) -> Result<Option<ObjRef>> {
    match v.get(key).and_then(Value::as_pptr).filter(|p| !p.is_null()) {
        Some(p) => b.universe.resolve(from, p),
        None => Ok(None),
    }
}

/// A list of pointers keeping every slot: an index into it means what it means in the game, null
/// entries included.
fn pptr_slots(
    b: &Builder<'_>,
    from: &SerializedFile<'_>,
    v: &Value,
    key: &str,
) -> Result<Vec<Option<ObjRef>>> {
    let mut out = Vec::new();
    for x in v.get(key).and_then(Value::as_array).unwrap_or_default() {
        out.push(match x.as_pptr().filter(|p| !p.is_null()) {
            Some(p) => b.universe.resolve(from, p)?,
            None => None,
        });
    }
    Ok(out)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn tex_desc(b: &Builder<'_>, from: &SerializedFile<'_>, d: &Value) -> Result<TexDesc> {
    let o = |k: &str| pptr_obj(b, from, d, k);
    Ok(TexDesc {
        active: o("ActiveTexture")?,
        diffuse: o("DiffuseTexture")?,
        normal: o("NormalTexture")?,
        mask: o("MaskTexture")?,
        ramp_shadow: o("RampShadowTexture")?,
        default_mask1: o("DefaultMask1")?,
        default_mask2: o("DefaultMask2")?,
        use_ramp1: flag(d, "UseRamp1Mask"),
        use_ramp2: flag(d, "UseRamp2Mask"),
        use_default1: flag(d, "UseDefaultMask1"),
        use_default2: flag(d, "UseDefaultMask2"),
        use_shadow: flag(d, "UseShadowMask"),
        dont_blend_alpha: flag(d, "DontBlendAlphaChannel"),
        primary_ramp: match d
            .get("Ramps")
            .and_then(|r| r.get("PrimaryRamp"))
            .and_then(Value::as_pptr)
            .filter(|p| !p.is_null())
        {
            Some(p) => b.universe.resolve(from, p)?,
            None => None,
        },
        secondary_ramp: match d
            .get("Ramps")
            .and_then(|r| r.get("SecondaryRamp"))
            .and_then(Value::as_pptr)
            .filter(|p| !p.is_null())
        {
            Some(p) => b.universe.resolve(from, p)?,
            None => None,
        },
        channel: d.i64_at("Channel").unwrap_or(0),
    })
}

/// A ramp list with its slots: `None` is a null entry.
type Ramps = Vec<Option<ObjRef>>;

/// The `CharacterColorsProfile` an EE reads its ramps from, or its own lists.
fn ramps_of(
    b: &Builder<'_>,
    from: &SerializedFile<'_>,
    v: &Value,
    profile: &str,
    own: &str,
) -> Result<(Ramps, Ramps)> {
    if let Some(p) = pptr_obj(b, from, v, profile)? {
        let pv = p.read()?;
        return Ok((
            pptr_slots(b, p.file, &pv, "Ramps")?,
            pptr_slots(b, p.file, &pv, "SpecialRamps")?,
        ));
    }
    Ok((pptr_slots(b, from, v, own)?, Vec::new()))
}

fn load_ee(b: &Builder<'_>, locations: &Locations, guid: &str) -> Result<Ee> {
    let bundle = locations
        .guid_to_bundle
        .get(guid)
        .with_context(|| format!("equipment entity {guid} is not in the location list"))?;
    for f in b.universe.files_of(bundle)? {
        for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = f.read(o)?;
            if !b.universe.script_of(f, &v)?.is_some_and(|s| s.class == "EquipmentEntity") {
                continue;
            }
            let (primary_ramps, special_primary) =
                ramps_of(b, f, &v, "PrimaryColorsProfile", "m_PrimaryRamps")?;
            let (secondary_ramps, _special_secondary) =
                ramps_of(b, f, &v, "SecondaryColorsProfile", "m_SecondaryRamps")?;
            let mut presets = Vec::new();
            if let Some(cp) = pptr_obj(b, f, &v, "ColorPresets")? {
                let pv = cp.read()?;
                for p in pv.get("IndexPairs").and_then(Value::as_array).unwrap_or_default() {
                    presets.push((
                        i32::try_from(p.i64_at("PrimaryIndex").unwrap_or(-1))?,
                        i32::try_from(p.i64_at("SecondaryIndex").unwrap_or(-1))?,
                    ));
                }
            }
            let mut parts = Vec::new();
            for p in v.get("BodyParts").and_then(Value::as_array).unwrap_or_default() {
                let mut textures = Vec::new();
                for t in p.get("Textures").and_then(Value::as_array).unwrap_or_default() {
                    textures.push(tex_desc(b, f, t)?);
                }
                let prefab = pptr_obj(b, f, p, "RendererPrefab")?;
                let material = pptr_obj(b, f, p, "Material")?;
                parts.push(Part {
                    ty: p.i64_at("m_Type").unwrap_or(0),
                    visible: prefab.is_some() && material.is_some(),
                    tex0: textures.first().and_then(|t| t.active.as_ref()).map(tex_id),
                    complete: textures.iter().all(|t| t.active.is_some()),
                    prefab,
                    material,
                    textures,
                });
            }
            return Ok(Ee {
                guid: guid.to_owned(),
                name: v.str_at("m_Name").unwrap_or_default().to_owned(),
                layer: i32::try_from(v.i64_at("Layer").unwrap_or(0))?,
                hide: v.i64_at("m_HideBodyParts").unwrap_or(0),
                show_lower: flag(&v, "ShowLowerMaterials"),
                primary_ramp_count: primary_ramps.len(),
                primary_ramps,
                secondary_ramps,
                special_primary,
                presets,
                parts,
            });
        }
    }
    bail!("{guid}: no EquipmentEntity")
}

/// `Character.SelectedRampIndices`.
#[derive(Debug, Clone, Copy)]
struct Selected {
    primary: i32,
    secondary: i32,
    special_primary: i32,
    special_secondary: i32,
}

/// The character's equipment list and ramp choices (`m_EquipmentEntities`, `m_RampIndices`).
#[derive(Debug, Default)]
struct Equipment {
    ees: Vec<Rc<Ee>>,
    ramps: HashMap<usize, Selected>,
}

impl Equipment {
    fn index(&self, guid: &str) -> Option<usize> {
        self.ees.iter().position(|e| e.guid == guid)
    }

    /// `AddEquipmentEntity(ee, saved, primaryRamp, secondaryRamp)`.
    fn add(&mut self, ee: Rc<Ee>, primary: i32, secondary: i32) {
        if self.index(&ee.guid).is_some() {
            return;
        }
        let has_ramps = ee.primary_ramp_count > 0;
        let i = self.ees.len();
        self.ees.push(ee);
        if primary > 0 || secondary > 0 {
            self.set(i, primary, secondary, -1, -1);
        } else if has_ramps {
            self.ramps.entry(i).or_insert(Selected {
                primary: 0,
                secondary: 0,
                special_primary: -1,
                special_secondary: -1,
            });
        }
    }

    /// `SetRampIndices`.
    fn set(&mut self, i: usize, primary: i32, secondary: i32, special_p: i32, special_s: i32) {
        match self.ramps.get_mut(&i) {
            Some(s) => {
                if primary >= 0 {
                    s.primary = primary;
                }
                if secondary >= 0 {
                    s.secondary = secondary;
                }
                s.special_primary = special_p;
                s.special_secondary = special_s;
            }
            None => {
                self.ramps.insert(
                    i,
                    Selected {
                        primary,
                        secondary,
                        special_primary: special_p,
                        special_secondary: special_s,
                    },
                );
            }
        }
    }

    fn set_primary(&mut self, i: usize, primary: i32) {
        let (s, sp, ss) = self
            .ramps
            .get(&i)
            .map_or((-1, -1, -1), |r| (r.secondary, r.special_primary, r.special_secondary));
        let _ = s;
        self.set(i, primary, -1, sp, ss);
    }

    fn set_secondary(&mut self, i: usize, secondary: i32) {
        let (sp, ss) =
            self.ramps.get(&i).map_or((-1, -1), |r| (r.special_primary, r.special_secondary));
        self.set(i, -1, secondary, sp, ss);
    }
}

/// A part of an EE in the equipment list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PartRef {
    ee: usize,
    part: usize,
}

/// `Dictionary<BodyPart, EquipmentEntity>` with .NET's enumeration order: entries stay in their
/// slot when others are removed, and a removed slot is reused by the next insertion.
#[derive(Debug, Default)]
struct OrderedDict {
    slots: Vec<Option<(PartRef, usize)>>,
    free: Vec<usize>,
}

impl OrderedDict {
    fn insert(&mut self, k: PartRef, v: usize) {
        if let Some(s) = self.slots.iter_mut().flatten().find(|(key, _)| *key == k) {
            s.1 = v;
            return;
        }
        match self.free.pop() {
            Some(i) => self.slots[i] = Some((k, v)),
            None => self.slots.push(Some((k, v))),
        }
    }

    fn remove(&mut self, k: PartRef) {
        if let Some(i) = self.slots.iter().position(|s| s.is_some_and(|(key, _)| key == k)) {
            self.slots[i] = None;
            self.free.push(i);
        }
    }

    fn iter(&self) -> impl Iterator<Item = (PartRef, usize)> + '_ {
        self.slots.iter().flatten().copied()
    }
}

/// `Character.UpdateCharacter`'s selection: the geometry parts (in dictionary order) and the
/// overlay list (`m_OverlayBodyParts`). Helmets and cloth are shown, nothing is forced naked.
fn select(eq: &Equipment) -> (Vec<(PartRef, usize)>, Vec<PartRef>) {
    let mut order: Vec<usize> =
        (0..eq.ees.len()).filter(|&i| !eq.ees[i].parts.is_empty()).collect();
    order.sort_by_key(|&i| eq.ees[i].layer);
    let visible = |p: &Part| p.visible;
    let mut hidden_ees: Vec<usize> = Vec::new();
    for &item in &order {
        let mut hide = 0i64;
        for (j, e) in eq.ees.iter().enumerate() {
            if j != item && !hidden_ees.contains(&j) {
                hide |= e.hide;
            }
        }
        let n = eq.ees[item].parts.iter().filter(|p| visible(p) && hide & p.ty == 0).count();
        if n == 0 {
            hidden_ees.push(item);
        }
    }
    let mut dict = OrderedDict::default();
    for &item in &order {
        let mut hide = 0i64;
        for (j, e) in eq.ees.iter().enumerate() {
            if j != item && !hidden_ees.contains(&j) {
                hide |= e.hide;
            }
        }
        for (pi, p) in eq.ees[item].parts.iter().enumerate() {
            if hide & p.ty == 0 && visible(p) {
                let existing = dict
                    .iter()
                    .find(|(k, _)| eq.ees[k.ee].parts[k.part].ty == p.ty)
                    .map(|(k, _)| k);
                if let Some(k) = existing {
                    dict.remove(k);
                }
                dict.insert(PartRef { ee: item, part: pi }, item);
            }
        }
    }
    let geometry: Vec<(PartRef, usize)> = dict.iter().collect();
    let mut overlay: Vec<PartRef> = Vec::new();
    let has_type = |ee: usize, t: i64| eq.ees[ee].parts.iter().any(|p| p.ty == t);
    let same_texture =
        |ee: usize, tex: &Option<TexId>| eq.ees[ee].parts.iter().any(|p| p.tex0 == *tex);
    let add = |overlay: &mut Vec<PartRef>, t: i64, ee: usize| {
        for (pi, p) in eq.ees[ee].parts.iter().enumerate() {
            if p.ty != t {
                continue;
            }
            // A texture layer with no source texture is an authoring error the game skips.
            if !p.complete {
                continue;
            }
            let me = PartRef { ee, part: pi };
            overlay.retain(|k| *k != me);
            overlay.push(me);
        }
    };
    for (key, entity) in &geometry {
        let part = &eq.ees[key.ee].parts[key.part];
        let t = part.ty;
        let layer = eq.ees[*entity].layer;
        if eq.ees[*entity].show_lower {
            let mut lower: Vec<usize> = (0..eq.ees.len())
                .filter(|&j| {
                    j != *entity
                        && eq.ees[j].layer < layer
                        && !same_texture(j, &part.tex0)
                        && has_type(j, t)
                })
                .collect();
            lower.sort_by_key(|&j| eq.ees[j].layer);
            for j in lower {
                add(&mut overlay, t, j);
            }
        }
        add(&mut overlay, t, *entity);
        let mut higher: Vec<usize> = (0..eq.ees.len())
            .filter(|&j| j != *entity && eq.ees[j].layer > layer && has_type(j, t))
            .collect();
        higher.sort_by_key(|&j| eq.ees[j].layer);
        for j in higher {
            add(&mut overlay, t, j);
        }
    }
    (geometry, overlay)
}

/// Everything shared between the characters of one pack run.
pub(super) struct Cache {
    scenes: HashMap<String, Rc<SceneIndex>>,
    ees: HashMap<String, Rc<Ee>>,
    images: HashMap<TexId, Rc<Image>>,
    /// Atlas size in pixels (the game's High texture setting gives 2048).
    pub atlas_size: usize,
    /// Things composed approximately or not at all, for the report.
    notes: Vec<String>,
}

impl Cache {
    pub fn new(atlas_size: usize) -> Self {
        Self {
            scenes: HashMap::new(),
            ees: HashMap::new(),
            images: HashMap::new(),
            atlas_size,
            notes: Vec::new(),
        }
    }

    fn scene(&mut self, file: &'static SerializedFile<'static>) -> Result<Rc<SceneIndex>> {
        if let Some(s) = self.scenes.get(&file.name) {
            return Ok(s.clone());
        }
        let s = Rc::new(SceneIndex::load(&file.name, file)?);
        self.scenes.insert(file.name.clone(), s.clone());
        Ok(s)
    }

    fn ee(&mut self, b: &Builder<'_>, locations: &Locations, guid: &str) -> Result<Rc<Ee>> {
        if let Some(e) = self.ees.get(guid) {
            return Ok(e.clone());
        }
        let mut ee = load_ee(b, locations, guid)?;
        // `BodyPart.SkinnedRenderer`: the renderer prefab must hold a `SkinnedMeshRenderer`.
        for p in &mut ee.parts {
            if let Some(pf) = p.prefab {
                let sc = self.scene(pf.file)?;
                p.visible = p.material.is_some() && find_smr(&sc, pf.info.path_id).is_some();
            }
        }
        let e = Rc::new(ee);
        self.ees.insert(guid.to_owned(), e.clone());
        Ok(e)
    }

    fn image(&mut self, b: &Builder<'_>, o: &ObjRef) -> Result<Rc<Image>> {
        let id = tex_id(o);
        if let Some(i) = self.images.get(&id) {
            return Ok(i.clone());
        }
        let img = Rc::new(image::decode(b.universe, &o.read()?)?);
        self.images.insert(id, img.clone());
        Ok(img)
    }
}

/// One skinned part: its mesh in the bind pose, with the names of its bones and their bind poses.
struct SkinnedPart {
    mesh: pack_mesh::DecodedMesh,
    /// The bones of the part's `SkinnedMeshRenderer`, by transform name.
    bones: Vec<String>,
    bind: Vec<Mat4>,
}

/// `GetComponentInChildren<T>()` over transforms: depth first from `root`, the root first. The queried
/// GameObject is always searched, whatever its own state; below it `includeInactive` (false by default)
/// skips inactive GameObjects, and an inactive one hides its whole subtree. The component's own
/// `enabled` flag plays no part. (Unity's documentation says exactly this about the root and about
/// "active child GameObjects"; whether a child of an inactive root counts as active is native
/// behaviour it doesn't spell out. No shipped equipment entity depends on it:
/// `wr-import ee-census` finds every renderer prefab root active.)
fn first_active_match(
    root: i64,
    children: &dyn Fn(i64) -> Vec<i64>,
    active: &dyn Fn(i64) -> bool,
    matches: &dyn Fn(i64) -> bool,
) -> Option<i64> {
    let mut stack = vec![root];
    while let Some(t) = stack.pop() {
        if t != root && !active(t) {
            continue;
        }
        if matches(t) {
            return Some(t);
        }
        stack.extend(children(t).into_iter().rev());
    }
    None
}

/// The `SkinnedMeshRenderer` in a body part's renderer prefab (`BodyPart.SkinnedRenderer`).
fn find_smr(sc: &SceneIndex, go: i64) -> Option<(i64, Value)> {
    let root = *sc.go_transform.get(&go)?;
    let go_of = |t: i64| sc.transform_go.get(&t).copied();
    let t = first_active_match(
        root,
        &|t| sc.children.get(&t).cloned().unwrap_or_default(),
        &|t| go_of(t).and_then(|g| sc.gos.get(&g)).is_some_and(|g| g.1),
        &|t| {
            go_of(t).is_some_and(|g| !sc.components(g, class_id::SKINNED_MESH_RENDERER).is_empty())
        },
    )?;
    let o = *sc.components(go_of(t)?, class_id::SKINNED_MESH_RENDERER).first()?;
    sc.file.read(o).ok().map(|v| (o.path_id, v))
}

fn skinned_part(b: &Builder<'_>, cache: &mut Cache, part: &Part) -> Result<Option<SkinnedPart>> {
    let Some(prefab) = part.prefab else { return Ok(None) };
    let sc = cache.scene(prefab.file)?;
    let Some((_, smr)) = find_smr(&sc, prefab.info.path_id) else { return Ok(None) };
    let Some(mp) = smr.get("m_Mesh").and_then(Value::as_pptr).filter(|p| !p.is_null()) else {
        return Ok(None);
    };
    let Some(mo) = b.universe.resolve(prefab.file, mp)? else { return Ok(None) };
    let mv = mo.read()?;
    let mesh = pack_mesh::decode(b.universe, &mv)?;
    let bind = bindposes(&mv)?;
    let bones: Vec<Option<i64>> = smr
        .get("m_Bones")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|x| x.as_pptr().filter(|p| p.file_id == 0 && !p.is_null()).map(|p| p.path_id))
        .collect();
    anyhow::ensure!(
        bind.len() <= bones.len(),
        "{}: {} bind poses for {} bones",
        mesh.name,
        bind.len(),
        bones.len()
    );
    let names: Vec<String> = bones
        .iter()
        .take(bind.len())
        .map(|bone| {
            bone.and_then(|t| sc.transform_go.get(&t))
                .and_then(|g| sc.gos.get(g))
                .map_or_else(String::new, |g| g.0.clone())
        })
        .collect();
    // The part must carry the skin the merge reads.
    if let Err(e) = skin::skin_data(&mesh) {
        let skin_len = mv.get("m_Skin").and_then(Value::as_array).map_or(0, <[Value]>::len);
        let vbcw = mv
            .path("m_VariableBoneCountWeights.m_Data")
            .and_then(Value::as_array)
            .map_or(0, <[Value]>::len);
        bail!(
            "{e:#}; channels {:?}, m_Skin {skin_len}, variable weights {vbcw}",
            mesh.channels.get(10..).unwrap_or_default()
        );
    }
    Ok(Some(SkinnedPart { mesh, bones: names, bind }))
}

/// `Character.CompareSkinningMatrices`: the first 12 elements agree within 1e-4.
fn same_skinning(a: &Mat4, b: &Mat4) -> bool {
    let (a, b) = (a.to_cols_array_2d(), b.to_cols_array_2d());
    (0..4).all(|c| (0..3).all(|r| ((a[c][r] - b[c][r]) as f64).abs() <= 0.0001))
}

/// The merged character mesh and its merged bone list.
struct Merged {
    /// The combined mesh, with the merged skin in stream 1 (channels 12 and 13).
    mesh: pack_mesh::DecodedMesh,
    /// The rig bone of each merged bone, and its bind pose.
    bones: Vec<u32>,
    bind: Vec<Mat4>,
}

/// Merges skinned parts into one mesh with a fixed layout: position and normal float3, tangent
/// float4, UV0 float2, one submesh (`Mesh.CombineMeshes` followed by `mesh.uv = uvs`), and their
/// bones into one list as `Character.EnsureBones` does: a part's bone is looked up by name in the
/// character's rig (the first miss ends the part's mapping, the bones after it keep merged bone
/// 0), and reuses a merged bone of the same name and bind pose.
fn merge(parts: &[SkinnedPart], name: &str, rig: &BuiltRig) -> Result<Merged> {
    let mut vertices: Vec<u8> = Vec::new();
    let mut skins: Vec<([u32; 4], [f32; 4])> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut count = 0u32;
    let mut merged_names: Vec<&str> = Vec::new();
    let mut merged_bones: Vec<u32> = Vec::new();
    let mut merged_bind: Vec<Mat4> = Vec::new();
    for p in parts {
        let m = &p.mesh;
        let mut mapping = vec![0u32; p.bind.len()];
        for (i, bone) in p.bones.iter().enumerate() {
            let Some(&rb) = rig.by_name.get(bone) else { break };
            let found = (0..merged_bind.len())
                .find(|&j| same_skinning(&merged_bind[j], &p.bind[i]) && merged_names[j] == bone);
            mapping[i] = match found {
                Some(j) => j as u32,
                None => {
                    merged_names.push(bone);
                    merged_bones.push(rb);
                    merged_bind.push(p.bind[i]);
                    (merged_bind.len() - 1) as u32
                }
            };
        }
        for (mut idx, wt) in skin::skin_data(m)? {
            for i in &mut idx {
                *i = *mapping.get(*i as usize).context("bone index past the part's bones")?;
            }
            skins.push((idx, wt));
        }
        let read = |ch: usize, v: usize, n: usize| -> Option<[f32; 4]> {
            let c = m.channels.get(ch).copied().flatten()?;
            skin::read_attr(m, c, v, n)
        };
        for v in 0..m.vertex_count as usize {
            let pos = read(0, v, 3).context("position")?;
            let nrm = read(1, v, 3).unwrap_or([0.0, 1.0, 0.0, 0.0]);
            let tan = read(2, v, 4).unwrap_or([1.0, 0.0, 0.0, 1.0]);
            let uv = read(4, v, 2).unwrap_or([0.0; 4]);
            for f in [
                pos[0], pos[1], pos[2], nrm[0], nrm[1], nrm[2], tan[0], tan[1], tan[2], tan[3],
                uv[0], uv[1],
            ] {
                vertices.extend_from_slice(&f.to_le_bytes());
            }
        }
        let isz = if m.index_u32 { 4 } else { 2 };
        // One `CombineInstance` per part with `subMeshIndex` 0: only the first submesh's triangles are
        // kept, though every vertex of the part is copied.
        for sm in m.submeshes.iter().take(1) {
            anyhow::ensure!(sm.topology == 0, "{}: non-triangle submesh", m.name);
            for i in sm.first_index..sm.first_index + sm.index_count {
                let o = i as usize * isz;
                let raw = m.indices.get(o..o + isz).context("index out of range")?;
                let idx = if m.index_u32 {
                    u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
                } else {
                    u32::from(u16::from_le_bytes([raw[0], raw[1]]))
                };
                let v = i64::from(sm.base_vertex) + i64::from(idx);
                anyhow::ensure!(
                    (0..i64::from(m.vertex_count)).contains(&v),
                    "{}: index {v} outside {} vertices",
                    m.name,
                    m.vertex_count
                );
                indices.push(u32::try_from(v)? + count);
            }
        }
        count += m.vertex_count;
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in vertices.as_chunks::<48>().0 {
        for k in 0..3 {
            let f = f32::from_le_bytes([v[k * 4], v[k * 4 + 1], v[k * 4 + 2], v[k * 4 + 3]]);
            lo[k] = lo[k].min(f);
            hi[k] = hi[k].max(f);
        }
    }
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let extent = [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0, (hi[2] - lo[2]) / 2.0];
    let mut channels = vec![None; 14];
    channels[0] = Some(MeshChannel { stream: 0, offset: 0, format: 0, dimension: 3 });
    channels[1] = Some(MeshChannel { stream: 0, offset: 12, format: 0, dimension: 3 });
    channels[2] = Some(MeshChannel { stream: 0, offset: 24, format: 0, dimension: 4 });
    channels[4] = Some(MeshChannel { stream: 0, offset: 40, format: 0, dimension: 2 });
    channels[12] = Some(MeshChannel { stream: 1, offset: 0, format: 0, dimension: 4 });
    channels[13] = Some(MeshChannel { stream: 1, offset: 16, format: 10, dimension: 4 });
    let mesh = pack_mesh::DecodedMesh {
        name: name.to_owned(),
        vertex_count: count,
        channels,
        streams: vec![(vertices, 48), (skin::skin_bytes(&skins), 32)],
        index_u32: true,
        indices: indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
        submeshes: vec![SubMesh {
            first_index: 0,
            index_count: u32::try_from(indices.len())?,
            base_vertex: 0,
            topology: 0,
            bounds: Some((lo, hi)),
        }],
        aabb: (centre, extent),
    };
    Ok(Merged { mesh, bones: merged_bones, bind: merged_bind })
}

/// `Character.MergeOverlays`: the three atlases.
fn compose_atlases(
    b: &Builder<'_>,
    cache: &mut Cache,
    eq: &Equipment,
    overlay: &[PartRef],
) -> Result<[Atlas; 3]> {
    let size = cache.atlas_size;
    // `EquipmentEntity.RepaintTextures` for every entity: grey-scale textures painted with their ramps.
    let mut painted: HashMap<(usize, usize, usize), Image> = HashMap::new();
    for (ei, ee) in eq.ees.iter().enumerate() {
        let sel = eq.ramps.get(&ei).copied();
        let (p, s, sp, ss) = sel.map_or((0, 0, -1, -1), |s| {
            (s.primary, s.secondary, s.special_primary, s.special_secondary)
        });
        let pick = |special: &[Option<ObjRef>], normal: &[Option<ObjRef>], si: i32, i: i32| {
            let slot = |list: &[Option<ObjRef>], k: i32| -> Option<Option<ObjRef>> {
                usize::try_from(k).ok().and_then(|k| list.get(k)).copied()
            };
            // A special index in range wins even over a null slot, as `RepaintTextures` does.
            slot(special, si).or_else(|| slot(normal, i)).flatten()
        };
        let primary = pick(&ee.special_primary, &ee.primary_ramps, sp, p);
        // The game's secondary pick reads the *primary special* list for a special index (a typo
        // in `RepaintTextures`); kept.
        let secondary = pick(&ee.special_primary, &ee.secondary_ramps, ss, s);
        if std::env::var_os("WR_DEBUG_CHARACTERS").is_some() {
            let name = |o: Option<ObjRef>| {
                o.and_then(|o| o.read().ok())
                    .and_then(|v| v.str_at("m_Name").map(str::to_owned))
                    .unwrap_or_else(|| "-".to_owned())
            };
            eprintln!(
                "character ee {} layer {} ramps {p}/{s} ({sp}/{ss}) of {}/{}: primary {} secondary {}",
                ee.name,
                ee.layer,
                ee.primary_ramps.len(),
                ee.secondary_ramps.len(),
                name(primary),
                name(secondary)
            );
        }
        for (pi, part) in ee.parts.iter().enumerate() {
            for (ti, d) in part.textures.iter().enumerate() {
                if !(d.use_ramp1 || d.use_ramp2 || d.use_default1 || d.use_default2) {
                    continue;
                }
                let Some(active) = d.active.as_ref() else { continue };
                let main = cache.image(b, active)?;
                let mut target = (*main).clone();
                if d.use_ramp1 || d.use_default1 {
                    let ramp = d.primary_ramp.or(primary);
                    let mask = if d.use_ramp1 && d.ramp_shadow.is_some() {
                        d.ramp_shadow
                    } else if d.use_default1 {
                        d.default_mask1
                    } else {
                        None
                    };
                    if let (Some(ramp), Some(mask)) = (ramp, mask) {
                        let (r, m) = (cache.image(b, &ramp)?, cache.image(b, &mask)?);
                        atlas::repaint(&mut target, &main, &r, &m, true);
                    }
                }
                if d.use_ramp2 {
                    let ramp = d.secondary_ramp.or(secondary);
                    let mask = if d.ramp_shadow.is_some() {
                        d.ramp_shadow
                    } else if d.use_default2 {
                        d.default_mask2
                    } else {
                        None
                    };
                    if let (Some(ramp), Some(mask)) = (ramp, mask) {
                        let (r, m) = (cache.image(b, &ramp)?, cache.image(b, &mask)?);
                        atlas::repaint(&mut target, &main, &r, &m, false);
                    }
                }
                painted.insert((ei, pi, ti), target);
            }
        }
    }
    // Which textures each atlas gets, in the order the overlay lists them.
    struct Entry {
        part: PartRef,
        tex: usize,
        /// The texture whose rectangle this one shares (the diffuse source).
        primary: TexId,
    }
    let mut lists: [Vec<Entry>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut seen: [HashSet<TexId>; 3] = [HashSet::new(), HashSet::new(), HashSet::new()];
    let mut rect_types: [Vec<(TexId, Vec<i64>)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for pr in overlay {
        let part = &eq.ees[pr.ee].parts[pr.part];
        for (ti, d) in part.textures.iter().enumerate() {
            let mut note = |ch: usize, key: TexId, primary: TexId, new_entry: bool| {
                if let Some(e) = rect_types[ch].iter_mut().find(|(k, _)| *k == key) {
                    if !e.1.contains(&part.ty) {
                        e.1.push(part.ty);
                    }
                } else {
                    rect_types[ch].push((key.clone(), vec![part.ty]));
                }
                if new_entry && seen[ch].insert(key) {
                    lists[ch].push(Entry { part: *pr, tex: ti, primary });
                }
            };
            if let Some(dif) = d.diffuse.as_ref()
                && d.channel == 0
            {
                let _ = dif;
                if let Some(active) = d.active.as_ref() {
                    note(0, tex_id(active), tex_id(active), true);
                }
            }
            if d.normal.is_some()
                && let Some(dif) = d.diffuse.as_ref()
            {
                note(1, tex_id(dif), tex_id(dif), true);
            }
            if d.mask.is_some()
                && let Some(dif) = d.diffuse.as_ref()
            {
                note(2, tex_id(dif), tex_id(dif), true);
            }
        }
    }
    let chans = [atlas::Channel::Diffuse, atlas::Channel::Normal, atlas::Channel::Masks];
    let mut out =
        [Atlas::new(size, chans[0]), Atlas::new(size, chans[1]), Atlas::new(size, chans[2])];
    for ch in 0..3 {
        // `CalculateRects`: a texture takes the rectangle of the first of its types that already has one.
        let mut by_type: HashMap<i64, Rect> = HashMap::new();
        let mut rects: HashMap<TexId, Rect> = HashMap::new();
        for (key, types) in &rect_types[ch] {
            let found = types.iter().find_map(|t| by_type.get(t).copied());
            let r = found.unwrap_or_else(|| {
                let mut last = (0.0, 0.0, 0.0, 0.0);
                for t in types {
                    last = map_rect(*t);
                    by_type.insert(*t, last);
                }
                last
            });
            rects.insert(key.clone(), r);
        }
        for e in &lists[ch] {
            let part = &eq.ees[e.part.ee].parts[e.part.part];
            let d = &part.textures[e.tex];
            let rect = *rects.get(&e.primary).context("atlas rectangle")?;
            let Some(dif) = d.diffuse.as_ref() else { continue };
            if d.use_shadow && ch == 0 {
                cache
                    .notes
                    .push("character texture with UseShadowMask (shadow bake not done)".to_owned());
            }
            let weight = cache.image(b, dif)?;
            match ch {
                0 => {
                    let key = (e.part.ee, e.part.part, e.tex);
                    let src = match painted.get(&key) {
                        Some(p) => Rc::new(p.clone()),
                        None => cache.image(b, d.active.as_ref().context("active texture")?)?,
                    };
                    out[0].blit_diffuse(rect, &src, &weight, d.dont_blend_alpha);
                }
                1 => {
                    let src = cache.image(b, d.normal.as_ref().context("normal")?)?;
                    out[1].blit_normal(rect, &src, &weight, d.dont_blend_alpha);
                }
                _ => {
                    let mask = d.mask.as_ref().context("mask")?;
                    let src = cache.image(b, mask)?;
                    let dont = d.dont_blend_alpha || !src.has_alpha;
                    let factors = match part.material {
                        Some(m) => {
                            let mv = m.read()?;
                            let f = |n: &str| {
                                mv.path("m_SavedProperties.m_Floats")
                                    .and_then(Value::as_array)
                                    .and_then(|a| a.iter().find(|p| p.str_at("first") == Some(n)))
                                    .and_then(|p| p.get("second"))
                                    .and_then(Value::as_f32)
                                    .unwrap_or(1.0)
                            };
                            [f("_Roughness"), f("_Emission"), f("_Metallic")]
                        }
                        None => [1.0; 3],
                    };
                    out[2].blit_masks(rect, Some(&src), &weight, factors, dont);
                }
            }
        }
    }
    Ok(out)
}

/// `WR_DUMP_ATLAS=<dir>`: writes each composed atlas as a binary PPM (rows bottom first, as stored)
/// for looking at; nothing reads them back.
fn dump_atlas(name: &str, at: &Atlas) {
    let Some(dir) = std::env::var_os("WR_DUMP_ATLAS") else { return };
    let mut out = format!("P6\n{} {}\n255\n", at.size, at.size).into_bytes();
    for y in (0..at.size).rev() {
        for x in 0..at.size {
            let p = at.px[y * at.size + x];
            out.extend_from_slice(&[p[0], p[1], p[2]]);
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.ppm")), out);
}

fn push_texture(b: &mut Builder<'_>, name: &str, at: &Atlas, unit: &str) -> Result<u32> {
    dump_atlas(name, at);
    let (data, mips) = at.with_mips();
    let blob = b.w.blob(&data)?;
    b.report.texture_bytes += data.len() as u64;
    let srgb = at.channel == atlas::Channel::Diffuse;
    b.meta.textures.push(Texture {
        id: AssetId { file: format!("character:{unit}"), path_id: i64::from(at.channel as u8) },
        name: name.to_owned(),
        dimension: TexDimension::D2,
        format: TexFormat::Rgba8,
        srgb,
        width: u32::try_from(at.size)?,
        height: u32::try_from(at.size)?,
        layers: 1,
        mips,
        data: blob,
        wrap: [0, 0, 0],
        filter: 1,
        aniso: 1,
        mip_bias: 0.0,
        unity_format: 4,
        transcoded: Some("character atlas composed on the CPU".to_owned()),
    });
    Ok(u32::try_from(b.meta.textures.len() - 1)?)
}

/// The name of a transform's GameObject.
fn av_name(sc: &SceneIndex, t: i64) -> String {
    sc.transform_go.get(&t).and_then(|g| sc.gos.get(g)).map_or_else(String::new, |g| g.0.clone())
}

/// Composes and adds a character unit.
pub(super) fn add(
    b: &mut Builder<'_>,
    locations: &Locations,
    cache: &mut Cache,
    anim: &mut AnimCache,
    prefab: &UnitPrefab,
    u: &UnitView,
) -> Result<()> {
    // The `Character` component.
    let mut character = None;
    for o in prefab.sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
        let v = prefab.sc.file.read(o)?;
        if b.universe.script_of(prefab.sc.file, &v)?.is_some_and(|s| s.class == "Character") {
            character = Some(v);
            break;
        }
    }
    let ch = character.context("no Character")?;
    // Rig and skeleton.
    let animator =
        pptr_obj(b, prefab.sc.file, &ch, "AnimatorPrefab")?.context("no AnimatorPrefab")?;
    let av = animator.read()?;
    let ago = av.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
    let rsc = cache.scene(animator.file)?;
    let root = *rsc.go_transform.get(&ago).context("animator without transform")?;
    let skeleton = pptr_obj(b, prefab.sc.file, &ch, "m_Skeleton")?;
    let sk_key = skeleton.as_ref().map(|s| (s.file.name.clone(), s.info.path_id));
    let rig_key = format!("{}#{root}#{sk_key:?}", animator.file.name);
    let (rig_index, rig) = anim.rig(b, &rig_key, || {
        let sv = skeleton.map(|s| s.read()).transpose()?;
        build_rig(&rsc, root, &av_name(&rsc, root), Mat4::IDENTITY, true, sv.as_ref())
    })?;
    // A character whose animation can't be read (its set included) is drawn in the rig's rest pose,
    // not left out. `Character.AnimationSet` is the human set unless the prefab names its own.
    let setup = (|| -> Result<u32> {
        let set = match ch.get("m_AnimationSet").and_then(Value::as_pptr).filter(|p| !p.is_null()) {
            Some(p) => b.universe.resolve(prefab.sc.file, p)?.context("animation set not found")?,
            None => anim.human_set(b)?,
        };
        let (loco, clips) = anim.locomotion(b, set)?;
        anim.animation(b, &u.id, rig_index, loco, clips)
    })();
    let animation = or_rest_pose(&mut anim.notes, &format!("{} {}", u.id, u.name), setup);
    // Equipment: the prefab's saved entities and ramps, then the spec's.
    let mut eq = Equipment::default();
    if !u.replace_base_equipment {
        let saved: Vec<String> = ch
            .get("m_SavedEquipmentEntities")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|l| l.str_at("AssetId").map(str::to_owned))
            .collect();
        for g in &saved {
            let ee = cache.ee(b, locations, g)?;
            eq.add(ee, -1, -1);
        }
        for r in ch.get("m_SavedRampIndices").and_then(Value::as_array).unwrap_or_default() {
            let Some(g) = r.str_at("EquipmentEntityLink.AssetId") else { continue };
            if let Some(i) = eq.index(g) {
                eq.set(
                    i,
                    i32::try_from(r.i64_at("PrimaryIndex").unwrap_or(-1))?,
                    i32::try_from(r.i64_at("SecondaryIndex").unwrap_or(-1))?,
                    -1,
                    -1,
                );
            }
        }
    }
    // `DollData.CreateUnitView`: the skin, clothes and chosen entities, then their ramps; then the
    // body items (`UpdateBodyEquipmentModel`) and the class clothes (`UpdateClassEquipment`).
    let is_doll = |e: &wr_view::EquipmentView| {
        !e.source.starts_with("item:") && !e.source.starts_with("class:")
    };
    for pass in [true, false] {
        for e in u.equipment.iter().filter(|e| is_doll(e) == pass) {
            let ee = match cache.ee(b, locations, &e.ee) {
                Ok(x) => x,
                Err(err) => {
                    skip(&mut b.report, format!("equipment entity: {err:#}"));
                    continue;
                }
            };
            if e.source.starts_with("item:") {
                // `AddItemEquipment`: no custom ramps (not tracked), the entity's colour preset otherwise.
                let (mut p, mut s) = (-1, -1);
                if let Some(&(pp, ss)) =
                    usize::try_from(e.preset).ok().and_then(|i| ee.presets.get(i))
                {
                    p = pp;
                    s = ss;
                }
                eq.add(ee, p, s);
            } else {
                eq.add(ee, -1, -1);
                // Doll ramps and the class clothes' indices are set whether or not the entity was new.
                if let Some(i) = eq.index(&e.ee) {
                    if e.primary >= 0 {
                        eq.set_primary(i, e.primary);
                    }
                    if e.secondary >= 0 {
                        eq.set_secondary(i, e.secondary);
                    }
                }
            }
        }
    }
    let (geometry, overlay) = select(&eq);
    anyhow::ensure!(!geometry.is_empty(), "no body parts to draw");
    // Meshes in dictionary order.
    let mut parts = Vec::new();
    for (key, _) in &geometry {
        let part = &eq.ees[key.ee].parts[key.part];
        match skinned_part(b, cache, part) {
            Ok(Some(p)) => parts.push(p),
            Ok(None) => {}
            Err(e) => skip(&mut b.report, format!("character part {}: {e:#}", eq.ees[key.ee].name)),
        }
    }
    anyhow::ensure!(!parts.is_empty(), "no skinned mesh");
    let mut merged = merge(&parts, &format!("Character_{}", u.id), &rig)?;
    let (skin_blob, binding) = match animation {
        Some(animation) => {
            // Bounds that cover the unit's idle and walk poses.
            let skin_blob = skin::skin_blob(&merged.mesh)?;
            let skins: Vec<Vec<Mat4>> = bound_poses(b, &b.meta.animations[animation as usize], 8)
                .iter()
                .map(|w| {
                    merged
                        .bones
                        .iter()
                        .zip(&merged.bind)
                        .map(|(rb, bind)| w[*rb as usize] * *bind)
                        .collect()
                })
                .collect();
            skin::animated_bounds(&mut merged.mesh, &skins)?;
            let binding = SkinBinding {
                animation,
                bones: merged.bones.clone(),
                bind_poses: merged.bind.iter().map(Mat4::to_cols_array).collect(),
                // (`push_renderer_for` sets it from the prefab.)
                forbid_rotation: false,
            };
            (Some(skin_blob), Some(binding))
        }
        None => {
            // Posed once in the rig's rest pose (the skeleton's overrides applied).
            let mut worlds = Vec::new();
            rig.rig.world_matrices(&rig.rig.posed_rest(), &mut worlds);
            let skin_mats: Vec<Mat4> = merged
                .bones
                .iter()
                .zip(&merged.bind)
                .map(|(rb, bind)| worlds[*rb as usize] * *bind)
                .collect();
            skin::pose(&mut merged.mesh, &skin_mats)?;
            skin::refresh_bounds(&mut merged.mesh)?;
            (None, None)
        }
    };
    merged.mesh.streams.truncate(1);
    merged.mesh.channels[12] = None;
    merged.mesh.channels[13] = None;
    let merged = merged.mesh;
    let mesh_index = {
        let mut streams = Vec::new();
        for (bytes, stride) in &merged.streams {
            streams.push(VertexStream { data: b.w.blob(bytes)?, stride: *stride });
            b.report.mesh_bytes += bytes.len() as u64;
        }
        let indices = b.w.blob(&merged.indices)?;
        b.report.mesh_bytes += merged.indices.len() as u64;
        let skin = skin_blob.as_ref().map(|s| b.w.blob(s)).transpose()?;
        b.report.mesh_bytes += skin_blob.as_ref().map_or(0, |s| s.len() as u64);
        b.meta.meshes.push(PackMesh {
            id: AssetId { file: format!("character:{}", u.id), path_id: 0 },
            name: merged.name.clone(),
            vertex_count: merged.vertex_count,
            channels: merged.channels.clone(),
            streams,
            index_u32: true,
            indices,
            submeshes: merged.submeshes.clone(),
            aabb: merged.aabb,
            skin,
        });
        u32::try_from(b.meta.meshes.len() - 1)?
    };
    // Atlases and the material: a copy of the first overlay part's material with the atlases.
    anyhow::ensure!(!overlay.is_empty(), "no overlay parts");
    if std::env::var_os("WR_DEBUG_CHARACTERS").is_some() {
        for k in &overlay {
            let part = &eq.ees[k.ee].parts[k.part];
            eprintln!(
                "character {} overlay {} part {:#x} layers {} geometry {}",
                u.name,
                eq.ees[k.ee].name,
                part.ty,
                part.textures.len(),
                geometry.iter().any(|(g, _)| g == k)
            );
        }
    }
    let atlases = compose_atlases(b, cache, &eq, &overlay)?;
    for n in cache.notes.drain(..) {
        skip(&mut b.report, n);
    }
    let first = overlay
        .iter()
        .find_map(|k| eq.ees[k.ee].parts[k.part].material)
        .context("no body part material")?;
    let base = b
        .material(first.file, PPtr { file_id: 0, path_id: first.info.path_id }, &[])?
        .context("body part material out of scope")?;
    let names = ["_BaseMap", "_BumpMap", "_MasksMap"];
    let mut tex_ids = Vec::new();
    for (i, a) in atlases.iter().enumerate() {
        tex_ids.push(push_texture(b, &format!("{}{}", u.name, names[i]), a, &u.id)?);
    }
    let mat = b.derive_material(
        base,
        &format!("Character_{}", u.name),
        &names.iter().zip(&tex_ids).map(|(n, t)| ((*n).to_owned(), *t)).collect::<Vec<_>>(),
        &[("_UseNormalMapAtlas", 1.0)],
        &["_NORMALMAP", "_MASKSMAP"],
    )?;
    let matrix = placement(u, prefab.root_scale(), prefab.forbid_rotation);
    push_renderer_for(b, prefab, u, "Character", mesh_index, vec![Some(mat)], matrix, 9, binding);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ee(layer: i32, hide: i64, parts: &[i64]) -> Rc<Ee> {
        Rc::new(Ee {
            guid: format!("ee{layer}-{hide}-{parts:?}"),
            name: String::new(),
            layer,
            hide,
            show_lower: false,
            primary_ramps: Vec::new(),
            primary_ramp_count: 0,
            secondary_ramps: Vec::new(),
            special_primary: Vec::new(),
            presets: Vec::new(),
            parts: parts
                .iter()
                .map(|t| Part {
                    ty: *t,
                    prefab: None,
                    material: None,
                    textures: Vec::new(),
                    visible: true,
                    tex0: None,
                    complete: true,
                })
                .collect(),
        })
    }

    #[test]
    fn higher_layers_replace_the_part_of_the_same_type() {
        let mut eq = Equipment::default();
        eq.add(ee(0, 0, &[ty::TORSO, ty::HEAD]), -1, -1);
        eq.add(ee(100, 0, &[ty::TORSO]), -1, -1);
        let (geometry, overlay) = select(&eq);
        let kinds: Vec<(usize, i64)> =
            geometry.iter().map(|(k, _)| (k.ee, eq.ees[k.ee].parts[k.part].ty)).collect();
        // The head stays with entity 0; the torso is entity 1's, taking the slot the first torso left.
        assert_eq!(kinds, vec![(1, ty::TORSO), (0, ty::HEAD)]);
        // Overlays: the torso layer stack lists entity 1's part for the torso type only here
        // (entity 0 has none left to add; lower layers aren't overlaid without `show_lower`).
        assert!(overlay.iter().any(|k| k.ee == 1));
    }

    #[test]
    fn hidden_body_parts_drop_lower_entities_parts() {
        let mut eq = Equipment::default();
        eq.add(ee(0, 0, &[ty::HEAD, ty::HAIR]), -1, -1);
        eq.add(ee(200, ty::HAIR, &[ty::HELMET]), -1, -1);
        let (geometry, _) = select(&eq);
        let types: Vec<i64> = geometry.iter().map(|(k, _)| eq.ees[k.ee].parts[k.part].ty).collect();
        assert!(types.contains(&ty::HEAD) && types.contains(&ty::HELMET));
        assert!(!types.contains(&ty::HAIR), "{types:?}");
    }

    #[test]
    fn dictionary_reuses_the_freed_slot() {
        let a = PartRef { ee: 0, part: 0 };
        let b = PartRef { ee: 1, part: 0 };
        let c = PartRef { ee: 2, part: 0 };
        let mut d = OrderedDict::default();
        d.insert(a, 0);
        d.insert(b, 1);
        d.remove(a);
        d.insert(c, 2);
        assert_eq!(d.iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![c, b]);
    }

    #[test]
    fn ramp_defaults_follow_add_and_set() {
        let mut eq = Equipment::default();
        let mut e = ee(0, 0, &[ty::TORSO]);
        Rc::get_mut(&mut e).unwrap().primary_ramp_count = 1;
        eq.add(e, -1, -1);
        let s = eq.ramps[&0];
        assert_eq!((s.primary, s.secondary), (0, 0));
        eq.set_primary(0, 5);
        eq.set_secondary(0, 7);
        let s = eq.ramps[&0];
        assert_eq!((s.primary, s.secondary), (5, 7));
        // A negative index leaves the choice alone.
        eq.set(0, -1, -1, -1, -1);
        assert_eq!(eq.ramps[&0].primary, 5);
    }

    #[test]
    fn atlas_rectangles_match_the_layout() {
        assert_eq!(map_rect(ty::TORSO), (685.0, 1628.0, 780.0, 420.0));
        assert_eq!(map_rect(ty::HEAD), map_rect(ty::NOSE));
        assert_eq!(map_rect(ty::HAIR), map_rect(ty::HELMET));
        assert_eq!(map_rect(1 << 50), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn lookup_skips_inactive_objects_and_their_subtrees() {
        // 1 -> (2 -> 4, 3); the renderer is on 4 and on 3.
        let kids = |t: i64| match t {
            1 => vec![2, 3],
            2 => vec![4],
            _ => vec![],
        };
        let has = |t: i64| t == 3 || t == 4;
        // All active: depth first reaches 4 (under the first child) before 3.
        assert_eq!(first_active_match(1, &kids, &|_| true, &has), Some(4));
        // 2 inactive: its renderer-bearing child is not entered, so 3 is found.
        assert_eq!(first_active_match(1, &kids, &|t| t != 2, &has), Some(3));
        // The queried object is searched even when inactive: a renderer on an inactive root is found,
        // and an inactive root with none still lets its active descendants be searched.
        let on_root = |t: i64| t == 1;
        assert_eq!(first_active_match(1, &kids, &|t| t != 1, &on_root), Some(1));
        assert_eq!(first_active_match(1, &kids, &|t| t != 1, &has), Some(4));
        // An inactive descendant is skipped even when it is the only match.
        assert_eq!(first_active_match(1, &kids, &|t| t != 3, &|t| t == 3), None);
        // The root itself can be the match.
        assert_eq!(first_active_match(3, &kids, &|_| true, &has), Some(3));
    }

    /// A part of `verts` vertices (positions only, plus a skin of one bone each) bound to `bones`.
    fn triangle_part(sub: &[(u32, u32)], verts: u32, bones: &[&str], bind: &[Mat4]) -> SkinnedPart {
        let positions: Vec<u8> = (0..verts)
            .flat_map(|i| [i as f32, 0.0, 0.0].into_iter().flat_map(f32::to_le_bytes))
            .collect();
        let skin: Vec<u8> = (0..verts)
            .flat_map(|_| {
                [1.0f32, 0.0, 0.0, 0.0]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .chain([0u32, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes))
            })
            .collect();
        let idx: Vec<u8> = (0..verts).flat_map(|i| (i as u16).to_le_bytes()).collect();
        let mut channels = vec![None; 14];
        channels[0] = Some(MeshChannel { stream: 0, offset: 0, format: 0, dimension: 3 });
        channels[12] = Some(MeshChannel { stream: 1, offset: 0, format: 0, dimension: 4 });
        channels[13] = Some(MeshChannel { stream: 1, offset: 16, format: 10, dimension: 4 });
        SkinnedPart {
            mesh: pack_mesh::DecodedMesh {
                name: "part".into(),
                vertex_count: verts,
                channels,
                streams: vec![(positions, 12), (skin, 32)],
                index_u32: false,
                indices: idx,
                submeshes: sub
                    .iter()
                    .map(|&(first, n)| SubMesh {
                        first_index: first,
                        index_count: n,
                        ..Default::default()
                    })
                    .collect(),
                aabb: Default::default(),
            },
            bones: bones.iter().map(|b| (*b).to_owned()).collect(),
            bind: bind.to_vec(),
        }
    }

    /// A rig of bones `a`, `b` and `c` (rig bones 0, 1, 2).
    fn rig3() -> BuiltRig {
        let bone = |n: &str, parent: u32| wr_anim::Bone {
            name: n.into(),
            hash: 0,
            parent,
            rest: wr_anim::Trs::IDENTITY,
        };
        let rig = wr_anim::Rig {
            name: "r".into(),
            bones: vec![bone("a", u32::MAX), bone("b", 0), bone("c", 1)],
            root: Mat4::IDENTITY.to_cols_array(),
            overrides: Vec::new(),
        };
        BuiltRig {
            by_name: [("a", 0u32), ("b", 1), ("c", 2)].map(|(n, i)| (n.to_owned(), i)).into(),
            by_transform: HashMap::new(),
            rig,
        }
    }

    #[test]
    fn merging_keeps_only_each_parts_first_submesh() {
        // Part A: 6 vertices, submeshes (0..3) and (3..6); part B: 3 vertices, one submesh.
        let a = triangle_part(&[(0, 3), (3, 3)], 6, &["a"], &[Mat4::IDENTITY]);
        let b = triangle_part(&[(0, 3)], 3, &["a"], &[Mat4::IDENTITY]);
        let m = merge(&[a, b], "merged", &rig3()).unwrap().mesh;
        // Every vertex of both parts is copied; only A's first and B's triangles are indexed, B's offset by 6.
        assert_eq!(m.vertex_count, 9);
        let idx: Vec<u32> =
            m.indices.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        assert_eq!(idx, vec![0, 1, 2, 6, 7, 8]);
        assert_eq!(m.submeshes.len(), 1);
        assert_eq!(m.submeshes[0].index_count, 6);
    }

    #[test]
    fn merged_bones_are_shared_by_name_and_bind_pose() {
        let shift = Mat4::from_translation(glam::Vec3::new(0.0, 1.0, 0.0));
        // Part A binds a and b; part B binds b again (same bind pose: shared), b once more with
        // another bind pose (a new merged bone) and a bone the rig doesn't have, then c, which the
        // game never reaches (the miss ends the mapping).
        let a = triangle_part(&[(0, 3)], 3, &["a", "b"], &[Mat4::IDENTITY, Mat4::IDENTITY]);
        let b = triangle_part(
            &[(0, 3)],
            3,
            &["b", "b", "nope", "c"],
            &[Mat4::IDENTITY, shift, Mat4::IDENTITY, Mat4::IDENTITY],
        );
        let m = merge(&[a, b], "merged", &rig3()).unwrap();
        assert_eq!(m.bones, vec![0, 1, 1]);
        assert_eq!(m.bind, vec![Mat4::IDENTITY, Mat4::IDENTITY, shift]);
    }

    #[test]
    fn bone_slots_are_remapped_into_the_merged_list() {
        let mut a = triangle_part(&[(0, 3)], 3, &["a"], &[Mat4::IDENTITY]);
        let mut b = triangle_part(&[(0, 3)], 3, &["b", "a"], &[Mat4::IDENTITY, Mat4::IDENTITY]);
        // Part B's vertices use its second bone, "a", which part A already merged as bone 0.
        for v in 0..3 {
            b.mesh.streams[1].0[v * 32 + 16..v * 32 + 20].copy_from_slice(&1u32.to_le_bytes());
        }
        a.mesh.streams[1].0[16..20].copy_from_slice(&0u32.to_le_bytes());
        let m = merge(&[a, b], "merged", &rig3()).unwrap();
        // The skin stream: part B's vertex 0 (merged vertex 3) points at merged bone 0.
        let skin = &m.mesh.streams[1].0;
        assert_eq!(u32::from_le_bytes(skin[3 * 32 + 16..3 * 32 + 20].try_into().unwrap()), 0);
        assert_eq!(m.bones, vec![0, 1]);
    }
}
