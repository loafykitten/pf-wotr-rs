//! A reader for the game's runtime pack, `Bundles/blueprints-pack.bbp`
//! (`BlueprintsCache` + `ReflectionBasedSerializer`), decoding records through the same
//! generated schema so they can be compared with the zip object by object.
//!
//! Layout: `i32` record count, then `count × (16-byte GUID, u32 offset)`; each record is
//! `GenericObject` (a 16-byte type GUID, then every runtime field as its C# name followed
//! by its value, see [`crate::packfmt`]), then the blueprint's `name` and its GUID. GUIDs
//! are in .NET `Guid.ToByteArray()` order, strings are `BinaryWriter` strings (7-bit
//! length prefix, UTF-8).
//!
//! What it can't do on its own: Unity objects are indices into `BlueprintReferencedAssets`
//! (in `blueprint.assets`), so assets and shared strings come back as
//! [`Node::AssetIndex`] until that list is decoded.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::guid::Guid;
use crate::node::{Curve, Gradient, Keyframe, Node, Object};
use crate::packfmt::{self, ElemKind, Item, PackKind};
use crate::schema::{self, Codec, TypeIdx, UnityKind, type_flags};

/// The pack, read into memory.
#[derive(Debug)]
pub struct Bbp {
    data: Vec<u8>,
    /// Record GUID and offset, in file order. Offset 0 means "no record".
    pub toc: Vec<(Guid, u32)>,
}

/// One decoded record.
#[derive(Debug)]
pub struct Record {
    pub guid: Guid,
    pub name: String,
    pub data: Option<Object>,
    /// Bytes from the record's offset to the end of its GUID.
    pub len: usize,
}

impl Bbp {
    /// Reads the pack and its table of contents.
    ///
    /// # Errors
    ///
    /// I/O errors or a truncated table.
    pub fn open(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::from_bytes(data)
    }

