//! Blueprints for the sim: the runtime pack `blueprints-pack.bbp` (what the game loads),
//! with the zip-derived content pack (`cache/content/blueprints.wrcpack`) filling gaps
//! (the coordinator's default for Decision 2, pending Ember). Records are decoded on first
//! use and cached; every blueprint remembers which source it came from.
//!
//! Field access goes through [`View`], which reads a decoded object by C# field name and
//! falls back to the field's C# initializer when the source didn't carry it (only the
//! zip side can omit fields; the `.bbp` writes every runtime field).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use wr_content::bbp::Bbp;
use wr_content::node::{Node, Object};
use wr_content::schema::{self, Def, TypeIdx};
use wr_content::{Guid, pack::Pack};

/// Where a blueprint was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Bbp,
    /// The zip-derived pack, for a GUID the `.bbp` doesn't have.
    Zip,
}

/// A decoded blueprint.
#[derive(Debug)]
pub struct Blueprint {
    pub guid: Guid,
    pub name: String,
    pub obj: Object,
    pub source: Source,
}

impl Blueprint {
    #[must_use]
    pub fn view(&self) -> View<'_> {
        View(&self.obj)
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        schema::ty(self.obj.ty).short
    }

    /// `ComponentsArray`: the blueprint's own components, in order (no `ComponentsList`
    /// expansion).
    #[must_use]
    pub fn components_array(&self) -> Vec<&Object> {
        self.view().objs("Components")
    }
}

/// Read access to a decoded object by C# field name.
#[derive(Debug, Clone, Copy)]
pub struct View<'a>(pub &'a Object);

impl<'a> View<'a> {
    #[must_use]
    pub fn ty(&self) -> TypeIdx {
        self.0.ty
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        schema::ty(self.0.ty).short
    }

    fn field(&self, name: &str) -> Option<(&'a Node, Def)> {
        let t = schema::ty(self.0.ty);
        let i = t.fields.iter().position(|f| f.name == name)?;
        Some((&self.0.fields[i], t.fields[i].default))
    }

    /// Whether the type has the field at all.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.field(name).is_some()
    }

    #[must_use]
    pub fn node(&self, name: &str) -> Option<&'a Node> {
        self.field(name).map(|(n, _)| unwrap(n))
    }

    /// An integer, enum, bool-as-int or char field; the initializer if absent; 0 if the
    /// field doesn't exist.
    #[must_use]
    pub fn int(&self, name: &str) -> i64 {
        match self.field(name) {
            Some((n, d)) => match unwrap(n) {
                Node::Int(i) | Node::Enum(i) => *i,
                Node::UInt(u) => i64::try_from(*u).unwrap_or(i64::MAX),
                Node::Bool(b) => i64::from(*b),
                Node::Absent => match d {
                    Def::Int(i) => i,
                    Def::Bool(b) => i64::from(b),
                    _ => 0,
                },
                _ => 0,
            },
            None => 0,
        }
    }

    #[must_use]
    pub fn i32(&self, name: &str) -> i32 {
        #[allow(clippy::cast_possible_truncation)]
        let v = self.int(name) as i32;
        v
    }

    #[must_use]
    pub fn bool(&self, name: &str) -> bool {
        self.int(name) != 0
    }

    #[must_use]
    pub fn f32(&self, name: &str) -> f32 {
        match self.field(name) {
            Some((n, d)) => match unwrap(n) {
                Node::F32(f) => *f,
                #[allow(clippy::cast_possible_truncation)]
                Node::F64(f) => *f as f32,
                #[allow(clippy::cast_precision_loss)]
                Node::Int(i) => *i as f32,
                #[allow(clippy::cast_possible_truncation)]
                Node::Absent => match d {
                    Def::Float(f) => f as f32,
                    #[allow(clippy::cast_precision_loss)]
                    Def::Int(i) => i as f32,
                    _ => 0.0,
                },
                _ => 0.0,
            },
            None => 0.0,
        }
    }

    #[must_use]
    pub fn str(&self, name: &str) -> Option<&'a str> {
        match self.node(name)? {
            Node::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A blueprint reference field (`BlueprintReference<T>`), or `None` when empty.
    #[must_use]
    pub fn guid(&self, name: &str) -> Option<Guid> {
        match self.node(name)? {
            Node::BpRef(g) if !g.is_empty() => Some(*g),
            _ => None,
        }
    }

    #[must_use]
    pub fn obj(&self, name: &str) -> Option<View<'a>> {
        self.node(name)?.as_object().map(View)
    }

    #[must_use]
    pub fn seq(&self, name: &str) -> &'a [Node] {
        match self.node(name) {
            Some(Node::Seq(v)) => v,
            _ => &[],
        }
    }

    /// The objects of a sequence field (nulls skipped).
    #[must_use]
    pub fn objs(&self, name: &str) -> Vec<&'a Object> {
        self.seq(name).iter().filter_map(|n| unwrap(n).as_object()).collect()
    }

    /// Blueprint references in a sequence field, keeping null entries as `None`.
    #[must_use]
    pub fn guids_with_nulls(&self, name: &str) -> Vec<Option<Guid>> {
        self.seq(name)
            .iter()
            .map(|n| match unwrap(n) {
                Node::BpRef(g) if !g.is_empty() => Some(*g),
                _ => None,
            })
            .collect()
    }

    /// Blueprint references in a sequence field (empty ones skipped).
    #[must_use]
    pub fn guids(&self, name: &str) -> Vec<Guid> {
        self.seq(name)
            .iter()
            .filter_map(|n| match unwrap(n) {
                Node::BpRef(g) if !g.is_empty() => Some(*g),
                _ => None,
            })
            .collect()
    }

    /// Whether this object's type is `T` or derives from it (by short or full name).
    #[must_use]
    pub fn is(&self, type_name: &str) -> bool {
        is_a(self.0.ty, type_name)
    }
}

