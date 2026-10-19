//! Units in the area pack (POC-7 stages 1 and 2): the unit views a save puts in the area, written
//! into the pack as skinned renderers with their rigs and animations.
//!
//! A unit prefab (`<guid>.unit`) is one of
//!
//! - a **plain skinned prefab** (monsters, elementals): `SkinnedMeshRenderer`s under the
//!   `UnitEntityView` root, skinned to the rig that sits in the same prefab;
//! - a **character** (`Character` component): no meshes of its own. The rig is the
//!   `AnimatorPrefab` (a human skeleton in `techart`), the body is composed at run time from
//!   equipment entities ([`character`]), and a **baked character** (`BakedCharacter`) names
//!   its finished meshes and materials.
//!
//! Each unit is a skinned renderer per mesh: the mesh is stored unposed with its skin and the renderer
//! carries the unit's placement (`T * R_y(orientation) * S(root scale)`) and a `SkinBinding` naming the rig
//! bones that pose it, so one mesh serves every unit of a prefab ([`anim`] reads the rig, the clips and the
//! locomotion; the game poses and skins it). A unit whose animation can't be read is baked once in the
//! rig's rest pose instead (stage 1's output) and the pack report says why.

pub mod anim;
pub mod atlas;
pub mod census;
pub mod character;
pub mod image;
pub mod skin;

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};
use serde::Serialize;
use wr_assets::{AssetId, Mesh as PackMesh, Renderer, RendererKind, SkinBinding, VertexStream};
use wr_view::{UnitView, UnitsSpec};

use super::{Builder, SceneIndex, mesh as pack_mesh, skip};
use crate::catalog::Locations;
use crate::unity::serialized::{ObjectInfo, SerializedFile, class_id};
use crate::unity::value::{PPtr, Value};

/// What `add` did, for the pack report.
#[derive(Debug, Default, Serialize)]
pub struct UnitsReport {
    pub spec_units: usize,
    /// Units that got at least one renderer.
    pub placed: usize,
    pub renderers: usize,
    /// Unit id, name, kind and why, for units left out or partly drawn.
    pub left_out: Vec<String>,
    /// Prefab kinds seen: plain, character, baked.
    pub kinds: HashMap<String, usize>,
    /// Units with an animation (a rig, clips and a locomotion), and what was left out or
    /// approximated in the animation data.
    pub animated: usize,
    pub anim_notes: Vec<String>,
}

/// A unit prefab opened for baking: its hierarchy and the `UnitEntityView` root.
pub(super) struct UnitPrefab {
    pub sc: SceneIndex,
    /// `UnitEntityView.ForbidRotation`: the view keeps the prefab's own orientation.
    pub forbid_rotation: bool,
    /// The root's transform (`UnitEntityView`'s GameObject).
    pub root: i64,
}

