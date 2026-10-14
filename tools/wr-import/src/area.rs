//! Opening bundles on demand and resolving object references across their serialized
//! files.
//!
//! [`Universe`] decompresses a bundle the first time one of its files is needed and keeps
//! it for the rest of the run. Decompressed data is leaked on purpose so that parsed
//! files can borrow it for `'static`: this is a short-lived offline tool, and the
//! memory is bounded by the bundles a run actually touches.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::catalog::CabIndex;
use crate::unity::bundle::Bundle;
use crate::unity::serialized::{ObjectInfo, SerializedFile};
use crate::unity::value::{PPtr, Value};

/// Whether a bundle entry is raw data rather than a serialized file: resource streams
/// (`.resS`, `.resource`: texture pixels, mesh buffers, audio) and other payloads such
/// as Enlighten GI data (`GI/…/*.pos`). Unity flags serialized files with bit 2 of the
/// directory entry's flags.
#[must_use]
pub fn is_stream(node: &crate::unity::bundle::Node) -> bool {
    node.flags & 4 == 0
}

/// A resolved object: which file and bundle it is in, and its table entry.
#[derive(Debug, Clone, Copy)]
pub struct ObjRef {
    pub file: &'static SerializedFile<'static>,
    pub bundle: &'static str,
    pub info: &'static ObjectInfo,
}

impl ObjRef {
    pub fn read(&self) -> Result<Value> {
        self.file.read(self.info)
    }

    #[must_use]
    pub fn path_id(&self) -> i64 {
        self.info.path_id
    }
}

/// Where a [`PPtr`] points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Null,
    /// Found: serialized file name, bundle and path id.
    Found {
        file: String,
        bundle: String,
        path_id: i64,
    },
    /// The file exists (bundle known) but has no object with that path id.
    MissingObject {
        file: String,
        bundle: String,
        path_id: i64,
    },
    /// No bundle in the install contains the file.
    MissingFile {
        file: String,
        path_id: i64,
    },
    /// `m_FileID` outside the external table: corrupt.
    BadFileId(i32),
}

struct State {
    /// Serialized file name → parsed file.
    files: HashMap<String, &'static SerializedFile<'static>>,
    /// Bundle name → resource streams it holds (name → bytes).
    streams: HashMap<String, &'static [u8]>,
    /// Bundles opened so far.
    opened: BTreeSet<String>,
    /// File name → bundle name, for files we've opened.
    file_bundle: HashMap<String, &'static str>,
}

/// Every bundle in the install, opened lazily.
pub struct Universe {
    pub bundles_dir: PathBuf,
    pub cabs: CabIndex,
    state: RefCell<State>,
}

impl std::fmt::Debug for Universe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Universe").field("bundles_dir", &self.bundles_dir).finish_non_exhaustive()
    }
}

impl Universe {
    #[must_use]
    pub fn new(bundles_dir: &Path, cabs: CabIndex) -> Self {
        Self {
            bundles_dir: bundles_dir.to_owned(),
            cabs,
            state: RefCell::new(State {
                files: HashMap::new(),
                streams: HashMap::new(),
                opened: BTreeSet::new(),
                file_bundle: HashMap::new(),
            }),
        }
    }

    /// Registers a loose serialized file shipped with the player (e.g. `unity default
    /// resources`, `unity_builtin_extra`) under its file name. Missing files are skipped.
    pub fn add_loose_file(&self, path: &Path) -> Result<()> {
        let Ok(bytes) = std::fs::read(path) else { return Ok(()) };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let f: &'static SerializedFile<'static> =
            Box::leak(Box::new(SerializedFile::parse(&name, data)?));
        let label: &'static str = Box::leak(format!("(player) {name}").into_boxed_str());
        let mut st = self.state.borrow_mut();
        st.files.insert(name.clone(), f);
        st.file_bundle.insert(name, label);
        Ok(())
    }