fn unwrap(n: &Node) -> &Node {
    match n {
        Node::Coerced(inner, _) => unwrap(inner),
        other => other,
    }
}

/// `t` is the type named `name` (short or full) or derives from it.
#[must_use]
pub fn is_a(t: TypeIdx, name: &str) -> bool {
    let mut cur = t;
    for _ in 0..64 {
        if cur == schema::NO_TYPE {
            return false;
        }
        let d = schema::ty(cur);
        if d.short == name || d.name == name {
            return true;
        }
        cur = d.base;
    }
    false
}

/// Whether the content type with short name `ty` is `name` or derives from it.
#[must_use]
pub fn is_a_name(ty: &str, name: &str) -> bool {
    if ty == name {
        return true;
    }
    schema::types()
        .iter()
        .position(|t| t.short == ty)
        .is_some_and(|i| is_a(TypeIdx::try_from(i).unwrap_or(schema::NO_TYPE), name))
}

/// The blueprint store.
pub struct Content {
    bbp: Option<Bbp>,
    bbp_index: HashMap<Guid, u32>,
    pack: Option<Pack>,
    cache: RefCell<HashMap<Guid, Option<Rc<Blueprint>>>>,
    /// Problems decoding records (GUID, error), for the run report.
    pub errors: RefCell<Vec<(Guid, String)>>,
    pub bbp_path: Option<PathBuf>,
}

impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content")
            .field("bbp_records", &self.bbp_index.len())
            .field("pack", &self.pack.as_ref().map(Pack::len))
            .field("cached", &self.cache.borrow().len())
            .finish_non_exhaustive()
    }
}

impl Content {
    /// Opens the game's `.bbp` (from `WRATH_APP` or Steam) and the zip-derived pack if it
    /// exists.
    ///
    /// # Errors
    ///
    /// If neither source can be opened.
    pub fn open_default() -> Result<Self> {
        let paths = wr_paths::GamePaths::from_env();
        let bbp = paths.bundles().join("blueprints-pack.bbp");
        let pack = wr_paths::cache_root().join("content/blueprints.wrcpack");
        Self::open(Some(&bbp), Some(&pack))
    }

