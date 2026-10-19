//! Units in the area pack (POC-7 stage 1): the unit views a save puts in the area, baked into
//! the pack as renderers in their bind pose.
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
//! Stage 1 poses every skinned mesh once on the CPU, in the rig's rest pose, in the unit's
//! root space; the renderer carries the unit's placement (`T * R_y(orientation) * S(root
//! scale)`), so one baked mesh serves every unit of a prefab. The pack keeps the skin
//! channels, so a later stage can pose from the same streams.

pub mod skin;

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};
use serde::Serialize;
use wr_assets::{AssetId, Mesh as PackMesh, Renderer, RendererKind, VertexStream};
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
}

/// A unit prefab opened for baking: its hierarchy and the `UnitEntityView` root.
pub(super) struct UnitPrefab {
    pub sc: SceneIndex,
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
        for o in file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = file.read(o)?;
            if b.universe.script_of(file, &v)?.is_some_and(|s| s.class == "UnitEntityView") {
                let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                root = sc.go_transform.get(&go).copied();
                break;
            }
        }
        let root = root.with_context(|| format!("{guid}: no UnitEntityView"))?;
        Ok(Self { sc, root })
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

/// The unit's placement in the world: `T(position) * R_y(orientation) * S(root scale)`.
#[must_use]
pub fn placement(u: &UnitView, root_scale: Vec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        root_scale,
        Quat::from_rotation_y(u.orientation.to_radians()),
        Vec3::from_array(u.position),
    )
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
) -> Result<UnitsReport> {
    let mut rep = UnitsReport { spec_units: spec.units.len(), ..Default::default() };
    let mut prefabs: HashMap<String, std::rc::Rc<UnitPrefab>> = HashMap::new();
    let mut baked_meshes: HashMap<(String, i64), Option<u32>> = HashMap::new();
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
        let character = prefab.has_script(b, "Character")?;
        let kind = if character { "character" } else { "plain" };
        *rep.kinds.entry(kind.to_owned()).or_default() += 1;
        let before = b.meta.renderers.len();
        let r = if character {
            Err(anyhow::anyhow!("Character prefabs: not yet"))
        } else {
            plain(b, &prefab, u, &mut baked_meshes)
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
    Ok(rep)
}

/// A plain skinned prefab: each `SkinnedMeshRenderer` under the root, posed in the prefab's
/// rest pose.
fn plain(
    b: &mut Builder<'_>,
    prefab: &UnitPrefab,
    u: &UnitView,
    baked: &mut HashMap<(String, i64), Option<u32>>,
) -> Result<()> {
    let sc = &prefab.sc;
    let matrix = placement(u, prefab.root_scale());
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
        let key = (u.prefab.clone(), o.path_id);
        let mesh = match baked.get(&key) {
            Some(m) => *m,
            None => {
                let m = bake_mesh(
                    b,
                    prefab,
                    sc.file,
                    mp,
                    &bones,
                    &format!("{}/{}", u.prefab, o.path_id),
                )?;
                baked.insert(key, m);
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
        push_renderer(b, sc, o, &v, go, mesh, mats, matrix, u);
    }
    if !any {
        bail!("no drawable SkinnedMeshRenderer");
    }
    Ok(())
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
    Ok(Some(intern_baked(
        b,
        decoded,
        AssetId { file: format!("baked:{id_hint}"), path_id: mo.info.path_id },
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
fn intern_baked(b: &mut Builder<'_>, d: pack_mesh::DecodedMesh, id: AssetId) -> Result<u32> {
    let mut streams = Vec::new();
    for (bytes, stride) in &d.streams {
        streams.push(VertexStream { data: b.w.blob(bytes)?, stride: *stride });
        b.report.mesh_bytes += bytes.len() as u64;
    }
    let indices = b.w.blob(&d.indices)?;
    b.report.mesh_bytes += d.indices.len() as u64;
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
    });
    Ok(u32::try_from(b.meta.meshes.len() - 1)?)
}
