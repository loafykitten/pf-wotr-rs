//! The canonical form of a decoded blueprint, for object-by-object comparison with the
//! live game (spec: `docs/poc2/content.md`, "Canonical form").
//!
//! It describes the object **as the running game holds it**: the game loads blueprints
//! from `blueprints-pack.bbp` through `ReflectionBasedSerializer`, so the canonical form
//! follows that serializer's field set and its normalisations, applied to what the JSON
//! says:
//!
//! - fields: `BlueprintFieldsTraverser.GetUnitySerializedFields` order (base class first,
//!   declaration order), keyed by C# field name; `[JsonIgnore]`, `[ExcludeFieldFromBuild]`,
//!   Unity's `name` property and JSON properties the runtime type lacks are left out;
//! - identified objects (blueprints, components, elements) carry `"$type"` (the TypeId) and
//!   may be null; other `[Serializable]` objects are never null (the pack writes a fresh
//!   instance for null), strings are never null (`""`), arrays and lists are never null
//!   (`[]`);
//! - a field missing from the JSON has its C# initializer value;
//! - enums are integers; blueprint references are GUID strings or null; assets are
//!   `{"guid", "fileid"}` or null; shared strings are `{"stringkey"}` or null; weak links
//!   are their `AssetId` (`""` when empty);
//! - floats are written as the shortest text that round-trips the `f32`/`f64`; non-finite
//!   values as `"NaN"`, `"Infinity"`, `"-Infinity"`. Comparison parses numbers back to the
//!   field's type and compares bits (any NaN equals any NaN), so the game side may format
//!   floats however it likes as long as they round-trip;
//! - curves are `{"keys": [[time, value, inTangent, outTangent, inWeight, outWeight,
//!   weightedMode], …]}` with keys sorted by time and later keys at an existing time
//!   dropped (`AnimationCurve.AddKey`); the pack doesn't store wrap modes; gradients are
//!   `{"colors": [[t, r, g, b]…], "alphas": [[t, a]…]}`; vectors, colours and bounds are
//!   number arrays;
//! - build-time rewrites the pack shows: a `LocalizedString` holds its shared string's key
//!   in `m_Key` and no `Shared`; a `ColorBlock` loses its selected colour; fields the pack can't encode read as zero
//!   (none in this game's schema, see `packfmt`).
//!
//! Anything that can't be stated exactly is written as `{"$unknown": "<why>"}` so a diff
//! flags it instead of passing silently.

use std::fmt::Write as _;

use crate::guid::Guid;
use crate::node::{Curve, Node, Object};
use crate::packfmt::{self, ElemKind, PackKind};
use crate::schema::{self, Codec, Def, TypeIdx, UnityKind, type_flags};

/// How Unity object references are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Assets {
    /// `{"guid", "fileid"}` / `{"stringkey"}` (or `{"asset_index"}` from the `.bbp`).
    #[default]
    Full,
    /// `"asset"` or null: for comparing the zip with the `.bbp` before the pack's asset
    /// list is decoded.
    Presence,
}

/// Canonical text of one blueprint: `{"guid", "name", "$type", fields…}` (pretty, two
/// spaces).
#[must_use]
pub fn blueprint(guid: Guid, name: &str, data: &Object) -> String {
    blueprint_with(guid, name, data, Options::default())
}

/// Where the decoded object came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The zip's JSON: the build-time rewrites the pack shows are applied, so the result
    /// predicts the runtime object.
    #[default]
    Json,
    /// The `.bbp`: already the runtime object; written literally.
    Pack,
}

/// Options for the canonical form.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub assets: Assets,
    pub source: Source,
}

/// [`blueprint`] with options.
#[must_use]
pub fn blueprint_with(guid: Guid, name: &str, data: &Object, opts: Options) -> String {
    let mut w =
        W { out: String::new(), indent: 0, depth: 0, assets: opts.assets, source: opts.source };
    w.out.push_str("{\n");
    w.indent = 1;
    w.key("guid");
    w.string(&guid.to_string());
    w.out.push_str(",\n");
    w.key("name");
    w.string(name);
    w.out.push_str(",\n");
    w.object_body(data.ty, Some(data));
    w.out.push_str("\n}\n");
    w.out
}

/// Canonical text of a value with a codec (for tests and tools).
#[must_use]
pub fn value(codec: Codec, n: &Node) -> String {
    let mut w =
        W { out: String::new(), indent: 0, depth: 0, assets: Assets::Full, source: Source::Json };
    w.value(codec, n);
    w.out
}

