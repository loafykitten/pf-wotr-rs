//! Unity type trees: the per-type field layout embedded in serialized files, and the
//! generic reader that decodes an object's bytes into a [`Value`] by walking it.

use anyhow::{Context, Result, bail};

use super::reader::{Endian, Reader};
use super::value::Value;

/// Field needs 4-byte alignment after it is read.
const ALIGN_FLAG: i32 = 0x4000;

/// Unity's shared string table for type tree names (engine type and field names), as of
/// 2020.3. Offsets into it are flagged with the top bit in the blob.
const COMMON_STRINGS: &str = "AABB\0AnimationClip\0AnimationCurve\0AnimationState\0Array\0Base\0BitField\0bitset\0bool\0char\0ColorRGBA\0Component\0data\0deque\0double\0dynamic_array\0FastPropertyName\0first\0float\0Font\0GameObject\0Generic Mono\0GradientNEW\0GUID\0GUIStyle\0int\0list\0long long\0map\0Matrix4x4f\0MdFour\0MonoBehaviour\0MonoScript\0m_ByteSize\0m_Curve\0m_EditorClassIdentifier\0m_EditorHideFlags\0m_Enabled\0m_ExtensionPtr\0m_GameObject\0m_Index\0m_IsArray\0m_IsStatic\0m_MetaFlag\0m_Name\0m_ObjectHideFlags\0m_PrefabInternal\0m_PrefabParentObject\0m_Script\0m_StaticEditorFlags\0m_Type\0m_Version\0Object\0pair\0PPtr<Component>\0PPtr<GameObject>\0PPtr<Material>\0PPtr<MonoBehaviour>\0PPtr<MonoScript>\0PPtr<Object>\0PPtr<Prefab>\0PPtr<Sprite>\0PPtr<TextAsset>\0PPtr<Texture>\0PPtr<Texture2D>\0PPtr<Transform>\0Prefab\0Quaternionf\0Rectf\0RectInt\0RectOffset\0second\0set\0short\0size\0SInt16\0SInt32\0SInt64\0SInt8\0staticvector\0string\0TextAsset\0TextMesh\0Texture\0Texture2D\0Transform\0TypelessData\0UInt16\0UInt32\0UInt64\0UInt8\0unsigned int\0unsigned long long\0unsigned short\0vector\0Vector2f\0Vector3f\0Vector4f\0m_ScriptingClassIdentifier\0Gradient\0Type*\0int2_storage\0int3_storage\0BoundsInt\0m_CorrespondingSourceObject\0m_PrefabInstance\0m_PrefabAsset\0FileSize\0";

fn common_string(offset: u32) -> Option<&'static str> {
    let rest = COMMON_STRINGS.get(offset as usize..)?;
    rest.split('\0').next()
}

/// One node of a type tree, with its children as indices into [`TypeTree::nodes`].
#[derive(Debug, Clone)]
pub struct TypeNode {
    pub type_name: String,
    pub name: String,
    pub byte_size: i32,
    pub level: u8,
    pub meta_flag: i32,
    pub children: Vec<usize>,
}

/// A `[SerializeReference]` type: its managed identity and type tree.
#[derive(Debug, Clone)]
pub struct RefType {
    pub class: String,
    pub namespace: String,
    pub assembly: String,
    pub tree: Option<TypeTree>,
}

/// Reading context passed down the tree.
#[derive(Debug, Clone, Copy)]
struct Ctx<'r> {
    refs: &'r [RefType],
    /// Inside `[SerializeReference]` data, where nested registries are not stored.
    in_registry: bool,
}

/// Unity's end-of-registry marker in version-1 `ManagedReferencesRegistry` data.
const TERMINUS: (&str, &str, &str) = ("Terminus", "UnityEngine.DMAT", "FAKE_ASM");

/// A parsed type tree; node 0 is the root.
#[derive(Debug, Clone, Default)]
pub struct TypeTree {
    pub nodes: Vec<TypeNode>,
}