impl UnitPrefab {
    /// Opens `<guid>.unit` through the location list.
    fn open(b: &Builder<'_>, locations: &Locations, guid: &str) -> Result<Self> {
        let bundle = locations
            .guid_to_bundle
            .get(guid)
            .with_context(|| format!("prefab {guid} is not in the location list"))?;
        let files = b.universe.files_of(bundle)?;
        let file = files
            .iter()
            .find(|f| f.objects.iter().any(|o| o.class_id == class_id::GAME_OBJECT))
            .with_context(|| format!("{bundle}: no scene objects"))?;
        let sc = SceneIndex::load(guid, file)?;
        let mut root = None;
        let mut forbid_rotation = false;
        for o in file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = file.read(o)?;
            if b.universe.script_of(file, &v)?.is_some_and(|s| s.class == "UnitEntityView") {
                let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                root = sc.go_transform.get(&go).copied();
                forbid_rotation = v.get("ForbidRotation").and_then(Value::as_bool).unwrap_or(false);
                break;
            }
        }
        let root = root.with_context(|| format!("{guid}: no UnitEntityView"))?;
        Ok(Self { sc, forbid_rotation, root })
    }

    /// `t`'s matrix relative to the root (the root itself is the identity).
    pub fn rel(&self, t: i64) -> Mat4 {
        let m = |t: i64| {
            self.sc.worlds.get(&t).map_or(Mat4::IDENTITY, |w| Mat4::from_cols_array(&w.matrix))
        };
        m(self.root).inverse() * m(t)
    }

    /// Whether transform `t` is the root or below it.
    pub fn under_root(&self, t: i64) -> bool {
        let mut cur = Some(t);
        for _ in 0..4096 {
            match cur {
                Some(c) if c == self.root => return true,
                Some(c) => cur = self.sc.parents.get(&c).copied().flatten(),
                None => return false,
            }
        }
        false
    }

    /// The root's local scale (`UnitEntityView.m_OriginalScale`).
    pub fn root_scale(&self) -> Vec3 {
        self.sc.worlds.get(&self.root).map_or(Vec3::ONE, |w| Vec3::from_array(w.lossy_scale))
    }

    /// The `Character` has a `BakedCharacter` asset.
    fn baked_character(&self, b: &Builder<'_>) -> Result<bool> {
        for o in self.sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = self.sc.file.read(o)?;
            if b.universe.script_of(self.sc.file, &v)?.is_some_and(|s| s.class == "Character") {
                return Ok(v
                    .get("BakedCharacter")
                    .and_then(Value::as_pptr)
                    .is_some_and(|p| !p.is_null()));
            }
        }
        Ok(false)
    }

    /// The `Skeleton` asset of the prefab's `Character` (per-bone scale and offset), if it has both.
    /// A baked character keeps its skeleton: `SkeletonUpdateService` updates every registered
    /// character's bones each late update, baked or not.
    fn character_skeleton(&self, b: &Builder<'_>) -> Result<Option<Value>> {
        for o in self.sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = self.sc.file.read(o)?;
            if !b.universe.script_of(self.sc.file, &v)?.is_some_and(|s| s.class == "Character") {
                continue;
            }
            let Some(p) = v.get("m_Skeleton").and_then(Value::as_pptr).filter(|p| !p.is_null())
            else {
                return Ok(None);
            };
            return b.universe.resolve(self.sc.file, p)?.map(|o| o.read()).transpose();
        }
        Ok(None)
    }

    fn has_script(&self, b: &Builder<'_>, class: &str) -> Result<bool> {
        for o in self.sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = self.sc.file.read(o)?;
            if b.universe.script_of(self.sc.file, &v)?.is_some_and(|s| s.class == class) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The unit's placement in the world: `T(position) * R_y(orientation) * S(root scale)`
/// (`UnitEntityData.SetViewPosition`; a view with `ForbidRotation` keeps the identity rotation).
/// The size scale of a unit whose size differs from its original is not applied.
#[must_use]
pub fn placement(u: &UnitView, root_scale: Vec3, forbid_rotation: bool) -> Mat4 {
    let rotation = if forbid_rotation {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_y(u.orientation.to_radians())
    };
    Mat4::from_scale_rotation_translation(root_scale, rotation, Vec3::from_array(u.position))
}

/// A pointer in the prefab's file resolved to its transform's id, if it points at one there.
fn transform_of(p: PPtr) -> Option<i64> {
    (p.file_id == 0 && !p.is_null()).then_some(p.path_id)
}

/// Adds every unit of the spec; a unit that can't be drawn is reported, never guessed.
pub(super) fn add(
    b: &mut Builder<'_>,
    locations: &Locations,
    spec: &UnitsSpec,
    atlas_size: usize,
) -> Result<UnitsReport> {
    let mut rep = UnitsReport { spec_units: spec.units.len(), ..Default::default() };
    let mut prefabs: HashMap<String, std::rc::Rc<UnitPrefab>> = HashMap::new();
    let mut baked_meshes = BakedMeshes::new();
    let mut characters = character::Cache::new(atlas_size);
    let mut anim = anim::AnimCache::default();
    for u in &spec.units {
        let prefab = match prefabs.get(&u.prefab) {
            Some(p) => p.clone(),
            None => match UnitPrefab::open(b, locations, &u.prefab) {
                Ok(p) => {
                    let p = std::rc::Rc::new(p);
                    prefabs.insert(u.prefab.clone(), p.clone());
                    p
                }
                Err(e) => {
                    rep.left_out.push(format!("{} {}: {e:#}", u.id, u.name));
                    continue;
                }
            },
        };
        // A baked character's prefab already holds its finished skinned renderer under the
        // animator (`Character.LoadBakedCharacter` takes it): drawn like a plain prefab.
        let character = prefab.has_script(b, "Character")? && !prefab.baked_character(b)?;
        let baked = !character && prefab.has_script(b, "Character")?;
        let kind = if character {
            "character"
        } else if baked {
            "baked"
        } else {
            "plain"
        };
        *rep.kinds.entry(kind.to_owned()).or_default() += 1;
        let before = b.meta.renderers.len();
        let r = if character {
            character::add(b, locations, &mut characters, &mut anim, &prefab, u)
        } else {
            plain(b, &mut anim, &prefab, u, &mut baked_meshes)
        };
        match r {
            Ok(()) if b.meta.renderers.len() > before => {
                rep.placed += 1;
                rep.renderers += b.meta.renderers.len() - before;
            }
            Ok(()) => rep.left_out.push(format!("{} {}: no renderer in scope", u.id, u.name)),
            Err(e) => rep.left_out.push(format!("{} {} ({kind}): {e:#}", u.id, u.name)),
        }
    }
    rep.animated = b.meta.animations.len();
    rep.anim_notes = std::mem::take(&mut anim.notes);
    Ok(rep)
}

/// A mesh baked once per prefab renderer: its pack index and, for a skinned one, the rig bone and
/// bind pose of each of its bones.
#[derive(Clone)]
struct BakedMesh {
    mesh: u32,
    bones: Vec<u32>,
    bind: Vec<[f32; 16]>,
}

type BakedMeshes = HashMap<(String, i64, bool), Option<BakedMesh>>;

/// The animation of a plain unit prefab: the rig of its `Animator` and the unit's locomotion.
/// The rig's rest pose must reproduce the prefab's own transforms, or the unit isn't animated.
fn plain_animation(
    b: &mut Builder<'_>,
    anim: &mut anim::AnimCache,
    prefab: &UnitPrefab,
    u: &UnitView,
) -> Result<(u32, std::rc::Rc<anim::BuiltRig>)> {
    let (animator, set) = anim::plain_animator(b, anim, prefab)?;
    let sc = &prefab.sc;
    let (loco, clips) = anim.locomotion(b, set)?;
    let t = anim::bind_root(b, sc, animator, &clips);
    let parent = match sc.parents.get(&t).copied().flatten() {
        Some(p) if prefab.under_root(p) => prefab.rel(p),
        _ => Mat4::IDENTITY,
    };
    let skeleton = prefab.character_skeleton(b)?;
    let key = format!("{}#{t}#{}", u.prefab, skeleton.is_some());
    let name = sc.transform_go.get(&t).and_then(|g| sc.gos.get(g)).map_or("", |g| g.0.as_str());
    let (rig_index, rig) = anim.rig(b, &key, || {
        anim::build_rig(sc, t, name, parent, t == prefab.root, skeleton.as_ref())
    })?;
    // The rig at rest (before any skeleton override) equals the prefab's transforms.
    let mut worlds = Vec::new();
    rig.rig.world_matrices(&rig.rig.rest_pose(), &mut worlds);
    for (tr, &bone) in &rig.by_transform {
        let want = prefab.rel(*tr).to_cols_array();
        let got = worlds[bone as usize].to_cols_array();
        let err = want.iter().zip(&got).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        anyhow::ensure!(err < 1e-3, "rig bone {bone} rests {err} away from the prefab's transform");
    }
    let animation = anim.animation(b, &u.id, rig_index, loco, clips)?;
    Ok((animation, rig))
}

/// A plain skinned prefab: each `SkinnedMeshRenderer` under the root. With an animation the mesh
/// is stored unposed with its skin and the renderer names its bones; without one, in the
/// prefab's rest pose.
fn plain(
    b: &mut Builder<'_>,
    anim: &mut anim::AnimCache,
    prefab: &UnitPrefab,
    u: &UnitView,
    baked: &mut BakedMeshes,
) -> Result<()> {
    let sc = &prefab.sc;
    let matrix = placement(u, prefab.root_scale(), prefab.forbid_rotation);
    let animated = match plain_animation(b, anim, prefab, u) {
        Ok(a) => Some(a),
        Err(e) => {
            anim.notes
                .push(format!("{} {}: drawn in the rest pose, not animated: {e:#}", u.id, u.name));
            None
        }
    };
    let mut any = false;
    for o in sc.file.objects.iter().filter(|o| o.class_id == class_id::SKINNED_MESH_RENDERER) {
        let v = sc.file.read(o)?;
        let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
        // Prefabs can hold other roots (a mount's rig and armour variants): only the unit's own.
        let under = sc.go_transform.get(&go).is_some_and(|t| prefab.under_root(*t));
        if !under
            || !sc.active_in_hierarchy(go)
            || !v.get("m_Enabled").and_then(Value::as_bool).unwrap_or(true)
        {
            continue;
        }
        let Some(mp) = v.get("m_Mesh").and_then(Value::as_pptr).filter(|p| !p.is_null()) else {
            continue;
        };
        let bones: Vec<Option<i64>> = v
            .get("m_Bones")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|x| x.as_pptr().and_then(transform_of))
            .collect();
        let key = (u.prefab.clone(), o.path_id, animated.is_some());
        let mesh = match baked.get(&key) {
            Some(m) => m.clone(),
            None => {
                let hint = format!("{}/{}", u.prefab, o.path_id);
                let m = match &animated {
                    Some((animation, rig)) => {
                        bake_skinned(b, prefab, sc.file, mp, &bones, rig, *animation, &hint)?
                    }
                    None => bake_mesh(b, prefab, sc.file, mp, &bones, &hint)?
                        .map(|mesh| BakedMesh { mesh, bones: Vec::new(), bind: Vec::new() }),
                };
                baked.insert(key, m.clone());
                m
            }
        };
        let Some(mesh) = mesh else { continue };
        let mut rkw = Vec::new();
        let _ = &mut rkw;
        let mut mats = Vec::new();
        let mut any_mat = false;
        for mp in v.get("m_Materials").and_then(Value::as_array).unwrap_or_default() {
            let m = match mp.as_pptr().filter(|p| !p.is_null()) {
                Some(p) => b.material(sc.file, p, &rkw)?,
                None => None,
            };
            any_mat |= m.is_some();
            mats.push(m);
        }
        if !any_mat {
            skip(&mut b.report, "unit SkinnedMeshRenderer with no in-scope material");
            continue;
        }
        any = true;
        let skin = animated.as_ref().map(|(animation, _)| SkinBinding {
            animation: *animation,
            bones: mesh.bones.clone(),
            bind_poses: mesh.bind.clone(),
        });
        push_renderer(b, sc, o, &v, go, mesh.mesh, mats, matrix, u, skin);
    }
    if !any {
        bail!("no drawable SkinnedMeshRenderer");
    }
    Ok(())
}

