//! Animation data for the units in the pack: the rig of each unit, the clips and controllers
//! its locomotion plays, and the unit's `UnitAnimation` record.
//!
//! The game picks a unit's locomotion from its `UnitAnimationManager`'s `AnimationSet`: the first
//! action of type `LocoMotion`, either a `UnitAnimationActionLocoMotion` (a Mecanim controller,
//! the monsters') or a `UnitAnimationActionLocoMotionHuman` (plain clips, the human rig's).
//! Characters have no set of their own: `Character.AnimationSet` is the blueprint root's
//! `HumanAnimationSet`, `Human_AnimationSet` in `humananimation.animations`.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Context, Result, bail, ensure};
use glam::Mat4;
use wr_anim::clip::path_hash;
use wr_anim::controller::{
    Condition, Controller, Layer, Motion, Param, ParamKind, State, Transition,
};
use wr_anim::loco::{Locomotion, UnitAnimation};
use wr_anim::rig::{Binder, Bone, Override, Rig, Trs, sample_pose};

use super::UnitPrefab;
use crate::anim::decode_clip;
use crate::area::ObjRef;
use crate::pack::{Builder, SceneIndex};
use crate::unity::serialized::{SerializedFile, class_id};
use crate::unity::value::{PPtr, Value};

/// Where `BlueprintRoot.HumanAnimationSet` lives and what it is called.
const HUMAN_BUNDLE: &str = "humananimation.animations";
const HUMAN_SET: &str = "Human_AnimationSet";

/// A rig built from a prefab hierarchy, with the lookups meshes need.
#[derive(Debug)]
pub(super) struct BuiltRig {
    pub rig: Rig,
    /// Bone by transform name, the first match in `Character.CacheHierarchy`'s walk order.
    pub by_name: HashMap<String, u32>,
    /// Bone by transform id in the prefab's file.
    pub by_transform: HashMap<i64, u32>,
}

fn trs_of(t: &crate::transform::Trs) -> Trs {
    Trs {
        position: t.position.to_array(),
        rotation: t.rotation.to_array(),
        scale: t.scale.to_array(),
    }
}

/// The hierarchy below `root` as a rig. `parent_matrix` places the root's parent in the unit's
/// root space. With `root_identity` the root's own transform is reset (`Character.Start` puts the
/// instantiated rig at the character's origin). `skeleton` is the `Character.m_Skeleton` asset.
pub(super) fn build_rig(
    sc: &SceneIndex,
    root: i64,
    name: &str,
    parent_matrix: Mat4,
    root_identity: bool,
    skeleton: Option<&Value>,
) -> Result<BuiltRig> {
    let mut bones: Vec<Bone> = Vec::new();
    let mut by_transform: HashMap<i64, u32> = HashMap::new();
    let go_name = |t: i64| {
        sc.transform_go
            .get(&t)
            .and_then(|g| sc.gos.get(g))
            .map_or_else(String::new, |g| g.0.clone())
    };
    // Preorder, children in their stored order: parents precede children.
    let mut stack: Vec<(i64, u32, String)> = vec![(root, u32::MAX, String::new())];
    while let Some((t, parent, ppath)) = stack.pop() {
        let Some(node) = sc.nodes.get(&t) else { bail!("{name}: transform {t} unreadable") };
        ensure!(bones.len() < 4096, "{name}: more than 4096 bones");
        let n = go_name(t);
        let path = if parent == u32::MAX {
            String::new()
        } else if ppath.is_empty() {
            n.clone()
        } else {
            format!("{ppath}/{n}")
        };
        let idx = u32::try_from(bones.len())?;
        let rest = if t == root && root_identity { Trs::IDENTITY } else { trs_of(&node.local) };
        bones.push(Bone { name: n, hash: path_hash(&path), parent, rest });
        by_transform.insert(t, idx);
        if let Some(kids) = sc.children.get(&t) {
            for k in kids.iter().rev() {
                stack.push((*k, idx, path.clone()));
            }
        }
    }
    // `CacheHierarchy`: a stack walk that pops the last child first; the first transform of a name
    // wins.
    let mut by_name: HashMap<String, u32> = HashMap::new();
    let mut st = vec![root];
    while let Some(t) = st.pop() {
        if let Some(&i) = by_transform.get(&t) {
            by_name.entry(go_name(t)).or_insert(i);
        }
        if let Some(kids) = sc.children.get(&t) {
            st.extend(kids.iter().copied());
        }
    }
    let mut overrides = Vec::new();
    if let Some(sk) = skeleton {
        for bone in sk.get("Bones").and_then(Value::as_array).unwrap_or_default() {
            let Some(&i) = bone.str_at("Name").and_then(|n| by_name.get(n)) else { continue };
            overrides.push(Override {
                bone: i,
                scale: bone.get("Scale").and_then(Value::vec3).unwrap_or([1.0; 3]),
                offset: bone
                    .get("ApplyOffset")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .then(|| bone.get("Offset").and_then(Value::vec3).unwrap_or([0.0; 3])),
            });
        }
    }
    let rig = Rig { name: name.to_owned(), bones, root: parent_matrix.to_cols_array(), overrides };
    rig.validate()?;
    Ok(BuiltRig { rig, by_name, by_transform })
}