    /// Parses a pack held in memory.
    ///
    /// # Errors
    ///
    /// A truncated table.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let mut r = Reader { b: &data, pos: 0 };
        let n = usize::try_from(r.i32()?).context("negative record count")?;
        let mut toc = Vec::with_capacity(n);
        for _ in 0..n {
            let g = Guid::from_dotnet_bytes(r.bytes16()?);
            let off = r.u32()?;
            toc.push((g, off));
        }
        Ok(Self { data, toc })
    }

    /// Decodes the record at `offset`.
    ///
    /// # Errors
    ///
    /// A field name that doesn't match the schema, an unknown type GUID, or truncation:
    /// any of these means our reading of the format (or the schema) is wrong.
    pub fn decode(&self, offset: u32) -> Result<Record> {
        let start = offset as usize;
        let mut r = Reader { b: &self.data, pos: start };
        let root = r.generic(None)?;
        // `Blueprint(ref bp)` reads the name and GUID only for a non-null blueprint.
        let data = match root {
            Node::Object(o) => *o,
            Node::Null => {
                return Ok(Record {
                    guid: Guid::EMPTY,
                    name: String::new(),
                    data: None,
                    len: r.pos - start,
                });
            }
            _ => bail!("root is not an object"),
        };
        let name = r.string()?;
        let guid = Guid::from_dotnet_bytes(r.bytes16()?);
        Ok(Record { guid, name, data: Some(data), len: r.pos - start })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else { bail!("truncated at {}", self.pos) };
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn bytes16(&mut self) -> Result<[u8; 16]> {
        let mut a = [0u8; 16];
        a.copy_from_slice(self.take(16)?);
        Ok(a)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    fn count(&mut self) -> Result<usize> {
        let n = self.i32()?;
        ensure!((0..=50_000_000).contains(&n), "bad count {n} at {}", self.pos - 4);
        Ok(usize::try_from(n)?)
    }

    /// `BinaryReader.ReadString`: 7-bit encoded byte length, then UTF-8.
    fn string(&mut self) -> Result<String> {
        let mut len: u32 = 0;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            len |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            ensure!(shift <= 28, "bad string length at {}", self.pos);
        }
        let s = self.take(len as usize)?;
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn floats(&mut self, n: usize) -> Result<Node> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.f32()?);
        }
        Ok(Node::Floats(v.into()))
    }

    /// `GenericObject(ref obj, fieldType)` in read mode. `declared` is `None` for the root.
    fn generic(&mut self, declared: Option<TypeIdx>) -> Result<Node> {
        let identified = declared.is_none_or(|t| schema::ty(t).has(type_flags::IDENTIFIED));
        let ty = if identified {
            let g = Guid::from_dotnet_bytes(self.bytes16()?);
            if g.is_empty() {
                return Ok(Node::Null);
            }
            match schema::by_type_id(g) {
                Some(t) => t,
                None => bail!("unknown type GUID {g} at {}", self.pos - 16),
            }
        } else {
            declared.unwrap_or(schema::NO_TYPE)
        };
        let def = schema::ty(ty);
        let mut fields = vec![Node::Absent; def.fields.len()];
        for (j, f) in def.fields.iter().enumerate() {
            if !f.in_runtime() {
                continue;
            }
            let at = self.pos;
            let name = self.string()?;
            if name != f.name {
                bail!("expected {}.{} but got {name:?} at {at}", def.name, f.name);
            }
            fields[j] = self.field(&f.codec)?;
        }
        Ok(Node::Object(Box::new(Object { ty, tagged: identified, fields, extra: Vec::new() })))
    }

    fn item(&mut self, i: Item) -> Result<Node> {
        Ok(match i {
            Item::I32 => Node::Int(i64::from(self.i32()?)),
            Item::U32 => Node::Int(i64::from(self.u32()?)),
            Item::F32 => Node::F32(self.f32()?),
            Item::I64 => Node::Int(self.i64()?),
            Item::U64 => Node::UInt(self.u64()?),
            Item::Str => Node::Str(self.string()?.into()),
            Item::Bool => Node::Bool(self.bool()?),
            Item::Color | Item::Vector4 => self.floats(4)?,
            Item::Color32 => Node::Int(i64::from(self.i32()?)),
            Item::Vector2 => self.floats(2)?,
            Item::Vector3 => self.floats(3)?,
            Item::Vector2Int => Node::Ints(vec![self.i32()?, self.i32()?].into()),
        })
    }

    fn field(&mut self, c: &Codec) -> Result<Node> {
        Ok(match packfmt::kind(c) {
            PackKind::I32 => Node::Int(i64::from(self.i32()?)),
            PackKind::U32 => Node::Int(i64::from(self.u32()?)),
            PackKind::F32 => Node::F32(self.f32()?),
            PackKind::F64 => Node::F64(self.f64()?),
            PackKind::I64 => Node::Int(self.i64()?),
            PackKind::U64 => Node::UInt(self.u64()?),
            PackKind::Str => Node::Str(self.string()?.into()),
            PackKind::Bool => Node::Bool(self.bool()?),
            PackKind::Enum => Node::Enum(i64::from(self.i32()?)),
            PackKind::Color | PackKind::Vector4 => self.floats(4)?,
            PackKind::Color32 => Node::Int(i64::from(self.i32()?)),
            PackKind::Vector2 => self.floats(2)?,
            PackKind::Vector3 => self.floats(3)?,
            PackKind::Bounds => self.floats(6)?,
            PackKind::Vector2Int => Node::Ints(vec![self.i32()?, self.i32()?].into()),
            PackKind::Gradient => {
                let n = self.count()?;
                let mut colors = Vec::with_capacity(n);
                for _ in 0..n {
                    colors.push([self.f32()?, self.f32()?, self.f32()?, self.f32()?]);
                }
                let n = self.count()?;
                let mut alphas = Vec::with_capacity(n);
                for _ in 0..n {
                    alphas.push([self.f32()?, self.f32()?]);
                }
                let mode = self.u8()?;
                Node::Gradient(Box::new(Gradient { colors, alphas, mode }))
            }
            PackKind::AnimationCurve => {
                let n = self.count()?;
                let mut keys = Vec::with_capacity(n);
                for _ in 0..n {
                    let time = self.f32()?;
                    let value = self.f32()?;
                    let weighted_mode = i32::from(self.u8()?);
                    let in_tangent = self.f32()?;
                    let in_weight = self.f32()?;
                    let out_tangent = self.f32()?;
                    let out_weight = self.f32()?;
                    keys.push(Keyframe {
                        time,
                        value,
                        in_tangent,
                        out_tangent,
                        in_weight,
                        out_weight,
                        weighted_mode,
                    });
                }
                // The pack doesn't store wrap modes.
                Node::Curve(Box::new(Curve { pre_wrap: 0, post_wrap: 0, keys }))
            }
            PackKind::BlueprintGuid | PackKind::BpRef => {
                Node::BpRef(Guid::from_dotnet_bytes(self.bytes16()?))
            }
            PackKind::UnityObject => Node::AssetIndex(self.i32()?),
            PackKind::ColorBlock => {
                let t = schema::special().color_block;
                let def = schema::ty(t);
                let mut fields = vec![Node::Absent; def.fields.len()];
                let mut set = |name: &str, n: Node| {
                    if let Some(j) = def.fields.iter().position(|f| f.name == name) {
                        fields[j] = n;
                    }
                };
                for name in
                    ["m_NormalColor", "m_PressedColor", "m_HighlightedColor", "m_DisabledColor"]
                {
                    let c = self.floats(4)?;
                    set(name, c);
                }
                set("m_SelectedColor", Node::Floats(vec![0.0; 4].into()));
                set("m_ColorMultiplier", Node::F32(self.f32()?));
                set("m_FadeDuration", Node::F32(self.f32()?));
                Node::Object(Box::new(Object { ty: t, tagged: false, fields, extra: Vec::new() }))
            }
            PackKind::Array(i) => {
                let n = self.count()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.item(i)?);
                }
                Node::Seq(v)
            }
            PackKind::RefSeq => {
                let n = self.count()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(Node::BpRef(Guid::from_dotnet_bytes(self.bytes16()?)));
                }
                Node::Seq(v)
            }
            PackKind::UnityObjectSeq => {
                let n = self.count()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(Node::AssetIndex(self.i32()?));
                }
                Node::Seq(v)
            }
            PackKind::GenericSeq => {
                let Codec::Seq { elem, .. } = c else { bail!("generic seq without element") };
                let n = self.count()?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(match packfmt::elem_kind(elem) {
                        ElemKind::Object => match elem {
                            Codec::Object(t) | Codec::WeakLink(t) => self.generic(Some(*t))?,
                            _ => bail!("object element without a type"),
                        },
                        ElemKind::FloatFields(names) => {
                            let mut f = Vec::with_capacity(names.len());
                            for want in names {
                                let got = self.string()?;
                                ensure!(got == *want, "expected element field {want}, got {got:?}");
                                f.push(self.f32()?);
                            }
                            Node::Floats(f.into())
                        }
                        ElemKind::Zero => zero(elem),
                    });
                }
                Node::Seq(v)
            }
            PackKind::Generic => match c {
                Codec::Object(t) | Codec::WeakLink(t) => self.generic(Some(*t))?,
                _ => bail!("generic field without a type"),
            },
            PackKind::Lost => zero(c),
        })
    }
}