impl TypeTree {
    /// Parses the blob form (serialized file format >= 12). For format >= 19 each
    /// node record is 32 bytes (with a ref-type hash), else 24.
    pub fn parse_blob(r: &mut Reader<'_>, format: u32) -> Result<Self> {
        let count = r.len_i32(24)?;
        let strings_len = r.len_i32(1)?;
        let rec = if format >= 19 { 32 } else { 24 };
        let records = r.take(count * rec)?;
        let strings = r.take(strings_len)?;
        let mut nodes: Vec<TypeNode> = Vec::with_capacity(count);
        let mut stack: Vec<usize> = Vec::new();
        let read_str = |off: u32| -> Result<String> {
            if off & 0x8000_0000 != 0 {
                let o = off & 0x7fff_ffff;
                Ok(common_string(o).map_or_else(|| o.to_string(), str::to_owned))
            } else {
                let s = strings.get(off as usize..).unwrap_or_default();
                let n = s.iter().position(|&b| b == 0).unwrap_or(s.len());
                Ok(String::from_utf8_lossy(&s[..n]).into_owned())
            }
        };
        for i in 0..count {
            let mut nr = Reader::new(&records[i * rec..(i + 1) * rec], r.endian);
            let _version = nr.u16()?;
            let level = nr.u8()?;
            let _type_flags = nr.u8()?;
            let type_off = nr.u32()?;
            let name_off = nr.u32()?;
            let byte_size = nr.i32()?;
            let _index = nr.i32()?;
            let meta_flag = nr.i32()?;
            let node = TypeNode {
                type_name: read_str(type_off)?,
                name: read_str(name_off)?,
                byte_size,
                level,
                meta_flag,
                children: Vec::new(),
            };
            // Parent is the nearest previous node with a lower level.
            while let Some(&top) = stack.last() {
                if nodes[top].level >= level {
                    stack.pop();
                } else {
                    break;
                }
            }
            if let Some(&parent) = stack.last() {
                nodes[parent].children.push(i);
            } else if i != 0 {
                bail!("type tree has more than one root");
            }
            nodes.push(node);
            stack.push(i);
        }
        if nodes.is_empty() {
            bail!("empty type tree");
        }
        Ok(Self { nodes })
    }

    #[must_use]
    pub fn root(&self) -> &TypeNode {
        &self.nodes[0]
    }

    /// Decodes one object's bytes. Fails if the tree doesn't consume exactly `data`.
    pub fn read(&self, data: &[u8], endian: Endian, refs: &[RefType]) -> Result<Value> {
        let mut r = Reader::new(data, endian);
        let v = self.read_node(0, &mut r, Ctx { refs, in_registry: false }, true)?;
        if r.remaining() != 0 {
            bail!(
                "type tree for {} left {} of {} bytes unread",
                self.root().type_name,
                r.remaining(),
                data.len()
            );
        }
        Ok(v)
    }

    /// Decodes only the leading fields of an object, stopping after the named top-level
    /// field. Used to read headers (e.g. `m_GameObject`, `m_Script`) of large objects.
    pub fn read_prefix(
        &self,
        data: &[u8],
        endian: Endian,
        last_field: &str,
        refs: &[RefType],
    ) -> Result<Value> {
        let mut r = Reader::new(data, endian);
        let root = &self.nodes[0];
        let mut fields = Vec::new();
        for &c in &root.children {
            let v = self.read_node(c, &mut r, Ctx { refs, in_registry: false }, true)?;
            let name = self.nodes[c].name.clone();
            let stop = name == last_field;
            fields.push((name, v));
            if stop {
                break;
            }
        }
        Ok(Value::Object(fields))
    }

