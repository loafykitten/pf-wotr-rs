//! Unity serialized files (the `CAB-…` entries inside bundles), format 22 as written by
//! Unity 2020.3: header, type table with type trees, object table, script table and
//! external file references.

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};

use super::reader::{Endian, Reader};
use super::typetree::{RefType, TypeTree};
use super::value::Value;

/// Unity class IDs this importer cares about.
pub mod class_id {
    pub const GAME_OBJECT: i32 = 1;
    pub const TRANSFORM: i32 = 4;
    pub const MATERIAL: i32 = 21;
    pub const MESH_RENDERER: i32 = 23;
    pub const TEXTURE_2D: i32 = 28;
    pub const MESH_FILTER: i32 = 33;
    pub const MESH: i32 = 43;
    pub const SHADER: i32 = 48;
    pub const TEXT_ASSET: i32 = 49;
    pub const BOX_COLLIDER: i32 = 65;
    pub const MESH_COLLIDER: i32 = 64;
    pub const ANIMATION_CLIP: i32 = 74;
    pub const AUDIO_CLIP: i32 = 83;
    pub const CUBEMAP: i32 = 89;
    pub const ANIMATOR_CONTROLLER: i32 = 91;
    pub const ANIMATOR: i32 = 95;
    pub const LIGHT: i32 = 108;
    pub const MONO_BEHAVIOUR: i32 = 114;
    pub const MONO_SCRIPT: i32 = 115;
    pub const SPHERE_COLLIDER: i32 = 135;
    pub const CAPSULE_COLLIDER: i32 = 136;
    pub const SKINNED_MESH_RENDERER: i32 = 137;
    pub const ASSET_BUNDLE: i32 = 142;
    pub const PARTICLE_SYSTEM: i32 = 198;
    pub const PARTICLE_SYSTEM_RENDERER: i32 = 199;
    pub const RECT_TRANSFORM: i32 = 224;
    pub const LIGHT_PROBES: i32 = 258;
    pub const TERRAIN: i32 = 218;
    pub const TERRAIN_DATA: i32 = 156;
    pub const REFLECTION_PROBE: i32 = 215;
}

/// Unity's name for a class ID (the subset we see in these bundles).
#[must_use]
pub fn class_name(id: i32) -> String {
    let s = match id {
        1 => "GameObject",
        4 => "Transform",
        20 => "Camera",
        21 => "Material",
        23 => "MeshRenderer",
        25 => "Renderer",
        28 => "Texture2D",
        33 => "MeshFilter",
        43 => "Mesh",
        48 => "Shader",
        49 => "TextAsset",
        54 => "Rigidbody",
        64 => "MeshCollider",
        65 => "BoxCollider",
        74 => "AnimationClip",
        82 => "AudioSource",
        83 => "AudioClip",
        89 => "Cubemap",
        90 => "Avatar",
        91 => "AnimatorController",
        95 => "Animator",
        96 => "TrailRenderer",
        104 => "RenderSettings",
        108 => "Light",
        111 => "Animation",
        114 => "MonoBehaviour",
        115 => "MonoScript",
        117 => "Texture3D",
        120 => "LineRenderer",
        135 => "SphereCollider",
        136 => "CapsuleCollider",
        137 => "SkinnedMeshRenderer",
        142 => "AssetBundle",
        150 => "PreloadData",
        154 => "TerrainCollider",
        156 => "TerrainData",
        157 => "LightmapSettings",
        187 => "Texture2DArray",
        196 => "NavMeshSettings",
        198 => "ParticleSystem",
        199 => "ParticleSystemRenderer",
        205 => "LODGroup",
        212 => "SpriteRenderer",
        213 => "Sprite",
        215 => "ReflectionProbe",
        218 => "Terrain",
        220 => "LightProbeGroup",
        221 => "AnimatorOverrideController",
        224 => "RectTransform",
        258 => "LightProbes",
        72 => "ComputeShader",
        134 => "PhysicMaterial",
        143 => "CharacterController",
        144 => "CharacterJoint",
        153 => "ConfigurableJoint",
        319 => "AvatarMask",
        1001 => "PrefabInstance",
        73398921 => "VFXRenderer",
        850595691 => "LightingSettings",
        2058629509 => "VisualEffectAsset",
        2083052967 => "VisualEffect",
        1953259897 => "TerrainLayer",
        _ => return format!("Class{id}"),
    };
    s.to_owned()
}

/// An entry of the type table.
#[derive(Debug, Clone)]
pub struct SerializedType {
    pub class_id: i32,
    pub script_type_index: i16,
    pub tree: Option<TypeTree>,
}

