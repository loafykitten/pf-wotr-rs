//! Typed decoding of one save document, following Newtonsoft 8 (the game's
//! `Newtonsoft.Json.dll`) with `DefaultJsonSettings`:
//!
//! - **References** (`PreserveReferencesHandling.Objects`, `DefaultReferenceResolver`):
//!   `$id`s are scoped to one document (one `Deserialize` call). An object's `$id` is
//!   registered when the object is created, before its properties are read, so references
//!   from inside it to it work; for types built through a constructor with parameters the
//!   registration happens only after every property has been read. A `$ref` to an id not
//!   registered yet resolves to null (`BidirectionalDictionary.TryGetByFirst` fails and
//!   no error is raised). Ids inside values that a converter reads as raw JSON, or inside
//!   members the game skips, are never registered.
//! - **Metadata** (`ReadMetadataProperties`): only a leading run of `$ref`, `$type`,
//!   `$id`, `$values` counts; a `$ref` object must have no other property.
//! - **Types** (`TypeNameHandling.Auto`, `DefaultSerializationBinder`): `$type` must name
//!   a type assignable to the declared type, from the named assembly.
//! - **Converters**: as `DefaultJsonSettings.CommonConverters`, matched on the declared
//!   type (see `schema::Codec`).
//! - **Numbers** (`JsonTextReader`): `int`, `short`, `byte` read through `ReadAsInt32`
//!   (integer tokens only), other integers as `Int64` then a checked conversion; `float`
//!   and `double` through `ReadAsDouble` (a double, then rounded to float).
//!
//! Whatever the schema can't type becomes [`Value::Opaque`] and is counted.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::clr;
use crate::json::{Json, is_int_lexeme};
use crate::schema::{self, Codec, NO_TYPE, RefKind, SeqKind, TypeIdx, member_flags, type_flags};
use crate::value::{
    Dict, DictForm, Doc, ENUM_NUMBER, Entry, EnumText, Id, Key, NO_TAG, Obj, Opaque, Prop, Reason,
    Ref, RefState, Value,
};

/// Per-type counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeStats {
    pub objects: u64,
    /// Objects whose own properties all decoded through the schema (no opaque value or
    /// unknown property directly inside them).
    pub clean: u64,
    pub props: u64,
    pub props_unknown: u64,
}

/// A blueprint GUID read by `BlueprintConverter` or `BlueprintRefConverter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BpUse {
    pub guid: [u8; 16],
    /// The declared (converter) or referenced (reference) blueprint type.
    pub target: TypeIdx,
    /// `BlueprintConverter` (the game throws on a missing blueprint) vs a reference.
    pub converter: bool,
    /// Inside a `DictionaryConverter` entry (a failure drops the entry instead).
    pub in_dict: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub per_type: HashMap<TypeIdx, TypeStats>,
    /// JSON values consumed by typed decoding.
    pub values_typed: u64,
    /// JSON values kept opaque (including unknown properties' values).
    pub values_untyped: u64,
    /// Of the typed values: JSON kept as-is because the declared type is `object` (without
    /// a `$type`), `JToken` or `decimal` (raw JSON is the C# value there).
    pub values_raw: u64,
    /// Opaque values by reason key: (occurrences, JSON values inside).
    pub opaque: BTreeMap<String, (u64, u64)>,
    /// Opaque values by reason and the member holding them (`reason @ Type.member`).
    pub opaque_at: BTreeMap<String, u64>,
    /// Coerced values by codec.
    pub coerced: BTreeMap<&'static str, u64>,
    /// Unknown properties: (type, name) → count.
    pub unknown_props: BTreeMap<(TypeIdx, Box<str>), u64>,
    pub ids: u64,
    pub refs: BTreeMap<&'static str, u64>,
    /// Properties of non-writable members the game skips.
    pub skipped_props: u64,
    /// Skipped members by `Type.member`.
    pub skipped_at: BTreeMap<String, u64>,
    /// Distinct `$type` keys (CLR names without assemblies) and their counts.
    pub tags: BTreeMap<String, u64>,
    pub bp_uses: Vec<BpUse>,
    pub entity_refs: Vec<Box<str>>,
    pub entity_ids: Vec<Box<str>>,
    /// `DictionaryConverter` entries the game would drop.
    pub dict_dropped: u64,
    /// Repeated keys in a dictionary (the game keeps the first).
    pub dict_repeats: u64,
    /// ISO-date-like strings inside `DictionaryConverter` entries: `JObject.Load` turns
    /// them into `DateTime` tokens (`DateParseHandling.DateTime`), so a `string` member
    /// there reads back reformatted. Counted, not modelled.
    pub date_strings_in_dict: u64,
}

impl Stats {
    pub fn merge(&mut self, o: Self) {
        for (k, v) in o.per_type {
            let e = self.per_type.entry(k).or_default();
            e.objects += v.objects;
            e.clean += v.clean;
            e.props += v.props;
            e.props_unknown += v.props_unknown;
        }
        self.values_typed += o.values_typed;
        self.values_untyped += o.values_untyped;
        self.values_raw += o.values_raw;
        for (k, (a, b)) in o.opaque {
            let e = self.opaque.entry(k).or_default();
            e.0 += a;
            e.1 += b;
        }
        for (k, v) in o.opaque_at {
            *self.opaque_at.entry(k).or_default() += v;
        }
        for (k, v) in o.coerced {
            *self.coerced.entry(k).or_default() += v;
        }
        for (k, v) in o.unknown_props {
            *self.unknown_props.entry(k).or_default() += v;
        }
        self.ids += o.ids;
        for (k, v) in o.refs {
            *self.refs.entry(k).or_default() += v;
        }
        self.skipped_props += o.skipped_props;
        for (k, v) in o.skipped_at {
            *self.skipped_at.entry(k).or_default() += v;
        }
        for (k, v) in o.tags {
            *self.tags.entry(k).or_default() += v;
        }
        self.bp_uses.extend(o.bp_uses);
        self.entity_refs.extend(o.entity_refs);
        self.entity_ids.extend(o.entity_ids);
        self.dict_dropped += o.dict_dropped;
        self.dict_repeats += o.dict_repeats;
        self.date_strings_in_dict += o.date_strings_in_dict;
    }
}

#[derive(Clone, Copy, Default)]
struct Ctx {
    /// Inside a member the game skips: ids aren't registered.
    skipped: bool,
    /// Inside a `DictionaryConverter` entry.
    in_dict: bool,
}

