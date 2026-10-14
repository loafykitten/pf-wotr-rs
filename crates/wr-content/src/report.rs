//! The POC-2 content reports, each separate: JSON readability, typed coverage (per root
//! type and per nested `$type`), reference resolution, initialisation needs, and
//! behaviour support (zero for now, listed as a column).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use serde::Serialize;

use crate::decode::{self, RefUse, Tally};
use crate::guid::Guid;
use crate::node::Reason;
use crate::refs::{Entry, Index, RefTally};
use crate::schema::{self, TypeIdx, hooks};
use crate::source::BlueprintZip;

/// One blueprint's outcome.
#[derive(Debug)]
pub struct Row {
    pub pos: u32,
    pub guid: Option<Guid>,
    pub data_type: Option<TypeIdx>,
    pub opaque: u64,
    pub refs: Vec<RefUse>,
    pub error: Option<String>,
    pub bytes: usize,
}

/// Fields whose JSON values the runtime pack drops (see `packfmt`): how often the JSON
/// carries a non-zero value there.
#[derive(Debug, Default, Clone)]
pub struct Lossy {
    /// `(type, field slot)` → (present in JSON, non-zero in JSON).
    pub per_field: HashMap<(TypeIdx, u32), (u64, u64)>,
}

impl Lossy {
    fn merge(&mut self, o: Self) {
        for (k, (a, b)) in o.per_field {
            let e = self.per_field.entry(k).or_default();
            e.0 += a;
            e.1 += b;
        }
    }

    fn visit(&mut self, n: &crate::Node) {
        use crate::Node;
        match n {
            Node::Seq(v) => v.iter().for_each(|x| self.visit(x)),
            Node::Coerced(n, _) => self.visit(n),
            Node::Object(o) => {
                let def = schema::ty(o.ty);
                for (j, (f, v)) in def.fields.iter().zip(&o.fields).enumerate() {
                    if f.in_runtime()
                        && !crate::packfmt::keeps_value(&f.codec)
                        && !matches!(v, Node::Absent)
                    {
                        let nonzero = match v {
                            Node::Int(0) | Node::Enum(0) | Node::Null => false,
                            Node::F32(x) => *x != 0.0,
                            Node::F64(x) => *x != 0.0,
                            Node::Bool(b) => *b,
                            Node::Seq(items) => items.iter().any(|i| {
                                !matches!(i, Node::Int(0) | Node::F32(0.0) | Node::Bool(false))
                            }),
                            _ => true,
                        };
                        let e = self
                            .per_field
                            .entry((o.ty, u32::try_from(j).unwrap_or(u32::MAX)))
                            .or_default();
                        e.0 += 1;
                        if nonzero {
                            e.1 += 1;
                        }
                    }
                    self.visit(v);
                }
            }
            _ => {}
        }
    }
}

/// Decodes every blueprint, keeping per-blueprint rows and merged statistics.
#[derive(Debug)]
pub struct Run {
    pub rows: Vec<Row>,
    pub tally: Tally,
    pub lossy: Lossy,
    pub index: Index,
    pub refs: RefTally,
    pub seconds: f64,
}