/// Decodes the mesh `mp` points at and stores it unposed with its skin. The renderer's `bones`
/// (transform ids of the prefab) are matched to the rig's bones; the mesh's bounds cover the
/// unit's idle and walk poses.
#[allow(clippy::too_many_arguments)]
fn bake_skinned(
    b: &mut Builder<'_>,
    prefab: &UnitPrefab,
    from: &SerializedFile<'_>,
    mp: PPtr,
    bones: &[Option<i64>],
    rig: &anim::BuiltRig,
    animation: u32,
    id_hint: &str,
) -> Result<Option<BakedMesh>> {
    let _ = prefab;
    let Some(mo) = b.universe.resolve(from, mp)? else { return Ok(None) };
    let mv = mo.read()?;
    let mut decoded = match pack_mesh::decode(b.universe, &mv) {
        Ok(d) => d,
        Err(e) => {
            skip(&mut b.report, format!("unit mesh: {e:#}"));
            return Ok(None);
        }
    };
    let bind = bindposes(&mv)?;
    anyhow::ensure!(
        bind.len() <= bones.len(),
        "{}: {} bind poses for {} bones",
        decoded.name,
        bind.len(),
        bones.len()
    );
    let mut rig_bones = Vec::with_capacity(bind.len());
    for (i, bone) in bones.iter().take(bind.len()).enumerate() {
        let t = bone.with_context(|| {
            format!("{}: bone {i} is not a transform of the prefab", decoded.name)
        })?;
        let r = rig
            .by_transform
            .get(&t)
            .with_context(|| format!("{}: bone {i} is not in the rig", decoded.name))?;
        rig_bones.push(*r);
    }
    let skin_blob = skin::skin_blob(&decoded)?;
    let skins: Vec<Vec<Mat4>> = anim::bound_poses(b, &b.meta.animations[animation as usize], 8)
        .iter()
        .map(|w| rig_bones.iter().zip(&bind).map(|(rb, bind)| w[*rb as usize] * *bind).collect())
        .collect();
    skin::animated_bounds(&mut decoded, &skins)?;
    let mesh = intern_baked(
        b,
        decoded,
        AssetId { file: format!("skinned:{id_hint}"), path_id: mo.info.path_id },
        Some(&skin_blob),
    )?;
    Ok(Some(BakedMesh {
        mesh,
        bones: rig_bones,
        bind: bind.iter().map(Mat4::to_cols_array).collect(),
    }))
}

