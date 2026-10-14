//! How the game's runtime pack (`blueprints-pack.bbp`, `ReflectionBasedSerializer`)
//! encodes each field, as a function of the field's codec.
//!
//! `ReadField`/`WriteField` test the field type against a fixed list of cases. Types that
//! miss every case but are `[Serializable]` (which includes mscorlib's `byte`, `short`,
//! `char`, `decimal`) go through `GenericObject`, which writes the type's serialized
//! fields: for primitives there are none, so **the value is lost and reads back as zero**.
//! The same happens to elements of lists and arrays without a dedicated case
//! (`List<float>`, `double[]`, `List<bool>`…), and to Unity value types whose fields are
//! private (`Vector2Int`, `Rect`, `Bounds` inside a generic list). Both the `.bbp` decoder
//! and the canonical form follow this table, so the canonical form says what the running
//! game holds, not what the JSON says.

use crate::schema::{self, Codec, UnityKind};

/// A field's encoding in the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    I32,
    U32,
    F32,
    F64,
    I64,
    U64,
    Str,
    Bool,
    /// Written as `i32`.
    Enum,
    Color,
    Color32,
    Vector2,
    Vector3,
    Vector4,
    Bounds,
    Vector2Int,
    Gradient,
    AnimationCurve,
    BlueprintGuid,
    /// A `UnityEngine.Object`: an `i32` index into `BlueprintReferencedAssets` (-1 null).
    UnityObject,
    /// `i32` count, then fixed-size items of the given kind.
    Array(Item),
    /// `i32` count, then 16-byte GUIDs.
    RefSeq,
    /// `i32` count, then asset indices.
    UnityObjectSeq,
    /// `i32` count, then each element through `GenericObject`.
    GenericSeq,
    /// 16-byte GUID.
    BpRef,
    /// `UnityEngine.UI.ColorBlock`: normal, pressed, highlighted and disabled colours, then
    /// the multiplier and fade duration (the selected colour isn't written).
    ColorBlock,
    /// `GenericObject`: a type GUID first for identified types, then the fields.
    Generic,
    /// No case matches and the type has no serialized fields: nothing is written after
    /// the field name, and the value reads back as zero.
    Lost,
}

/// Items of the dedicated array and list cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    I32,
    U32,
    F32,
    I64,
    U64,
    Str,
    Bool,
    Color,
    Color32,
    Vector2,
    Vector3,
    Vector4,
    Vector2Int,
}

/// The `ReadField` case for a codec, in `ReadField`'s order.
#[must_use]
pub fn kind(c: &Codec) -> PackKind {
    match *c {
        Codec::Int { bits: 32, signed: true } => PackKind::I32,
        Codec::Int { bits: 32, signed: false } => PackKind::U32,
        Codec::F32 => PackKind::F32,
        Codec::F64 => PackKind::F64,
        Codec::Int { bits: 64, signed: true } => PackKind::I64,
        Codec::Int { bits: 64, signed: false } => PackKind::U64,
        Codec::Str => PackKind::Str,
        Codec::Bool => PackKind::Bool,
        Codec::Enum(_) => PackKind::Enum,
        Codec::Unity(UnityKind::Color) => PackKind::Color,
        Codec::Unity(UnityKind::Color32) => PackKind::Color32,
        Codec::Unity(UnityKind::Vector2) => PackKind::Vector2,
        Codec::Unity(UnityKind::Vector3) => PackKind::Vector3,
        Codec::Unity(UnityKind::Vector4) => PackKind::Vector4,
        Codec::Unity(UnityKind::Bounds) => PackKind::Bounds,
        Codec::Unity(UnityKind::Vector2Int) => PackKind::Vector2Int,
        Codec::Unity(UnityKind::Gradient) => PackKind::Gradient,
        Codec::Unity(UnityKind::AnimationCurve) => PackKind::AnimationCurve,
        Codec::BlueprintGuid => PackKind::BlueprintGuid,
        Codec::Asset(_) | Codec::SharedString | Codec::PrototypeLink => PackKind::UnityObject,
        Codec::Seq { list, elem } => seq_kind(list, elem),
        Codec::BpRef { .. } => PackKind::BpRef,
        // `IsOrSubclassOf<BlueprintReferenceBase>` includes the base class itself.
        Codec::Object(t) if t == schema::special().bp_ref_base => PackKind::BpRef,
        Codec::Object(t) if t == schema::special().color_block => PackKind::ColorBlock,
        Codec::Object(_) | Codec::WeakLink(_) => PackKind::Generic,
        // Vector3Int, Quaternion and Rect aren't `[Serializable]`; byte/short/char/decimal
        // are, but have no serialized fields; `OverridesManager` is never in the closure.
        Codec::Unity(UnityKind::Vector3Int | UnityKind::Quaternion | UnityKind::Rect)
        | Codec::Int { .. }
        | Codec::Char
        | Codec::Decimal
        | Codec::Overrides
        | Codec::Unsupported(_) => PackKind::Lost,
    }
}

