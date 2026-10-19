//! `wr-import pack`: builds an area's runtime asset pack (`wr_assets` format) from its
//! scene bundles: static mesh renderers with their meshes, materials (shader, keywords,
//! properties) and textures (shipped BC blocks unchanged), the Owlcat shader variants
//! those materials select (MSL as shipped, compiled offline to metallibs), the render
//! pipeline's own programs, lights, lightmaps and each scene's render settings. Every
//! object keeps its original identity (`AssetId`).
//!
//! POC-6 first-light scope: `MeshRenderer`s whose materials use `Owlcat/Lit` in the
//! opaque range (queue ≤ 2500), drawn through the G-buffer pass. Everything else is
//! counted in `PackMeta::skipped` with the reason.

pub mod mesh;
pub mod metal;
pub mod msl;
pub mod shader;
pub mod tex;
pub mod units;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use glam::Mat4;
use serde::Serialize;
use wr_assets::{
    AssetId, Light, Lightmap, Material, MaterialVariant, Mesh as PackMesh, OwlcatLight, PackMeta,
    PackWriter, PipelineProgram, Program, Renderer, RendererKind, SceneSettings, Shader, Stage,
    TexProp, VertexStream,
};

use crate::area::{ObjRef, Universe};
use crate::transform::{self, Node, Trs};
use crate::unity::serialized::{SerializedFile, class_id};
use crate::unity::value::{PPtr, Value};

const CLASS_RENDER_SETTINGS: i32 = 104;
const CLASS_LIGHTMAP_SETTINGS: i32 = 157;
const CLASS_TEXTURE_2D_ARRAY: i32 = 187;
const CLASS_COMPUTE_SHADER: i32 = 72;

/// Shaders whose materials go into the G-buffer pass in this first-light pack.
const GBUFFER_SHADERS: &[&str] = &["Owlcat/Lit", "Owlcat/Unlit", "Owlcat/Terrain"];

/// Pipeline shaders (in `utility_shaders`) and the roles we take from them.
const PIPELINE_SHADERS: &[&str] =
    &["Hidden/Owlcat/DeferredLighting", "Hidden/Owlcat/DeferredReflections", "Hidden/Owlcat/Fog"];

/// Import statistics written next to the pack.
#[derive(Debug, Default, Serialize)]
pub struct PackReport {
    pub area: String,
    pub pack: String,
    pub pack_bytes: u64,
    pub renderers: usize,
    pub meshes: usize,
    pub materials: usize,
    pub textures: usize,
    pub texture_bytes: u64,
    pub mesh_bytes: u64,
    pub programs: usize,
    pub metallibs: usize,
    pub metallib_failures: Vec<String>,
    pub lights: usize,
    pub lights_realtime: usize,
    pub variants_exact: usize,
    pub variants_nearest: usize,
    pub shadowmask_enabled: bool,
    pub transcoded_textures: Vec<String>,
    pub skipped: BTreeMap<String, u32>,
    pub timings_ms: BTreeMap<String, u128>,
    /// The unit views baked in (`--units`).
    pub units: Option<units::UnitsReport>,
}

/// One scene file's objects, indexed for the walk.
struct SceneIndex {
    name: String,
    file: &'static SerializedFile<'static>,
    gos: HashMap<i64, (String, bool, u32, Vec<i64>)>,
    go_transform: HashMap<i64, i64>,
    transform_go: HashMap<i64, i64>,
    parents: HashMap<i64, Option<i64>>,
    worlds: HashMap<i64, transform::World>,
}

impl SceneIndex {
    fn load(name: &str, file: &'static SerializedFile<'static>) -> Result<Self> {
        let mut gos = HashMap::new();
        let mut go_transform = HashMap::new();
        let mut parents = HashMap::new();
        let mut nodes: HashMap<i64, Node<i64>> = HashMap::new();
        for o in &file.objects {
            match o.class_id {
                class_id::GAME_OBJECT => {
                    let v = file.read(o)?;
                    let comps = v
                        .get("m_Component")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|c| c.get("component").and_then(Value::as_pptr))
                        .filter(|p| p.file_id == 0)
                        .map(|p| p.path_id)
                        .collect();
                    gos.insert(
                        o.path_id,
                        (
                            v.str_at("m_Name").unwrap_or_default().to_owned(),
                            v.get("m_IsActive").and_then(Value::as_bool).unwrap_or(true),
                            u32::try_from(v.i64_at("m_Layer").unwrap_or(0)).unwrap_or(0),
                            comps,
                        ),
                    );
                }
                class_id::TRANSFORM | class_id::RECT_TRANSFORM => {
                    let v = file.read(o)?;
                    let go =
                        v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                    let parent = v
                        .get("m_Father")
                        .and_then(Value::as_pptr)
                        .filter(|p| !p.is_null())
                        .map(|p| p.path_id);
                    let local = Trs {
                        position: v
                            .get("m_LocalPosition")
                            .and_then(Value::vec3)
                            .unwrap_or([0.0; 3])
                            .into(),
                        rotation: glam::Quat::from_array(
                            v.get("m_LocalRotation")
                                .and_then(Value::vec4)
                                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
                        ),
                        scale: v
                            .get("m_LocalScale")
                            .and_then(Value::vec3)
                            .unwrap_or([1.0; 3])
                            .into(),
                    };
                    nodes.insert(o.path_id, Node { local, parent });
                    parents.insert(o.path_id, parent);
                    go_transform.insert(go, o.path_id);
                }
                _ => {}
            }
        }
        let (worlds, _errors) = transform::world_transforms(&nodes);
        let transform_go = go_transform.iter().map(|(g, t)| (*t, *g)).collect();
        Ok(Self { name: name.to_owned(), file, gos, go_transform, transform_go, parents, worlds })
    }

    fn transform_go(&self, t: i64) -> Option<i64> {
        self.transform_go.get(&t).copied()
    }

    fn active_in_hierarchy(&self, go: i64) -> bool {
        let mut t = self.go_transform.get(&go).copied();
        let mut guard = 0;
        while let Some(tr) = t {
            guard += 1;
            if guard > 4096 {
                return false;
            }
            let Some(g) = self.transform_go(tr) else { return false };
            if !self.gos.get(&g).is_some_and(|x| x.1) {
                return false;
            }
            t = self.parents.get(&tr).copied().flatten();
        }
        true
    }

    fn path(&self, go: i64) -> String {
        let mut names = Vec::new();
        let mut t = self.go_transform.get(&go).copied();
        let mut guard = 0;
        while let Some(tr) = t {
            guard += 1;
            if guard > 4096 {
                break;
            }
            if let Some(g) = self.transform_go(tr) {
                names.push(self.gos.get(&g).map_or("?", |x| x.0.as_str()).to_owned());
            }
            t = self.parents.get(&tr).copied().flatten();
        }
        names.reverse();
        names.join("/")
    }

    fn matrix(&self, go: i64) -> [f32; 16] {
        self.go_transform
            .get(&go)
            .and_then(|t| self.worlds.get(t))
            .map_or(Mat4::IDENTITY.to_cols_array(), |w| w.matrix)
    }

    /// Components of a GameObject with a class id.
    fn components(
        &self,
        go: i64,
        class: i32,
    ) -> Vec<&'static crate::unity::serialized::ObjectInfo> {
        let Some((_, _, _, comps)) = self.gos.get(&go) else { return Vec::new() };
        comps.iter().filter_map(|c| self.file.object(*c)).filter(|o| o.class_id == class).collect()
    }
}