#[allow(clippy::too_many_arguments)]
fn push_renderer(
    b: &mut Builder<'_>,
    sc: &SceneIndex,
    o: &ObjectInfo,
    v: &Value,
    go: i64,
    mesh: u32,
    materials: Vec<Option<u32>>,
    matrix: Mat4,
    u: &UnitView,
    skin: Option<SkinBinding>,
) {
    b.meta.renderers.push(Renderer {
        renderer: AssetId { file: sc.file.name.clone(), path_id: o.path_id },
        game_object: AssetId { file: sc.file.name.clone(), path_id: go },
        scene: format!("unit:{}", u.id),
        path: format!("unit:{}/{}", u.id, sc.path(go)),
        kind: RendererKind::Skinned,
        enabled: true,
        active: true,
        layer: sc.gos.get(&go).map_or(0, |g| g.2),
        matrix: matrix.to_cols_array(),
        mesh,
        materials,
        lightmap_index: 65535,
        lightmap_st: [1.0, 1.0, 0.0, 0.0],
        cast_shadows: v.i64_at("m_CastShadows").unwrap_or(1) as u8,
        receive_shadows: v.i64_at("m_ReceiveShadows").unwrap_or(1) != 0,
        light_probe_usage: v.i64_at("m_LightProbeUsage").unwrap_or(1) as u8,
        reflection_probe_usage: v.i64_at("m_ReflectionProbeUsage").unwrap_or(1) as u8,
        static_batch: (0, 0),
        rendering_layer_mask: u32::try_from(
            v.i64_at("m_RenderingLayerMask").unwrap_or(1) & 0xFFFF_FFFF,
        )
        .unwrap_or(1),
        property_block: None,
        skin,
    });
}