fn seq_kind(list: bool, elem: &Codec) -> PackKind {
    let item = match (*elem, list) {
        (Codec::Int { bits: 32, signed: true }, _) | (Codec::Enum(_), _) => Some(Item::I32),
        (Codec::Int { bits: 32, signed: false }, false) => Some(Item::U32),
        (Codec::F32, false) => Some(Item::F32),
        (Codec::Int { bits: 64, signed: true }, false) => Some(Item::I64),
        (Codec::Int { bits: 64, signed: false }, false) => Some(Item::U64),
        (Codec::Str, _) => Some(Item::Str),
        (Codec::Bool, false) => Some(Item::Bool),
        (Codec::Unity(UnityKind::Color), false) => Some(Item::Color),
        (Codec::Unity(UnityKind::Color32), _) => Some(Item::Color32),
        (Codec::Unity(UnityKind::Vector2), false) => Some(Item::Vector2),
        (Codec::Unity(UnityKind::Vector3), false) => Some(Item::Vector3),
        (Codec::Unity(UnityKind::Vector4), false) => Some(Item::Vector4),
        (Codec::Unity(UnityKind::Vector2Int), _) => Some(Item::Vector2Int),
        _ => None,
    };
    if let Some(i) = item {
        return PackKind::Array(i);
    }
    match elem {
        Codec::BpRef { .. } => PackKind::RefSeq,
        Codec::Object(t) if *t == schema::special().bp_ref_base => PackKind::RefSeq,
        Codec::Asset(_) | Codec::SharedString | Codec::PrototypeLink => PackKind::UnityObjectSeq,
        _ => PackKind::GenericSeq,
    }
}

/// What an element of a [`PackKind::GenericSeq`] keeps through `GenericObject`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElemKind {
    /// An object with serialized fields (written like a [`PackKind::Generic`] field).
    Object,
    /// A Unity value type with public float fields, written by name (`x`, `y`…).
    FloatFields(&'static [&'static str]),
    /// No serialized fields: each element reads back as the type's zero (or a fresh
    /// empty instance for classes such as `AnimationCurve`).
    Zero,
}

#[must_use]
pub fn elem_kind(elem: &Codec) -> ElemKind {
    match elem {
        Codec::Object(_) | Codec::WeakLink(_) => ElemKind::Object,
        Codec::Unity(UnityKind::Vector2) => ElemKind::FloatFields(&["x", "y"]),
        Codec::Unity(UnityKind::Vector3) => ElemKind::FloatFields(&["x", "y", "z"]),
        Codec::Unity(UnityKind::Vector4 | UnityKind::Quaternion) => {
            ElemKind::FloatFields(&["x", "y", "z", "w"])
        }
        Codec::Unity(UnityKind::Color) => ElemKind::FloatFields(&["r", "g", "b", "a"]),
        _ => ElemKind::Zero,
    }
}

/// Whether the pack keeps a field's value (anything but [`PackKind::Lost`] and generic
/// sequences of zeroed elements).
#[must_use]
pub fn keeps_value(c: &Codec) -> bool {
    match kind(c) {
        PackKind::Lost => false,
        PackKind::GenericSeq => match c {
            Codec::Seq { elem, .. } => elem_kind(elem) != ElemKind::Zero,
            _ => true,
        },
        _ => true,
    }
}

/// Runtime fields of every schema type whose values the pack drops, as
/// `(type, field, codec)`.
#[must_use]
pub fn lossy_fields() -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    for (i, t) in schema::types().iter().enumerate() {
        for f in t.fields {
            if f.in_runtime() && f.declared_in as usize == i && !keeps_value(&f.codec) {
                out.push((t.name, f.name, format!("{:?}", f.codec)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases_follow_read_field() {
        assert_eq!(kind(&Codec::Int { bits: 32, signed: true }), PackKind::I32);
        assert_eq!(kind(&Codec::Int { bits: 8, signed: false }), PackKind::Lost);
        let floats = Codec::Seq { list: true, elem: &Codec::F32 };
        assert_eq!(kind(&floats), PackKind::GenericSeq);
        assert!(!keeps_value(&floats));
        let farr = Codec::Seq { list: false, elem: &Codec::F32 };
        assert_eq!(kind(&farr), PackKind::Array(Item::F32));
        let enums = Codec::Seq { list: true, elem: &Codec::Enum(0) };
        assert_eq!(kind(&enums), PackKind::Array(Item::I32));
        let v3 = Codec::Seq { list: true, elem: &Codec::Unity(UnityKind::Vector3) };
        assert_eq!(kind(&v3), PackKind::GenericSeq);
        assert!(keeps_value(&v3));
    }
}

#[cfg(test)]
mod schema_facts {
    /// No runtime field in this game's schema hits a lossy pack case, so the pack keeps
    /// every value the JSON has. If a schema change breaks this, the canonical form
    /// already writes zeros there; this test just makes it visible.
    #[test]
    fn no_lossy_fields_in_this_schema() {
        assert_eq!(super::lossy_fields(), Vec::<(&str, &str, String)>::new());
    }
}