struct W {
    out: String,
    indent: usize,
    depth: usize,
    assets: Assets,
    source: Source,
}

const MAX_DEPTH: usize = 64;

impl W {
    fn pad(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    fn key(&mut self, k: &str) {
        self.pad();
        self.string(k);
        self.out.push_str(": ");
    }

    fn string(&mut self, s: &str) {
        // serde_json's escaping is the JSON standard one.
        self.out.push_str(&serde_json::to_string(s).unwrap_or_default());
    }

    fn unknown(&mut self, why: &str) {
        self.out.push_str("{\"$unknown\": ");
        self.string(why);
        self.out.push('}');
    }

    fn f32(&mut self, v: f32) {
        if v.is_nan() {
            self.out.push_str("\"NaN\"");
        } else if v.is_infinite() {
            self.out.push_str(if v > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
        } else {
            let _ = write!(self.out, "{v}");
        }
    }

    fn f64(&mut self, v: f64) {
        if v.is_nan() {
            self.out.push_str("\"NaN\"");
        } else if v.is_infinite() {
            self.out.push_str(if v > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
        } else {
            let _ = write!(self.out, "{v}");
        }
    }

    fn floats(&mut self, v: &[f32]) {
        self.out.push('[');
        for (i, x) in v.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.f32(*x);
        }
        self.out.push(']');
    }

    /// Writes the fields of an object of type `t`, without braces.
    /// `o` is `None` for a default instance.
    fn object_body(&mut self, t: TypeIdx, o: Option<&Object>) {
        let def = schema::ty(t);
        let mut first = true;
        if def.has(type_flags::IDENTIFIED) {
            self.key("$type");
            self.string(&format!("{:032x}", def.guid));
            first = false;
        }
        for (j, f) in def.fields.iter().enumerate() {
            if !f.in_runtime() {
                continue;
            }
            if !first {
                self.out.push_str(",\n");
            }
            first = false;
            self.key(f.name);
            match o.map(|o| &o.fields[j]) {
                Some(Node::Absent) | None => self.default(f.codec, f.default),
                Some(n) => self.value(f.codec, n),
            }
        }
    }

    fn object(&mut self, t: TypeIdx, o: Option<&Object>) {
        let rewritten =
            if self.source == Source::Json { o.and_then(pack_time_rewrite) } else { None };
        let o = rewritten.as_ref().or(o);
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.unknown("nesting too deep");
            self.depth -= 1;
            return;
        }
        let def = schema::ty(t);
        let has_fields =
            def.has(type_flags::IDENTIFIED) || def.fields.iter().any(schema::Field::in_runtime);
        if has_fields {
            self.out.push_str("{\n");
            self.indent += 1;
            self.object_body(t, o);
            self.indent -= 1;
            self.out.push('\n');
            self.pad();
            self.out.push('}');
        } else {
            self.out.push_str("{}");
        }
        self.depth -= 1;
    }

    /// A field's value when the JSON doesn't have it.
    fn default(&mut self, codec: Codec, d: Def) {
        match (d, codec) {
            (Def::Zero, _) => self.zero(codec),
            (Def::Null, _) => self.value(codec, &Node::Null),
            (Def::Bool(b), _) => self.out.push_str(if b { "true" } else { "false" }),
            (Def::Int(i), Codec::F32) => {
                #[allow(clippy::cast_precision_loss)]
                self.f32(i as f32);
            }
            (Def::Int(i), Codec::F64) => {
                #[allow(clippy::cast_precision_loss)]
                self.f64(i as f64);
            }
            (Def::Int(i), _) => {
                let _ = write!(self.out, "{i}");
            }
            #[allow(clippy::cast_possible_truncation)]
            (Def::Float(v), Codec::F32) => self.f32(v as f32),
            (Def::Float(v), Codec::F64) => self.f64(v),
            (Def::Float(_), _) => self.unknown("float default on a non-float field"),
            (Def::EmptyStr, _) => self.string(""),
            (Def::EmptySeq, _) => self.out.push_str("[]"),
            (Def::NewObject, Codec::Object(t)) => self.object(t, None),
            (Def::NewObject, Codec::Seq { .. }) => self.out.push_str("[]"),
            (Def::NewObject, _) => self.unknown("new object default"),
            (Def::Floats(v), Codec::Unity(_)) => {
                #[allow(clippy::cast_possible_truncation)]
                let f: Vec<f32> = v.iter().map(|x| *x as f32).collect();
                self.floats(&f);
            }
            (Def::Floats(_), _) => self.unknown("vector default on a non-vector field"),
            (Def::NonEmptyStr, _) => self.unknown("string initializer"),
            (Def::Unknown, _) => self.unknown("non-literal initializer"),
        }
    }

    /// `default(T)`: zero for value types, null (then the pack's normalisation) otherwise.
    fn zero(&mut self, codec: Codec) {
        match codec {
            Codec::Bool => self.out.push_str("false"),
            Codec::Int { .. } | Codec::Char | Codec::Enum(_) => self.out.push('0'),
            Codec::F32 => self.f32(0.0),
            Codec::F64 => self.f64(0.0),
            Codec::Object(t) if schema::ty(t).has(type_flags::STRUCT) => {
                // A zeroed struct: every field zero, ignoring initializers.
                self.zero_struct(t);
            }
            Codec::Unity(k) => self.unity_zero(k),
            _ => self.value(codec, &Node::Null),
        }
    }

    fn zero_struct(&mut self, t: TypeIdx) {
        let def = schema::ty(t);
        self.out.push_str("{\n");
        self.indent += 1;
        let mut first = true;
        for f in def.fields.iter().filter(|f| f.in_runtime()) {
            if !first {
                self.out.push_str(",\n");
            }
            first = false;
            self.key(f.name);
            self.zero(f.codec);
        }
        self.indent -= 1;
        self.out.push('\n');
        self.pad();
        self.out.push('}');
    }

    /// An element of a generic list whose type has no serialized fields.
    fn zero_elem(&mut self, elem: Codec) {
        match elem {
            Codec::Unity(UnityKind::AnimationCurve) => self.out.push_str("{\"keys\": []}"),
            Codec::Unity(UnityKind::Gradient) => self.unknown("default Gradient"),
            Codec::Str => self.string(""),
            c => self.zero(c),
        }
    }

    fn unity_zero(&mut self, k: UnityKind) {
        match k {
            UnityKind::Vector2 => self.floats(&[0.0; 2]),
            UnityKind::Vector3 => self.floats(&[0.0; 3]),
            UnityKind::Vector4 | UnityKind::Quaternion | UnityKind::Color | UnityKind::Rect => {
                self.floats(&[0.0; 4]);
            }
            UnityKind::Bounds => self.floats(&[0.0; 6]),
            UnityKind::Vector2Int => self.out.push_str("[0, 0]"),
            UnityKind::Vector3Int => self.out.push_str("[0, 0, 0]"),
            UnityKind::Color32 => self.out.push_str("[0, 0, 0, 0]"),
            // Reference types: null; the pack writes null curves/gradients as empty.
            UnityKind::AnimationCurve | UnityKind::Gradient => self.out.push_str("null"),
        }
    }

    fn value(&mut self, codec: Codec, n: &Node) {
        match packfmt::kind(&codec) {
            // The pack drops the value: the game holds zero whatever the JSON says.
            PackKind::Lost => return self.zero(codec),
            PackKind::GenericSeq => {
                if let Codec::Seq { elem, .. } = codec
                    && packfmt::elem_kind(elem) == ElemKind::Zero
                {
                    match n {
                        Node::Seq(items) => {
                            self.out.push('[');
                            for i in 0..items.len() {
                                if i > 0 {
                                    self.out.push_str(", ");
                                }
                                self.zero_elem(*elem);
                            }
                            self.out.push(']');
                        }
                        _ => self.out.push_str("[]"),
                    }
                    return;
                }
            }
            _ => {}
        }
        match (codec, n) {
            (_, Node::Opaque(o)) => {
                self.unknown(&format!("opaque: {:?}", o.reason));
            }
            (_, Node::Absent) => self.zero(codec),
            (_, Node::Coerced(inner, _)) => self.value(codec, inner),
            (Codec::Bool, Node::Bool(b)) => self.out.push_str(if *b { "true" } else { "false" }),
            (Codec::Int { .. } | Codec::Char | Codec::Enum(_), Node::Int(i) | Node::Enum(i)) => {
                let _ = write!(self.out, "{i}");
            }
            (Codec::Int { .. }, Node::UInt(u)) => {
                let _ = write!(self.out, "{u}");
            }
            (Codec::F32, Node::F32(v)) => self.f32(*v),
            (Codec::F64, Node::F64(v)) => self.f64(*v),
            (Codec::Str, Node::Str(s)) => self.string(s),
            (Codec::Str, Node::Null) => self.string(""),
            (Codec::Object(t), Node::Object(o)) if t == schema::special().bp_ref_base => {
                // From the JSON: Newtonsoft fills the private `guid` string, but nothing
                // calls `OnAfterDeserialize`, so `deserializedGuid` (what the pack writes)
                // stays empty.
                let _ = o;
                self.out.push_str("null");
            }
            (Codec::Object(_), Node::Object(o)) => self.object(o.ty, Some(o)),
            (Codec::Object(t), Node::Null) => {
                if schema::ty(t).has(type_flags::IDENTIFIED) {
                    self.out.push_str("null");
                } else {
                    // The pack writes `Activator.CreateInstance(fieldType)` for null.
                    self.object(t, None);
                }
            }
            (Codec::Seq { elem, .. }, Node::Seq(items)) => {
                if items.is_empty() {
                    self.out.push_str("[]");
                    return;
                }
                self.out.push_str("[\n");
                self.indent += 1;
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(",\n");
                    }
                    self.pad();
                    self.value(*elem, it);
                }
                self.indent -= 1;
                self.out.push('\n');
                self.pad();
                self.out.push(']');
            }
            (Codec::Seq { .. }, Node::Null) => self.out.push_str("[]"),
            (
                Codec::BpRef { .. } | Codec::BlueprintGuid | Codec::SharedString | Codec::Asset(_),
                Node::Null,
            ) => {
                self.out.push_str("null");
            }
            // The pack creates the link object for null; its `AssetId` reads back as "".
            (Codec::WeakLink(_), Node::Null) => self.string(""),
            (Codec::BpRef { .. } | Codec::BlueprintGuid, Node::BpRef(g)) => {
                if g.is_empty() {
                    self.out.push_str("null");
                } else {
                    self.string(&g.to_string());
                }
            }
            (Codec::WeakLink(_), Node::WeakLink(id)) => self.string(id.as_deref().unwrap_or("")),
            (Codec::WeakLink(_), Node::WeakLinkLegacy(g, _)) => self.string(g),
            // From the pack: the link object with its `AssetId` field.
            (Codec::WeakLink(_), Node::Object(o)) => match o.get("AssetId") {
                Some(Node::Str(s)) => self.string(s),
                _ => self.string(""),
            },
            // `BlueprintReferenceBase` itself: the pack keeps the GUID it was created with.
            (Codec::Object(t), Node::BpRef(g)) if t == schema::special().bp_ref_base => {
                if g.is_empty() {
                    self.out.push_str("null");
                } else {
                    self.string(&g.to_string());
                }
            }
            (
                Codec::Asset(_) | Codec::SharedString,
                Node::Asset(Some(_)) | Node::SharedString(Some(_)),
            ) if self.assets == Assets::Presence => {
                self.out.push_str("\"asset\"");
            }
            (Codec::SharedString, Node::SharedString(k)) => match k {
                Some(k) => {
                    self.out.push_str("{\"stringkey\": ");
                    self.string(&k.key);
                    self.out.push('}');
                }
                None => self.out.push_str("null"),
            },
            (_, Node::AssetIndex(i)) => {
                if *i < 0 {
                    self.out.push_str("null");
                } else if self.assets == Assets::Presence {
                    self.out.push_str("\"asset\"");
                } else {
                    let _ = write!(self.out, "{{\"asset_index\": {i}}}");
                }
            }
            (Codec::Asset(_), Node::Asset(a)) => match a {
                Some(a) => {
                    self.out.push_str("{\"guid\": ");
                    self.string(&a.guid);
                    let _ = write!(self.out, ", \"fileid\": {}}}", a.file_id);
                }
                None => self.out.push_str("null"),
            },
            (Codec::Unity(UnityKind::Color32), Node::Int(v)) => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let b = (*v as i32 as u32).to_le_bytes();
                let _ = write!(self.out, "[{}, {}, {}, {}]", b[0], b[1], b[2], b[3]);
            }
            (Codec::Unity(_), Node::Floats(f)) => self.floats(f),
            (Codec::Unity(_), Node::Ints(v)) => {
                self.out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    let _ = write!(self.out, "{x}");
                }
                self.out.push(']');
            }
            (Codec::Unity(UnityKind::AnimationCurve), Node::Curve(c)) => self.curve(c),
            (Codec::Unity(UnityKind::Gradient), Node::Gradient(g)) => {
                let mut colors = g.colors.clone();
                colors.sort_by(|a, b| a[0].total_cmp(&b[0]));
                let mut alphas = g.alphas.clone();
                alphas.sort_by(|a, b| a[0].total_cmp(&b[0]));
                self.out.push_str("{\"colors\": [");
                for (i, c) in colors.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    self.floats(c);
                }
                self.out.push_str("], \"alphas\": [");
                for (i, a) in alphas.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    self.floats(a);
                }
                let _ = write!(self.out, "], \"mode\": {}}}", g.mode);
            }
            (Codec::Unity(k), Node::Null) => self.unity_zero(k),
            (Codec::PrototypeLink | Codec::Overrides, _) => self.out.push_str("null"),
            (c, n) => self.unknown(&format!("{c:?} holding {}", node_kind(n))),
        }
    }

    fn curve(&mut self, c: &Curve) {
        // `AddKey`: sorted by time; a key at an existing time is refused.
        let mut keys: Vec<&crate::node::Keyframe> = Vec::with_capacity(c.keys.len());
        for k in &c.keys {
            if keys.iter().any(|x| x.time == k.time) {
                continue;
            }
            keys.push(k);
        }
        keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        // Wrap modes aren't in the pack: the game's curves have Unity's defaults.
        self.out.push_str("{\"keys\": [");
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.out.push('[');
            for (j, v) in [k.time, k.value, k.in_tangent, k.out_tangent, k.in_weight, k.out_weight]
                .iter()
                .enumerate()
            {
                if j > 0 {
                    self.out.push_str(", ");
                }
                self.f32(*v);
            }
            let _ = write!(self.out, ", {}]", k.weighted_mode);
        }
        self.out.push_str("]}");
    }
}