/// Decodes the mesh `mp` points at, poses it with `bones` (transform ids of the prefab, in the
/// renderer's bone order) in the prefab's rest pose, and interns the result.
fn bake_mesh(
    b: &mut Builder<'_>,
    prefab: &UnitPrefab,
    from: &SerializedFile<'_>,
    mp: PPtr,
    bones: &[Option<i64>],
    id_hint: &str,
) -> Result<Option<u32>> {
    let Some(mo) = b.universe.resolve(from, mp)? else { return Ok(None) };
    let mv = mo.read()?;
    let mut decoded = match pack_mesh::decode(b.universe, &mv) {
        Ok(d) => d,
        Err(e) => {
            skip(&mut b.report, format!("unit mesh: {e:#}"));
            return Ok(None);
        }
    };
    let bind = bindposes(&mv)?;
    // A renderer may list more bones than the mesh binds; the vertices index the first ones.
    anyhow::ensure!(
        bind.len() <= bones.len(),
        "{}: {} bind poses for {} bones",
        decoded.name,
        bind.len(),
        bones.len()
    );
    let mut skin_mats = Vec::with_capacity(bind.len());
    for (i, bone) in bones.iter().take(bind.len()).enumerate() {
        let Some(t) = bone else {
            bail!("{}: bone {i} is not a transform of the prefab", decoded.name)
        };
        anyhow::ensure!(
            prefab.sc.worlds.contains_key(t),
            "{}: bone {i} has no transform",
            decoded.name
        );
        skin_mats.push(prefab.rel(*t) * bind[i]);
    }
    skin::pose(&mut decoded, &skin_mats)?;
    skin::refresh_bounds(&mut decoded)?;
    Ok(Some(intern_baked(
        b,
        decoded,
        AssetId { file: format!("baked:{id_hint}"), path_id: mo.info.path_id },
        None,
    )?))
}