/// The value a field without serialized data reads back as.
fn zero(c: &Codec) -> Node {
    match c {
        Codec::Bool => Node::Bool(false),
        Codec::F32 => Node::F32(0.0),
        Codec::F64 => Node::F64(0.0),
        Codec::Unity(UnityKind::AnimationCurve) => {
            Node::Curve(Box::new(Curve { pre_wrap: 0, post_wrap: 0, keys: Vec::new() }))
        }
        Codec::Unity(UnityKind::Vector2Int) => Node::Ints(vec![0, 0].into()),
        Codec::Unity(UnityKind::Vector3Int) => Node::Ints(vec![0, 0, 0].into()),
        Codec::Unity(UnityKind::Rect | UnityKind::Quaternion) => Node::Floats(vec![0.0; 4].into()),
        Codec::Unity(UnityKind::Bounds) => Node::Floats(vec![0.0; 6].into()),
        Codec::Str => Node::Str("".into()),
        _ => Node::Int(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dotnet(g: &str) -> [u8; 16] {
        let b = Guid::parse(g).unwrap().to_bytes();
        // Inverse of `from_dotnet_bytes`.
        [
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ]
    }

    fn s(out: &mut Vec<u8>, v: &str) {
        assert!(v.len() < 128);
        out.push(u8::try_from(v.len()).unwrap());
        out.extend_from_slice(v.as_bytes());
    }

    #[test]
    fn decodes_a_record() {
        // One `BlueprintScriptableObject` record, written the way `WriteField` does.
        let mut rec = Vec::new();
        rec.extend_from_slice(&dotnet("3ec4f91d40b87d34197f44f40a969d92"));
        s(&mut rec, "m_PrototypeId");
        s(&mut rec, "");
        s(&mut rec, "m_Overrides");
        rec.extend_from_slice(&1i32.to_le_bytes());
        s(&mut rec, "Comment");
        s(&mut rec, "Components");
        rec.extend_from_slice(&0i32.to_le_bytes());
        s(&mut rec, "Comment");
        s(&mut rec, "hi");
        s(&mut rec, "Thing");
        rec.extend_from_slice(&dotnet("00112233445566778899aabbccddeeff"));
        let mut file = Vec::new();
        file.extend_from_slice(&1i32.to_le_bytes());
        file.extend_from_slice(&dotnet("00112233445566778899aabbccddeeff"));
        file.extend_from_slice(&24u32.to_le_bytes());
        file.extend_from_slice(&rec);
        let p = Bbp::from_bytes(file).unwrap();
        assert_eq!(p.toc[0].0.to_string(), "00112233445566778899aabbccddeeff");
        let r = p.decode(p.toc[0].1).unwrap();
        assert_eq!(r.name, "Thing");
        assert_eq!(r.guid, p.toc[0].0);
        assert_eq!(r.len, rec.len());
        let d = r.data.unwrap();
        assert_eq!(schema::ty(d.ty).short, "BlueprintScriptableObject");
        assert_eq!(d.get("Comment"), Some(&Node::Str("hi".into())));
        assert_eq!(d.get("m_Overrides"), Some(&Node::Seq(vec![Node::Str("Comment".into())])));
        // A field name out of order means our reading of the format is wrong.
        let mut bad = p.data.clone();
        let at = 24 + 16 + 1;
        bad[at] = b'X';
        assert!(Bbp::from_bytes(bad).unwrap().decode(24).is_err());
    }
}