fn asset_id(o: &ObjRef) -> AssetId {
    AssetId { file: o.file.name.clone(), path_id: o.info.path_id }
}

/// Builder state: interned objects and the pack being written.
struct Builder<'u> {
    universe: &'u Universe,
    w: PackWriter,
    meta: PackMeta,
    report: PackReport,
    textures: HashMap<AssetId, Option<u32>>,
    meshes: HashMap<AssetId, Option<u32>>,
    materials: HashMap<AssetId, Option<u32>>,
    shaders: HashMap<AssetId, (u32, shader::DecodedShader)>,
    programs: HashMap<(String, usize, u8), u32>,
    /// Material index → pass variants already selected, keyed by renderer keywords.
    selected: HashMap<(u32, Vec<String>), ()>,
    msl_sources: HashMap<u64, String>,
    global_keywords: BTreeSet<String>,
}

fn skip(report: &mut PackReport, why: impl Into<String>) {
    *report.skipped.entry(why.into()).or_default() += 1;
}

impl Builder<'_> {
    fn texture(&mut self, from: &SerializedFile<'_>, p: PPtr) -> Result<Option<u32>> {
        let Some(o) = self.universe.resolve(from, p)? else { return Ok(None) };
        let id = asset_id(&o);
        if let Some(&t) = self.textures.get(&id) {
            return Ok(t);
        }
        let class = match o.info.class_id {
            class_id::TEXTURE_2D => "Texture2D",
            class_id::CUBEMAP => "Cubemap",
            CLASS_TEXTURE_2D_ARRAY => "Texture2DArray",
            other => {
                skip(&mut self.report, format!("texture class {other}"));
                self.textures.insert(id, None);
                return Ok(None);
            }
        };
        let v = o.read()?;
        let r = match tex::convert(self.universe, class, &v) {
            Ok((mut t, data)) => {
                t.id = id.clone();
                t.data = self.w.blob(&data)?;
                self.report.texture_bytes += data.len() as u64;
                if let Some(why) = &t.transcoded {
                    self.report.transcoded_textures.push(format!("{}: {why}", t.name));
                }
                self.meta.textures.push(t);
                Some(u32::try_from(self.meta.textures.len() - 1)?)
            }
            Err(e) => {
                skip(&mut self.report, format!("texture: {e:#}"));
                None
            }
        };
        self.textures.insert(id, r);
        Ok(r)
    }

    fn mesh(&mut self, from: &SerializedFile<'_>, p: PPtr) -> Result<Option<u32>> {
        let Some(o) = self.universe.resolve(from, p)? else { return Ok(None) };
        let id = asset_id(&o);
        if let Some(&m) = self.meshes.get(&id) {
            return Ok(m);
        }
        let v = o.read()?;
        let r = match mesh::decode(self.universe, &v) {
            Ok(d) => {
                let mut streams = Vec::new();
                for (bytes, stride) in &d.streams {
                    streams.push(VertexStream { data: self.w.blob(bytes)?, stride: *stride });
                    self.report.mesh_bytes += bytes.len() as u64;
                }
                let indices = self.w.blob(&d.indices)?;
                self.report.mesh_bytes += d.indices.len() as u64;
                self.meta.meshes.push(PackMesh {
                    id: id.clone(),
                    name: d.name,
                    vertex_count: d.vertex_count,
                    channels: d.channels,
                    streams,
                    index_u32: d.index_u32,
                    indices,
                    submeshes: d.submeshes,
                    aabb: d.aabb,
                });
                Some(u32::try_from(self.meta.meshes.len() - 1)?)
            }
            Err(e) => {
                skip(&mut self.report, format!("mesh: {e:#}"));
                None
            }
        };
        self.meshes.insert(id, r);
        Ok(r)
    }

    /// Decodes (once) the shader an object points at; returns its pack index.
    fn shader(&mut self, from: &SerializedFile<'_>, p: PPtr) -> Result<Option<(u32, String)>> {
        let Some(o) = self.universe.resolve(from, p)? else { return Ok(None) };
        let id = asset_id(&o);
        if let Some((i, s)) = self.shaders.get(&id) {
            return Ok(Some((*i, s.name.clone())));
        }
        let v = o.read()?;
        let d = shader::DecodedShader::decode(&v)?;
        let i = u32::try_from(self.meta.shaders.len())?;
        self.meta.shaders.push(Shader {
            id: id.clone(),
            name: d.name.clone(),
            passes: d.passes.iter().map(|p| p.info.clone()).collect(),
            properties: d.properties.clone(),
        });
        let name = d.name.clone();
        self.shaders.insert(id, (i, d));
        Ok(Some((i, name)))
    }

    fn program(
        &mut self,
        shader_id: &AssetId,
        pass: usize,
        stage: Stage,
        blob: usize,
    ) -> Result<u32> {
        let (_, d) = self.shaders.get_mut(shader_id).context("shader not decoded")?;
        let key = (d.name.clone(), blob, stage as u8);
        if let Some(&p) = self.programs.get(&key) {
            return Ok(p);
        }
        let (_gtype, keywords, msl_src) = d.subprogram(blob)?;
        let pass_name = d.passes[pass].info.name.clone();
        let shader_name = d.name.clone();
        self.add_program(
            shader_name,
            pass_name,
            stage,
            keywords.into_iter().collect(),
            msl_src,
            [0; 3],
            key,
        )
    }

    #[expect(clippy::too_many_arguments, reason = "plain data")]
    fn add_program(
        &mut self,
        shader: String,
        pass: String,
        stage: Stage,
        keywords: Vec<String>,
        msl_src: String,
        threads: [u32; 3],
        key: (String, usize, u8),
    ) -> Result<u32> {
        let iface = msl::parse(&msl_src).map_err(|e| anyhow::anyhow!("{shader}/{pass}: {e}"))?;
        msl::check_contiguous(&iface).map_err(|e| anyhow::anyhow!("{shader}/{pass}: {e}"))?;
        let hash = wr_assets::fnv1a64(msl_src.as_bytes());
        let p = Program {
            stage,
            shader,
            pass,
            keywords,
            entry: iface.entry.clone(),
            msl_hash: hash,
            bindings: iface.bindings.clone(),
            vertex_inputs: iface.vertex_inputs.clone(),
            color_outputs: iface.color_outputs,
            cbuffers: iface.cbuffers(),
            threads,
            ..Default::default()
        };
        self.msl_sources.entry(hash).or_insert(msl_src);
        self.meta.programs.push(p);
        let i = u32::try_from(self.meta.programs.len() - 1)?;
        self.programs.insert(key, i);
        Ok(i)
    }

    /// Interns a material and selects its G-buffer variant for `renderer_keywords`.
    fn material(
        &mut self,
        from: &SerializedFile<'_>,
        p: PPtr,
        renderer_keywords: &[String],
    ) -> Result<Option<u32>> {
        let Some(o) = self.universe.resolve(from, p)? else { return Ok(None) };
        let id = asset_id(&o);
        let mi = match self.materials.get(&id) {
            Some(None) => return Ok(None),
            Some(Some(m)) => *m,
            None => {
                let r = self.new_material(&o, id.clone())?;
                self.materials.insert(id, r);
                match r {
                    Some(m) => m,
                    None => return Ok(None),
                }
            }
        };
        let key = (mi, renderer_keywords.to_vec());
        if !self.selected.contains_key(&key) {
            self.select_variant(mi, renderer_keywords)?;
            self.selected.insert(key, ());
        }
        Ok(Some(mi))
    }

    fn new_material(&mut self, o: &ObjRef, id: AssetId) -> Result<Option<u32>> {
        let v = o.read()?;
        let name = v.str_at("m_Name").unwrap_or_default().to_owned();
        let Some(sp) = v.get("m_Shader").and_then(Value::as_pptr) else {
            skip(&mut self.report, "material without shader");
            return Ok(None);
        };
        let Some((si, sname)) = self.shader(o.file, sp)? else {
            skip(&mut self.report, "material shader unresolved");
            return Ok(None);
        };
        let queue = v.i64_at("m_CustomRenderQueue").unwrap_or(-1) as i32;
        if !GBUFFER_SHADERS.contains(&sname.as_str()) {
            skip(&mut self.report, format!("material shader {sname} (not in first-light scope)"));
            return Ok(None);
        }
        if queue > 2500 {
            skip(&mut self.report, format!("material {sname} queue {queue} (not G-buffer)"));
            return Ok(None);
        }
        let keywords: Vec<String> = v
            .str_at("m_ShaderKeywords")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let props = v.get("m_SavedProperties");
        let pairs = |k: &str| -> Vec<(String, Value)> {
            props
                .and_then(|p| p.get(k))
                .and_then(Value::as_array)
                .unwrap_or_default()
                .iter()
                .map(|pr| {
                    (
                        pr.str_at("first").unwrap_or_default().to_owned(),
                        pr.get("second").cloned().unwrap_or(Value::Object(Vec::new())),
                    )
                })
                .collect()
        };
        let mut textures = Vec::new();
        for (tname, tv) in pairs("m_TexEnvs") {
            let texture = match tv.get("m_Texture").and_then(Value::as_pptr) {
                Some(p) if !p.is_null() => self.texture(o.file, p)?,
                _ => None,
            };
            textures.push(TexProp {
                name: tname,
                texture,
                scale: [
                    tv.f32_at("m_Scale.x").unwrap_or(1.0),
                    tv.f32_at("m_Scale.y").unwrap_or(1.0),
                ],
                offset: [
                    tv.f32_at("m_Offset.x").unwrap_or(0.0),
                    tv.f32_at("m_Offset.y").unwrap_or(0.0),
                ],
            });
        }
        let floats =
            pairs("m_Floats").into_iter().map(|(k, v)| (k, v.as_f32().unwrap_or(0.0))).collect();
        let colors = pairs("m_Colors")
            .into_iter()
            .map(|(k, c)| {
                let g = |f: &str| c.f32_at(f).unwrap_or(0.0);
                (k, [g("r"), g("g"), g("b"), g("a")])
            })
            .collect();
        let disabled_passes = v
            .get("disabledShaderPasses")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect();
        self.meta.materials.push(Material {
            id,
            name,
            shader: si,
            keywords,
            render_queue: queue,
            disabled_passes,
            textures,
            floats,
            colors,
            variants: Vec::new(),
            casters: Vec::new(),
        });
        let mi = u32::try_from(self.meta.materials.len() - 1)?;
        self.select_casters(mi)?;
        Ok(Some(mi))
    }

    /// Selects the material's `ShadowCaster` programs for both `GEOMETRY_CLIP` states
    /// (`ShadowCasterPass` toggles that keyword per light type; all other caster keywords
    /// come from the material). Nothing is selected when the shader has no such pass or
    /// the material disables it.
    fn select_casters(&mut self, mi: u32) -> Result<()> {
        let m = &self.meta.materials[mi as usize];
        if m.disabled_passes.iter().any(|p| p.eq_ignore_ascii_case("ShadowCaster")) {
            return Ok(());
        }
        let shader_id = self.meta.shaders[m.shader as usize].id.clone();
        let mut base: BTreeSet<String> = m.keywords.iter().cloned().collect();
        base.extend(self.global_keywords.iter().cloned());
        let (_, d) = self.shaders.get_mut(&shader_id).context("shader")?;
        let Some(pi) = d.pass_index("ShadowCaster") else { return Ok(()) };
        let vblobs = d.passes[pi].vertex.clone();
        let fblobs = d.passes[pi].fragment.clone();
        if vblobs.is_empty() || fblobs.is_empty() {
            return Ok(());
        }
        let mut out = Vec::new();
        for clip in [false, true] {
            let mut enabled = base.clone();
            if clip {
                enabled.insert("GEOMETRY_CLIP".to_owned());
            } else {
                enabled.remove("GEOMETRY_CLIP");
            }
            let pick = |blobs: &[usize], d: &mut shader::DecodedShader| -> Result<(usize, bool)> {
                let mut kws = Vec::new();
                for &b in blobs {
                    kws.push(d.subprogram(b)?.1);
                }
                let (i, exact) = shader::select_variant(&kws, &enabled)
                    .context("no ShadowCaster subprograms for stage")?;
                Ok((blobs[i], exact))
            };
            let (vb, ve) = pick(&vblobs, d)?;
            let (fb, fe) = pick(&fblobs, d)?;
            out.push((clip, vb, fb, ve && fe));
        }
        for (clip, vb, fb, exact) in out {
            let vertex = self.program(&shader_id, pi, Stage::Vertex, vb)?;
            let fragment = self.program(&shader_id, pi, Stage::Fragment, fb)?;
            self.meta.materials[mi as usize].casters.push(wr_assets::CasterVariant {
                geometry_clip: clip,
                vertex,
                fragment,
                exact,
            });
        }
        Ok(())
    }

    fn select_variant(&mut self, mi: u32, renderer_keywords: &[String]) -> Result<()> {
        let m = &self.meta.materials[mi as usize];
        let shader_id = self.meta.shaders[m.shader as usize].id.clone();
        let mut enabled: BTreeSet<String> = m.keywords.iter().cloned().collect();
        enabled.extend(self.global_keywords.iter().cloned());
        enabled.extend(renderer_keywords.iter().cloned());
        let (_, d) = self.shaders.get_mut(&shader_id).context("shader")?;
        let Some(pi) = d.pass_index("GBuffer") else {
            bail!("{}: no GBuffer pass", d.name);
        };
        let pass_name = d.passes[pi].info.name.clone();
        let pick = |blobs: Vec<usize>, d: &mut shader::DecodedShader| -> Result<(usize, bool)> {
            let mut kws = Vec::new();
            for &b in &blobs {
                kws.push(d.subprogram(b)?.1);
            }
            let (i, exact) =
                shader::select_variant(&kws, &enabled).context("no subprograms for stage")?;
            Ok((blobs[i], exact))
        };
        let vblobs = d.passes[pi].vertex.clone();
        let fblobs = d.passes[pi].fragment.clone();
        let (vb, ve) = pick(vblobs, d)?;
        let (fb, fe) = pick(fblobs, d)?;
        let vertex = self.program(&shader_id, pi, Stage::Vertex, vb)?;
        let fragment = self.program(&shader_id, pi, Stage::Fragment, fb)?;
        if ve && fe {
            self.report.variants_exact += 1;
        } else {
            self.report.variants_nearest += 1;
        }
        self.meta.materials[mi as usize].variants.push(MaterialVariant {
            pass: pass_name,
            renderer_keywords: renderer_keywords.to_vec(),
            vertex,
            fragment,
            exact: ve && fe,
        });
        Ok(())
    }
}