    /// Like [`Universe::add_loose_file`] for player files written without type trees
    /// (`globalgamemanagers.assets`, `level*`): types missing a tree borrow one for the
    /// same built-in class from `donors` (files of the same Unity version that have
    /// trees). Objects whose class no donor has stay unreadable.
    pub fn add_loose_file_with_donors(
        &self,
        path: &Path,
        donors: &[&SerializedFile<'_>],
    ) -> Result<()> {
        let Ok(bytes) = std::fs::read(path) else { return Ok(()) };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let mut f = SerializedFile::parse(&name, data)?;
        for t in &mut f.types {
            if t.tree.is_none() {
                t.tree = donors
                    .iter()
                    .flat_map(|d| d.types.iter())
                    .find(|d| {
                        d.class_id == t.class_id
                            && d.class_id != crate::unity::serialized::class_id::MONO_BEHAVIOUR
                    })
                    .and_then(|d| d.tree.clone());
            }
        }
        let f: &'static SerializedFile<'static> = Box::leak(Box::new(f));
        let label: &'static str = Box::leak(format!("(player) {name}").into_boxed_str());
        let mut st = self.state.borrow_mut();
        st.files.insert(name.clone(), f);
        st.file_bundle.insert(name, label);
        Ok(())
    }

    /// Opens a bundle (once) and parses its serialized files.
    pub fn open_bundle(&self, name: &str) -> Result<()> {
        if self.state.borrow().opened.contains(name) {
            return Ok(());
        }
        let bundle: &'static Bundle = Box::leak(Box::new(
            Bundle::open(&self.bundles_dir.join(name)).with_context(|| format!("open {name}"))?,
        ));
        let bname: &'static str = Box::leak(name.to_owned().into_boxed_str());
        let mut st = self.state.borrow_mut();
        st.opened.insert(name.to_owned());
        for node in &bundle.header.nodes {
            let data = bundle.node_data(node)?;
            if is_stream(node) {
                st.streams.insert(node.path.clone(), data);
                continue;
            }
            let f: &'static SerializedFile<'static> =
                Box::leak(Box::new(SerializedFile::parse(&node.path, data)?));
            st.files.insert(node.path.clone(), f);
            st.file_bundle.insert(node.path.clone(), bname);
        }
        Ok(())
    }

    #[must_use]
    pub fn opened_bundles(&self) -> Vec<String> {
        self.state.borrow().opened.iter().cloned().collect()
    }

    /// All serialized files of a bundle (opening it if needed).
    pub fn files_of(&self, bundle: &str) -> Result<Vec<&'static SerializedFile<'static>>> {
        self.open_bundle(bundle)?;
        let st = self.state.borrow();
        let mut v: Vec<_> = st
            .files
            .iter()
            .filter(|(n, _)| st.file_bundle.get(*n).is_some_and(|b| *b == bundle))
            .map(|(_, f)| *f)
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    /// A serialized file by name, opening its bundle via the CAB index if needed.
    pub fn file(&self, name: &str) -> Result<Option<&'static SerializedFile<'static>>> {
        if let Some(f) = self.state.borrow().files.get(name) {
            return Ok(Some(*f));
        }
        let Some(bundle) = self.cabs.bundle_of(name).map(str::to_owned) else { return Ok(None) };
        self.open_bundle(&bundle)?;
        Ok(self.state.borrow().files.get(name).copied())
    }

    /// The bundle holding a (loaded) file.
    #[must_use]
    pub fn bundle_of_file(&self, name: &str) -> Option<&'static str> {
        self.state.borrow().file_bundle.get(name).copied()
    }

    /// A resource stream (`CAB-….resS`) by name, opening its bundle if needed.
    pub fn stream(&self, name: &str) -> Result<Option<&'static [u8]>> {
        if let Some(s) = self.state.borrow().streams.get(name) {
            return Ok(Some(*s));
        }
        let Some(bundle) = self.cabs.bundle_of(name).map(str::to_owned) else { return Ok(None) };
        self.open_bundle(&bundle)?;
        Ok(self.state.borrow().streams.get(name).copied())
    }

    pub fn get(&self, file: &str, path_id: i64) -> Result<Option<ObjRef>> {
        let Some(f) = self.file(file)? else { return Ok(None) };
        let bundle = self.bundle_of_file(file).ok_or_else(|| anyhow!("no bundle for {file}"))?;
        Ok(f.object(path_id).map(|info| ObjRef { file: f, bundle, info }))
    }

    /// Resolves a pointer found in `from`, opening the target bundle if needed.
    pub fn target(&self, from: &SerializedFile<'_>, p: PPtr) -> Result<Target> {
        if p.is_null() {
            return Ok(Target::Null);
        }
        let file = if p.file_id == 0 {
            from.name.clone()
        } else {
            let ext = usize::try_from(p.file_id - 1).ok().and_then(|i| from.externals.get(i));
            let Some(ext) = ext else { return Ok(Target::BadFileId(p.file_id)) };
            ext.file_name().to_owned()
        };
        let Some(f) = self.file(&file)? else {
            return Ok(Target::MissingFile { file, path_id: p.path_id });
        };
        let bundle = self.bundle_of_file(&file).unwrap_or_default().to_owned();
        Ok(if f.object(p.path_id).is_some() {
            Target::Found { file, bundle, path_id: p.path_id }
        } else {
            Target::MissingObject { file, bundle, path_id: p.path_id }
        })
    }

    pub fn resolve(&self, from: &SerializedFile<'_>, p: PPtr) -> Result<Option<ObjRef>> {
        match self.target(from, p)? {
            Target::Found { file, path_id, .. } => self.get(&file, path_id),
            _ => Ok(None),
        }
    }

    /// `Namespace.ClassName` of a MonoBehaviour's script, plus its assembly.
    pub fn script_of(&self, file: &SerializedFile<'_>, mb: &Value) -> Result<Option<ScriptName>> {
        let Some(p) = mb.get("m_Script").and_then(Value::as_pptr) else { return Ok(None) };
        let Some(s) = self.resolve(file, p)? else { return Ok(None) };
        let v = s.read()?;
        Ok(Some(ScriptName {
            namespace: v.str_at("m_Namespace").unwrap_or_default().to_owned(),
            class: v.str_at("m_ClassName").unwrap_or_default().to_owned(),
            assembly: v.str_at("m_AssemblyName").unwrap_or_default().to_owned(),
        }))
    }
}

/// A MonoScript's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub struct ScriptName {
    pub namespace: String,
    pub class: String,
    pub assembly: String,
}

impl ScriptName {
    #[must_use]
    pub fn full(&self) -> String {
        if self.namespace.is_empty() {
            self.class.clone()
        } else {
            format!("{}.{}", self.namespace, self.class)
        }
    }
}