/// What the importer shares between units: interned clips, controllers and rigs.
#[derive(Debug, Default)]
pub(super) struct AnimCache {
    clips: HashMap<(String, i64), u32>,
    controllers: HashMap<(String, i64), u32>,
    rigs: HashMap<String, (u32, Rc<BuiltRig>)>,
    human_set: Option<ObjRef>,
    /// Things left out or approximated, for the pack report.
    pub notes: Vec<String>,
}

impl AnimCache {
    /// A rig interned under `key`, built by `make` the first time.
    pub fn rig(
        &mut self,
        b: &mut Builder<'_>,
        key: &str,
        make: impl FnOnce() -> Result<BuiltRig>,
    ) -> Result<(u32, Rc<BuiltRig>)> {
        if let Some(r) = self.rigs.get(key) {
            return Ok(r.clone());
        }
        let built = Rc::new(make()?);
        b.meta.rigs.push(built.rig.clone());
        let r = (u32::try_from(b.meta.rigs.len() - 1)?, built);
        self.rigs.insert(key.to_owned(), r.clone());
        Ok(r)
    }

    /// The pack index of the clip object `o`.
    fn clip(&mut self, b: &mut Builder<'_>, o: ObjRef) -> Result<u32> {
        let key = (o.file.name.clone(), o.info.path_id);
        if let Some(&i) = self.clips.get(&key) {
            return Ok(i);
        }
        ensure!(o.info.class_id == class_id::ANIMATION_CLIP, "object is not an AnimationClip");
        let clip = decode_clip(&o.read()?)?;
        b.meta.clips.push(clip);
        let i = u32::try_from(b.meta.clips.len() - 1)?;
        self.clips.insert(key, i);
        Ok(i)
    }

    /// The clip an `AnimationClipWrapper` points at.
    fn wrapper_clip(
        &mut self,
        b: &mut Builder<'_>,
        from: &SerializedFile<'_>,
        p: PPtr,
    ) -> Result<Option<u32>> {
        if p.is_null() {
            return Ok(None);
        }
        let Some(w) = b.universe.resolve(from, p)? else { return Ok(None) };
        let wv = w.read()?;
        let Some(cp) = wv.get("m_AnimationClip").and_then(Value::as_pptr).filter(|p| !p.is_null())
        else {
            return Ok(None);
        };
        let Some(c) = b.universe.resolve(w.file, cp)? else { return Ok(None) };
        Ok(Some(self.clip(b, c)?))
    }