/// `Mesh.m_BindPose` (serialized as `Matrix4x4f` with `e{row}{col}`) as column-major matrices.
pub fn bindposes(mesh: &Value) -> Result<Vec<Mat4>> {
    mesh.get("m_BindPose")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let mut cols = [0f32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    cols[c * 4 + r] = m
                        .f32_at(&format!("e{r}{c}"))
                        .with_context(|| format!("bind pose e{r}{c}"))?;
                }
            }
            Ok(Mat4::from_cols_array(&cols))
        })
        .collect()
}

/// Writes a decoded (and already posed) mesh into the pack.
fn intern_baked(
    b: &mut Builder<'_>,
    d: pack_mesh::DecodedMesh,
    id: AssetId,
    skin: Option<&[u8]>,
) -> Result<u32> {
    let mut streams = Vec::new();
    for (bytes, stride) in &d.streams {
        streams.push(VertexStream { data: b.w.blob(bytes)?, stride: *stride });
        b.report.mesh_bytes += bytes.len() as u64;
    }
    let indices = b.w.blob(&d.indices)?;
    b.report.mesh_bytes += d.indices.len() as u64;
    let skin = skin.map(|s| b.w.blob(s)).transpose()?;
    b.report.mesh_bytes += skin.map_or(0, |s| s.len);
    b.meta.meshes.push(PackMesh {
        id,
        name: d.name,
        vertex_count: d.vertex_count,
        channels: d.channels,
        streams,
        index_u32: d.index_u32,
        indices,
        submeshes: d.submeshes,
        aabb: d.aabb,
        skin,
    });
    Ok(u32::try_from(b.meta.meshes.len() - 1)?)
}

/// Pushes a renderer for a composed unit mesh (no object of its own in the prefab).
#[allow(clippy::too_many_arguments)]
pub(super) fn push_renderer_for(
    b: &mut Builder<'_>,
    prefab: &UnitPrefab,
    u: &UnitView,
    label: &str,
    mesh: u32,
    materials: Vec<Option<u32>>,
    matrix: Mat4,
    layer: u32,
    skin: Option<SkinBinding>,
) {
    b.meta.renderers.push(Renderer {
        renderer: AssetId { file: prefab.sc.file.name.clone(), path_id: 0 },
        game_object: AssetId { file: prefab.sc.file.name.clone(), path_id: 0 },
        scene: format!("unit:{}", u.id),
        path: format!("unit:{}/{label}", u.id),
        kind: RendererKind::Skinned,
        enabled: true,
        active: true,
        layer,
        matrix: matrix.to_cols_array(),
        mesh,
        materials,
        lightmap_index: 65535,
        lightmap_st: [1.0, 1.0, 0.0, 0.0],
        cast_shadows: 1,
        receive_shadows: true,
        light_probe_usage: 1,
        reflection_probe_usage: 1,
        static_batch: (0, 0),
        rendering_layer_mask: u32::MAX,
        property_block: None,
        skin,
    });
}

impl Builder<'_> {
    /// A copy of material `base` with textures, floats and keywords set, its shader variants
    /// selected for the new keyword set (a `new Material(source)` the game then edits).
    pub(super) fn derive_material(
        &mut self,
        base: u32,
        name: &str,
        textures: &[(String, u32)],
        floats: &[(&str, f32)],
        keywords: &[&str],
    ) -> Result<u32> {
        let mut m = self.meta.materials[base as usize].clone();
        m.id =
            AssetId { file: format!("derived:{name}"), path_id: self.meta.materials.len() as i64 };
        m.name = name.to_owned();
        m.variants.clear();
        m.casters.clear();
        for (n, t) in textures {
            match m.textures.iter_mut().find(|p| &p.name == n) {
                Some(p) => p.texture = Some(*t),
                None => m.textures.push(wr_assets::TexProp {
                    name: n.clone(),
                    texture: Some(*t),
                    scale: [1.0, 1.0],
                    offset: [0.0, 0.0],
                }),
            }
        }
        for (n, v) in floats {
            match m.floats.iter_mut().find(|(k, _)| k == n) {
                Some(f) => f.1 = *v,
                None => m.floats.push(((*n).to_owned(), *v)),
            }
        }
        for k in keywords {
            if !m.keywords.iter().any(|x| x == k) {
                m.keywords.push((*k).to_owned());
            }
        }
        self.meta.materials.push(m);
        let mi = u32::try_from(self.meta.materials.len() - 1)?;
        self.select_casters(mi)?;
        self.select_variant(mi, &[])?;
        Ok(mi)
    }
}
