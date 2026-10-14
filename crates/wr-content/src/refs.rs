//! Blueprint reference resolution: GUID → blueprint, with typed references checked
//! against the target's type.
//!
//! The game resolves a `BlueprintReference<T>` lazily (`ResourcesLibrary.TryGetBlueprint`)
//! and casts with `as T`, so a missing GUID and a GUID of the wrong type both read as
//! null at run time. We report the two separately.

use std::collections::HashMap;

use crate::decode::RefUse;
use crate::guid::Guid;
use crate::schema::{self, TypeIdx};

/// Where each blueprint is and what it is.
#[derive(Debug, Default)]
pub struct Index {
    pub by_guid: HashMap<Guid, Entry>,
    /// GUIDs that more than one file claims (`AssetId` collisions).
    pub duplicates: Vec<Guid>,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Position among the zip's `.jbp` entries.
    pub pos: u32,
    /// Concrete type of `Data`, if it decoded through the schema.
    pub ty: Option<TypeIdx>,
}

impl Index {
    pub fn insert(&mut self, guid: Guid, e: Entry) {
        if self.by_guid.insert(guid, e).is_some() {
            self.duplicates.push(guid);
        }
    }

    #[must_use]
    pub fn get(&self, g: Guid) -> Option<&Entry> {
        self.by_guid.get(&g)
    }
}

/// Outcome for one reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Resolved,
    /// No blueprint with that GUID.
    Dangling,
    /// The blueprint exists but isn't a `T`.
    WrongType {
        actual: TypeIdx,
    },
    /// The target's own type is unknown (its `Data` didn't decode).
    TargetUntyped,
}

#[must_use]
pub fn resolve(index: &Index, r: RefUse) -> Resolution {
    match index.get(r.guid) {
        None => Resolution::Dangling,
        Some(Entry { ty: None, .. }) => Resolution::TargetUntyped,
        Some(Entry { ty: Some(t), .. }) if schema::is_assignable(*t, r.target) => {
            Resolution::Resolved
        }
        Some(Entry { ty: Some(t), .. }) => Resolution::WrongType { actual: *t },
    }
}

/// Totals over all references, per target type.
#[derive(Debug, Default)]
pub struct RefTally {
    pub total: u64,
    pub resolved: u64,
    pub dangling: u64,
    pub wrong_type: u64,
    pub target_untyped: u64,
    /// Target type → (resolved, dangling, wrong type).
    pub per_target: HashMap<TypeIdx, [u64; 3]>,
    /// (expected, actual) → count.
    pub wrong_pairs: HashMap<(TypeIdx, TypeIdx), u64>,
    /// A few examples of each failure: (source blueprint, referenced GUID).
    pub dangling_examples: Vec<(Guid, Guid)>,
    pub wrong_examples: Vec<(Guid, Guid)>,
}

impl RefTally {
    pub fn add(&mut self, index: &Index, source: Guid, r: RefUse) {
        self.total += 1;
        let e = self.per_target.entry(r.target).or_default();
        match resolve(index, r) {
            Resolution::Resolved => {
                self.resolved += 1;
                e[0] += 1;
            }
            Resolution::Dangling => {
                self.dangling += 1;
                e[1] += 1;
                if self.dangling_examples.len() < 20 {
                    self.dangling_examples.push((source, r.guid));
                }
            }
            Resolution::WrongType { actual } => {
                self.wrong_type += 1;
                e[2] += 1;
                *self.wrong_pairs.entry((r.target, actual)).or_default() += 1;
                if self.wrong_examples.len() < 20 {
                    self.wrong_examples.push((source, r.guid));
                }
            }
            Resolution::TargetUntyped => self.target_untyped += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_existence_and_type() {
        let feature = schema::find("Kingmaker.Blueprints.Classes.BlueprintFeature").unwrap();
        let fact = schema::find("Kingmaker.Blueprints.Facts.BlueprintUnitFact").unwrap();
        let buff = schema::find("Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff").unwrap();
        let mut idx = Index::default();
        let a = Guid(1);
        let b = Guid(2);
        idx.insert(a, Entry { pos: 0, ty: Some(feature) });
        idx.insert(b, Entry { pos: 1, ty: Some(buff) });
        // A feature is a unit fact.
        assert_eq!(resolve(&idx, RefUse { guid: a, target: fact }), Resolution::Resolved);
        assert_eq!(
            resolve(&idx, RefUse { guid: b, target: feature }),
            Resolution::WrongType { actual: buff }
        );
        assert_eq!(resolve(&idx, RefUse { guid: Guid(3), target: fact }), Resolution::Dangling);
    }
}
