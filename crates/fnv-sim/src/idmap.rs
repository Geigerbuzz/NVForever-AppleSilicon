//! [`IdMap`]: a map keyed by [`FormId`] that iterates in FormID order like a
//! `BTreeMap` and looks a key up in O(1) through a hash index.
//!
//! [design] The placed references of a load order (hundreds of thousands) are looked
//! up by FormID thousands of times a frame (scripts, the compass, ownership and crime
//! checks); a `BTreeMap` walk took a cache miss per level. The entries live in one
//! vector sorted by FormID, so iteration and ranges walk contiguous memory in the same
//! order a `BTreeMap` gives, and a hash map from FormID to the entry's position answers
//! `get`. Appending a FormID above every key (how [`crate::refs::RefTable`] is built)
//! is O(1); inserting or removing in the middle is O(n), as the positions after it
//! move. The table is built once and only read afterwards outside tests.

use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::{Bound, Index, RangeBounds};

use crate::form::FormId;

/// A fast hasher for [`FormId`] keys (one 32-bit word): a folded 64x64->128 multiply,
/// which mixes every input bit into the low bits (the bucket) and the high bits (the
/// control byte), so FormIDs that differ only in their load-order byte spread too.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

impl Hasher for IdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(b)).wrapping_mul(SEED);
        }
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.0 = (self.0.rotate_left(32) ^ u64::from(n)).wrapping_mul(SEED);
    }

    #[inline]
    fn finish(&self) -> u64 {
        let p = u128::from(self.0 ^ SEED.rotate_left(17)) * u128::from(SEED);
        (p as u64) ^ ((p >> 64) as u64)
    }
}

pub type IdBuildHasher = BuildHasherDefault<IdHasher>;

/// A map from [`FormId`] to `V` with `BTreeMap`'s API and iteration order (ascending
/// FormID) and O(1) lookups (module docs).
pub struct IdMap<V> {
    /// Sorted by FormID, no duplicate keys.
    entries: Vec<(FormId, V)>,
    /// FormID -> its position in `entries`.
    index: HashMap<FormId, u32, IdBuildHasher>,
}

pub type Iter<'a, V> = std::iter::Map<std::slice::Iter<'a, (FormId, V)>, fn(&'a (FormId, V)) -> (&'a FormId, &'a V)>;
pub type IterMut<'a, V> =
    std::iter::Map<std::slice::IterMut<'a, (FormId, V)>, fn(&'a mut (FormId, V)) -> (&'a FormId, &'a mut V)>;