    /// Reads one node. `own_align` is false for elements of an aligned array, whose
    /// alignment is applied once after the whole array (UnityPy's `read_value_array`).
    fn read_node(
        &self,
        idx: usize,
        r: &mut Reader<'_>,
        ctx: Ctx<'_>,
        own_align: bool,
    ) -> Result<Value> {
        let node = &self.nodes[idx];
        let mut align = own_align && node.meta_flag & ALIGN_FLAG != 0;
        let v = match node.type_name.as_str() {
            "SInt8" => Value::Int(i64::from(r.i8()?)),
            "UInt8" | "char" => Value::UInt(u64::from(r.u8()?)),
            "short" | "SInt16" => Value::Int(i64::from(r.i16()?)),
            "UInt16" | "unsigned short" => Value::UInt(u64::from(r.u16()?)),
            "int" | "SInt32" => Value::Int(i64::from(r.i32()?)),
            "UInt32" | "unsigned int" | "Type*" => Value::UInt(u64::from(r.u32()?)),
            "long long" | "SInt64" => Value::Int(r.i64()?),
            "UInt64" | "unsigned long long" | "FileSize" => Value::UInt(r.u64()?),
            "float" => Value::Float(r.f32()?),
            "double" => Value::Double(r.f64()?),
            "bool" => Value::Bool(r.bool()?),
            "string" => {
                let n = r.len_i32(1)?;
                let b = r.take(n)?;
                r.align(4)?;
                // Binary payloads (e.g. `TextAsset.m_Script` holding a zip) are declared
                // as strings; keep them as bytes rather than corrupting them.
                match std::str::from_utf8(b) {
                    Ok(s) => Value::Str(s.to_owned()),
                    Err(_) => Value::Bytes(b.to_vec()),
                }
            }
            "TypelessData" => {
                let n = r.len_i32(1)?;
                Value::Bytes(r.take(n)?.to_vec())
            }
            "pair" => {
                let [a, b] = node.children[..] else { bail!("pair without two children") };
                Value::Object(vec![
                    ("first".into(), self.read_node(a, r, ctx, true)?),
                    ("second".into(), self.read_node(b, r, ctx, true)?),
                ])
            }
            // A registry nested inside referenced data is not serialized again.
            "ManagedReferencesRegistry" if ctx.in_registry => Value::Object(Vec::new()),
            "ManagedReferencesRegistry" => self.read_registry(idx, r, ctx.refs)?,
            "ReferencedObject" => bail!("ReferencedObject outside a registry"),
            _ => {
                let array = node.children.first().filter(|&&c| self.nodes[c].type_name == "Array");
                if let Some(&arr) = array {
                    let arr_node = &self.nodes[arr];
                    if own_align && arr_node.meta_flag & ALIGN_FLAG != 0 {
                        align = true;
                    }
                    let Some(&elem) = arr_node.children.get(1) else {
                        bail!("array node without element type")
                    };
                    let elem_node = &self.nodes[elem];
                    let elem_aligned = elem_node.meta_flag & ALIGN_FLAG != 0;
                    let size = r.len_i32(0)?;
                    if matches!(elem_node.type_name.as_str(), "UInt8" | "char") {
                        let b = Value::Bytes(r.take(size)?.to_vec());
                        if elem_aligned {
                            r.align(4)?;
                        }
                        b
                    } else {
                        // Guard against corrupt sizes before allocating.
                        if size > r.remaining() {
                            bail!("array of {size} at {} exceeds remaining bytes", r.pos());
                        }
                        let mut items = Vec::with_capacity(size);
                        for _ in 0..size {
                            items.push(self.read_node(elem, r, ctx, !elem_aligned)?);
                        }
                        if elem_aligned {
                            r.align(4)?;
                        }
                        Value::Array(items)
                    }
                } else {
                    let mut fields = Vec::with_capacity(node.children.len());
                    for &c in &node.children {
                        fields.push((self.nodes[c].name.clone(), self.read_node(c, r, ctx, true)?));
                    }
                    Value::Object(fields)
                }
            }
        };
        if align {
            r.align(4)?;
        }
        Ok(v)
    }
}