    /// `Human_AnimationSet`.
    pub fn human_set(&mut self, b: &Builder<'_>) -> Result<ObjRef> {
        if let Some(s) = self.human_set {
            return Ok(s);
        }
        for f in b.universe.files_of(HUMAN_BUNDLE)? {
            for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
                let v = f.read_prefix(o, "m_Name")?;
                if v.str_at("m_Name") == Some(HUMAN_SET) {
                    let full = f.read(o)?;
                    if b.universe.script_of(f, &full)?.is_some_and(|s| s.class == "AnimationSet") {
                        let r = b
                            .universe
                            .resolve(f, PPtr { file_id: 0, path_id: o.path_id })?
                            .context("human set")?;
                        self.human_set = Some(r);
                        return Ok(r);
                    }
                }
            }
        }
        bail!("{HUMAN_BUNDLE}: no {HUMAN_SET}")
    }

    /// The pack index of an `AnimatorController`.
    fn controller(&mut self, b: &mut Builder<'_>, o: ObjRef) -> Result<u32> {
        let key = (o.file.name.clone(), o.info.path_id);
        if let Some(&i) = self.controllers.get(&key) {
            return Ok(i);
        }
        ensure!(
            o.info.class_id == class_id::ANIMATOR_CONTROLLER,
            "object is not an AnimatorController"
        );
        let v = o.read()?;
        let mut clips = Vec::new();
        for p in v.get("m_AnimationClips").and_then(Value::as_array).unwrap_or_default() {
            let c = p
                .as_pptr()
                .filter(|p| !p.is_null())
                .map(|p| b.universe.resolve(o.file, p))
                .transpose()?
                .flatten()
                .context("controller clip not found")?;
            clips.push(self.clip(b, c)?);
        }
        let c = convert_controller(&v, clips)?;
        wr_anim::controller::check_supported(&c)?;
        b.meta.controllers.push(c);
        let i = u32::try_from(b.meta.controllers.len() - 1)?;
        self.controllers.insert(key, i);
        Ok(i)
    }

    /// The unit's locomotion from its `AnimationSet`: the first action that is a `LocoMotion`.
    /// Returns the locomotion and the pack clips it plays.
    pub fn locomotion(
        &mut self,
        b: &mut Builder<'_>,
        set: ObjRef,
    ) -> Result<(Locomotion, Vec<u32>)> {
        let sv = set.read()?;
        for ap in sv.get("m_Actions").and_then(Value::as_array).unwrap_or_default() {
            let Some(p) = ap.as_pptr().filter(|p| !p.is_null()) else { continue };
            let Some(a) = b.universe.resolve(set.file, p)? else { continue };
            let av = a.read()?;
            let class = b.universe.script_of(a.file, &av)?.map(|s| s.class).unwrap_or_default();
            match class.as_str() {
                "UnitAnimationActionLocoMotionHuman" => return self.human_loco(b, a, &av),
                "UnitAnimationActionLocoMotion" => return self.controller_loco(b, a, &av),
                _ => {}
            }
        }
        bail!("the animation set has no locomotion action")
    }

    fn human_loco(
        &mut self,
        b: &mut Builder<'_>,
        a: ObjRef,
        v: &Value,
    ) -> Result<(Locomotion, Vec<u32>)> {
        let idle = v
            .get("NonCombatIdleWrapper")
            .and_then(Value::as_pptr)
            .map(|p| self.wrapper_clip(b, a.file, p))
            .transpose()?
            .flatten()
            .context("no idle clip")?;
        let mut walk = Vec::new();
        for w in v.get("NonCombatWalkWrappers").and_then(Value::as_array).unwrap_or_default() {
            walk.push(match w.as_pptr() {
                Some(p) => self.wrapper_clip(b, a.file, p)?,
                None => None,
            });
        }
        let transition = v.f32_at("TransitionIn").unwrap_or(0.0);
        let mut clips: Vec<u32> =
            std::iter::once(idle).chain(walk.iter().flatten().copied()).collect();
        clips.sort_unstable();
        clips.dedup();
        Ok((Locomotion::Clips { idle, walk, transition }, clips))
    }

    fn controller_loco(
        &mut self,
        b: &mut Builder<'_>,
        a: ObjRef,
        v: &Value,
    ) -> Result<(Locomotion, Vec<u32>)> {
        ensure!(
            v.get("m_WeaponStyleOverrides").and_then(Value::as_array).is_none_or(<[_]>::is_empty),
            "weapon style layers are not modelled"
        );
        let cp = v
            .get("m_AnimatorController")
            .and_then(Value::as_pptr)
            .filter(|p| !p.is_null())
            .context("no animator controller")?;
        let co = b.universe.resolve(a.file, cp)?.context("animator controller not found")?;
        let controller = self.controller(b, co)?;
        let c = &b.meta.controllers[controller as usize];
        let mut clips = c.clips.clone();
        clips.sort_unstable();
        clips.dedup();
        let layer = u32::try_from(v.i64_at("m_DefaultLayerIndex").unwrap_or(1))?;
        ensure!((layer as usize) < c.layers.len(), "default layer {layer}");
        ensure!(
            !c.layers[layer as usize].states.is_empty(),
            "default layer {layer} has no states the player models (masked or additive)"
        );
        Ok((
            Locomotion::Controller {
                controller,
                layer,
                default_speed_mps: v.f32_at("DefaultSpeedMps").unwrap_or(3.3),
                has_move_type: v.get("m_HasMoveType").and_then(Value::as_bool).unwrap_or(false),
                has_idle_offset: v.get("m_HasOffset").and_then(Value::as_bool).unwrap_or(false),
            },
            clips,
        ))
    }

    /// Records a unit's animation: its rig, a locomotion read with [`AnimCache::locomotion`] and the
    /// clips it plays. Notes (to the pack report) any clip property no bone of the rig has.
    pub fn animation(
        &mut self,
        b: &mut Builder<'_>,
        unit: &str,
        rig: u32,
        loco: Locomotion,
        clips: Vec<u32>,
    ) -> Result<u32> {
        let anim = UnitAnimation { unit: unit.to_owned(), rig, clips, loco };
        anim.validate(b.meta.rigs.len(), b.meta.clips.len(), &b.meta.controllers)?;
        let r = &b.meta.rigs[rig as usize];
        for &c in &anim.clips {
            let clip = &b.meta.clips[c as usize];
            let binder = Binder::new(clip, r);
            if binder.unbound > 0 {
                self.notes.push(format!(
                    "{}: clip {} binds {} of {} transform properties to no bone of {}",
                    unit,
                    clip.name,
                    binder.unbound,
                    binder.unbound + binder.entries.len(),
                    r.name
                ));
            }
        }
        b.meta.animations.push(anim);
        Ok(u32::try_from(b.meta.animations.len() - 1)?)
    }
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).unwrap_or_default()
}