/// Decodes the whole zip.
#[must_use]
pub fn run(zip: &BlueprintZip) -> Run {
    let t = Instant::now();
    let parts: Vec<(Vec<Row>, Tally, Lossy)> = zip.par_fold(
        || (Vec::new(), Tally::default(), Lossy::default()),
        |(rows, tally, lossy), pos, _name, bytes| {
            let pos = u32::try_from(pos).unwrap_or(u32::MAX);
            let row = match bytes {
                Err(e) => Row {
                    pos,
                    guid: None,
                    data_type: None,
                    opaque: 0,
                    refs: Vec::new(),
                    error: Some(format!("read: {e}")),
                    bytes: 0,
                },
                Ok(b) => match decode::decode_jbp(&b, tally) {
                    Ok(d) => {
                        lossy.visit(&d.wrapper);
                        Row {
                            pos,
                            guid: d.guid,
                            data_type: d.data_type,
                            opaque: d.opaque,
                            refs: d.refs,
                            error: None,
                            bytes: b.len(),
                        }
                    }
                    Err(e) => Row {
                        pos,
                        guid: None,
                        data_type: None,
                        opaque: 0,
                        refs: Vec::new(),
                        error: Some(format!("json: {e}")),
                        bytes: b.len(),
                    },
                },
            };
            rows.push(row);
        },
    );
    let mut rows = Vec::with_capacity(zip.len());
    let mut tally = Tally::default();
    let mut lossy = Lossy::default();
    for (r, t, l) in parts {
        rows.extend(r);
        tally.merge(t);
        lossy.merge(l);
    }
    rows.sort_by_key(|r| r.pos);
    let mut index = Index::default();
    for r in &rows {
        if let Some(g) = r.guid {
            index.insert(g, Entry { pos: r.pos, ty: r.data_type });
        }
    }
    let mut refs = RefTally::default();
    for r in &rows {
        let src = r.guid.unwrap_or_default();
        for u in &r.refs {
            refs.add(&index, src, *u);
        }
    }
    Run { rows, tally, lossy, index, refs, seconds: t.elapsed().as_secs_f64() }
}