struct Dec {
    tags: Vec<Box<str>>,
    tag_index: HashMap<Box<str>, u32>,
    /// Registered ids → whether registration is complete.
    ids: HashSet<Id>,
    /// Ids seen anywhere (for forward vs dangling).
    all_ids: HashSet<Id>,
    opaque_ids: HashSet<Id>,
    skipped_ids: HashSet<Id>,
    pending: bool,
    /// Dirty flags of the objects being decoded (innermost last).
    dirty: Vec<bool>,
    /// The member being decoded (type, member index), innermost last.
    at: Vec<(TypeIdx, u16)>,
    stats: Stats,
}

/// Decodes a parsed document whose root is declared as `root` (`None`: untyped).
#[must_use]
pub fn decode(doc: &crate::json::Document, root: Option<TypeIdx>) -> (Doc, Stats) {
    let mut d = Dec {
        tags: Vec::new(),
        tag_index: HashMap::new(),
        ids: HashSet::new(),
        all_ids: HashSet::new(),
        opaque_ids: HashSet::new(),
        skipped_ids: HashSet::new(),
        pending: false,
        dirty: vec![false],
        at: Vec::new(),
        stats: Stats::default(),
    };
    let mut value = match root {
        Some(t) => d.object(&doc.root, Some(t), false, Ctx::default()),
        None => d.value(&doc.root, &Codec::Any, Ctx::default()),
    };
    if d.pending {
        let ids = std::mem::take(&mut d.all_ids);
        value.walk_mut(&mut |v| {
            if let Value::Ref(r) = v
                && r.state == RefState::Pending
            {
                r.state = if ids.contains(&r.id) { RefState::Forward } else { RefState::Dangling };
            }
        });
    }
    value.walk(&mut |v| {
        if let Value::Ref(r) = v {
            let k = match r.state {
                RefState::Resolved => "resolved",
                RefState::Forward => "forward (null in game)",
                RefState::Dangling => "dangling (null in game)",
                RefState::IntoOpaque => "into opaque value",
                RefState::IntoSkipped => "into skipped member (null in game)",
                RefState::Pending => "pending",
            };
            *d.stats.refs.entry(k).or_default() += 1;
        }
    });
    (Doc { bom: doc.bom, root: value, tags: d.tags }, d.stats)
}

fn parse_hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 || !s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return None;
    }
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(g)
}

/// A GUID string as the game writes it (`N` or `D`, lowercase).
fn guid_value(s: &str) -> Option<Value> {
    if let Some(g) = parse_hex16(s) {
        return Some(Value::GuidN(g));
    }
    let b = s.as_bytes();
    if s.len() == 36 && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-') {
        let compact: String = s.chars().filter(|&c| c != '-').collect();
        return parse_hex16(&compact).map(Value::GuidD);
    }
    None
}

/// Whether `Guid.TryParse` would read the text (any of its formats, any case).
fn guid_parses(s: &str) -> bool {
    let hex: String = s.chars().filter(char::is_ascii_hexdigit).collect();
    let t = s.trim();
    let shape = t.len() == 32
        || t.len() == 36
        || (t.len() == 38 && (t.starts_with('{') || t.starts_with('(')));
    shape && hex.len() == 32
}

/// TimeSpan constant format: `[-][d.]hh:mm:ss[.fffffff]`.
#[must_use]
pub fn fmt_timespan(t: i64) -> String {
    let neg = t < 0;
    let a = t.unsigned_abs();
    let day = 864_000_000_000u64;
    let (days, rem) = (a / day, a % day);
    let h = rem / 36_000_000_000;
    let m = (rem / 600_000_000) % 60;
    let s = (rem / 10_000_000) % 60;
    let f = rem % 10_000_000;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if days > 0 {
        out.push_str(&format!("{days}."));
    }
    out.push_str(&format!("{h:02}:{m:02}:{s:02}"));
    if f > 0 {
        out.push_str(&format!(".{f:07}"));
    }
    out
}

fn parse_timespan(s: &str) -> Option<i64> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let (days, hms) = match body.split_once('.') {
        Some((d, rest)) if !d.contains(':') => (d.parse::<u64>().ok()?, rest),
        _ => (0, body),
    };
    let (hms, frac) = match hms.rsplit_once('.') {
        Some((a, f)) if f.len() == 7 && f.bytes().all(|c| c.is_ascii_digit()) => {
            (a, f.parse::<u64>().ok()?)
        }
        Some(_) => return None,
        None => (hms, 0),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.len() != 2) {
        return None;
    }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: u64 = parts[2].parse().ok()?;
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let ticks = days
        .checked_mul(864_000_000_000)?
        .checked_add(h * 36_000_000_000 + m * 600_000_000 + sec * 10_000_000 + frac)?;
    let t = i64::try_from(ticks).ok()?;
    Some(if neg { -t } else { t })
}

/// ISO 8601 as Newtonsoft writes `DateTime`/`DateTimeOffset`, with calendar and clock
/// ranges checked (`DateTimeParser` rejects an impossible date).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let x = b.get(r)?;
        if !x.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(x).ok()?.parse().ok()
    };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) =
        (num(0..4), num(5..7), num(8..10), num(11..13), num(14..16), num(17..19))
    else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if y == 0 || d == 0 || d > days || h > 23 || mi > 59 || sec > 59 {
        return false;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let s0 = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == s0 || i - s0 > 7 {
            return false;
        }
    }
    match b.get(i) {
        None => true,
        Some(b'Z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            b.len() == i + 6
                && b[i + 3] == b':'
                && num(i + 1..i + 3).is_some_and(|x| x <= 14)
                && num(i + 4..i + 6).is_some_and(|x| x <= 59)
        }
        _ => false,
    }
}

/// An unsigned number with a leading zero that isn't `0.…`/`0e…`: octal to Newtonsoft.
fn is_octal_lexeme(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() > 1 && b[0] == b'0' && !matches!(b[1], b'.' | b'e' | b'E')
}

/// Assembly check for one generic argument of a `$type`.
fn arg_assembly_ok(key: &str, asm: Option<&str>, top: &str) -> bool {
    let want = schema::assembly_of(key);
    match (asm, want) {
        (Some(a), Some(w)) => a == w,
        // Not in the schema: only mscorlib types can be checked by name.
        (Some(a), None) => key.starts_with("System.") && a == "mscorlib",
        // Unqualified arguments resolve in the generic definition's assembly or mscorlib.
        (None, Some(w)) => w == top || w == "mscorlib",
        (None, None) => key.starts_with("System."),
    }
}

/// Whether an object has a repeated property name (`JObject.Load` keeps only the last,
/// which a converter would then read).
fn has_dup_keys(props: &[(Box<str>, Json)]) -> bool {
    let mut seen: HashSet<&str> = HashSet::with_capacity(props.len());
    props.iter().any(|(k, _)| !seen.insert(k))
}