fn data(v: &Value) -> &Value {
    v.get("data").unwrap_or(v)
}

fn u32_at(v: &Value, key: &str) -> u32 {
    v.i64_at(key).unwrap_or(0) as u32
}

/// `ControllerConstant` to the player's controller.
fn convert_controller(v: &Value, clips: Vec<u32>) -> Result<Controller> {
    let name = v.str_at("m_Name").unwrap_or_default().to_owned();
    let cc = v.get("m_Controller").context("no controller constant")?;
    let names: HashMap<u32, String> = arr(v, "m_TOS")
        .iter()
        .map(|p| (u32_at(p, "first"), p.str_at("second").unwrap_or_default().to_owned()))
        .collect();
    let defaults = data(cc.get("m_DefaultValues").context("no default values")?);
    let nth = |key: &str, i: usize| -> f32 {
        arr(defaults, key)
            .get(i)
            .and_then(|x| x.as_f32().or_else(|| x.as_bool().map(f32::from)))
            .unwrap_or(0.0)
    };
    let mut params = Vec::new();
    for p in arr(data(cc.get("m_Values").context("no values")?), "m_ValueArray") {
        let id = u32_at(p, "m_ID");
        let idx = p.i64_at("m_Index").unwrap_or(0) as usize;
        let (kind, default) = match p.i64_at("m_Type").unwrap_or(0) {
            1 => (ParamKind::Float, nth("m_FloatValues", idx)),
            3 => (ParamKind::Int, nth("m_IntValues", idx)),
            4 => (ParamKind::Bool, nth("m_BoolValues", idx)),
            9 => (ParamKind::Trigger, 0.0),
            // Positions, rotations and scales of the animated hierarchy's defaults.
            _ => continue,
        };
        params.push(Param { id, name: names.get(&id).cloned().unwrap_or_default(), kind, default });
    }
    let param_of = |id: u32| -> Result<u32> {
        let i =
            params.iter().position(|p| p.id == id).with_context(|| format!("parameter {id}"))?;
        Ok(i as u32)
    };
    let machines = arr(cc, "m_StateMachineArray");
    let mut layers = Vec::new();
    for l in arr(cc, "m_LayerArray") {
        let l = data(l);
        let mi = l.i64_at("m_StateMachineIndex").unwrap_or(0) as usize;
        let sm = data(machines.get(mi).context("layer without a state machine")?);
        let states_v = arr(sm, "m_StateConstantArray");
        let layer_name = names.get(&u32_at(l, "m_Binding")).cloned().unwrap_or_default();
        let additive = l.i64_at("(int&)m_LayerBlendingMode").unwrap_or(0) != 0;
        let masked = l
            .path("m_SkeletonMask.data.m_Data")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        let transitions_of = |arr_v: &[Value]| -> Result<Vec<Transition>> {
            let mut out = Vec::new();
            for t in arr_v {
                let t = data(t);
                let dest = u32_at(t, "m_DestinationState");
                // Exits and sub-state machine entries are numbered past the states.
                if dest as usize >= states_v.len() {
                    continue;
                }
                let mut conditions = Vec::new();
                for c in arr(t, "m_ConditionConstantArray") {
                    let c = data(c);
                    conditions.push(Condition {
                        mode: c.i64_at("m_ConditionMode").unwrap_or(0) as u8,
                        param: param_of(u32_at(c, "m_EventID"))?,
                        threshold: c.f32_at("m_EventThreshold").unwrap_or(0.0),
                    });
                }
                out.push(Transition {
                    dest,
                    conditions,
                    has_exit_time: t.get("m_HasExitTime").and_then(Value::as_bool).unwrap_or(false),
                    exit_time: t.f32_at("m_ExitTime").unwrap_or(0.0),
                    fixed_duration: t
                        .get("m_HasFixedDuration")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    duration: t.f32_at("m_TransitionDuration").unwrap_or(0.0),
                    offset: t.f32_at("m_TransitionOffset").unwrap_or(0.0),
                    can_transition_to_self: t
                        .get("m_CanTransitionToSelf")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                });
            }
            Ok(out)
        };
        let mut states = Vec::new();
        for s in states_v {
            let s = data(s);
            let tree_i = arr(s, "m_BlendTreeConstantIndexArray")
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let tree = usize::try_from(tree_i)
                .ok()
                .and_then(|i| arr(s, "m_BlendTreeConstantArray").get(i))
                .context("state without a blend tree")?;
            let nodes = arr(data(tree), "m_NodeArray");
            let optional = |id: u32| (id != 0).then(|| param_of(id)).transpose();
            states.push(State {
                name: names.get(&u32_at(s, "m_NameID")).cloned().unwrap_or_default(),
                motion: motion(nodes, 0, &param_of)?,
                speed: s.f32_at("m_Speed").unwrap_or(1.0),
                speed_param: optional(u32_at(s, "m_SpeedParamID"))?,
                cycle_offset: s.f32_at("m_CycleOffset").unwrap_or(0.0),
                cycle_param: optional(u32_at(s, "m_CycleOffsetParamID"))?,
                looping: s.get("m_Loop").and_then(Value::as_bool).unwrap_or(true),
                transitions: transitions_of(arr(s, "m_TransitionConstantArray"))?,
            });
        }
        let any_state = transitions_of(arr(sm, "m_AnyStateTransitionConstantArray"))?;
        let mut layer = Layer {
            name: layer_name,
            default_state: u32_at(sm, "m_DefaultState"),
            states,
            any_state,
            default_weight: l.f32_at("m_DefaultWeight").unwrap_or(0.0),
        };
        if additive || masked {
            // The player doesn't model additive or masked layers: drop their states so they are
            // never played, and let the unit's layer choice fail if it picks one.
            layer.states.clear();
            layer.any_state.clear();
        }
        layers.push(layer);
    }
    let c = Controller { name, params, clips, layers };
    Ok(c)
}