#[derive(Debug, Serialize)]
pub struct Readability {
    pub entries: usize,
    pub parsed: usize,
    pub errors: usize,
    pub error_examples: Vec<(String, String)>,
    /// Parsed, with an `AssetId` GUID and a `Data` object.
    pub well_formed: usize,
    pub missing_asset_id: usize,
    pub duplicate_guids: usize,
    pub bytes: usize,
    pub decode_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct TypeRow {
    pub name: &'static str,
    pub type_id: String,
    /// Blueprints with this root type (root table) or tagged objects (nested table).
    pub count: u64,
    /// Decoded fully through real schemas: no opaque value anywhere in the object.
    pub typed: u64,
    pub typed_share: f64,
    pub fields_typed: u64,
    pub fields_opaque: u64,
    pub fields_absent: u64,
    /// JSON properties the runtime schema doesn't have (kept, not decoded).
    pub unknown_fields: u64,
    /// Lifecycle hooks a loader would have to run (see `init`).
    pub init: Vec<&'static str>,
    /// Behaviour implemented in wr-sim: none yet.
    pub behaviour: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Coverage {
    pub blueprints: usize,
    pub blueprints_typed: usize,
    pub blueprints_typed_share: f64,
    pub objects: u64,
    pub objects_clean: u64,
    pub opaque_values: u64,
    pub opaque_reasons: Vec<(String, u64)>,
    pub unknown_type_ids: Vec<(String, u64)>,
    pub unknown_fields_total: u64,
    /// Most frequent unknown properties: (type, property, count).
    pub unknown_fields_top: Vec<(String, String, u64)>,
    pub absent_fields_total: u64,
    pub absent_unknown_default: u64,
    pub absent_fields_top: Vec<(String, String, u64)>,
    /// Absent fields whose C# initializer we can't state: (type, field, count).
    pub absent_unknown_default_fields: Vec<(String, String, u64)>,
    /// Runtime fields whose values the pack drops: (type, field, codec, present in
    /// JSON, non-zero in JSON).
    pub pack_lossy_fields: Vec<(String, String, String, u64, u64)>,
    pub notes: BTreeMap<&'static str, u64>,
    pub root_types: Vec<TypeRow>,
    pub nested_types: Vec<TypeRow>,
    pub schema_types: usize,
    pub schema_type_ids: usize,
    pub type_ids_used: usize,
}

#[derive(Debug, Serialize)]
pub struct References {
    pub total: u64,
    pub resolved: u64,
    pub dangling: u64,
    pub wrong_type: u64,
    pub target_untyped: u64,
    pub per_target: Vec<(String, u64, u64, u64)>,
    pub wrong_pairs: Vec<(String, String, u64)>,
    pub dangling_examples: Vec<(String, String)>,
    pub wrong_examples: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
pub struct InitRow {
    pub name: &'static str,
    /// Objects of this type in the data (roots and nested).
    pub objects: u64,
    pub hooks: Vec<&'static str>,
    /// Instance fields that aren't serialized: state a loader or getter computes.
    pub runtime_fields: u16,
}

#[derive(Debug, Serialize)]
pub struct Init {
    /// Types in use whose `OnEnable`/`OnEnableWithLibrary` has a body (runs on load).
    pub on_enable_types: usize,
    pub on_enable_objects: u64,
    /// Types in use with non-serialized instance fields.
    pub runtime_field_types: usize,
    pub rows: Vec<InitRow>,
}

fn share(a: u64, b: u64) -> f64 {
    if b == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let s = a as f64 / b as f64;
    (s * 1e6).round() / 1e6
}

fn hook_names(h: u32) -> Vec<&'static str> {
    hooks::NAMES.iter().filter(|(b, _)| h & b != 0).map(|(_, n)| *n).collect()
}

fn reason_text(r: &Reason) -> String {
    match r {
        Reason::UnknownTypeId(_) => "unknown $type GUID".into(),
        Reason::NotAssignable { declared, .. } => {
            format!("$type not assignable to {}", schema::ty(*declared).name)
        }
        Reason::AbstractWithoutTag(t) => format!("no $type for abstract {}", schema::ty(*t).name),
        Reason::BadTypeTag => "malformed $type".into(),
        Reason::Mismatch { codec, json } => format!("{json} where {codec} expected"),
        Reason::Unsupported(w) => format!("unsupported type {w}"),
        Reason::ConverterError(w) => format!("converter would fail: {w}"),
    }
}

fn guid_hex(t: &schema::TypeDef) -> String {
    if t.guid == 0 { String::new() } else { format!("{:032x}", t.guid) }
}

impl Run {
    #[must_use]
    pub fn readability(&self, zip: &BlueprintZip) -> Readability {
        let names: Vec<&str> = zip.names().collect();
        let errors: Vec<&Row> = self.rows.iter().filter(|r| r.error.is_some()).collect();
        Readability {
            entries: self.rows.len(),
            parsed: self.rows.len() - errors.len(),
            errors: errors.len(),
            error_examples: errors
                .iter()
                .take(20)
                .map(|r| (names[r.pos as usize].to_owned(), r.error.clone().unwrap_or_default()))
                .collect(),
            well_formed: self
                .rows
                .iter()
                .filter(|r| r.error.is_none() && r.guid.is_some() && r.data_type.is_some())
                .count(),
            missing_asset_id: self
                .rows
                .iter()
                .filter(|r| r.error.is_none() && r.guid.is_none())
                .count(),
            duplicate_guids: self.index.duplicates.len(),
            bytes: self.rows.iter().map(|r| r.bytes).sum(),
            decode_seconds: (self.seconds * 100.0).round() / 100.0,
        }
    }

    #[must_use]
    pub fn coverage(&self) -> Coverage {
        let types = schema::types();
        // Root counts.
        let mut roots: HashMap<TypeIdx, (u64, u64)> = HashMap::new();
        for r in &self.rows {
            if let Some(t) = r.data_type {
                let e = roots.entry(t).or_default();
                e.0 += 1;
                if r.opaque == 0 {
                    e.1 += 1;
                }
            }
        }
        let row = |t: TypeIdx, count: u64, typed: u64| {
            let d = &types[t as usize];
            let tt = self.tally.per_type.get(t as usize).cloned().unwrap_or_default();
            TypeRow {
                name: d.name,
                type_id: guid_hex(d),
                count,
                typed,
                typed_share: share(typed, count),
                fields_typed: tt.fields_typed,
                fields_opaque: tt.fields_opaque,
                fields_absent: tt.fields_absent,
                unknown_fields: tt.unknown_fields,
                init: hook_names(d.hooks & (hooks::ON_ENABLE | hooks::ON_ENABLE_WITH_LIBRARY)),
                behaviour: "none",
            }
        };
        let mut root_types: Vec<TypeRow> =
            roots.iter().map(|(&t, &(n, typed))| row(t, n, typed)).collect();
        root_types.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(b.name)));
        // Nested: tagged objects that aren't blueprint roots.
        let mut nested_types = Vec::new();
        for (i, tt) in self.tally.per_type.iter().enumerate() {
            let t = TypeIdx::try_from(i).unwrap_or(u32::MAX);
            let root_n = roots.get(&t).map_or(0, |x| x.0);
            let nested = tt.tagged.saturating_sub(root_n);
            if nested == 0 {
                continue;
            }
            // `clean` counts all objects of the type; scale to the nested ones when the
            // type is also a root (never the case for components and elements).
            let root_clean = roots.get(&t).map_or(0, |x| x.1);
            let typed = tt.clean.saturating_sub(root_clean).min(nested);
            nested_types.push(row(t, nested, typed));
        }
        nested_types.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(b.name)));
        let mut opaque_reasons: HashMap<String, u64> = HashMap::new();
        for (r, n) in &self.tally.opaque {
            *opaque_reasons.entry(reason_text(r)).or_default() += n;
        }
        let mut opaque_reasons: Vec<(String, u64)> = opaque_reasons.into_iter().collect();
        opaque_reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut unknown_type_ids: Vec<(String, u64)> =
            self.tally.unknown_tags.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        unknown_type_ids.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut unknown_fields_top: Vec<(String, String, u64)> = self
            .tally
            .unknown_fields
            .iter()
            .map(|((t, k), v)| (types[*t as usize].name.to_owned(), k.to_string(), *v))
            .collect();
        unknown_fields_top.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        let unknown_fields_total = unknown_fields_top.iter().map(|x| x.2).sum();
        unknown_fields_top.truncate(60);
        let mut absent_fields_top: Vec<(String, String, u64)> = self
            .tally
            .absent
            .iter()
            .map(|((t, j), v)| {
                let d = &types[*t as usize];
                (d.name.to_owned(), d.fields[*j as usize].name.to_owned(), *v)
            })
            .collect();
        absent_fields_top.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        let absent_fields_total = absent_fields_top.iter().map(|x| x.2).sum();
        let absent_unknown_default_fields: Vec<(String, String, u64)> = self
            .tally
            .absent
            .iter()
            .filter(|((t, j), _)| {
                matches!(
                    types[*t as usize].fields[*j as usize].default,
                    schema::Def::Unknown | schema::Def::NonEmptyStr
                )
            })
            .map(|((t, j), v)| {
                let d = &types[*t as usize];
                (d.name.to_owned(), d.fields[*j as usize].name.to_owned(), *v)
            })
            .collect();
        absent_fields_top.truncate(60);
        let mut pack_lossy_fields: Vec<(String, String, String, u64, u64)> = self
            .lossy
            .per_field
            .iter()
            .map(|((t, j), (a, b))| {
                let d = &types[*t as usize];
                let f = &d.fields[*j as usize];
                (d.name.to_owned(), f.name.to_owned(), format!("{:?}", f.codec), *a, *b)
            })
            .collect();
        pack_lossy_fields.sort_by(|a, b| b.4.cmp(&a.4).then(b.3.cmp(&a.3)).then(a.0.cmp(&b.0)));
        let blueprints = self.rows.iter().filter(|r| r.error.is_none()).count();
        let typed = self.rows.iter().filter(|r| r.error.is_none() && r.opaque == 0).count();
        let objects: u64 = self.tally.per_type.iter().map(|t| t.objects).sum();
        let objects_clean: u64 = self.tally.per_type.iter().map(|t| t.clean).sum();
        let type_ids_used = self
            .tally
            .per_type
            .iter()
            .enumerate()
            .filter(|(i, t)| t.objects > 0 && types[*i].guid != 0)
            .count();
        Coverage {
            blueprints,
            blueprints_typed: typed,
            blueprints_typed_share: share(typed as u64, blueprints as u64),
            objects,
            objects_clean,
            opaque_values: self.tally.opaque_nodes,
            opaque_reasons,
            unknown_type_ids,
            unknown_fields_total,
            unknown_fields_top,
            absent_fields_total,
            absent_unknown_default: self.tally.absent_unknown_default,
            absent_fields_top,
            absent_unknown_default_fields,
            pack_lossy_fields,
            notes: self.tally.notes.iter().map(|(k, v)| (*k, *v)).collect(),
            root_types,
            nested_types,
            schema_types: types.len(),
            schema_type_ids: schema::type_id_count(),
            type_ids_used,
        }
    }

    #[must_use]
    pub fn references(&self) -> References {
        let types = schema::types();
        let r = &self.refs;
        let mut per_target: Vec<(String, u64, u64, u64)> = r
            .per_target
            .iter()
            .map(|(t, [a, b, c])| (types[*t as usize].name.to_owned(), *a, *b, *c))
            .collect();
        per_target.sort_by(|a, b| (b.1 + b.2 + b.3).cmp(&(a.1 + a.2 + a.3)).then(a.0.cmp(&b.0)));
        let mut wrong_pairs: Vec<(String, String, u64)> = r
            .wrong_pairs
            .iter()
            .map(|((e, a), n)| {
                (types[*e as usize].name.to_owned(), types[*a as usize].name.to_owned(), *n)
            })
            .collect();
        wrong_pairs.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        References {
            total: r.total,
            resolved: r.resolved,
            dangling: r.dangling,
            wrong_type: r.wrong_type,
            target_untyped: r.target_untyped,
            per_target,
            wrong_pairs,
            dangling_examples: r
                .dangling_examples
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            wrong_examples: r
                .wrong_examples
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    #[must_use]
    pub fn init(&self) -> Init {
        let types = schema::types();
        let mut rows: Vec<InitRow> = self
            .tally
            .per_type
            .iter()
            .enumerate()
            .filter(|(_, t)| t.objects > 0)
            .filter_map(|(i, t)| {
                let d = &types[i];
                let h = d.hooks & !(hooks::ON_VALIDATE | hooks::PACKING_CALLBACK);
                (h != 0 || d.runtime_fields > 0).then(|| InitRow {
                    name: d.name,
                    objects: t.objects,
                    hooks: hook_names(h),
                    runtime_fields: d.runtime_fields,
                })
            })
            .collect();
        rows.sort_by(|a, b| b.objects.cmp(&a.objects).then(a.name.cmp(b.name)));
        let on_enable =
            |r: &&InitRow| r.hooks.iter().any(|h| *h == "OnEnable" || *h == "OnEnableWithLibrary");
        Init {
            on_enable_types: rows.iter().filter(on_enable).count(),
            on_enable_objects: rows.iter().filter(on_enable).map(|r| r.objects).sum(),
            runtime_field_types: rows.iter().filter(|r| r.runtime_fields > 0).count(),
            rows,
        }
    }

    /// Writes every report under `dir`.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn write(&self, zip: &BlueprintZip, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let w = |name: &str, v: &dyn erased::Ser| -> Result<()> {
            std::fs::write(dir.join(name), v.to_json()?)?;
            Ok(())
        };
        w("readability.json", &self.readability(zip))?;
        w("coverage.json", &self.coverage())?;
        w("references.json", &self.references())?;
        w("init.json", &self.init())?;
        Ok(())
    }
}

/// Lets the writer take any `Serialize` behind one closure.
mod erased {
    pub trait Ser {
        fn to_json(&self) -> serde_json::Result<Vec<u8>>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_json(&self) -> serde_json::Result<Vec<u8>> {
            serde_json::to_vec_pretty(self)
        }
    }
}
