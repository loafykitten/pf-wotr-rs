//! `System.Collections.Generic.Dictionary<TKey, TValue>` iteration order.
//!
//! The game iterates dictionaries in places where order is observable (for example
//! `ModifiableValue.ModifierList`, a `Dictionary<ModifierDescriptor, List<Modifier>>`, whose
//! order is the order of modifier breakdowns and of `PersistentModifierList`). Mono's
//! `Dictionary` (the reference-source implementation, checked in the shipped mscorlib)
//! stores entries in an array: a new key takes the most recently freed slot if there is
//! one (`freeList`, LIFO), else the next unused slot; `Remove` frees a slot; enumeration
//! walks the array in index order skipping freed slots; `Clear` resets everything; `Resize`
//! copies entries in order. Bucket layout doesn't affect enumeration, so it isn't modelled.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone)]
enum Slot<K, V> {
    Used(K, V),
    /// Freed; the next slot on the free list (-1 for none).
    Free(i32),
}

/// A dictionary that enumerates in Mono's order.
#[derive(Debug, Clone)]
pub struct CsDict<K, V> {
    entries: Vec<Slot<K, V>>,
    index: HashMap<K, usize>,
    free_list: i32,
    free_count: usize,
}

impl<K: Copy + Eq + Hash, V> Default for CsDict<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq + Hash, V> CsDict<K, V> {
    #[must_use]
    pub fn new() -> Self {
        Self { entries: Vec::new(), index: HashMap::new(), free_list: -1, free_count: 0 }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    #[must_use]
    pub fn get(&self, k: &K) -> Option<&V> {
        match self.entries.get(*self.index.get(k)?) {
            Some(Slot::Used(_, v)) => Some(v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let i = *self.index.get(k)?;
        match self.entries.get_mut(i) {
            Some(Slot::Used(_, v)) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn contains_key(&self, k: &K) -> bool {
        self.index.contains_key(k)
    }

    /// `dict[k] = v`: overwrites in place if present, else inserts (`TryInsert`).
    pub fn insert(&mut self, k: K, v: V) {
        if let Some(&i) = self.index.get(&k) {
            self.entries[i] = Slot::Used(k, v);
            return;
        }
        let i = if self.free_count > 0 {
            let i = usize::try_from(self.free_list).expect("free list consistent");
            let Slot::Free(next) = self.entries[i] else { unreachable!("free slot") };
            self.free_list = next;
            self.free_count -= 1;
            self.entries[i] = Slot::Used(k, v);
            i
        } else {
            self.entries.push(Slot::Used(k, v));
            self.entries.len() - 1
        };
        self.index.insert(k, i);
    }

    /// Returns the value for `k`, inserting `make()` first if absent.
    pub fn get_or_insert_with(&mut self, k: K, make: impl FnOnce() -> V) -> &mut V {
        if !self.index.contains_key(&k) {
            self.insert(k, make());
        }
        self.get_mut(&k).expect("just inserted")
    }

    /// `Remove(k)`: frees the slot (it becomes the head of the free list).
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.index.remove(k)?;
        let slot = std::mem::replace(&mut self.entries[i], Slot::Free(self.free_list));
        self.free_list = i32::try_from(i).expect("dictionary under 2^31 entries");
        self.free_count += 1;
        match slot {
            Slot::Used(_, v) => Some(v),
            Slot::Free(_) => unreachable!("indexed slot is used"),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.free_list = -1;
        self.free_count = 0;
    }

    /// Entries in Mono's enumeration order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().filter_map(|s| match s {
            Slot::Used(k, v) => Some((k, v)),
            Slot::Free(_) => None,
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.entries.iter_mut().filter_map(|s| match s {
            Slot::Used(k, v) => Some((&*k, v)),
            Slot::Free(_) => None,
        })
    }

    /// Keys in enumeration order.
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.iter().map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freed_slots_are_reused_lifo() {
        let mut d = CsDict::new();
        for k in [10, 20, 30, 40] {
            d.insert(k, k);
        }
        d.remove(&20);
        d.remove(&40);
        // 40's slot was freed last, so it is reused first; then 20's.
        d.insert(50, 50);
        d.insert(60, 60);
        d.insert(70, 70);
        assert_eq!(d.keys().collect::<Vec<_>>(), vec![10, 60, 30, 50, 70]);
        // Overwrite keeps the position.
        d.insert(30, 31);
        assert_eq!(d.keys().collect::<Vec<_>>(), vec![10, 60, 30, 50, 70]);
        assert_eq!(d.get(&30), Some(&31));
        d.clear();
        d.insert(1, 1);
        assert_eq!(d.keys().collect::<Vec<_>>(), vec![1]);
    }
}