/// A blend tree node as a motion: a leaf plays a clip; a 1D tree blends its children by the
/// parameter its `m_BlendEventID` names.
fn motion(nodes: &[Value], i: usize, param_of: &dyn Fn(u32) -> Result<u32>) -> Result<Motion> {
    let n = data(nodes.get(i).context("blend tree node out of range")?);
    let kids: Vec<usize> =
        arr(n, "m_ChildIndices").iter().filter_map(Value::as_i64).map(|k| k as usize).collect();
    if kids.is_empty() {
        return Ok(Motion::Clip(u32_at(n, "m_ClipID")));
    }
    ensure!(n.i64_at("m_BlendType") == Some(0), "blend tree type {:?}", n.i64_at("m_BlendType"));
    let thresholds: Vec<f32> =
        arr(data(n.get("m_Blend1dData").context("1D data")?), "m_ChildThresholdArray")
            .iter()
            .filter_map(Value::as_f32)
            .collect();
    ensure!(thresholds.len() == kids.len(), "blend tree thresholds");
    let mut children = Vec::new();
    for (k, t) in kids.iter().zip(thresholds) {
        children.push((t, motion(nodes, *k, param_of)?));
    }
    Ok(Motion::Blend1D { param: param_of(u32_at(n, "m_BlendEventID"))?, children })
}