/// Build-time rewrites that the pack shows and the JSON doesn't:
///
/// - `LocalizedString` with a shared string: the pack holds the shared string's key in
///   `m_Key` and no `Shared` (its `OnPrePacked`, stripped from the runtime build,
///   presumably does this);
/// - `ColorBlock`: the pack doesn't write the selected colour, so it reads back as zero.
fn pack_time_rewrite(o: &Object) -> Option<Object> {
    let sp = schema::special();
    if schema::is_assignable(o.ty, sp.localized_string) {
        let def = schema::ty(o.ty);
        let shared = def.fields.iter().position(|f| f.name == "Shared")?;
        let key = def.fields.iter().position(|f| f.name == "m_Key")?;
        let mut k: Option<Box<str>> = match &o.fields[key] {
            Node::Str(s) => Some(s.clone()),
            _ => None,
        };
        if let Node::SharedString(Some(sk)) = &o.fields[shared] {
            k = Some(sk.key.clone());
        }
        let mut c = o.clone();
        if let Some(k) = k {
            c.fields[key] = Node::Str(k);
        }
        c.fields[shared] = Node::SharedString(None);
        return Some(c);
    }
    if o.ty == sp.color_block {
        let def = schema::ty(o.ty);
        let sel = def.fields.iter().position(|f| f.name == "m_SelectedColor")?;
        let mut c = o.clone();
        c.fields[sel] = Node::Floats(vec![0.0; 4].into());
        return Some(c);
    }
    None
}

fn node_kind(n: &Node) -> &'static str {
    match n {
        Node::Absent => "absent",
        Node::Null => "null",
        Node::Bool(_) => "bool",
        Node::Int(_) | Node::UInt(_) => "int",
        Node::F32(_) | Node::F64(_) => "float",
        Node::Str(_) => "string",
        Node::Enum(_) => "enum",
        Node::BpRef(_) => "reference",
        Node::WeakLink(_) | Node::WeakLinkLegacy(..) => "weak link",
        Node::SharedString(_) => "shared string",
        Node::Asset(_) => "asset",
        Node::AssetIndex(_) => "asset index",
        Node::Floats(_) => "floats",
        Node::Ints(_) => "ints",
        Node::Curve(_) => "curve",
        Node::Gradient(_) => "gradient",
        Node::Seq(_) => "array",
        Node::Object(_) => "object",
        Node::Discarded(_) => "discarded",
        Node::Coerced(..) => "coerced",
        Node::Opaque(_) => "opaque",
    }
}