/// Whether any object inside has a repeated property name. A converter that loads its
/// input with `JObject.Load`/`JArray.Load` replaces repeated properties at every depth
/// before deserializing anything, so the earlier values never exist for the game.
fn has_dup_keys_deep(j: &Json) -> bool {
    match j {
        Json::Obj(p) => has_dup_keys(p) || p.iter().any(|(_, v)| has_dup_keys_deep(v)),
        Json::Arr(a) => a.iter().any(has_dup_keys_deep),
        _ => false,
    }
}

/// Whether a decoded value holds an opaque value anywhere (the game's conversion of it
/// would have failed, or we don't model it).
fn contains_opaque(v: &Value) -> bool {
    let mut found = false;
    v.walk(&mut |x| found |= matches!(x, Value::Opaque(_)));
    found
}

/// What a dictionary key means to `Dictionary.Contains` (repeats are ignored).
fn key_identity(v: &Value) -> Option<String> {
    Some(match v {
        // Reference keys: the same object whether written in full or as a `$ref`.
        Value::Obj(o) => format!("o{}", o.id.as_ref()?.text()),
        Value::Ref(r) => format!("o{}", r.id.text()),
        Value::Str(s) => format!("s{s}"),
        Value::Int(i) => format!("i{i}"),
        Value::UInt(u) => format!("i{u}"),
        Value::Enum { value, .. } => format!("i{value}"),
        Value::EnumText(t) => format!("i{}", t.value),
        Value::GuidN(g) | Value::GuidD(g) => format!("g{g:?}"),
        Value::GuidEmpty => "g0".into(),
        Value::Coerced(c) => return key_identity(&c.0),
        _ => return None,
    })
}

/// A finite or special float lexeme as `double.Parse` reads it.
fn parse_f64(n: &str) -> Option<f64> {
    match n {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => n.parse::<f64>().ok(),
    }
}

fn int_range(bits: u8, signed: bool) -> (i128, i128) {
    match (bits, signed) {
        (8, true) => (i128::from(i8::MIN), i128::from(i8::MAX)),
        (8, false) => (0, i128::from(u8::MAX)),
        (16, true) => (i128::from(i16::MIN), i128::from(i16::MAX)),
        (16, false) => (0, i128::from(u16::MAX)),
        (32, true) => (i128::from(i32::MIN), i128::from(i32::MAX)),
        (32, false) => (0, i128::from(u32::MAX)),
        (64, false) => (0, i128::from(u64::MAX)),
        _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
    }
}