/// The human animation set, or the manager's own, for a unit prefab: `Animator` transform and set.
pub(super) fn plain_animator(
    b: &Builder<'_>,
    cache: &mut AnimCache,
    prefab: &UnitPrefab,
) -> Result<(i64, ObjRef)> {
    let sc = &prefab.sc;
    for o in sc.file.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
        let v = sc.file.read(o)?;
        if !b.universe.script_of(sc.file, &v)?.is_some_and(|s| s.class == "UnitAnimationManager") {
            continue;
        }
        let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
        let Some(&t) = sc.go_transform.get(&go) else { continue };
        if !prefab.under_root(t) {
            continue;
        }
        let set = match v.get("m_AnimationSet").and_then(Value::as_pptr).filter(|p| !p.is_null()) {
            Some(p) => b.universe.resolve(sc.file, p)?.context("animation set not found")?,
            None if prefab.has_script(b, "Character")? => cache.human_set(b)?,
            None => bail!("the animation manager has no animation set"),
        };
        return Ok((t, set));
    }
    bail!("no UnitAnimationManager under the unit's root")
}

/// The transform the clips' paths start from. That is the `Animator`'s own transform, except for
/// prefabs that nest the rig below it (the monitor lizard's animator sits on a `FakePivot` above
/// the rig's root): the descendant under which most of the clips' transform properties find a
/// bone. Ties go to the shallowest.
pub(super) fn bind_root(b: &Builder<'_>, sc: &SceneIndex, animator: i64, clips: &[u32]) -> i64 {
    let hashes: Vec<u32> = clips
        .iter()
        .flat_map(|c| &b.meta.clips[*c as usize].bindings)
        .filter(|x| x.type_id == wr_anim::clip::TYPE_TRANSFORM)
        .map(|x| x.path)
        .collect();
    let score = |root: i64| -> usize {
        let mut set = std::collections::HashSet::new();
        let mut stack = vec![(root, String::new(), true)];
        while let Some((t, path, is_root)) = stack.pop() {
            set.insert(path_hash(&path));
            let Some(kids) = sc.children.get(&t) else { continue };
            for k in kids {
                let n =
                    sc.transform_go.get(k).and_then(|g| sc.gos.get(g)).map_or("", |g| g.0.as_str());
                let p = if is_root { n.to_owned() } else { format!("{path}/{n}") };
                stack.push((*k, p, false));
            }
        }
        hashes.iter().filter(|h| set.contains(h)).count()
    };
    let mut best = (score(animator), animator);
    let mut stack: Vec<i64> = sc.children.get(&animator).cloned().unwrap_or_default();
    while let Some(t) = stack.pop() {
        let s = score(t);
        if s > best.0 {
            best = (s, t);
        }
        stack.extend(sc.children.get(&t).into_iter().flatten().copied());
    }
    best.1
}