impl TypeTree {
    /// `[SerializeReference]` storage. Version 1 (Unity 2019.3–2021.1): `version`, then
    /// `{type, data}` records in order (ids are their indices) until the `Terminus`
    /// type. Version 2: `version`, then a `RefIds` vector of `{rid, type, data}`.
    fn read_registry(&self, idx: usize, r: &mut Reader<'_>, refs: &[RefType]) -> Result<Value> {
        let node = &self.nodes[idx];
        let version = r.i32()?;
        let mut entries = Vec::new();
        let read_type = |r: &mut Reader<'_>| -> Result<(String, String, String)> {
            let mut s = || -> Result<String> {
                let n = r.len_i32(1)?;
                let v = String::from_utf8_lossy(r.take(n)?).into_owned();
                r.align(4)?;
                Ok(v)
            };
            Ok((s()?, s()?, s()?))
        };
        let read_data = |r: &mut Reader<'_>, ty: &(String, String, String)| -> Result<Value> {
            if ty.0.is_empty() {
                return Ok(Value::Object(Vec::new()));
            }
            let Some(rt) =
                refs.iter().find(|t| t.class == ty.0 && t.namespace == ty.1 && t.assembly == ty.2)
            else {
                bail!("referenced type {}.{} ({}) not in the file's ref types", ty.1, ty.0, ty.2);
            };
            let tree = rt.tree.as_ref().context("ref type without type tree")?;
            tree.read_node(0, r, Ctx { refs, in_registry: true }, true)
        };
        let type_value = |ty: &(String, String, String)| {
            Value::Object(vec![
                ("class".into(), Value::Str(ty.0.clone())),
                ("ns".into(), Value::Str(ty.1.clone())),
                ("asm".into(), Value::Str(ty.2.clone())),
            ])
        };
        match version {
            1 => {
                for rid in 0i64.. {
                    let ty = read_type(r)?;
                    if (ty.0.as_str(), ty.1.as_str(), ty.2.as_str()) == TERMINUS {
                        break;
                    }
                    let data = read_data(r, &ty)?;
                    entries.push(Value::Object(vec![
                        ("rid".into(), Value::Int(rid)),
                        ("type".into(), type_value(&ty)),
                        ("data".into(), data),
                    ]));
                }
            }
            2 => {
                let n = r.len_i32(12)?;
                for _ in 0..n {
                    let rid = r.i64()?;
                    let ty = read_type(r)?;
                    let data = read_data(r, &ty)?;
                    entries.push(Value::Object(vec![
                        ("rid".into(), Value::Int(rid)),
                        ("type".into(), type_value(&ty)),
                        ("data".into(), data),
                    ]));
                }
            }
            v => bail!("unsupported ManagedReferencesRegistry version {v} in {}", node.name),
        }
        Ok(Value::Object(vec![
            ("version".into(), Value::Int(i64::from(version))),
            ("RefIds".into(), Value::Array(entries)),
        ]))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a blob (format 22) from `(level, type, name, meta)` rows.
    pub(crate) fn blob(rows: &[(u8, &str, &str, i32)]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut recs = Vec::new();
        let put = |s: &str, strings: &mut Vec<u8>| -> u32 {
            let off = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            off
        };
        for &(level, ty, name, meta) in rows {
            let t = put(ty, &mut strings);
            let n = put(name, &mut strings);
            recs.extend_from_slice(&1u16.to_le_bytes());
            recs.push(level);
            recs.push(0);
            recs.extend_from_slice(&t.to_le_bytes());
            recs.extend_from_slice(&n.to_le_bytes());
            recs.extend_from_slice(&(-1i32).to_le_bytes());
            recs.extend_from_slice(&0i32.to_le_bytes());
            recs.extend_from_slice(&meta.to_le_bytes());
            recs.extend_from_slice(&0u64.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(rows.len() as i32).to_le_bytes());
        out.extend_from_slice(&(strings.len() as i32).to_le_bytes());
        out.extend_from_slice(&recs);
        out.extend_from_slice(&strings);
        out
    }

    #[test]
    fn common_strings_resolve_by_offset() {
        assert_eq!(common_string(0), Some("AABB"));
        assert_eq!(common_string(5), Some("AnimationClip"));
        let off = COMMON_STRINGS.find("m_Name\0").unwrap() as u32;
        assert_eq!(common_string(off), Some("m_Name"));
    }

    #[test]
    fn reads_nested_fields_arrays_and_alignment() {
        let rows = [
            (0, "Thing", "Base", 0),
            (1, "bool", "m_Enabled", ALIGN_FLAG),
            (1, "string", "m_Name", 0),
            (2, "Array", "Array", ALIGN_FLAG),
            (3, "int", "size", 0),
            (3, "char", "data", 0),
            (1, "vector", "m_List", 0),
            (2, "Array", "Array", 0),
            (3, "int", "size", 0),
            (3, "PPtr<GameObject>", "data", 0),
            (4, "int", "m_FileID", 0),
            (4, "SInt64", "m_PathID", 0),
            (1, "float", "m_F", 0),
        ];
        let b = blob(&rows);
        let tt = TypeTree::parse_blob(&mut Reader::new(&b, Endian::Little), 22).unwrap();
        assert_eq!(tt.root().children.len(), 4);
        let mut d = Vec::new();
        d.extend_from_slice(&[1, 0, 0, 0]); // bool + align
        d.extend_from_slice(&3i32.to_le_bytes());
        d.extend_from_slice(b"abc\0"); // string + align
        d.extend_from_slice(&1i32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&77i64.to_le_bytes());
        d.extend_from_slice(&1.5f32.to_le_bytes());
        let v = tt.read(&d, Endian::Little, &[]).unwrap();
        assert_eq!(v.get("m_Enabled"), Some(&Value::Bool(true)));
        assert_eq!(v.str_at("m_Name"), Some("abc"));
        let list = v.get("m_List").unwrap().as_array().unwrap();
        assert_eq!(list[0].as_pptr().unwrap().path_id, 77);
        assert_eq!(v.f32_at("m_F"), Some(1.5));
        // One byte too many is an error, not silently ignored.
        d.push(0);
        assert!(tt.read(&d, Endian::Little, &[]).is_err());
    }

    #[test]
    fn aligned_element_arrays_align_once_after_the_array() {
        // vector<UInt8> whose element is flagged aligned: 3 packed bytes, one alignment,
        // then the next int (UnityPy's read_value_array semantics).
        let rows = [
            (0, "Thing", "Base", 0),
            (1, "vector", "m_Bytes", 0),
            (2, "Array", "Array", 0),
            (3, "int", "size", 0),
            (3, "UInt8", "data", ALIGN_FLAG),
            (1, "vector", "m_Shorts", 0),
            (2, "Array", "Array", 0),
            (3, "int", "size", 0),
            (3, "SInt16", "data", ALIGN_FLAG),
            (1, "int", "m_Next", 0),
        ];
        let b = blob(&rows);
        let tt = TypeTree::parse_blob(&mut Reader::new(&b, Endian::Little), 22).unwrap();
        let mut d = Vec::new();
        d.extend_from_slice(&3i32.to_le_bytes());
        d.extend_from_slice(&[10, 20, 30, 0]);
        d.extend_from_slice(&3i32.to_le_bytes());
        for x in [1i16, 2, 3] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&99i32.to_le_bytes());
        let v = tt.read(&d, Endian::Little, &[]).unwrap();
        assert_eq!(v.get("m_Bytes"), Some(&Value::Bytes(vec![10, 20, 30])));
        assert_eq!(v.get("m_Shorts").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(v.i64_at("m_Next"), Some(99));
    }

    fn aligned_str(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as i32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }

    #[test]
    fn reads_version_1_managed_reference_registry() {
        // Host: one int, then the registry. The ReferencedObject template node is
        // ignored; records are read through the file's ref types until Terminus.
        let host = blob(&[
            (0, "MonoBehaviour", "Base", 0),
            (1, "int", "x", 0),
            (1, "ManagedReferencesRegistry", "references", 0),
            (2, "int", "version", 0),
            (2, "ReferencedObject", "00000000", 0),
        ]);
        let host = TypeTree::parse_blob(&mut Reader::new(&host, Endian::Little), 22).unwrap();
        // The referenced type itself declares a registry; inside referenced data it
        // isn't serialized again and must read nothing.
        let ev = blob(&[
            (0, "Ev", "Base", 0),
            (1, "float", "time", 0),
            (1, "ManagedReferencesRegistry", "references", 0),
            (2, "int", "version", 0),
        ]);
        let ev = TypeTree::parse_blob(&mut Reader::new(&ev, Endian::Little), 22).unwrap();
        let refs = [RefType {
            class: "Ev".into(),
            namespace: "K".into(),
            assembly: "A".into(),
            tree: Some(ev),
        }];
        let mut d = Vec::new();
        d.extend_from_slice(&7i32.to_le_bytes());
        d.extend_from_slice(&1i32.to_le_bytes());
        for t in [0.5f32, 2.0] {
            aligned_str(&mut d, "Ev");
            aligned_str(&mut d, "K");
            aligned_str(&mut d, "A");
            d.extend_from_slice(&t.to_le_bytes());
        }
        aligned_str(&mut d, TERMINUS.0);
        aligned_str(&mut d, TERMINUS.1);
        aligned_str(&mut d, TERMINUS.2);
        let v = host.read(&d, Endian::Little, &refs).unwrap();
        let ids = v.path("references.RefIds").unwrap().as_array().unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[1].i64_at("rid"), Some(1));
        assert_eq!(ids[1].f32_at("data.time"), Some(2.0));
        // An unknown referenced type is an error, not skipped.
        assert!(host.read(&d, Endian::Little, &[]).is_err());
    }
}