/// `"a|b|c"` with exactly `n` parts, each read by `int.Parse`. Returns the values and
/// whether the text is exactly what the game writes for them.
fn parse_cm(s: &str, n: usize) -> Option<(Vec<i32>, bool)> {
    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() < n {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for p in &parts[..n] {
        out.push(p.trim().parse::<i32>().ok()?);
    }
    let exact = parts.len() == n && parts.iter().zip(&out).all(|(p, v)| *p == v.to_string());
    Some((out, exact))
}

impl Dec {
    fn typed(&mut self, n: u64) {
        self.stats.values_typed += n;
    }

    fn mark_dirty(&mut self) {
        if let Some(d) = self.dirty.last_mut() {
            *d = true;
        }
    }

    fn opaque(&mut self, j: &Json, reason: Reason) -> Value {
        let n = j.count();
        self.stats.values_untyped += n;
        let key = reason.key();
        let at = self.at.last().map_or_else(
            || "<root>".to_owned(),
            |&(t, m)| {
                let td = schema::ty(t);
                format!("{}.{}", td.clr, td.members.get(usize::from(m)).map_or("?", |x| x.json))
            },
        );
        *self.stats.opaque_at.entry(format!("{key} @ {at}")).or_default() += 1;
        let e = self.stats.opaque.entry(key).or_default();
        e.0 += 1;
        e.1 += n;
        self.mark_dirty();
        // Ids inside: references to them point into an opaque value.
        collect_ids(j, &mut self.opaque_ids, &mut self.all_ids);
        Value::Opaque(Box::new(Opaque { json: j.clone(), reason }))
    }

    fn coerced(&mut self, game: Value, j: &Json, codec: &'static str) -> Value {
        *self.stats.coerced.entry(codec).or_default() += 1;
        self.typed(j.count());
        Value::Coerced(Box::new((game, j.clone())))
    }

    fn tag(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.tag_index.get(s) {
            return i;
        }
        let i = u32::try_from(self.tags.len()).unwrap_or(u32::MAX - 1);
        self.tags.push(s.into());
        self.tag_index.insert(s.into(), i);
        i
    }

    fn register(&mut self, id: &Id, ctx: Ctx) {
        self.stats.ids += 1;
        self.all_ids.insert(id.clone());
        if ctx.skipped {
            self.skipped_ids.insert(id.clone());
        } else {
            self.ids.insert(id.clone());
        }
    }

    /// Whether registering `id` would make Newtonsoft throw (`BidirectionalDictionary.Set`
    /// refuses a second object under the same id).
    fn duplicate(&self, id: &Id, ctx: Ctx) -> bool {
        !ctx.skipped && self.ids.contains(id)
    }

    fn reference(&mut self, id: &str) -> Value {
        let id = Id::parse(id);
        let state = if self.ids.contains(&id) {
            RefState::Resolved
        } else if self.opaque_ids.contains(&id) {
            RefState::IntoOpaque
        } else if self.skipped_ids.contains(&id) {
            RefState::IntoSkipped
        } else {
            self.pending = true;
            RefState::Pending
        };
        self.typed(2);
        Value::Ref(Box::new(Ref { id, state }))
    }

    /// An object contract value. `declared`: the declared type (`None` for
    /// `System.Object`). `nullable`: declared as `Nullable<declared>`.
    #[allow(clippy::too_many_lines)]
    fn object(&mut self, j: &Json, declared: Option<TypeIdx>, nullable: bool, ctx: Ctx) -> Value {
        let Json::Obj(props) = j else {
            if matches!(j, Json::Null)
                && (nullable || declared.is_none_or(|d| !schema::ty(d).has(type_flags::STRUCT)))
            {
                self.typed(1);
                return Value::Null;
            }
            return self.opaque(j, Reason::Mismatch("object".into(), j.kind().into()));
        };
        // Leading metadata.
        let mut i = 0;
        let mut id: Option<Id> = None;
        let mut tag: Option<&str> = None;
        let mut type_first = false;
        while i < props.len() {
            let (k, v) = (&*props[i].0, &props[i].1);
            match k {
                "$ref" => {
                    return match v {
                        Json::Str(r) if i == 0 && props.len() == 1 => self.reference(r),
                        Json::Str(_) => {
                            self.opaque(j, Reason::BadMetadata("$ref with other properties".into()))
                        }
                        _ => self.opaque(j, Reason::BadMetadata("$ref not a string".into())),
                    };
                }
                "$type" => {
                    let Json::Str(s) = v else {
                        return self.opaque(j, Reason::BadMetadata("$type not a string".into()));
                    };
                    if tag.is_some() {
                        return self.opaque(j, Reason::BadMetadata("repeated $type".into()));
                    }
                    type_first = id.is_none();
                    tag = Some(s);
                }
                "$id" => {
                    let Json::Str(s) = v else {
                        return self.opaque(j, Reason::BadMetadata("$id not a string".into()));
                    };
                    if id.is_some() {
                        return self.opaque(j, Reason::BadMetadata("repeated $id".into()));
                    }
                    id = Some(Id::parse(s));
                }
                "$values" => return self.opaque(j, Reason::BadMetadata("$values wrapper".into())),
                _ => break,
            }
            i += 1;
        }
        let type_first = type_first && id.is_some();
        // Concrete type.
        let concrete = match tag {
            Some(t) => {
                let Some(tn) = clr::parse(t) else {
                    return self.opaque(j, Reason::BadMetadata("unparsable $type".into()));
                };
                // `Nullable<T>` written through T's object contract names `Nullable<T>`.
                let key = match (nullable, declared) {
                    (true, Some(d))
                        if tn.key == format!("System.Nullable`1[[{}]]", schema::ty(d).clr) =>
                    {
                        schema::ty(d).clr.to_owned()
                    }
                    _ => tn.key.clone(),
                };
                let Some(ti) = schema::by_name(&key) else {
                    return self.opaque(j, Reason::UnknownType(key.into()));
                };
                // `DefaultSerializationBinder`: the named assembly must hold the type; without
                // one, `Type.GetType` only searches Newtonsoft's assembly and mscorlib.
                let top_asm = if key == tn.key { schema::ty(ti).assembly } else { "mscorlib" };
                let asm_ok = tn.assembly.as_deref().map_or(top_asm == "mscorlib", |a| a == top_asm)
                    && tn.args.iter().all(|(k, a)| arg_assembly_ok(k, a.as_deref(), top_asm));
                if !asm_ok {
                    return self.opaque(j, Reason::WrongAssembly(t.into()));
                }
                if let Some(d) = declared
                    && !schema::is_assignable(ti, d)
                {
                    return self.opaque(j, Reason::NotAssignable(key.into()));
                }
                ti
            }
            None => match declared {
                Some(d) if !schema::ty(d).has(type_flags::ABSTRACT) => d,
                Some(d) => {
                    return self.opaque(j, Reason::AbstractWithoutTag(schema::ty(d).clr.into()));
                }
                None => {
                    // `object` without `$type`: Newtonsoft builds a JObject.
                    self.typed(j.count());
                    self.stats.values_raw += j.count();
                    return Value::Json(Box::new(j.clone()));
                }
            },
        };
        let t = schema::ty(concrete);
        if t.has(type_flags::ABSTRACT) {
            return self.opaque(j, Reason::AbstractWithoutTag(t.clr.into()));
        }
        if t.has(type_flags::NO_CREATOR) {
            return self.opaque(j, Reason::NoCreator(t.clr.into()));
        }
        if t.has(type_flags::ARRAY_CONTRACT | type_flags::DICT_CONTRACT) {
            return self
                .opaque(j, Reason::Unsupported(format!("collection type {}", t.clr).into()));
        }
        let late = t.has(type_flags::CREATOR_PARAMS);
        if let Some(id) = &id {
            if self.duplicate(id, ctx) {
                return self.opaque(j, Reason::BadMetadata("duplicate $id".into()));
            }
            if !late {
                self.register(id, ctx);
            }
        }
        let counts = (self.stats.values_typed, self.stats.values_untyped);
        let tag_idx = tag.map_or(NO_TAG, |s| self.tag(s));
        if tag.is_some() {
            *self.stats.tags.entry(t.clr.to_owned()).or_default() += 1;
        }
        self.typed(1 + u64::from(id.is_some()) + u64::from(tag.is_some()));
        self.dirty.push(false);
        let mut out = Vec::with_capacity(props.len() - i);
        let mut unknown = 0u64;
        for (k, v) in &props[i..] {
            match t.member(k) {
                Some(mi) => {
                    let m = &t.members[mi];
                    // A member without a setter is skipped unless it feeds a constructor
                    // parameter or populates its existing value (a non-null object or a
                    // resizable collection; we assume the initializer made one).
                    let populates = match m.codec {
                        Codec::Object(ot) => !schema::ty(ot).has(type_flags::STRUCT),
                        Codec::Seq { kind, .. } => kind != SeqKind::Array,
                        Codec::Dict { .. } | Codec::DictContract { .. } => true,
                        _ => false,
                    };
                    let skip = m.flags & (member_flags::WRITABLE | member_flags::CTOR_PARAM) == 0
                        && !populates;
                    if skip {
                        self.stats.skipped_props += 1;
                        *self
                            .stats
                            .skipped_at
                            .entry(format!("{}.{}", t.clr, m.json))
                            .or_default() += 1;
                    }
                    let sub = Ctx { skipped: ctx.skipped || skip, ..ctx };
                    let mi = u16::try_from(mi).unwrap_or(u16::MAX);
                    self.at.push((concrete, mi));
                    let value = self.value(v, &m.codec, sub);
                    self.at.pop();
                    let key = if m.json == &**k {
                        Key::Member(mi)
                    } else {
                        Key::MemberCase(mi, k.clone())
                    };
                    if t.has(type_flags::ENTITY)
                        && (m.name == "UniqueId" || m.name == "m_Id")
                        && let Json::Str(s) = v
                        && &**s != "-"
                    {
                        self.stats.entity_ids.push(s.clone());
                    }
                    out.push(Prop { key, value });
                }
                None => {
                    unknown += 1;
                    let n = v.count();
                    self.stats.values_untyped += n;
                    *self.stats.unknown_props.entry((concrete, k.clone())).or_default() += 1;
                    collect_ids(v, &mut self.opaque_ids, &mut self.all_ids);
                    if let Some(d) = self.dirty.last_mut() {
                        *d = true;
                    }
                    out.push(Prop {
                        key: Key::Unknown(k.clone()),
                        value: Value::Json(Box::new(v.clone())),
                    });
                }
            }
        }
        let dirty = self.dirty.pop().unwrap_or(false);
        if let Some(id) = &id
            && late
        {
            // Registered after construction: a child may have taken the id meanwhile.
            if self.duplicate(id, ctx) {
                (self.stats.values_typed, self.stats.values_untyped) = counts;
                return self.opaque(j, Reason::BadMetadata("duplicate $id".into()));
            }
            self.register(id, ctx);
        }
        let st = self.stats.per_type.entry(concrete).or_default();
        st.objects += 1;
        st.clean += u64::from(!dirty);
        st.props += out.len() as u64;
        st.props_unknown += unknown;
        Value::Obj(Box::new(Obj { ty: concrete, tag: tag_idx, id, type_first, props: out }))
    }

    #[allow(clippy::too_many_lines)]
    fn value(&mut self, j: &Json, c: &Codec, ctx: Ctx) -> Value {
        match c {
            Codec::Object(t) => self.object(j, Some(*t), false, ctx),
            Codec::Nullable(inner) => match (j, inner) {
                (Json::Null, _) => {
                    self.typed(1);
                    Value::Null
                }
                (_, Codec::Object(t)) => self.object(j, Some(*t), true, ctx),
                (_, Codec::Enum(e)) => self.enumeration(j, *e, true),
                _ => self.value(j, inner, ctx),
            },
            Codec::Any => match j {
                Json::Obj(_) => self.object(j, None, false, ctx),
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Bool(b) => {
                    self.typed(1);
                    Value::Bool(*b)
                }
                Json::Str(s) => {
                    self.typed(1);
                    Value::Str(s.clone())
                }
                _ => {
                    // Numbers and arrays under `object`: Newtonsoft's long/double/JArray.
                    self.typed(j.count());
                    self.stats.values_raw += j.count();
                    Value::Json(Box::new(j.clone()))
                }
            },
            Codec::JToken => {
                self.typed(j.count());
                self.stats.values_raw += j.count();
                Value::Json(Box::new(j.clone()))
            }
            Codec::Bool => match j {
                Json::Bool(b) => {
                    self.typed(1);
                    Value::Bool(*b)
                }
                _ => self.opaque(j, Reason::Mismatch("bool".into(), j.kind().into())),
            },
            Codec::Int { bits, signed } => self.int(j, *bits, *signed),
            Codec::F32 | Codec::F64 => self.float(j, matches!(c, Codec::F32)),
            Codec::Decimal => match j {
                Json::Num(_) => {
                    self.typed(1);
                    self.stats.values_raw += 1;
                    Value::Json(Box::new(j.clone()))
                }
                _ => self.opaque(j, Reason::Mismatch("decimal".into(), j.kind().into())),
            },
            Codec::Str | Codec::Char | Codec::Version | Codec::StringEntry | Codec::Bytes => {
                match j {
                    Json::Str(s) => {
                        if ctx.in_dict && is_iso_date(s) {
                            self.stats.date_strings_in_dict += 1;
                        }
                        self.typed(1);
                        Value::Str(s.clone())
                    }
                    Json::Null => {
                        self.typed(1);
                        Value::Null
                    }
                    Json::Num(n) if matches!(c, Codec::Str) => {
                        self.coerced(Value::Str(n.clone()), j, "string")
                    }
                    Json::Bool(b) if matches!(c, Codec::Str) => self.coerced(
                        Value::Str(if *b { "true" } else { "false" }.into()),
                        j,
                        "string",
                    ),
                    _ => self.opaque(j, Reason::Mismatch(c.name().into(), j.kind().into())),
                }
            }
            Codec::Guid => match j {
                Json::Str(s) => match guid_value(s) {
                    Some(v) => {
                        self.typed(1);
                        v
                    }
                    None if guid_parses(s) => self.coerced(Value::Str(s.clone()), j, "Guid"),
                    None => self.opaque(j, Reason::Converter("unparsable Guid".into())),
                },
                _ => self.opaque(j, Reason::Mismatch("Guid".into(), j.kind().into())),
            },
            Codec::TimeSpan => match j {
                Json::Str(s) => match parse_timespan(s) {
                    Some(t) if fmt_timespan(t) == **s => {
                        self.typed(1);
                        Value::TimeSpan(t)
                    }
                    Some(t) => self.coerced(Value::TimeSpan(t), j, "TimeSpan"),
                    None => self.opaque(j, Reason::Converter("unparsed TimeSpan format".into())),
                },
                _ => self.opaque(j, Reason::Mismatch("TimeSpan".into(), j.kind().into())),
            },
            Codec::DateTime | Codec::DateTimeOffset => match j {
                Json::Str(s) if is_iso_date(s) => {
                    self.typed(1);
                    Value::Date(s.clone())
                }
                _ => self.opaque(j, Reason::Mismatch(c.name().into(), j.kind().into())),
            },
            Codec::Enum(e) => self.enumeration(j, *e, false),
            Codec::Blueprint(target) => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Str(s) if &**s == "null" => {
                    self.typed(1);
                    Value::NullText
                }
                Json::Str(s) => match guid_value(s) {
                    Some(v) => {
                        self.bp_use(&v, *target, true, ctx);
                        self.typed(1);
                        v
                    }
                    None => self
                        .opaque(j, Reason::Converter("blueprint GUID not in N or D form".into())),
                },
                _ => self.opaque(j, Reason::Mismatch("BlueprintConverter".into(), j.kind().into())),
            },
            Codec::BpRef(target) => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Str(s) if s.is_empty() => {
                    self.typed(1);
                    Value::GuidEmpty
                }
                Json::Str(s) => match guid_value(s) {
                    Some(v) => {
                        self.bp_use(&v, *target, false, ctx);
                        self.typed(1);
                        v
                    }
                    None => self
                        .opaque(j, Reason::Converter("reference GUID not in N or D form".into())),
                },
                Json::Obj(_) if has_dup_keys_deep(j) => self
                    .opaque(j, Reason::Converter("repeated property in converter object".into())),
                Json::Obj(_) => match j.get("guid") {
                    Some(Json::Str(s)) if guid_value(s).is_some() => {
                        let v = guid_value(s).unwrap_or(Value::GuidEmpty);
                        self.bp_use(&v, *target, false, ctx);
                        self.coerced(v, j, "BlueprintRefConverter")
                    }
                    Some(Json::Null) | None => {
                        self.coerced(Value::Null, j, "BlueprintRefConverter")
                    }
                    _ => {
                        self.opaque(j, Reason::Converter("reference object without a GUID".into()))
                    }
                },
                _ => self
                    .opaque(j, Reason::Mismatch("BlueprintRefConverter".into(), j.kind().into())),
            },
            Codec::BlueprintGuid => match j {
                Json::Str(s) if s.is_empty() => {
                    self.typed(1);
                    Value::GuidEmpty
                }
                Json::Str(s) => match guid_value(s) {
                    Some(v) => {
                        self.typed(1);
                        v
                    }
                    None => {
                        self.opaque(j, Reason::Converter("BlueprintGuid not in N or D form".into()))
                    }
                },
                _ => self
                    .opaque(j, Reason::Mismatch("BlueprintGuidConverter".into(), j.kind().into())),
            },
            Codec::EntityRef(kind) => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Str(s) => {
                    self.stats.entity_refs.push(s.clone());
                    self.typed(1);
                    Value::Str(s.clone())
                }
                Json::Obj(_) if has_dup_keys_deep(j) => self
                    .opaque(j, Reason::Converter("repeated property in converter object".into())),
                Json::Obj(props) => match props.first() {
                    // `(string)JObject.Properties().First().Value`.
                    Some((_, Json::Str(s))) => {
                        self.stats.entity_refs.push(s.clone());
                        self.coerced(Value::Str(s.clone()), j, "EntityRefConverter")
                    }
                    Some((_, Json::Null)) if *kind == RefKind::Typed => {
                        self.coerced(Value::Null, j, "EntityRefConverter")
                    }
                    _ => self.opaque(j, Reason::Converter("entity reference object".into())),
                },
                _ => self.opaque(j, Reason::Mismatch("EntityRefConverter".into(), j.kind().into())),
            },
            Codec::Vector2 | Codec::Vector3 | Codec::Quaternion => {
                let n = if matches!(c, Codec::Vector2) { 2 } else { 3 };
                self.vector(j, n, c.name(), matches!(c, Codec::Quaternion))
            }
            Codec::BoneTransform => match j {
                Json::Str(s) => match parse_cm(s, 6) {
                    Some((v, true)) => {
                        self.typed(1);
                        Value::Cm(v.into())
                    }
                    Some((v, false)) => {
                        self.coerced(Value::Cm(v.into()), j, "BoneTransformDataConverter")
                    }
                    None => self.opaque(j, Reason::Converter("bone transform text".into())),
                },
                _ => self.opaque(
                    j,
                    Reason::Mismatch("BoneTransformDataConverter".into(), j.kind().into()),
                ),
            },
            Codec::Bounds => match j {
                Json::Obj(props)
                    if props.len() == 2 && &*props[0].0 == "center" && &*props[1].0 == "size" =>
                {
                    self.typed(1);
                    let a = self.vector(&props[0].1, 3, "BoundsConverter", false);
                    let b = self.vector(&props[1].1, 3, "BoundsConverter", false);
                    Value::Fields(vec![("center".into(), a), ("size".into(), b)].into())
                }
                Json::Obj(_) => self.opaque(j, Reason::Converter("bounds object shape".into())),
                // Anything but an object reads as `default(Bounds)`.
                _ => self.coerced(Value::Null, j, "BoundsConverter"),
            },
            Codec::CountableFlag => match j {
                Json::Num(n) if is_int_lexeme(n) => self.int(j, 32, true),
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Obj(_) if has_dup_keys_deep(j) => self
                    .opaque(j, Reason::Converter("repeated property in converter object".into())),
                Json::Obj(_) => match j.get("m_Count") {
                    Some(Json::Num(n)) if is_int_lexeme(n) => match n.parse::<i32>() {
                        Ok(v) => {
                            self.coerced(Value::Int(i64::from(v)), j, "CountableFlagConverter")
                        }
                        Err(_) => self.opaque(j, Reason::Converter("CountableFlag count".into())),
                    },
                    _ => self.opaque(j, Reason::Converter("CountableFlag object".into())),
                },
                _ => self
                    .opaque(j, Reason::Mismatch("CountableFlagConverter".into(), j.kind().into())),
            },
            Codec::Sprite => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Str(_) | Json::Num(_) | Json::Bool(_) => {
                    self.coerced(Value::Null, j, "SpriteConverter")
                }
                // The converter doesn't consume an object or array: the reader breaks.
                _ => self.opaque(
                    j,
                    Reason::Converter("SpriteConverter leaves a container unread".into()),
                ),
            },
            Codec::UnityObject(_) => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                _ => self.opaque(j, Reason::Unsupported("Unity object in a save".into())),
            },
            Codec::Unsupported(w) => self.opaque(j, Reason::Unsupported((*w).into())),
            Codec::Seq { elem, .. } => match j {
                Json::Null => {
                    self.typed(1);
                    Value::Null
                }
                Json::Arr(items) => {
                    self.typed(1);
                    let v: Vec<Value> = items.iter().map(|x| self.value(x, elem, ctx)).collect();
                    Value::Seq(v.into())
                }
                _ => self.opaque(j, Reason::Mismatch("array".into(), j.kind().into())),
            },
            Codec::Dict { key, value } => self.dictionary(j, key, value, ctx),
            Codec::DictContract { .. } => {
                self.opaque(j, Reason::Unsupported("dictionary contract".into()))
            }
            Codec::KeyValue { key, value } => match j {
                Json::Obj(p) if p.len() == 2 && &*p[0].0 == "Key" && &*p[1].0 == "Value" => {
                    self.typed(1);
                    let k = self.value(&p[0].1, key, ctx);
                    let v = self.value(&p[1].1, value, ctx);
                    Value::Dict(Box::new(Dict {
                        form: DictForm::KeyValue,
                        entries: vec![Entry::Pair(k, v)],
                    }))
                }
                _ => self.opaque(j, Reason::Mismatch("KeyValuePair".into(), j.kind().into())),
            },
        }
    }

    fn bp_use(&mut self, v: &Value, target: TypeIdx, converter: bool, ctx: Ctx) {
        if let Value::GuidN(g) | Value::GuidD(g) = v {
            self.stats.bp_uses.push(BpUse { guid: *g, target, converter, in_dict: ctx.in_dict });
        }
    }

    fn int(&mut self, j: &Json, bits: u8, signed: bool) -> Value {
        let Json::Num(n) = j else {
            return self.opaque(j, Reason::Mismatch("integer".into(), j.kind().into()));
        };
        if !is_int_lexeme(n) {
            return self.opaque(j, Reason::Mismatch("integer".into(), "float".into()));
        }
        let Ok(v) = n.parse::<i128>() else {
            return self.opaque(j, Reason::Converter("integer out of range".into()));
        };
        let (lo, hi) = int_range(bits, signed);
        if v < lo || v > hi {
            return self.opaque(j, Reason::Converter("integer out of range".into()));
        }
        let typed = if v > i128::from(i64::MAX) {
            Value::UInt(u64::try_from(v).unwrap_or(u64::MAX))
        } else {
            Value::Int(i64::try_from(v).unwrap_or(0))
        };
        if v.to_string() == **n {
            self.typed(1);
            typed
        } else {
            // `-0`, leading zeros (octal for `ReadAsInt32`!): not what the game writes.
            self.opaque(j, Reason::Converter("non-canonical integer".into()))
        }
    }

    fn float(&mut self, j: &Json, single: bool) -> Value {
        let make = |x: f64| if single { Value::F32(x as f32) } else { Value::F64(x) };
        match j {
            // `JsonTextReader` reads an unsigned number with a leading zero as octal.
            Json::Num(n) if is_octal_lexeme(n) => {
                self.opaque(j, Reason::Converter("leading-zero (octal) number".into()))
            }
            Json::Num(n) => match parse_f64(n) {
                Some(x) => {
                    self.typed(1);
                    make(x)
                }
                None => self.opaque(j, Reason::Converter("unparsable number".into())),
            },
            // FloatFormatHandling.String writes NaN and infinities as strings;
            // `ReadAsDouble` parses strings.
            Json::Str(s) => match parse_f64(s.trim()) {
                Some(x) if matches!(&**s, "NaN" | "Infinity" | "-Infinity") => {
                    self.typed(1);
                    make(x)
                }
                Some(x) => self.coerced(make(x), j, "float"),
                None => self.opaque(j, Reason::Converter("unparsable float string".into())),
            },
            _ => self.opaque(j, Reason::Mismatch("float".into(), j.kind().into())),
        }
    }

    /// `StringEnumConverter.ReadJson`. `nullable`: declared `Nullable<enum>`, where a
    /// failed parse yields null (`Activator.CreateInstance` of the nullable type).
    fn enumeration(&mut self, j: &Json, e: u32, nullable: bool) -> Value {
        let def = schema::enum_def(e);
        let e16 = u16::try_from(e).unwrap_or(u16::MAX);
        match j {
            Json::Str(s) => {
                if let Some(mi) = def.member_index(s) {
                    self.typed(1);
                    return Value::Enum {
                        e: e16,
                        name: u16::try_from(mi).unwrap_or(ENUM_NUMBER - 1),
                        value: def.members[mi].1,
                    };
                }
                match def.parse(s) {
                    Some(v) => {
                        self.typed(1);
                        Value::EnumText(Box::new(EnumText { e: e16, value: v, text: s.clone() }))
                    }
                    // `StringEnumConverter` logs and returns the default value.
                    None if nullable => self.coerced(Value::Null, j, "enum"),
                    None => {
                        self.coerced(Value::Enum { e: e16, name: ENUM_NUMBER, value: 0 }, j, "enum")
                    }
                }
            }
            Json::Num(n) if is_int_lexeme(n) => match n.parse::<i64>() {
                Ok(v) if v.to_string() == **n && def.wrap(v) == v => {
                    self.typed(1);
                    Value::Enum { e: e16, name: ENUM_NUMBER, value: v }
                }
                // `Enum.ToObject` wraps to the underlying type: keep what was written.
                Ok(v) if v.to_string() == **n => self.coerced(
                    Value::Enum { e: e16, name: ENUM_NUMBER, value: def.wrap(v) },
                    j,
                    "enum",
                ),
                _ => self.opaque(j, Reason::Converter("enum integer".into())),
            },
            Json::Null => {
                self.coerced(Value::Enum { e: e16, name: ENUM_NUMBER, value: 0 }, j, "enum")
            }
            _ => self.opaque(j, Reason::Mismatch("enum".into(), j.kind().into())),
        }
    }

    fn vector(&mut self, j: &Json, n: usize, codec: &'static str, quaternion: bool) -> Value {
        match j {
            Json::Str(s) => match parse_cm(s, n) {
                Some((v, true)) => {
                    self.typed(1);
                    Value::Cm(v.into())
                }
                Some((v, false)) => self.coerced(Value::Cm(v.into()), j, codec),
                None => self.opaque(j, Reason::Converter("vector text".into())),
            },
            Json::Obj(props) => {
                let names: &[&str] = match (n, quaternion) {
                    (2, _) => &["x", "y"],
                    (_, true) => &["x", "y", "z", "w"],
                    _ => &["x", "y", "z"],
                };
                let ok = props.len() == names.len()
                    && props.iter().zip(names).all(|((k, v), want)| {
                        &**k == *want
                            && matches!(v, Json::Num(x) if parse_f64(x).is_some() && !is_octal_lexeme(x))
                    });
                if !ok {
                    return self.opaque(j, Reason::Converter("vector object shape".into()));
                }
                let fs: Vec<f32> = props
                    .iter()
                    .map(|(_, v)| match v {
                        Json::Num(x) => parse_f64(x).unwrap_or(0.0) as f32,
                        _ => 0.0,
                    })
                    .collect();
                self.typed(j.count());
                Value::Floats(fs.into())
            }
            _ => self.opaque(j, Reason::Mismatch(codec.into(), j.kind().into())),
        }
    }

    /// `DictionaryConverter.ReadJson`. The whole input is loaded with `JArray.Load` /
    /// `JObject.Load` first (repeated properties replaced at every depth), then each entry
    /// converted inside a try: a failed conversion logs and skips the entry. Array form:
    /// key, then value, each in its own try. Object form: value in a try, then the key
    /// parsed outside it (a bad int or enum key fails the document).
    #[allow(clippy::too_many_lines)]
    fn dictionary(&mut self, j: &Json, key: &Codec, value: &Codec, ctx: Ctx) -> Value {
        let sub = Ctx { in_dict: true, ..ctx };
        if !matches!(j, Json::Null) && has_dup_keys_deep(j) {
            return self.opaque(
                j,
                Reason::Converter("repeated property inside a converter-loaded value".into()),
            );
        }
        match j {
            Json::Null => {
                self.typed(1);
                Value::Null
            }
            Json::Arr(items) => {
                // `foreach (JObject item in …)`: a non-object element throws.
                if items.iter().any(|x| !matches!(x, Json::Obj(_))) {
                    return self.opaque(
                        j,
                        Reason::Converter("dictionary array element not an object".into()),
                    );
                }
                self.typed(1);
                let mut entries = Vec::with_capacity(items.len());
                let mut seen: HashSet<String> = HashSet::new();
                for item in items {
                    let Json::Obj(p) = item else { continue };
                    // `item["Key"]` by name; missing, `ToObject` on null throws: skipped.
                    let Some(kj) = item.get("Key") else {
                        self.skip_entry(item, "entry without Key");
                        entries.push(Entry::Raw(
                            item.clone(),
                            Reason::Converter("entry without Key".into()),
                        ));
                        continue;
                    };
                    self.typed(1);
                    let k = self.value(kj, key, sub);
                    let vj = item.get("Value");
                    // Values in the element besides the decoded key and value (extra
                    // properties, a missing or unread value) are kept but not typed.
                    let decoded_n = 1 + kj.count();
                    if contains_opaque(&k) || vj.is_none() {
                        // Key conversion failed (the value is never read), or there is no
                        // value (the key was converted, then `ToObject` on null threw).
                        self.stats.dict_dropped += 1;
                        if let Some(vj) = vj {
                            collect_ids(vj, &mut self.skipped_ids, &mut self.all_ids);
                        }
                        self.stats.values_untyped += item.count() - decoded_n;
                        let raw = vj.map_or(Value::Null, |v| Value::Json(Box::new(v.clone())));
                        entries.push(Entry::Laid(k, raw, item.clone()));
                        continue;
                    }
                    let vj = vj.unwrap_or(&Json::Null);
                    let v = self.value(vj, value, sub);
                    if contains_opaque(&v) || matches!(k, Value::Null) {
                        // A failed value conversion (or a null key) drops the entry. Ids the
                        // game registered before the failure point stay registered; we
                        // don't model which (not seen in saves).
                        self.stats.dict_dropped += 1;
                    } else if let Some(id) = key_identity(&k)
                        && !seen.insert(id)
                    {
                        self.stats.dict_repeats += 1;
                    }
                    let plain = p.len() == 2 && &*p[0].0 == "Key" && &*p[1].0 == "Value";
                    if plain {
                        entries.push(Entry::Pair(k, v));
                    } else {
                        self.stats.values_untyped += item.count() - decoded_n - vj.count();
                        *self.stats.coerced.entry("dictionary entry layout").or_default() += 1;
                        entries.push(Entry::Laid(k, v, item.clone()));
                    }
                }
                Value::Dict(Box::new(Dict { form: DictForm::Array, entries }))
            }
            Json::Obj(props) => {
                if !matches!(
                    key,
                    Codec::Str | Codec::Int { bits: 32, signed: true } | Codec::Enum(_)
                ) {
                    // Other key types: `Dictionary.Add` with a string key throws.
                    return self.opaque(
                        j,
                        Reason::Converter("object-form dictionary with non-string keys".into()),
                    );
                }
                let counts = (self.stats.values_typed, self.stats.values_untyped);
                self.typed(1);
                let mut entries = Vec::with_capacity(props.len());
                let mut seen: HashSet<String> = HashSet::new();
                for (k, v) in props {
                    let vv = self.value(v, value, sub);
                    if contains_opaque(&vv) {
                        // The value failed: skipped before its key is parsed.
                        self.stats.dict_dropped += 1;
                        entries.push(Entry::Pair(Value::Str(k.clone()), vv));
                        continue;
                    }
                    let kv = match key {
                        Codec::Int { .. } => match k.trim().parse::<i32>() {
                            Ok(x) if x.to_string() == **k => Value::Int(i64::from(x)),
                            Ok(x) => {
                                *self.stats.coerced.entry("dictionary key").or_default() += 1;
                                Value::Coerced(Box::new((
                                    Value::Int(i64::from(x)),
                                    Json::Str(k.clone()),
                                )))
                            }
                            // `int.Parse` throws outside the converter's try.
                            Err(_) => {
                                (self.stats.values_typed, self.stats.values_untyped) = counts;
                                return self
                                    .opaque(j, Reason::Converter("dictionary int key".into()));
                            }
                        },
                        Codec::Enum(e) => {
                            let def = schema::enum_def(*e);
                            let e16 = u16::try_from(*e).unwrap_or(u16::MAX);
                            match (def.member_index(k), def.parse(k)) {
                                (Some(mi), _) => Value::Enum {
                                    e: e16,
                                    name: u16::try_from(mi).unwrap_or(ENUM_NUMBER - 1),
                                    value: def.members[mi].1,
                                },
                                (None, Some(x)) => Value::EnumText(Box::new(EnumText {
                                    e: e16,
                                    value: x,
                                    text: k.clone(),
                                })),
                                // `Enum.Parse` throws outside the converter's try.
                                (None, None) => {
                                    (self.stats.values_typed, self.stats.values_untyped) = counts;
                                    return self.opaque(
                                        j,
                                        Reason::Converter("dictionary enum key".into()),
                                    );
                                }
                            }
                        }
                        _ => Value::Str(k.clone()),
                    };
                    if let Some(id) = key_identity(&kv)
                        && !seen.insert(id)
                    {
                        self.stats.dict_repeats += 1;
                    }
                    entries.push(Entry::Pair(kv, vv));
                }
                Value::Dict(Box::new(Dict { form: DictForm::Object, entries }))
            }
            _ => self.opaque(j, Reason::Mismatch("DictionaryConverter".into(), j.kind().into())),
        }
    }

    fn skip_entry(&mut self, item: &Json, why: &str) {
        self.stats.dict_dropped += 1;
        let n = item.count();
        self.stats.values_untyped += n;
        let e = self.stats.opaque.entry(format!("dictionary entry skipped: {why}")).or_default();
        e.0 += 1;
        e.1 += n;
        collect_ids(item, &mut self.skipped_ids, &mut self.all_ids);
        self.mark_dirty();
    }
}