pub type Keys<'a, V> = std::iter::Map<std::slice::Iter<'a, (FormId, V)>, fn(&'a (FormId, V)) -> &'a FormId>;
pub type Values<'a, V> = std::iter::Map<std::slice::Iter<'a, (FormId, V)>, fn(&'a (FormId, V)) -> &'a V>;
pub type ValuesMut<'a, V> = std::iter::Map<std::slice::IterMut<'a, (FormId, V)>, fn(&'a mut (FormId, V)) -> &'a mut V>;
pub type IntoIter<V> = std::vec::IntoIter<(FormId, V)>;

fn pair<V>(e: &(FormId, V)) -> (&FormId, &V) {
    (&e.0, &e.1)
}

fn pair_mut<V>(e: &mut (FormId, V)) -> (&FormId, &mut V) {
    (&e.0, &mut e.1)
}

fn key<V>(e: &(FormId, V)) -> &FormId {
    &e.0
}

fn value<V>(e: &(FormId, V)) -> &V {
    &e.1
}

fn value_mut<V>(e: &mut (FormId, V)) -> &mut V {
    &mut e.1
}

impl<V> Default for IdMap<V> {
    fn default() -> Self {
        IdMap { entries: Vec::new(), index: HashMap::default() }
    }
}

impl<V: Clone> Clone for IdMap<V> {
    fn clone(&self) -> Self {
        IdMap { entries: self.entries.clone(), index: self.index.clone() }
    }
}

impl<V: fmt::Debug> fmt::Debug for IdMap<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<V: PartialEq> PartialEq for IdMap<V> {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl<V> IdMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }

    #[inline]
    fn position(&self, id: &FormId) -> Option<usize> {
        self.index.get(id).map(|&i| i as usize)
    }

    #[inline]
    pub fn contains_key(&self, id: &FormId) -> bool {
        self.index.contains_key(id)
    }

    #[inline]
    pub fn get(&self, id: &FormId) -> Option<&V> {
        self.position(id).map(|i| &self.entries[i].1)
    }

    #[inline]
    pub fn get_mut(&mut self, id: &FormId) -> Option<&mut V> {
        self.position(id).map(|i| &mut self.entries[i].1)
    }

    #[inline]
    pub fn get_key_value(&self, id: &FormId) -> Option<(&FormId, &V)> {
        self.position(id).map(|i| pair(&self.entries[i]))
    }

    pub fn first_key_value(&self) -> Option<(&FormId, &V)> {
        self.entries.first().map(pair)
    }

    pub fn last_key_value(&self) -> Option<(&FormId, &V)> {
        self.entries.last().map(pair)
    }

    /// Positions from `from` on moved: point the index at them again.
    fn reindex_from(&mut self, from: usize) {
        for (i, (k, _)) in self.entries.iter().enumerate().skip(from) {
            self.index.insert(*k, i as u32);
        }
    }

    fn position_u32(i: usize) -> u32 {
        u32::try_from(i).expect("IdMap holds at most u32::MAX entries")
    }

    /// As `BTreeMap::insert`: the old value when `id` was present (its key is kept).
    pub fn insert(&mut self, id: FormId, v: V) -> Option<V> {
        if let Some(i) = self.position(&id) {
            return Some(std::mem::replace(&mut self.entries[i].1, v));
        }
        let at = match self.entries.last() {
            Some((last, _)) if *last < id => self.entries.len(),
            None => 0,
            _ => self.entries.partition_point(|(k, _)| *k < id),
        };
        let pos = Self::position_u32(at);
        self.entries.insert(at, (id, v));
        self.index.insert(id, pos);
        if at + 1 < self.entries.len() {
            self.reindex_from(at + 1);
        }
        None
    }

    pub fn remove(&mut self, id: &FormId) -> Option<V> {
        self.remove_entry(id).map(|(_, v)| v)
    }

    pub fn remove_entry(&mut self, id: &FormId) -> Option<(FormId, V)> {
        let i = self.index.remove(id)? as usize;
        let e = self.entries.remove(i);
        self.reindex_from(i);
        Some(e)
    }

    /// As `BTreeMap::retain`: visits every entry in FormID order.
    pub fn retain(&mut self, mut f: impl FnMut(&FormId, &mut V) -> bool) {
        let before = self.entries.len();
        self.entries.retain_mut(|(k, v)| f(k, v));
        if self.entries.len() != before {
            self.rebuild_index();
        }
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        self.index.reserve(self.entries.len());
        for (i, (k, _)) in self.entries.iter().enumerate() {
            self.index.insert(*k, Self::position_u32(i));
        }
    }

    pub fn iter(&self) -> Iter<'_, V> {
        self.entries.iter().map(pair as fn(&(FormId, V)) -> (&FormId, &V))
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, V> {
        self.entries.iter_mut().map(pair_mut as fn(&mut (FormId, V)) -> (&FormId, &mut V))
    }

    pub fn keys(&self) -> Keys<'_, V> {
        self.entries.iter().map(key as fn(&(FormId, V)) -> &FormId)
    }

    pub fn values(&self) -> Values<'_, V> {
        self.entries.iter().map(value as fn(&(FormId, V)) -> &V)
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, V> {
        self.entries.iter_mut().map(value_mut as fn(&mut (FormId, V)) -> &mut V)
    }

    pub fn into_keys(self) -> impl DoubleEndedIterator<Item = FormId> + ExactSizeIterator {
        self.entries.into_iter().map(|(k, _)| k)
    }

    pub fn into_values(self) -> impl DoubleEndedIterator<Item = V> + ExactSizeIterator {
        self.entries.into_iter().map(|(_, v)| v)
    }

    /// The positions of `range` in `entries`; panics where `BTreeMap::range` does.
    fn bounds(&self, range: &impl RangeBounds<FormId>) -> (usize, usize) {
        match (range.start_bound(), range.end_bound()) {
            (Bound::Excluded(s), Bound::Excluded(e)) if s == e => {
                panic!("range start and end are equal and excluded in BTreeMap")
            }
            (Bound::Included(s) | Bound::Excluded(s), Bound::Included(e) | Bound::Excluded(e)) if s > e => {
                panic!("range start is greater than range end in BTreeMap")
            }
            _ => {}
        }
        let lo = match range.start_bound() {
            Bound::Included(s) => self.entries.partition_point(|(k, _)| k < s),
            Bound::Excluded(s) => self.entries.partition_point(|(k, _)| k <= s),
            Bound::Unbounded => 0,
        };
        let hi = match range.end_bound() {
            Bound::Included(e) => self.entries.partition_point(|(k, _)| k <= e),
            Bound::Excluded(e) => self.entries.partition_point(|(k, _)| k < e),
            Bound::Unbounded => self.entries.len(),
        };
        (lo, hi.max(lo))
    }

    /// As `BTreeMap::range`, in FormID order.
    pub fn range(&self, range: impl RangeBounds<FormId>) -> Iter<'_, V> {
        let (lo, hi) = self.bounds(&range);
        self.entries[lo..hi].iter().map(pair as fn(&(FormId, V)) -> (&FormId, &V))
    }

    pub fn range_mut(&mut self, range: impl RangeBounds<FormId>) -> IterMut<'_, V> {
        let (lo, hi) = self.bounds(&range);
        self.entries[lo..hi].iter_mut().map(pair_mut as fn(&mut (FormId, V)) -> (&FormId, &mut V))
    }
}