/// An entry of the object table.
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub path_id: i64,
    pub offset: usize,
    pub size: usize,
    pub type_index: usize,
    pub class_id: i32,
}

/// A reference to another serialized file (e.g. `archive:/CAB-…/CAB-…`).
#[derive(Debug, Clone)]
pub struct External {
    pub path: String,
}

impl External {
    /// The file name part, e.g. `CAB-1dc8…` (with any `.sharedAssets` suffix).
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// A parsed serialized file borrowing its bytes.
#[derive(Debug)]
pub struct SerializedFile<'a> {
    pub name: String,
    pub format: u32,
    pub unity_version: String,
    pub endian: Endian,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    pub by_path_id: HashMap<i64, usize>,
    pub externals: Vec<External>,
    pub ref_types: Vec<RefType>,
    data: &'a [u8],
}

impl<'a> SerializedFile<'a> {
    pub fn parse(name: &str, data: &'a [u8]) -> Result<Self> {
        Self::parse_inner(name, data).with_context(|| format!("serialized file {name}"))
    }

    fn parse_inner(name: &str, data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data, Endian::Big);
        let _metadata_size = r.u32()?;
        let _file_size = r.u32()?;
        let format = r.u32()?;
        let mut data_offset = u64::from(r.u32()?);
        ensure!((17..=22).contains(&format), "unsupported serialized file format {format}");
        let big_endian = r.bool()?;
        r.take(3)?;
        if format >= 22 {
            let _metadata_size = r.u32()?;
            let _file_size = r.i64()?;
            data_offset = r.u64()?;
            let _unknown = r.i64()?;
        }
        r.endian = if big_endian { Endian::Big } else { Endian::Little };
        let unity_version = r.cstr()?;
        let _platform = r.i32()?;
        let has_trees = r.bool()?;
        let ntypes = r.len_i32(4)?;
        let mut types = Vec::with_capacity(ntypes);
        for _ in 0..ntypes {
            let class_id = r.i32()?;
            let _stripped = r.bool()?;
            let script_type_index = r.i16()?;
            if class_id == class_id::MONO_BEHAVIOUR {
                r.take(16)?; // script id
            }
            r.take(16)?; // old type hash
            let tree = if has_trees { Some(TypeTree::parse_blob(&mut r, format)?) } else { None };
            // Type dependencies are only written alongside type trees.
            if has_trees && format >= 21 {
                let ndeps = r.len_i32(4)?;
                r.take(ndeps * 4)?;
            }
            types.push(SerializedType { class_id, script_type_index, tree });
        }
        let nobjects = r.len_i32(20)?;
        let mut objects = Vec::with_capacity(nobjects);
        let mut by_path_id = HashMap::with_capacity(nobjects);
        let data_offset = usize::try_from(data_offset)?;
        for i in 0..nobjects {
            r.align(4)?;
            let path_id = r.i64()?;
            let start = if format >= 22 { usize::try_from(r.i64()?)? } else { r.u32()? as usize };
            let size = r.u32()? as usize;
            let type_index = usize::try_from(r.i32()?)?;
            let Some(t) = types.get(type_index) else { bail!("object type index out of range") };
            let offset = data_offset + start;
            ensure!(offset + size <= data.len(), "object {path_id} overruns file");
            objects.push(ObjectInfo { path_id, offset, size, type_index, class_id: t.class_id });
            by_path_id.insert(path_id, i);
        }
        let nscripts = r.len_i32(12)?;
        for _ in 0..nscripts {
            let _file_index = r.i32()?;
            r.align(4)?;
            let _id = r.i64()?;
        }
        let nexternals = r.len_i32(21)?;
        let mut externals = Vec::with_capacity(nexternals);
        for _ in 0..nexternals {
            let _temp = r.cstr()?;
            r.take(16)?; // guid
            let _type = r.i32()?;
            externals.push(External { path: r.cstr()? });
        }
        let mut ref_types = Vec::new();
        if format >= 20 {
            let nref = r.len_i32(4)?;
            for _ in 0..nref {
                let _class_id = r.i32()?;
                let _stripped = r.bool()?;
                let script_type_index = r.i16()?;
                if script_type_index >= 0 {
                    r.take(16)?; // script id
                }
                r.take(16)?; // old type hash
                let tree =
                    if has_trees { Some(TypeTree::parse_blob(&mut r, format)?) } else { None };
                let (class, namespace, assembly) = if has_trees && format >= 21 {
                    (r.cstr()?, r.cstr()?, r.cstr()?)
                } else {
                    Default::default()
                };
                ref_types.push(RefType { class, namespace, assembly, tree });
            }
        }
        Ok(Self {
            name: name.to_owned(),
            format,
            unity_version,
            endian: r.endian,
            types,
            objects,
            by_path_id,
            externals,
            ref_types,
            data,
        })
    }

    #[must_use]
    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.by_path_id.get(&path_id).map(|&i| &self.objects[i])
    }

    #[must_use]
    pub fn raw(&self, obj: &ObjectInfo) -> &'a [u8] {
        &self.data[obj.offset..obj.offset + obj.size]
    }

    fn tree(&self, obj: &ObjectInfo) -> Result<&TypeTree> {
        self.types[obj.type_index]
            .tree
            .as_ref()
            .with_context(|| format!("no type tree for object {}", obj.path_id))
    }

    /// Decodes a whole object through its type tree.
    pub fn read(&self, obj: &ObjectInfo) -> Result<Value> {
        self.tree(obj)?.read(self.raw(obj), self.endian, &self.ref_types).with_context(|| {
            format!("{} object {} in {}", class_name(obj.class_id), obj.path_id, self.name)
        })
    }

    /// Decodes an object's leading fields up to and including `last_field`.
    pub fn read_prefix(&self, obj: &ObjectInfo, last_field: &str) -> Result<Value> {
        self.tree(obj)?.read_prefix(self.raw(obj), self.endian, last_field, &self.ref_types)
    }

    /// The type tree's root type name (for MonoBehaviours this is just `MonoBehaviour`).
    #[must_use]
    pub fn type_name(&self, obj: &ObjectInfo) -> String {
        self.types[obj.type_index]
            .tree
            .as_ref()
            .map_or_else(|| class_name(obj.class_id), |t| t.root().type_name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::typetree::tests::blob;
    use super::*;

    /// Builds a format-22 serialized file with one GameObject-like object.
    fn fake_file() -> Vec<u8> {
        let tree = blob(&[(0, "GameObject", "Base", 0), (1, "string", "m_Name", 0)]);
        let mut meta = Vec::new();
        meta.extend_from_slice(b"2020.3.48f1\0");
        meta.extend_from_slice(&2i32.to_le_bytes()); // platform
        meta.push(1); // has type trees
        meta.extend_from_slice(&1i32.to_le_bytes()); // one type
        meta.extend_from_slice(&1i32.to_le_bytes()); // class id 1
        meta.push(0);
        meta.extend_from_slice(&(-1i16).to_le_bytes());
        meta.extend_from_slice(&[0u8; 16]);
        meta.extend_from_slice(&tree);
        meta.extend_from_slice(&0i32.to_le_bytes()); // deps
        meta.extend_from_slice(&1i32.to_le_bytes()); // one object
        let header_len = 48;
        while (header_len + meta.len()) % 4 != 0 {
            meta.push(0);
        }
        meta.extend_from_slice(&5i64.to_le_bytes()); // path id
        meta.extend_from_slice(&0i64.to_le_bytes()); // start
        meta.extend_from_slice(&8u32.to_le_bytes()); // size
        meta.extend_from_slice(&0i32.to_le_bytes()); // type index
        meta.extend_from_slice(&0i32.to_le_bytes()); // scripts
        meta.extend_from_slice(&1i32.to_le_bytes()); // externals
        meta.push(0);
        meta.extend_from_slice(&[0u8; 16]);
        meta.extend_from_slice(&0i32.to_le_bytes());
        meta.extend_from_slice(b"archive:/CAB-x/CAB-x\0");
        meta.extend_from_slice(&0i32.to_le_bytes()); // ref types
        meta.push(0); // user info
        let data_offset = (header_len + meta.len()).next_multiple_of(16);
        let mut f = Vec::new();
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&22u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&[0, 0, 0, 0]); // little endian + reserved
        f.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        f.extend_from_slice(&0i64.to_be_bytes());
        f.extend_from_slice(&(data_offset as u64).to_be_bytes());
        f.extend_from_slice(&0i64.to_be_bytes());
        assert_eq!(f.len(), header_len);
        f.extend_from_slice(&meta);
        f.resize(data_offset, 0);
        f.extend_from_slice(&4i32.to_le_bytes());
        f.extend_from_slice(b"Cave");
        f
    }

    #[test]
    fn parses_objects_and_externals() {
        let bytes = fake_file();
        let sf = SerializedFile::parse("CAB-test", &bytes).unwrap();
        assert_eq!(sf.format, 22);
        assert_eq!(sf.externals[0].file_name(), "CAB-x");
        let obj = sf.object(5).unwrap();
        assert_eq!(obj.class_id, class_id::GAME_OBJECT);
        assert_eq!(sf.read(obj).unwrap().str_at("m_Name"), Some("Cave"));
    }
}