/// Ids defined inside a JSON value kept raw.
fn collect_ids(j: &Json, into: &mut HashSet<Id>, all: &mut HashSet<Id>) {
    match j {
        Json::Obj(props) => {
            for (k, v) in props {
                if &**k == "$id"
                    && let Json::Str(s) = v
                {
                    let id = Id::parse(s);
                    all.insert(id.clone());
                    into.insert(id);
                }
                collect_ids(v, into, all);
            }
        }
        Json::Arr(items) => items.iter().for_each(|x| collect_ids(x, into, all)),
        _ => {}
    }
}

/// The declared root type of a document, by its zip entry name
/// (`ThreadedGameLoader`, `AreaDataStash`). `None` for non-JSON or unknown entries.
#[must_use]
pub fn root_type(name: &str) -> Option<&'static str> {
    let stem = name.strip_suffix(".json")?;
    Some(match stem {
        "header" => "Kingmaker.EntitySystem.Persistence.SaveInfo",
        "player" => "Kingmaker.Player",
        "party" => "Kingmaker.EntitySystem.SceneEntitiesState",
        "tbm" => "TurnBased.Controllers.CombatController",
        "clockwork" => "Kingmaker.QA.Clockwork.PlayData",
        _ if stem.len() == 32 && stem.bytes().all(|c| c.is_ascii_hexdigit()) => {
            "Kingmaker.EntitySystem.AreaPersistentState"
        }
        _ if stem.len() > 32 && stem.as_bytes()[..32].iter().all(u8::is_ascii_hexdigit) => {
            "Kingmaker.EntitySystem.SceneEntitiesState"
        }
        _ => return None,
    })
}

/// Root type index for an entry, if the schema has it.
#[must_use]
pub fn root_index(name: &str) -> Option<TypeIdx> {
    root_type(name).and_then(schema::by_name).filter(|&t| t != NO_TYPE)
}

#[cfg(test)]
mod tests;