/// World matrices of the rig at a few times of every clip the unit's locomotion plays, plus the
/// rest pose: the poses a unit's mesh bounds must cover.
pub(super) fn bound_poses(b: &Builder<'_>, anim: &UnitAnimation, samples: usize) -> Vec<Vec<Mat4>> {
    let rig = &b.meta.rigs[anim.rig as usize];
    let mut out = Vec::new();
    let mut w = Vec::new();
    rig.world_matrices(&rig.posed_rest(), &mut w);
    out.push(w.clone());
    let mut scratch = Vec::new();
    for &c in &anim.clips {
        let clip = &b.meta.clips[c as usize];
        let binder = Binder::new(clip, rig);
        for k in 0..samples {
            let t = clip.start + clip.length() * (k as f32) / (samples as f32);
            let mut pose = sample_pose(clip, &binder, rig, t, &mut scratch);
            rig.apply_overrides(&mut pose);
            rig.world_matrices(&pose, &mut w);
            out.push(w.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(fields: &[(&str, Value)]) -> Value {
        Value::Object(fields.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
    }
    fn int(i: i64) -> Value {
        Value::Int(i)
    }
    fn arr(v: Vec<Value>) -> Value {
        Value::Array(v)
    }
    fn data(v: Value) -> Value {
        obj(&[("data", v)])
    }

    /// A blend tree constant: one leaf node playing `clip`, or a 1D tree over two clips.
    fn tree(clips: &[i64]) -> Value {
        let leaf = |c: i64| {
            data(obj(&[
                ("m_BlendType", int(0)),
                ("m_ChildIndices", arr(vec![])),
                ("m_ClipID", int(c)),
                ("m_BlendEventID", int(4_294_967_295)),
                ("m_Blend1dData", data(obj(&[("m_ChildThresholdArray", arr(vec![]))]))),
            ]))
        };
        let mut nodes = Vec::new();
        if clips.len() == 1 {
            nodes.push(leaf(clips[0]));
        } else {
            nodes.push(data(obj(&[
                ("m_BlendType", int(0)),
                ("m_ChildIndices", arr((1..=clips.len() as i64).map(int).collect())),
                ("m_ClipID", int(0)),
                ("m_BlendEventID", int(10)),
                (
                    "m_Blend1dData",
                    data(obj(&[(
                        "m_ChildThresholdArray",
                        arr((0..clips.len()).map(|i| Value::Float(i as f32)).collect()),
                    )])),
                ),
            ])));
            nodes.extend(clips.iter().map(|c| leaf(*c)));
        }
        data(obj(&[("m_NodeArray", arr(nodes))]))
    }

    fn transition(dest: i64, mode: i64, event: i64, threshold: f32) -> Value {
        data(obj(&[
            ("m_DestinationState", int(dest)),
            (
                "m_ConditionConstantArray",
                arr(vec![data(obj(&[
                    ("m_ConditionMode", int(mode)),
                    ("m_EventID", int(event)),
                    ("m_EventThreshold", Value::Float(threshold)),
                ]))]),
            ),
            ("m_HasExitTime", Value::Bool(false)),
            ("m_ExitTime", Value::Float(0.9)),
            ("m_HasFixedDuration", Value::Bool(true)),
            ("m_TransitionDuration", Value::Float(0.15)),
            ("m_TransitionOffset", Value::Float(0.0)),
            ("m_CanTransitionToSelf", Value::Bool(true)),
        ]))
    }

    fn state(name: i64, clips: &[i64], speed_param: i64, trs: Vec<Value>) -> Value {
        data(obj(&[
            ("m_BlendTreeConstantArray", arr(vec![tree(clips)])),
            ("m_BlendTreeConstantIndexArray", arr(vec![int(0)])),
            ("m_Speed", Value::Float(1.5)),
            ("m_SpeedParamID", int(speed_param)),
            ("m_CycleOffset", Value::Float(0.0)),
            ("m_CycleOffsetParamID", int(0)),
            ("m_Loop", Value::Bool(true)),
            ("m_NameID", int(name)),
            ("m_PathID", int(name)),
            ("m_TransitionConstantArray", arr(trs)),
        ]))
    }

    fn controller_value() -> Value {
        let names = |h: i64, n: &str| obj(&[("first", int(h)), ("second", Value::Str(n.into()))]);
        obj(&[
            ("m_Name", Value::Str("ctl".into())),
            (
                "m_TOS",
                arr(vec![
                    names(10, "WalkSpeed"),
                    names(11, "MoveType"),
                    names(20, "Idle"),
                    names(21, "Walk"),
                ]),
            ),
            (
                "m_Controller",
                obj(&[
                    (
                        "m_Values",
                        data(obj(&[(
                            "m_ValueArray",
                            arr(vec![
                                obj(&[("m_ID", int(10)), ("m_Index", int(0)), ("m_Type", int(1))]),
                                obj(&[("m_ID", int(11)), ("m_Index", int(0)), ("m_Type", int(3))]),
                                // A default position: not a parameter.
                                obj(&[("m_ID", int(12)), ("m_Index", int(0)), ("m_Type", int(5))]),
                            ]),
                        )])),
                    ),
                    (
                        "m_DefaultValues",
                        data(obj(&[
                            ("m_FloatValues", arr(vec![Value::Float(0.25)])),
                            ("m_IntValues", arr(vec![int(2)])),
                            ("m_BoolValues", arr(vec![])),
                        ])),
                    ),
                    (
                        "m_LayerArray",
                        arr(vec![data(obj(&[
                            ("m_StateMachineIndex", int(0)),
                            ("m_DefaultWeight", Value::Float(0.0)),
                            ("m_Binding", int(20)),
                            ("(int&)m_LayerBlendingMode", int(0)),
                        ]))]),
                    ),
                    (
                        "m_StateMachineArray",
                        arr(vec![data(obj(&[
                            ("m_DefaultState", int(0)),
                            ("m_AnyStateTransitionConstantArray", arr(vec![])),
                            (
                                "m_StateConstantArray",
                                arr(vec![
                                    state(
                                        20,
                                        &[0],
                                        0,
                                        vec![
                                            transition(1, 3, 10, 0.0),
                                            transition(30000, 3, 10, 1.0),
                                        ],
                                    ),
                                    state(21, &[1, 2], 10, vec![transition(0, 4, 10, 0.01)]),
                                ]),
                            ),
                        ]))]),
                    ),
                ]),
            ),
        ])
    }

    #[test]
    fn a_controller_constant_becomes_the_players_controller() {
        let c = convert_controller(&controller_value(), vec![7, 8, 9]).unwrap();
        c.validate(10).unwrap();
        wr_anim::controller::check_supported(&c).unwrap();
        // Parameters keep their types and defaults; the position default isn't one.
        assert_eq!(c.params.len(), 2);
        assert_eq!(c.params[0].name, "WalkSpeed");
        assert_eq!((c.params[0].kind, c.params[0].default), (ParamKind::Float, 0.25));
        assert_eq!((c.params[1].kind, c.params[1].default), (ParamKind::Int, 2.0));
        let l = &c.layers[0];
        assert_eq!(l.name, "Idle");
        assert_eq!(l.states.len(), 2);
        assert_eq!(l.states[0].name, "Idle");
        assert_eq!(l.states[0].motion, Motion::Clip(0));
        // Idle -> Walk on WalkSpeed > 0; the transition to state 30000 (an exit) is dropped.
        assert_eq!(l.states[0].transitions.len(), 1);
        let t = &l.states[0].transitions[0];
        assert_eq!((t.dest, t.conditions[0].mode, t.conditions[0].param), (1, 3, 0));
        assert!(t.fixed_duration && (t.duration - 0.15).abs() < 1e-6);
        // The walk state multiplies its speed by WalkSpeed and blends two clips by it.
        assert_eq!(l.states[1].speed_param, Some(0));
        assert!((l.states[1].speed - 1.5).abs() < 1e-6);
        match &l.states[1].motion {
            Motion::Blend1D { param, children } => {
                assert_eq!(*param, 0);
                assert_eq!(children.len(), 2);
                assert_eq!(children[1], (1.0, Motion::Clip(2)));
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn unknown_parameters_and_blend_types_are_errors() {
        assert!(convert_controller(&controller_value(), vec![0, 1, 2]).is_ok());
        // A condition on a parameter the controller doesn't have.
        let mut bad = controller_value();
        replace_int(&mut bad, "m_EventID", 10, 99);
        assert!(convert_controller(&bad, vec![0, 1, 2]).is_err());
        let mut tree2d = controller_value();
        replace_int(&mut tree2d, "m_BlendType", 0, 1);
        // Only the multi-child node is a tree; leaves ignore the type, so this must still convert
        // for leaves and fail for the blend node.
        assert!(convert_controller(&tree2d, vec![0, 1, 2]).is_err());
    }

    /// Sets every integer field `key` equal to `from` to `to`, anywhere in the value.
    fn replace_int(v: &mut Value, key: &str, from: i64, to: i64) {
        match v {
            Value::Object(f) => {
                for (k, x) in f.iter_mut() {
                    if k == key && x.as_i64() == Some(from) {
                        *x = Value::Int(to);
                    } else {
                        replace_int(x, key, from, to);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| replace_int(x, key, from, to)),
            _ => {}
        }
    }
}