impl<V> Extend<(FormId, V)> for IdMap<V> {
    /// As `BTreeMap::extend`: a later value for a key replaces an earlier one. Entries
    /// in ascending order above every key are appended; otherwise the whole map is
    /// sorted once (O(n log n), not O(n) per entry).
    fn extend<I: IntoIterator<Item = (FormId, V)>>(&mut self, iter: I) {
        let start = self.entries.len();
        let mut sorted = true;
        for (k, v) in iter {
            match self.entries.last() {
                Some((last, _)) if *last >= k => sorted = false,
                _ => {}
            }
            self.entries.push((k, v));
        }
        if sorted {
            self.index.reserve(self.entries.len() - start);
            for i in start..self.entries.len() {
                self.index.insert(self.entries[i].0, Self::position_u32(i));
            }
            return;
        }
        // Stable: equal keys stay in insertion order, so the last one is the newest.
        self.entries.sort_by_key(|(k, _)| *k);
        let mut out: Vec<(FormId, V)> = Vec::with_capacity(self.entries.len());
        for e in self.entries.drain(..) {
            match out.last_mut() {
                Some(last) if last.0 == e.0 => *last = e,
                _ => out.push(e),
            }
        }
        self.entries = out;
        self.rebuild_index();
    }
}

impl<'a, V: Copy> Extend<(&'a FormId, &'a V)> for IdMap<V> {
    fn extend<I: IntoIterator<Item = (&'a FormId, &'a V)>>(&mut self, iter: I) {
        self.extend(iter.into_iter().map(|(&k, &v)| (k, v)));
    }
}

impl<V> FromIterator<(FormId, V)> for IdMap<V> {
    fn from_iter<I: IntoIterator<Item = (FormId, V)>>(iter: I) -> Self {
        let mut m = IdMap::new();
        m.extend(iter);
        m
    }
}

impl<V, const N: usize> From<[(FormId, V); N]> for IdMap<V> {
    fn from(a: [(FormId, V); N]) -> Self {
        a.into_iter().collect()
    }
}

impl<V> IntoIterator for IdMap<V> {
    type Item = (FormId, V);
    type IntoIter = IntoIter<V>;
    fn into_iter(self) -> IntoIter<V> {
        self.entries.into_iter()
    }
}