    /// Opens the given sources (either may be missing).
    ///
    /// # Errors
    ///
    /// If neither opens.
    pub fn open(bbp: Option<&Path>, pack: Option<&Path>) -> Result<Self> {
        let bbp_reader = match bbp {
            Some(p) if p.is_file() => Some(Bbp::open(p).with_context(|| p.display().to_string())?),
            _ => None,
        };
        let mut bbp_index = HashMap::new();
        if let Some(b) = &bbp_reader {
            for (g, off) in &b.toc {
                if *off != 0 {
                    bbp_index.insert(*g, *off);
                }
            }
        }
        let pack = match pack {
            // SAFETY: the pack is written by `wr_content::pack::build` (unique name,
            // read-only, renamed into place) and nothing modifies it while mapped.
            Some(p) if p.is_file() => unsafe { Pack::open(p) }.ok(),
            _ => None,
        };
        anyhow::ensure!(bbp_reader.is_some() || pack.is_some(), "no blueprint source found");
        Ok(Self {
            bbp: bbp_reader,
            bbp_index,
            pack,
            cache: RefCell::new(HashMap::new()),
            errors: RefCell::new(Vec::new()),
            bbp_path: bbp.map(Path::to_path_buf),
        })
    }

    /// An empty store, for tests that insert synthetic blueprints.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bbp: None,
            bbp_index: HashMap::new(),
            pack: None,
            cache: RefCell::new(HashMap::new()),
            errors: RefCell::new(Vec::new()),
            bbp_path: None,
        }
    }

    /// Adds a synthetic blueprint (tests).
    pub fn insert(&self, bp: Blueprint) {
        self.cache.borrow_mut().insert(bp.guid, Some(Rc::new(bp)));
    }

    #[must_use]
    pub fn has_bbp(&self) -> bool {
        self.bbp.is_some()
    }

    /// The blueprint with this GUID, or `None` (the game reads a missing blueprint as
    /// null).
    #[must_use]
    pub fn get(&self, g: Guid) -> Option<Rc<Blueprint>> {
        if let Some(b) = self.cache.borrow().get(&g) {
            return b.clone();
        }
        let b = self.load(g).map(Rc::new);
        self.cache.borrow_mut().insert(g, b.clone());
        b
    }

    fn load(&self, g: Guid) -> Option<Blueprint> {
        if let (Some(bbp), Some(off)) = (&self.bbp, self.bbp_index.get(&g)) {
            match bbp.decode(*off) {
                Ok(r) => {
                    return r.data.map(|obj| Blueprint {
                        guid: g,
                        name: r.name,
                        obj,
                        source: Source::Bbp,
                    });
                }
                Err(e) => self.errors.borrow_mut().push((g, format!("bbp: {e:#}"))),
            }
        }
        let pack = self.pack.as_ref()?;
        let e = pack.find(g)?;
        let p = match pack.load(&e) {
            Ok(p) => p,
            Err(err) => {
                self.errors.borrow_mut().push((g, format!("pack: {err:#}")));
                return None;
            }
        };
        let node = wr_content::pack::to_node(&p);
        let obj = match node {
            Node::Object(o) => *o,
            _ => return None,
        };
        let name = View(&obj).str("name").unwrap_or_default().to_owned();
        Some(Blueprint { guid: g, name, obj, source: Source::Zip })
    }

    /// Blueprints decoded so far, by source.
    #[must_use]
    pub fn loaded_by_source(&self) -> (usize, usize) {
        let c = self.cache.borrow();
        let bbp = c.values().flatten().filter(|b| b.source == Source::Bbp).count();
        let zip = c.values().flatten().filter(|b| b.source == Source::Zip).count();
        (bbp, zip)
    }
}

/// `BlueprintFact.CollectComponents`: the fact blueprint's components with every
/// `ComponentsList` expanded breadth-first (each list once), in order. Returned as
/// `(blueprint, index into its ComponentsArray)` pairs so callers can reach the object.
#[must_use]
pub fn collect_components(content: &Content, bp: &Rc<Blueprint>) -> Vec<(Rc<Blueprint>, usize)> {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    let mut seen = std::collections::HashSet::new();
    queue.push_back(bp.clone());
    seen.insert(bp.guid);
    while let Some(b) = queue.pop_front() {
        for (i, c) in b.components_array().into_iter().enumerate() {
            if is_a(c.ty, "ComponentsList") {
                if let Some(g) = View(c).guid("m_List")
                    && let Some(list) = content.get(g)
                    && seen.insert(g)
                {
                    queue.push_back(list);
                }
            } else {
                out.push((b.clone(), i));
            }
        }
    }
    out
}