/// The area's scene bundles and, for each, its scene names and serialized files.
fn area_scenes(
    universe: &Universe,
    deps: &crate::catalog::Dependencies,
    area: &str,
) -> Result<Vec<(String, &'static SerializedFile<'static>)>> {
    let prefix = format!("{area}_");
    let mut bundles: Vec<&String> =
        deps.map.keys().filter(|k| k.starts_with(&prefix) && k.ends_with(".scenes")).collect();
    bundles.sort();
    let mut out = Vec::new();
    for b in bundles {
        let files = universe.files_of(b)?;
        for f in &files {
            for o in f.objects.iter().filter(|o| o.class_id == class_id::ASSET_BUNDLE) {
                let v = f.read(o)?;
                for pair in v.get("m_SceneHashes").and_then(Value::as_array).unwrap_or_default() {
                    let scene = pair.str_at("first").unwrap_or_default();
                    let cab = pair.str_at("second").unwrap_or_default();
                    if let Some(file) = files.iter().find(|f| f.name == cab) {
                        let short =
                            scene.rsplit('/').next().unwrap_or(scene).trim_end_matches(".unity");
                        out.push((short.to_owned(), *file));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn color(v: Option<&Value>) -> [f32; 4] {
    let g = |f: &str| v.and_then(|c| c.f32_at(f)).unwrap_or(0.0);
    [g("r"), g("g"), g("b"), g("a")]
}

/// Builds the pack for `area` at `out`.
pub fn build(
    data_dir: &Path,
    universe: &Universe,
    deps: &crate::catalog::Dependencies,
    area: &str,
    out: &Path,
    metal_cache: &Path,
    units: Option<(&crate::catalog::Locations, &wr_view::UnitsSpec)>,
) -> Result<PackReport> {
    let t_all = Instant::now();
    let mut t = Instant::now();
    let scenes = area_scenes(universe, deps, area)?;
    if scenes.is_empty() {
        bail!("no scenes for area {area}");
    }
    let mut indexes = Vec::new();
    for (name, file) in &scenes {
        indexes.push(SceneIndex::load(name, file)?);
    }
    let mut b = Builder {
        universe,
        w: PackWriter::create(out)?,
        meta: PackMeta {
            schema: wr_assets::SCHEMA.to_owned(),
            area: area.to_owned(),
            ..Default::default()
        },
        report: PackReport {
            area: area.to_owned(),
            pack: out.display().to_string(),
            ..Default::default()
        },
        textures: HashMap::new(),
        meshes: HashMap::new(),
        materials: HashMap::new(),
        shaders: HashMap::new(),
        programs: HashMap::new(),
        selected: HashMap::new(),
        msl_sources: HashMap::new(),
        global_keywords: BTreeSet::new(),
    };
    b.report.timings_ms.insert("scenes".into(), t.elapsed().as_millis());

    // Lights first: the shadowmask global keyword depends on them.
    t = Instant::now();
    let mut shadowmask = false;
    for sc in &indexes {
        for o in sc.file.objects.iter().filter(|o| o.class_id == class_id::LIGHT) {
            let v = sc.file.read(o)?;
            let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
            let bake =
                v.i64_at("m_BakingOutput.lightmapBakeMode.lightmapBakeType").unwrap_or(4) as i32;
            let mixed =
                v.i64_at("m_BakingOutput.lightmapBakeMode.mixedLightingMode").unwrap_or(0) as i32;
            let channel = v.i64_at("m_BakingOutput.occlusionMaskChannel").unwrap_or(-1) as i32;
            let enabled = v.get("m_Enabled").and_then(Value::as_bool).unwrap_or(true);
            let active = sc.active_in_hierarchy(go);
            if enabled && active && bake == 1 && mixed == 2 && channel != -1 {
                shadowmask = true;
            }
            // OwlcatAdditionalLightData on the same GameObject.
            let mut owlcat = None;
            for mb in sc.components(go, class_id::MONO_BEHAVIOUR) {
                let mv = sc.file.read(mb)?;
                let script = universe.script_of(sc.file, &mv)?;
                if script.is_some_and(|s| s.class == "OwlcatAdditionalLightData") {
                    owlcat = Some(OwlcatLight {
                        falloff: mv.i64_at("m_FalloffType").unwrap_or(0) as i32,
                        inner_radius: mv.f32_at("m_InnerRadius").unwrap_or(0.0),
                        snap_specular_to_inner_radius: mv
                            .get("m_SnapSpecularToInnerRadius")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        use_pipeline_settings: mv
                            .get("m_UsePipelineSettings")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                    });
                }
            }
            if bake != 2 {
                b.report.lights_realtime += 1;
            }
            b.meta.lights.push(Light {
                id: AssetId { file: sc.file.name.clone(), path_id: o.path_id },
                game_object: AssetId { file: sc.file.name.clone(), path_id: go },
                scene: sc.name.clone(),
                path: sc.path(go),
                enabled,
                active,
                kind: v.i64_at("m_Type").unwrap_or(2) as i32,
                color: color(v.get("m_Color")),
                intensity: v.f32_at("m_Intensity").unwrap_or(1.0),
                range: v.f32_at("m_Range").unwrap_or(10.0),
                spot_angle: v.f32_at("m_SpotAngle").unwrap_or(30.0),
                inner_spot_angle: v.f32_at("m_InnerSpotAngle").unwrap_or(0.0),
                shadows: v.i64_at("m_Shadows.m_Type").unwrap_or(0) as i32,
                shadow_strength: v.f32_at("m_Shadows.m_Strength").unwrap_or(1.0),
                shadow_bias: v.f32_at("m_Shadows.m_Bias").unwrap_or(0.05),
                shadow_normal_bias: v.f32_at("m_Shadows.m_NormalBias").unwrap_or(0.4),
                shadow_near_plane: v.f32_at("m_Shadows.m_NearPlane").unwrap_or(0.2),
                bake_type: bake,
                mixed_mode: mixed,
                occlusion_mask_channel: channel,
                culling_mask: u32::try_from(
                    v.i64_at("m_CullingMask.m_Bits").unwrap_or(-1) & 0xFFFF_FFFF,
                )
                .unwrap_or(u32::MAX),
                rendering_layer_mask: u32::try_from(
                    v.i64_at("m_RenderingLayerMask").unwrap_or(1) & 0xFFFF_FFFF,
                )
                .unwrap_or(1),
                render_mode: v.i64_at("m_RenderMode").unwrap_or(0) as i32,
                use_color_temperature: v
                    .get("m_UseColorTemperature")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                color_temperature: v.f32_at("m_ColorTemperature").unwrap_or(6570.0),
                matrix: sc.matrix(go),
                owlcat,
            });
        }
    }
    b.report.shadowmask_enabled = shadowmask;
    b.global_keywords.insert("DEFERRED_ON".into());
    if shadowmask {
        b.global_keywords.insert("SHADOWS_SHADOWMASK".into());
    }
    b.report.timings_ms.insert("lights".into(), t.elapsed().as_millis());

    // Scene settings and lightmaps.
    t = Instant::now();
    for sc in &indexes {
        let mut s = SceneSettings { scene: sc.name.clone(), ..Default::default() };
        let mut has = false;
        for o in &sc.file.objects {
            match o.class_id {
                CLASS_RENDER_SETTINGS => {
                    let v = sc.file.read(o)?;
                    has = true;
                    s.fog = v.get("m_Fog").and_then(Value::as_bool).unwrap_or(false);
                    s.fog_color = color(v.get("m_FogColor"));
                    s.fog_mode = v.i64_at("m_FogMode").unwrap_or(3) as i32;
                    s.fog_density = v.f32_at("m_FogDensity").unwrap_or(0.0);
                    s.fog_start = v.f32_at("m_LinearFogStart").unwrap_or(0.0);
                    s.fog_end = v.f32_at("m_LinearFogEnd").unwrap_or(0.0);
                    s.ambient_mode = v.i64_at("m_AmbientMode").unwrap_or(0) as i32;
                    s.ambient_sky = color(v.get("m_AmbientSkyColor"));
                    s.ambient_equator = color(v.get("m_AmbientEquatorColor"));
                    s.ambient_ground = color(v.get("m_AmbientGroundColor"));
                    s.ambient_intensity = v.f32_at("m_AmbientIntensity").unwrap_or(1.0);
                    s.subtractive_shadow_color = color(v.get("m_SubtractiveShadowColor"));
                    s.reflection_intensity = v.f32_at("m_ReflectionIntensity").unwrap_or(1.0);
                    if let Some(rp) = v
                        .get("m_GeneratedSkyboxReflection")
                        .and_then(Value::as_pptr)
                        .filter(|p| !p.is_null())
                    {
                        s.reflection = b.texture(sc.file, rp)?;
                    }
                    s.ambient_probe = (0..27)
                        .map(|i| v.f32_at(&format!("m_AmbientProbe.sh[{i:2}]")).unwrap_or(0.0))
                        .collect();
                }
                class_id::REFLECTION_PROBE => {
                    let v = sc.file.read(o)?;
                    let go =
                        v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                    let tex = match v
                        .get("m_BakedTexture")
                        .and_then(Value::as_pptr)
                        .filter(|p| !p.is_null())
                    {
                        Some(p) => b.texture(sc.file, p)?,
                        None => None,
                    };
                    let m = sc.matrix(go);
                    b.meta.reflection_probes.push(wr_assets::ReflectionProbe {
                        id: AssetId { file: sc.file.name.clone(), path_id: o.path_id },
                        scene: sc.name.clone(),
                        path: sc.path(go),
                        enabled: v.get("m_Enabled").and_then(Value::as_bool).unwrap_or(true),
                        active: sc.active_in_hierarchy(go),
                        position: [m[12], m[13], m[14]],
                        box_size: v.get("m_BoxSize").and_then(Value::vec3).unwrap_or([1.0; 3]),
                        box_offset: v.get("m_BoxOffset").and_then(Value::vec3).unwrap_or([0.0; 3]),
                        box_projection: v
                            .get("m_BoxProjection")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        blend_distance: v.f32_at("m_BlendDistance").unwrap_or(1.0),
                        importance: v.i64_at("m_Importance").unwrap_or(1) as i32,
                        intensity: v.f32_at("m_IntensityMultiplier").unwrap_or(1.0),
                        hdr: v.get("m_HDR").and_then(Value::as_bool).unwrap_or(true),
                        texture: tex,
                    });
                }
                CLASS_LIGHTMAP_SETTINGS => {
                    let v = sc.file.read(o)?;
                    s.lightmaps_mode = v.i64_at("m_LightmapsMode").unwrap_or(0) as i32;
                    for (i, lm) in v
                        .get("m_Lightmaps")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                    {
                        let get =
                            |k: &str| lm.get(k).and_then(Value::as_pptr).filter(|p| !p.is_null());
                        let color = match get("m_Lightmap") {
                            Some(p) => b.texture(sc.file, p)?,
                            None => None,
                        };
                        let direction = match get("m_DirLightmap") {
                            Some(p) => b.texture(sc.file, p)?,
                            None => None,
                        };
                        let shadowmask = match get("m_ShadowMask") {
                            Some(p) => b.texture(sc.file, p)?,
                            None => None,
                        };
                        b.meta.lightmaps.push(Lightmap {
                            scene: sc.name.clone(),
                            index: u32::try_from(i)?,
                            color,
                            direction,
                            shadowmask,
                        });
                    }
                }
                _ => {}
            }
        }
        if has {
            b.meta.scenes.push(s);
        }
    }
    b.report.timings_ms.insert("settings_lightmaps".into(), t.elapsed().as_millis());

    // BakedTerrain: renderers it activates, with OwlcatTerrain's property block.
    let mut baked: HashMap<(String, i64), wr_assets::PropertyBlock> = HashMap::new();
    for sc in &indexes {
        for o in sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = sc.file.read(o)?;
            if !universe.script_of(sc.file, &v)?.is_some_and(|s| s.class == "BakedTerrain") {
                continue;
            }
            let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
            let mut pb = wr_assets::PropertyBlock {
                source: "BakedTerrain/OwlcatTerrain".into(),
                ..Default::default()
            };
            if let Some(tp) = v.get("Terrain").and_then(Value::as_pptr)
                && let Some(t) = sc.file.object(tp.path_id)
            {
                let tv = sc.file.read(t)?;
                if let Some(dp) = tv.get("m_TerrainData").and_then(Value::as_pptr)
                    && let Some(td) = universe.resolve(sc.file, dp)?
                {
                    let tdv = td.read()?;
                    let size_x = tdv.f32_at("m_Heightmap.m_Scale.x").unwrap_or(1.0)
                        * (tdv.i64_at("m_Heightmap.m_Resolution").unwrap_or(1) as f32 - 1.0)
                            .max(1.0);
                    pb.floats.push(("_TerrainMaxHeight".into(), 1.0 / size_x.max(1e-6)));
                    let n = tdv
                        .path("m_SplatDatabase.m_AlphaTextures")
                        .and_then(Value::as_array)
                        .map_or(0, <[Value]>::len);
                    // `SetInt`: stored as an int's bits.
                    pb.floats.push(("_ControlTexturesCount".into(), f32::from_bits(n as u32)));
                }
            }
            for mb in sc.components(go, class_id::MONO_BEHAVIOUR) {
                let mv = sc.file.read(mb)?;
                if !universe.script_of(sc.file, &mv)?.is_some_and(|s| s.class == "OwlcatTerrain") {
                    continue;
                }
                for (field, name) in [
                    ("m_LayerDataMasksScale", "_TerrainLayerMasksScale"),
                    ("m_LayerDataUvMatrix", "_TerrainLayerUvMatrix"),
                    ("m_LayerDataParams", "_TerrainLayerParams"),
                ] {
                    let vals: Vec<[f32; 4]> = mv
                        .get(field)
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(Value::vec4)
                        .collect();
                    pb.vectors.push((name.into(), vals));
                }
                for (field, name) in [
                    ("m_Diffuse", "_DiffuseArray"),
                    ("m_Normal", "_NormalArray"),
                    ("m_Masks", "_MasksArray"),
                    ("m_SplatArray", "_SplatArray"),
                ] {
                    if let Some(p) = mv.get(field).and_then(Value::as_pptr).filter(|p| !p.is_null())
                        && let Some(ti) = b.texture(sc.file, p)?
                    {
                        pb.textures.push((name.into(), ti));
                    }
                }
            }
            for m in v.get("Meshes").and_then(Value::as_array).unwrap_or_default() {
                if let Some(p) = m.as_pptr() {
                    baked.insert((sc.name.clone(), p.path_id), pb.clone());
                }
            }
        }
    }
    b.meta.lighting = lighting_setup(universe, &indexes)?;
    if let Some(l) = &mut b.meta.lighting {
        l.rig_light = camera_rig_light(data_dir, &indexes).unwrap_or_else(|e| {
            b.report.skipped.insert(format!("camera rig light: {e:#}"), 1);
            None
        });
    }
    // Renderers.
    t = Instant::now();
    for sc in &indexes {
        let lightmaps_mode =
            b.meta.scenes.iter().find(|s| s.scene == sc.name).map_or(0, |s| s.lightmaps_mode);
        for o in &sc.file.objects {
            match o.class_id {
                class_id::SKINNED_MESH_RENDERER => {
                    skip(&mut b.report, "SkinnedMeshRenderer (units/animated props: later)")
                }
                class_id::TERRAIN => skip(&mut b.report, "Terrain (Owlcat/Terrain: later)"),
                class_id::PARTICLE_SYSTEM_RENDERER => {
                    skip(&mut b.report, "ParticleSystemRenderer (Shuriken: later)")
                }
                _ => {}
            }
            if o.class_id != class_id::MESH_RENDERER {
                continue;
            }
            let v = sc.file.read(o)?;
            let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
            let Some(mf) = sc.components(go, class_id::MESH_FILTER).first().copied() else {
                skip(&mut b.report, "MeshRenderer without MeshFilter");
                continue;
            };
            let mfv = sc.file.read(mf)?;
            let Some(mp) = mfv.get("m_Mesh").and_then(Value::as_pptr).filter(|p| !p.is_null())
            else {
                skip(&mut b.report, "MeshFilter without mesh");
                continue;
            };
            let lightmap_index =
                u16::try_from(v.i64_at("m_LightmapIndex").unwrap_or(65535)).unwrap_or(65535);
            let has_lightmap = lightmap_index < 65534
                && b.meta
                    .lightmaps
                    .iter()
                    .any(|l| l.scene == sc.name && l.index == u32::from(lightmap_index));
            let mut rkw = Vec::new();
            if has_lightmap {
                rkw.push("LIGHTMAP_ON".to_owned());
                let dir = b.meta.lightmaps.iter().any(|l| {
                    l.scene == sc.name
                        && l.index == u32::from(lightmap_index)
                        && l.direction.is_some()
                });
                if lightmaps_mode == 1 && dir {
                    rkw.push("DIRLIGHTMAP_COMBINED".to_owned());
                }
            }
            let mut mats = Vec::new();
            let mut any = false;
            for mp in v.get("m_Materials").and_then(Value::as_array).unwrap_or_default() {
                let m = match mp.as_pptr().filter(|p| !p.is_null()) {
                    Some(p) => b.material(sc.file, p, &rkw)?,
                    None => None,
                };
                any |= m.is_some();
                mats.push(m);
            }
            if !any {
                skip(&mut b.report, "MeshRenderer with no in-scope material");
                continue;
            }
            let Some(mesh) = b.mesh(sc.file, mp)? else { continue };
            let sb_first = v.i64_at("m_StaticBatchInfo.firstSubMesh").unwrap_or(0);
            let sb_count = v.i64_at("m_StaticBatchInfo.subMeshCount").unwrap_or(0);
            let st = v
                .get("m_LightmapTilingOffset")
                .and_then(Value::vec4)
                .unwrap_or([1.0, 1.0, 0.0, 0.0]);
            b.meta.renderers.push(Renderer {
                renderer: AssetId { file: sc.file.name.clone(), path_id: o.path_id },
                game_object: AssetId { file: sc.file.name.clone(), path_id: go },
                scene: sc.name.clone(),
                path: sc.path(go),
                kind: RendererKind::Mesh,
                enabled: v.get("m_Enabled").and_then(Value::as_bool).unwrap_or(true),
                active: sc.active_in_hierarchy(go),
                layer: sc.gos.get(&go).map_or(0, |g| g.2),
                matrix: sc.matrix(go),
                mesh,
                materials: mats,
                lightmap_index,
                lightmap_st: st,
                cast_shadows: v.i64_at("m_CastShadows").unwrap_or(1) as u8,
                receive_shadows: v.i64_at("m_ReceiveShadows").unwrap_or(1) != 0,
                light_probe_usage: v.i64_at("m_LightProbeUsage").unwrap_or(1) as u8,
                reflection_probe_usage: v.i64_at("m_ReflectionProbeUsage").unwrap_or(1) as u8,
                static_batch: (
                    u16::try_from(sb_first).unwrap_or(0),
                    u16::try_from(sb_count).unwrap_or(0),
                ),
                rendering_layer_mask: u32::try_from(v.i64_at("m_RenderingLayerMask").unwrap_or(1))
                    .unwrap_or(1),
                property_block: None,
            });
            if let Some(bt) = baked.get(&(sc.name.clone(), o.path_id)) {
                let r = b.meta.renderers.last_mut().expect("just pushed");
                // BakedTerrain.OnEnable activates the mesh and sets the terrain's material
                // template and OwlcatTerrain's property block.
                r.active = true;
                r.property_block = Some(bt.clone());
            }
        }
    }
    b.report.timings_ms.insert("renderers".into(), t.elapsed().as_millis());

    // Units: the save's unit views, baked in their bind pose.
    if let Some((locations, spec)) = units {
        t = Instant::now();
        b.report.units = Some(units::add(&mut b, locations, spec)?);
        b.report.timings_ms.insert("units".into(), t.elapsed().as_millis());
    }

    // Pipeline programs.
    t = Instant::now();
    pipeline_programs(&mut b, data_dir)?;
    b.report.timings_ms.insert("pipeline_programs".into(), t.elapsed().as_millis());

    // Metal libraries.
    t = Instant::now();
    let sources: Vec<(u64, &str)> = b.msl_sources.iter().map(|(h, s)| (*h, s.as_str())).collect();
    let libs = metal::compile_all(metal_cache, &sources, 8);
    b.report.timings_ms.insert("metallib".into(), t.elapsed().as_millis());
    let mut msl_blobs = HashMap::new();
    let mut lib_blobs = HashMap::new();
    for (h, s) in &b.msl_sources {
        msl_blobs.insert(*h, b.w.blob(s.as_bytes())?);
        match libs.get(h) {
            Some(Ok(bytes)) => {
                lib_blobs.insert(*h, b.w.blob(bytes)?);
            }
            Some(Err(e)) => b.report.metallib_failures.push(format!("{h:016x}: {e}")),
            None => {}
        }
    }
    b.report.metallibs = lib_blobs.len();
    for p in &mut b.meta.programs {
        p.msl = msl_blobs[&p.msl_hash];
        p.metallib = lib_blobs.get(&p.msl_hash).copied();
    }

    let mut rep = b.report;
    rep.renderers = b.meta.renderers.len();
    rep.meshes = b.meta.meshes.len();
    rep.materials = b.meta.materials.len();
    rep.textures = b.meta.textures.len();
    rep.programs = b.meta.programs.len();
    rep.lights = b.meta.lights.len();
    b.meta.skipped = rep.skipped.iter().map(|(k, v)| (k.clone(), *v)).collect();
    b.meta.source = vec![
        ("importer".into(), format!("wr-import {}", env!("CARGO_PKG_VERSION"))),
        ("scenes".into(), scenes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(",")),
    ];
    rep.pack_bytes = b.w.finish(&b.meta)?;
    rep.timings_ms.insert("total".into(), t_all.elapsed().as_millis());
    Ok(rep)
}

/// The render pipeline's own programs: deferred lighting and reflections (fullscreen),
/// fog, and the light-culling compute kernel.
/// Imports a pipeline shader's programs (those whose keywords `keep` accepts) as
/// `PipelineProgram`s named `<shader>#<pass>.<vs|fs>[<keywords>]`.
fn import_pipeline_shader(
    b: &mut Builder<'_>,
    f: &SerializedFile<'_>,
    o: &crate::unity::serialized::ObjectInfo,
    keep: &dyn Fn(&str) -> bool,
) -> Result<()> {
    let v = f.read(o)?;
    let name = v.str_at("m_ParsedForm.m_Name").unwrap_or_default().to_owned();
    let id = AssetId { file: f.name.clone(), path_id: o.path_id };
    let d = shader::DecodedShader::decode(&v)?;
    let si = u32::try_from(b.meta.shaders.len())?;
    b.meta.shaders.push(Shader {
        id: id.clone(),
        name: d.name.clone(),
        passes: d.passes.iter().map(|p| p.info.clone()).collect(),
        properties: d.properties.clone(),
    });
    let passes: Vec<(usize, Vec<usize>, Vec<usize>, wr_assets::PassState)> = d
        .passes
        .iter()
        .enumerate()
        .map(|(i, p)| (i, p.vertex.clone(), p.fragment.clone(), p.info.state.clone()))
        .collect();
    b.shaders.insert(id.clone(), (si, d));
    for (pi, vbs, fbs, state) in passes {
        for (stage, blobs) in [(Stage::Vertex, vbs), (Stage::Fragment, fbs)] {
            for blob in blobs {
                let kw = {
                    let (_, d) = b.shaders.get_mut(&id).context("shader")?;
                    d.subprogram(blob)?.1.into_iter().collect::<Vec<_>>().join(" ")
                };
                if !keep(&kw) {
                    continue;
                }
                let p = b.program(&id, pi, stage, blob)?;
                b.meta.pipeline_programs.push(PipelineProgram {
                    role: format!(
                        "{name}#{pi}.{}[{kw}]",
                        if stage == Stage::Vertex { "vs" } else { "fs" }
                    ),
                    program: p,
                    state: Some(state.clone()),
                });
            }
        }
    }
    Ok(())
}

/// Post-processing shaders in the player's `globalgamemanagers.assets`, with the
/// keyword sets the post path uses (`None`: every variant).
const POST_SHADERS: &[(&str, Option<&[&str]>)] = &[
    (
        "Hidden/Owlcat Render Pipeline/LutBuilderHdr",
        Some(&["", "_TONEMAP_NEUTRAL", "_TONEMAP_ACES"]),
    ),
    (
        "Hidden/Owlcat Render Pipeline/UberPost",
        Some(&[
            "",
            "_HDR_GRADING",
            "_FILM_GRAIN _HDR_GRADING",
            "_BLOOM_HQ _HDR_GRADING",
            "_BLOOM_HQ _FILM_GRAIN _HDR_GRADING",
            "_BLOOM_HQ _DITHERING _HDR_GRADING",
            "_BLOOM_HQ _DITHERING _FILM_GRAIN _HDR_GRADING",
        ]),
    ),
    // `PostProcessPass.Render` always starts with a Stop NaN blit, which also decodes the
    // camera colour from the sRGB-encoded values lighting and fog write back to linear.
    ("Hidden/Owlcat Render Pipeline/Stop NaN", None),
    ("Hidden/Owlcat Render Pipeline/BloomEnhanced", None),
];

fn pipeline_programs(b: &mut Builder<'_>, data_dir: &Path) -> Result<()> {
    // The player's asset files have no type trees: borrow the Shader tree from the
    // `utility_shaders` bundle (same Unity version).
    let donors = b.universe.files_of("utility_shaders")?;
    let ggm = data_dir.join("globalgamemanagers.assets");
    b.universe.add_loose_file_with_donors(&ggm, &donors)?;
    if let Some(f) = b.universe.file("globalgamemanagers.assets")? {
        for o in f.objects.iter().filter(|o| o.class_id == class_id::SHADER) {
            let name = f
                .read_prefix(o, "m_ParsedForm")?
                .str_at("m_ParsedForm.m_Name")
                .unwrap_or_default()
                .to_owned();
            if let Some((_, kws)) = POST_SHADERS.iter().find(|(n, _)| *n == name) {
                import_pipeline_shader(b, f, o, &|kw: &str| kws.is_none_or(|k| k.contains(&kw)))?;
            }
        }
    }
    let files = b.universe.files_of("utility_shaders")?;
    for f in files {
        for o in &f.objects {
            if o.class_id == class_id::SHADER {
                let v = f.read_prefix(o, "m_ParsedForm")?;
                let name = v.str_at("m_ParsedForm.m_Name").unwrap_or_default().to_owned();
                if PIPELINE_SHADERS.contains(&name.as_str()) {
                    // Every variant: the runtime picks by keywords (shadows on/off).
                    import_pipeline_shader(b, f, o, &|_| true)?;
                }
            }
            if o.class_id == CLASS_COMPUTE_SHADER {
                // ComputeShader.
                let v = f.read(o)?;
                let name = v.str_at("m_Name").unwrap_or_default().to_owned();
                if name != "LightCulling" {
                    continue;
                }
                for k in shader::compute_kernels(&v)? {
                    let key = (format!("compute:{name}"), b.programs.len(), Stage::Compute as u8);
                    let kw: Vec<String> =
                        k.keywords.split_whitespace().map(str::to_owned).collect();
                    let p = b.add_program(
                        name.clone(),
                        k.name.clone(),
                        Stage::Compute,
                        kw,
                        k.msl,
                        k.threads,
                        key,
                    )?;
                    b.meta.pipeline_programs.push(PipelineProgram {
                        role: format!("{name}.{}[{}]", k.name, k.keywords),
                        program: p,
                        state: None,
                    });
                }
            }
        }
    }
    Ok(())
}

fn vec3f(v: Option<&Value>) -> [f32; 3] {
    v.and_then(Value::vec3).unwrap_or_default()
}

/// The light scene's `LightController`: the config it applies (solo, else day) and the
/// post-processing volumes it enables.
fn lighting_setup(
    universe: &Universe,
    indexes: &[SceneIndex],
) -> Result<Option<wr_assets::LightingSetup>> {
    for sc in indexes {
        for o in sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = sc.file.read(o)?;
            if !universe.script_of(sc.file, &v)?.is_some_and(|s| s.class == "LightController") {
                continue;
            }
            let pp = |k: &str| v.get(k).and_then(Value::as_pptr).filter(|p| !p.is_null());
            let (kind, cfg, vols) = match pp("m_SoloConfig") {
                Some(p) => ("solo", Some(p), "m_SoloPostProcessingVolumes"),
                None => ("day", pp("m_DayConfig"), "m_DayPostProcessingVolumes"),
            };
            let mut ls = wr_assets::LightingSetup {
                scene: sc.name.clone(),
                config_kind: kind.into(),
                camera_clear_flag: v.i64_at("CameraClearFlag").unwrap_or(2) as i32,
                camera_light: v.i64_at("CameraLight").unwrap_or(0) as i32,
                main_light: pp("MainLight")
                    .map(|p| AssetId { file: sc.file.name.clone(), path_id: p.path_id }),
                ..Default::default()
            };
            if let Some(cp) = cfg
                && let Some(c) = universe.resolve(sc.file, cp)?
            {
                let cv = c.read()?;
                ls.main_light_rotation = vec3f(cv.get("MainLightRotation"));
                ls.main_light_color = color(cv.get("MainLightColor"));
                ls.main_light_intensity = cv.f32_at("MainLightIntensity").unwrap_or(1.0);
                ls.main_light_indirect_intensity =
                    cv.f32_at("MainLightIndirectIntensity").unwrap_or(1.0);
                ls.main_light_shadow_strength = cv.f32_at("MainLightShadowStrength").unwrap_or(0.7);
                ls.sky_ambient = color(cv.get("SkyAmbientColor"));
                ls.equator_ambient = color(cv.get("EquatorAmbientColor"));
                ls.ground_ambient = color(cv.get("GroundAmbientColor"));
                ls.fog_color = color(cv.get("FogColor"));
                ls.fog_start = cv.f32_at("FogStartDistance").unwrap_or(25.0);
                ls.fog_end = cv.f32_at("FogEndDistance").unwrap_or(65.0);
            }
            for vp in v.get(vols).and_then(Value::as_array).unwrap_or_default() {
                let Some(p) = vp.as_pptr().filter(|p| !p.is_null()) else { continue };
                let Some(vo) = sc.file.object(p.path_id) else { continue };
                let vv = sc.file.read(vo)?;
                let go = vv.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                let mut vol = wr_assets::Volume {
                    path: sc.path(go),
                    is_global: vv.get("isGlobal").and_then(Value::as_bool).unwrap_or(true),
                    priority: vv.f32_at("priority").unwrap_or(0.0),
                    weight: vv.f32_at("weight").unwrap_or(1.0),
                    blend_distance: vv.f32_at("blendDistance").unwrap_or(0.0),
                    ..Default::default()
                };
                if let Some(pp) =
                    vv.get("sharedProfile").and_then(Value::as_pptr).filter(|p| !p.is_null())
                    && let Some(prof) = universe.resolve(sc.file, pp)?
                {
                    let pv = prof.read()?;
                    vol.profile = pv.str_at("m_Name").unwrap_or_default().to_owned();
                    for cp in pv.get("components").and_then(Value::as_array).unwrap_or_default() {
                        let Some(cp) = cp.as_pptr().filter(|p| !p.is_null()) else { continue };
                        let Some(co) = universe.resolve(prof.file, cp)? else { continue };
                        let cv = co.read()?;
                        let class =
                            universe.script_of(co.file, &cv)?.map(|s| s.full()).unwrap_or_default();
                        vol.components
                            .push((class, serde_json::to_string(&cv).unwrap_or_default()));
                    }
                }
                ls.volumes.push(vol);
            }
            return Ok(Some(ls));
        }
    }
    Ok(None)
}

/// The camera rig's `CameraLight` from the player's `level2` (where the game's
/// `CameraRig` prefab instance lives): its light settings and local transform.
///
/// Player level files carry no type trees; the scene bundles of the same Unity version
/// do, so their trees for the built-in classes are borrowed.
fn camera_rig_light(data_dir: &Path, donors: &[SceneIndex]) -> Result<Option<Light>> {
    let Ok(bytes) = std::fs::read(data_dir.join("level2")) else { return Ok(None) };
    let mut f = SerializedFile::parse("level2", &bytes)?;
    for t in &mut f.types {
        if t.tree.is_none() {
            t.tree = donors
                .iter()
                .flat_map(|d| d.file.types.iter())
                .find(|d| d.class_id == t.class_id && d.class_id != class_id::MONO_BEHAVIOUR)
                .and_then(|d| d.tree.clone());
        }
    }
    for o in f.objects.iter().filter(|o| o.class_id == class_id::GAME_OBJECT) {
        // Objects of classes no donor carries can't be decoded; they aren't needed.
        let Ok(go) = f.read(o) else { continue };
        if go.str_at("m_Name") != Some("CameraLight") {
            continue;
        }
        let comps: Vec<i64> = go
            .get("m_Component")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|c| c.get("component").and_then(Value::as_pptr))
            .map(|p| p.path_id)
            .collect();
        let (mut light, mut matrix) = (None, Mat4::IDENTITY);
        for c in comps {
            let Some(co) = f.object(c) else { continue };
            let Ok(v) = f.read(co) else { continue };
            match co.class_id {
                class_id::TRANSFORM => {
                    let p = v.get("m_LocalPosition").and_then(Value::vec3).unwrap_or_default();
                    let r = v
                        .get("m_LocalRotation")
                        .and_then(Value::vec4)
                        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    let s = v.get("m_LocalScale").and_then(Value::vec3).unwrap_or([1.0; 3]);
                    matrix = Mat4::from_scale_rotation_translation(
                        s.into(),
                        glam::Quat::from_array(r),
                        p.into(),
                    );
                }
                class_id::LIGHT => light = Some(v),
                _ => {}
            }
        }
        let Some(v) = light else { continue };
        return Ok(Some(Light {
            id: AssetId { file: "level2".into(), path_id: o.path_id },
            path: "CameraRig/CameraLight".into(),
            scene: "level2".into(),
            enabled: true,
            active: true,
            kind: v.i64_at("m_Type").unwrap_or(0) as i32,
            color: color(v.get("m_Color")),
            intensity: v.f32_at("m_Intensity").unwrap_or(1.0),
            range: v.f32_at("m_Range").unwrap_or(10.0),
            spot_angle: v.f32_at("m_SpotAngle").unwrap_or(30.0),
            inner_spot_angle: v.f32_at("m_InnerSpotAngle").unwrap_or(0.0),
            shadows: v.i64_at("m_Shadows.m_Type").unwrap_or(0) as i32,
            shadow_strength: v.f32_at("m_Shadows.m_Strength").unwrap_or(1.0),
            bake_type: v.i64_at("m_BakingOutput.lightmapBakeMode.lightmapBakeType").unwrap_or(4)
                as i32,
            culling_mask: u32::MAX,
            rendering_layer_mask: 1,
            matrix: matrix.to_cols_array(),
            ..Default::default()
        }));
    }
    Ok(None)
}