impl<'a, V> IntoIterator for &'a IdMap<V> {
    type Item = (&'a FormId, &'a V);
    type IntoIter = Iter<'a, V>;
    fn into_iter(self) -> Iter<'a, V> {
        self.iter()
    }
}

impl<'a, V> IntoIterator for &'a mut IdMap<V> {
    type Item = (&'a FormId, &'a mut V);
    type IntoIter = IterMut<'a, V>;
    fn into_iter(self) -> IterMut<'a, V> {
        self.iter_mut()
    }
}

impl<V> Index<&FormId> for IdMap<V> {
    type Output = V;
    /// Panics when `id` is absent, as `BTreeMap`'s index does.
    fn index(&self, id: &FormId) -> &V {
        self.get(id).expect("no entry found for key")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every operation agrees with a `BTreeMap` given the same sequence.
    #[test]
    fn matches_btreemap() {
        let mut state = 0x1234_5678_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut m: IdMap<u64> = IdMap::new();
        let mut b: BTreeMap<FormId, u64> = BTreeMap::new();
        for step in 0..20_000 {
            let r = next();
            // Small key space so inserts, replacements and removes collide.
            let k = FormId(((r >> 8) % 600) as u32 | (((r >> 40) % 3) as u32) << 24);
            match r % 7 {
                0..=2 => assert_eq!(m.insert(k, step), b.insert(k, step)),
                3 => assert_eq!(m.remove(&k), b.remove(&k)),
                4 => {
                    if let (Some(x), Some(y)) = (m.get_mut(&k), b.get_mut(&k)) {
                        *x += 1;
                        *y += 1;
                    }
                }
                5 => {
                    let lo = k;
                    let hi = FormId(lo.0.saturating_add(((r >> 50) % 400) as u32));
                    assert!(m.range(lo..hi).eq(b.range(lo..hi)));
                    assert!(m.range(lo..=hi).rev().eq(b.range(lo..=hi).rev()));
                    assert!(m.range(..hi).eq(b.range(..hi)));
                    assert!(m.range((Bound::Excluded(lo), Bound::Unbounded)).eq(b.range((Bound::Excluded(lo), Bound::Unbounded))));
                }
                _ => {
                    if step.is_multiple_of(997) {
                        m.retain(|k, v| !(k.0 ^ *v as u32).is_multiple_of(5));
                        b.retain(|k, v| !(k.0 ^ *v as u32).is_multiple_of(5));
                    }
                    let add: Vec<(FormId, u64)> = (0..(r % 6)).map(|i| (FormId(((next() >> 8) % 700) as u32), i)).collect();
                    m.extend(add.clone());
                    b.extend(add);
                }
            }
            assert_eq!(m.get(&k), b.get(&k));
            assert_eq!(m.contains_key(&k), b.contains_key(&k));
            assert_eq!(m.len(), b.len());
        }
        assert!(m.iter().eq(b.iter()));
        assert!(m.keys().eq(b.keys()));
        assert!(m.values().eq(b.values()));
        assert_eq!(m.first_key_value(), b.first_key_value());
        assert_eq!(m.last_key_value(), b.last_key_value());
        for (k, v) in &b {
            assert_eq!(m[k], *v);
        }
        let collected: IdMap<u64> = b.clone().into_iter().rev().collect();
        assert!(collected.iter().eq(b.iter()));
        assert!(m.into_iter().eq(b));
    }

    #[test]
    fn extend_keeps_the_last_duplicate() {
        let mut m: IdMap<u32> = IdMap::new();
        m.extend([(FormId(5), 1), (FormId(2), 2), (FormId(5), 3), (FormId(2), 4), (FormId(9), 5)]);
        assert_eq!(m.iter().map(|(k, v)| (k.0, *v)).collect::<Vec<_>>(), [(2, 4), (5, 3), (9, 5)]);
        assert_eq!(m.get(&FormId(5)), Some(&3));
    }

    #[test]
    #[should_panic(expected = "range start is greater than range end")]
    fn backwards_range_panics_like_btreemap() {
        let m: IdMap<u32> = IdMap::new();
        let _ = m.range(FormId(5)..FormId(2));
    }
}
