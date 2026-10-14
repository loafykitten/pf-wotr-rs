//! `System.Collections.Generic.HashSet<T>` iteration order (the reference source's, which the
//! game's System.Core ships: its `m_lastIndex` field, measured in WrathOracle's `imp_s` slots).
//!
//! Like [`super::dict::CsDict`], entries live in an array: a new one takes the most recently
//! freed slot (`m_freeList`, LIFO), else the next unused one (`m_lastIndex`); enumeration walks
//! the array in index order skipping freed slots. Unlike `Dictionary`, a `Remove` that leaves the
//! set empty resets it (`m_lastIndex = 0`, `m_freeList = -1`), and `Clear` resets it too.
//! `null` is a value like any other (`Option<K>`).

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone)]
enum Slot<K> {
    Used(Option<K>),
    /// Freed; the next slot on the free list (-1 for none).
    Free(i32),
}

/// A hash set that enumerates in the reference source's order.
#[derive(Debug, Clone)]
pub struct CsHashSet<K> {
    slots: Vec<Slot<K>>,
    index: HashMap<Option<K>, usize>,
    free_list: i32,
}

impl<K: Copy + Eq + Hash> Default for CsHashSet<K> {
    fn default() -> Self {
        Self { slots: Vec::new(), index: HashMap::new(), free_list: -1 }
    }
}

impl<K: Copy + Eq + Hash> CsHashSet<K> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// `m_lastIndex`: the slots ever used since the set was last empty (freed ones included).
    #[must_use]
    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn contains(&self, k: Option<K>) -> bool {
        self.index.contains_key(&k)
    }

    /// `Add(k)` (`AddIfNotPresent`): whether it was added.
    pub fn add(&mut self, k: Option<K>) -> bool {
        if self.index.contains_key(&k) {
            return false;
        }
        let i = if self.free_list >= 0 {
            let i = usize::try_from(self.free_list).expect("free list consistent");
            let Slot::Free(next) = self.slots[i] else { unreachable!("free slot") };
            self.free_list = next;
            self.slots[i] = Slot::Used(k);
            i
        } else {
            self.slots.push(Slot::Used(k));
            self.slots.len() - 1
        };
        self.index.insert(k, i);
        true
    }

    /// `Remove(k)`: whether it was there. Emptied, the set starts over.
    pub fn remove(&mut self, k: Option<K>) -> bool {
        let Some(i) = self.index.remove(&k) else { return false };
        if self.index.is_empty() {
            self.clear();
        } else {
            self.slots[i] = Slot::Free(self.free_list);
            self.free_list = i32::try_from(i).expect("set under 2^31 entries");
        }
        true
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.index.clear();
        self.free_list = -1;
    }

    /// The values in enumeration order.
    pub fn iter(&self) -> impl Iterator<Item = Option<K>> + '_ {
        self.slots.iter().filter_map(|s| match s {
            Slot::Used(k) => Some(*k),
            Slot::Free(_) => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CsHashSet;

    #[test]
    fn a_freed_slot_is_reused_first_and_an_emptied_set_starts_over() {
        let mut s = CsHashSet::default();
        assert!(s.add(Some(1)) && s.add(Some(2)) && s.add(Some(3)));
        assert!(!s.add(Some(2)));
        assert!(s.remove(Some(1)));
        assert!(s.add(None));
        assert_eq!(s.iter().collect::<Vec<_>>(), [None, Some(2), Some(3)]);
        assert_eq!(s.slots(), 3);
        for k in [None, Some(2), Some(3)] {
            assert!(s.remove(k));
        }
        assert_eq!(s.slots(), 0);
        assert!(s.add(Some(9)));
        assert_eq!(s.iter().collect::<Vec<_>>(), [Some(9)]);
    }
}
