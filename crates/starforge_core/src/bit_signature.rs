use core::fmt;
use core::hash::{Hash, Hasher};

/// A growable bitset used to identify a set of component types (e.g. an archetype signature).
#[derive(Clone, Eq)]
pub struct BitSignature(Vec<u64>);

/// Number of bits stored in each backing word.
pub const BITS_PER_WORD: usize = 64;

impl Default for BitSignature {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl BitSignature {
    /// Grows the backing storage, if needed, so that `bit` can be addressed.
    pub fn reserve(&mut self, bit: usize) {
        let word = bit / BITS_PER_WORD;
        if word >= self.0.len() {
            self.0.resize(word + 1, 0);
        }
    }

    /// Returns the number of bits currently set.
    pub fn count(&self) -> usize {
        self.0.iter().map(|word| word.count_ones() as usize).sum()
    }

    /// Returns the total number of bits the current storage can address without growing.
    pub fn capacity(&self) -> usize {
        self.0.len() * BITS_PER_WORD
    }

    /// Returns `true` if no bits are set.
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|word| *word == 0)
    }

    /// Returns the index of the highest set bit, or `None` if no bits are set.
    pub fn last_set_bit(&self) -> Option<usize> {
        self.0.iter().enumerate().rev().find_map(|(word, bits)| {
            (*bits != 0)
                .then(|| word * BITS_PER_WORD + (BITS_PER_WORD - 1 - bits.leading_zeros() as usize))
        })
    }

    /// Sets `bit`, growing the storage if necessary.
    pub fn set(&mut self, bit: usize) {
        self.reserve(bit);
        self.0[bit / BITS_PER_WORD] |= 1 << (bit % BITS_PER_WORD);
    }

    /// Clears `bit`. Out-of-range bits are treated as already clear.
    pub fn clear(&mut self, bit: usize) {
        if let Some(word) = self.0.get_mut(bit / BITS_PER_WORD) {
            *word &= !(1 << (bit % BITS_PER_WORD));
        }
    }

    /// Returns whether `bit` is set. Out-of-range bits are treated as clear.
    pub fn test(&self, bit: usize) -> bool {
        self.0
            .get(bit / BITS_PER_WORD)
            .is_some_and(|word| word & (1 << (bit % BITS_PER_WORD)) != 0)
    }

    /// Returns an iterator over the indices of all set bits, in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word_idx, word)| {
            let word = *word;
            (0..BITS_PER_WORD)
                .filter(move |bit_idx| word & (1 << bit_idx) != 0)
                .map(move |bit_idx| word_idx * BITS_PER_WORD + bit_idx)
        })
    }

    /// Returns `true` if every bit set in `other` is also set in `self`.
    pub fn contains(&self, other: &Self) -> bool {
        let min_len = self.0.len().min(other.0.len());
        let subset_in_common = self.0[..min_len]
            .iter()
            .zip(&other.0[..min_len])
            .all(|(a, b)| a & b == *b);
        subset_in_common && other.0[min_len..].iter().all(|word| *word == 0)
    }

    /// Returns `true` if `self` and `other` have at least one bit in common.
    pub fn intersects(&self, other: &Self) -> bool {
        let min_len = self.0.len().min(other.0.len());
        self.0[..min_len]
            .iter()
            .zip(&other.0[..min_len])
            .any(|(a, b)| a & b != 0)
    }

    /// Sets every bit that is set in `other`, growing `self` if needed.
    pub fn union_with(&mut self, other: &Self) {
        if other.0.len() > self.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= b;
        }
    }

    /// Returns a new signature containing the union of `self` and `other`.
    pub fn union(&self, other: &Self) -> Self {
        let mut result = self.clone();
        result.union_with(other);
        result
    }

    /// Clears every bit not also set in `other`, shrinking `self` to the shorter length.
    pub fn intersection_with(&mut self, other: &Self) {
        let min_len = self.0.len().min(other.0.len());
        for (a, b) in self.0[..min_len].iter_mut().zip(&other.0[..min_len]) {
            *a &= b;
        }
        self.0.truncate(min_len);
    }

    /// Returns a new signature containing the intersection of `self` and `other`.
    pub fn intersection(&self, other: &Self) -> Self {
        let min_len = self.0.len().min(other.0.len());
        let words = self.0[..min_len]
            .iter()
            .zip(&other.0[..min_len])
            .map(|(a, b)| a & b)
            .collect();
        Self(words)
    }

    /// Clears every bit that is set in `other` (set difference: `self` minus `other`).
    pub fn complement_with(&mut self, other: &Self) {
        let min_len = self.0.len().min(other.0.len());
        for (a, b) in self.0[..min_len].iter_mut().zip(&other.0[..min_len]) {
            *a &= !b;
        }
    }

    /// Returns a new signature containing the bits of `self` not present in `other`.
    pub fn complement(&self, other: &Self) -> Self {
        let mut result = self.clone();
        result.complement_with(other);
        result
    }
}

impl PartialEq for BitSignature {
    fn eq(&self, other: &Self) -> bool {
        let min_len = self.0.len().min(other.0.len());
        self.0[..min_len] == other.0[..min_len]
            && self.0[min_len..].iter().all(|word| *word == 0)
            && other.0[min_len..].iter().all(|word| *word == 0)
    }
}

impl Hash for BitSignature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash only up to the last non-zero word so that equal signatures (which
        // may differ in trailing zero words) always hash identically.
        let end = self
            .0
            .iter()
            .rposition(|word| *word != 0)
            .map_or(0, |i| i + 1);
        for word in &self.0[..end] {
            word.hash(state);
        }
    }
}

pub const INDEX_LIST_THRESHOLD: usize = 32;

impl fmt::Debug for BitSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitSignature[")?;
        let count = self.count();
        if count <= INDEX_LIST_THRESHOLD {
            let mut bits = self.iter();
            if let Some(first) = bits.next() {
                write!(f, "{first}")?;
                for bit in bits {
                    write!(f, ", {bit}")?;
                }
            }
        } else {
            write!(f, "set={count}/{} ", self.capacity())?;
            let mut words = self.0.iter();
            if let Some(first) = words.next() {
                write!(f, "{first:016x}")?;
                for word in words {
                    write!(f, " {word:016x}")?;
                }
            }
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let sig = BitSignature::default();
        assert!(sig.is_empty());
        assert_eq!(sig.count(), 0);
        assert_eq!(sig.capacity(), 0);
        assert_eq!(sig.last_set_bit(), None);
        assert_eq!(sig.iter().count(), 0);
    }

    #[test]
    fn set_test_clear() {
        let mut sig = BitSignature::default();
        sig.set(0);
        sig.set(63);
        sig.set(64);
        sig.set(1000);
        assert!(sig.test(0));
        assert!(sig.test(63));
        assert!(sig.test(64));
        assert!(sig.test(1000));
        assert!(!sig.test(1));
        assert!(!sig.test(62));
        assert!(!sig.test(65));

        sig.clear(63);
        assert!(!sig.test(63));
        assert!(sig.test(64));
    }

    #[test]
    fn clear_out_of_range_is_noop() {
        let mut sig = BitSignature::default();
        sig.clear(1000);
        assert!(sig.is_empty());
    }

    #[test]
    fn test_out_of_range_is_false() {
        let sig = BitSignature::default();
        assert!(!sig.test(1000));
    }

    #[test]
    fn reserve_grows_capacity() {
        let mut sig = BitSignature::default();
        assert_eq!(sig.capacity(), 0);
        sig.reserve(63);
        assert_eq!(sig.capacity(), BITS_PER_WORD);
        sig.reserve(64);
        assert_eq!(sig.capacity(), 2 * BITS_PER_WORD);
    }

    #[test]
    fn count_matches_iter_len() {
        let mut sig = BitSignature::default();
        for bit in [3, 5, 8, 13, 21, 34, 55, 89, 144] {
            sig.set(bit);
        }
        assert_eq!(sig.count(), sig.iter().count());
        assert_eq!(sig.count(), 9);
    }

    #[test]
    fn iter_yields_ascending() {
        let mut sig = BitSignature::default();
        for bit in [55, 3, 34, 13, 5, 8, 21] {
            sig.set(bit);
        }
        assert_eq!(sig.iter().collect::<Vec<_>>(), vec![3, 5, 8, 13, 21, 34, 55]);
    }

    #[test]
    fn iter_skips_zero_words() {
        let mut sig = BitSignature::default();
        sig.set(0);
        sig.set(200);
        assert_eq!(sig.iter().collect::<Vec<_>>(), vec![0, 200]);
    }

    #[test]
    fn last_set_bit() {
        let mut sig = BitSignature::default();
        sig.set(3);
        assert_eq!(sig.last_set_bit(), Some(3));
        sig.set(129);
        assert_eq!(sig.last_set_bit(), Some(129));
        sig.clear(129);
        assert_eq!(sig.last_set_bit(), Some(3));
    }

    #[test]
    fn contains_subset() {
        let mut a = BitSignature::default();
        let mut b = BitSignature::default();
        for bit in [0, 1, 2] {
            a.set(bit);
            b.set(bit);
        }
        assert!(a.contains(&b));
        assert!(b.contains(&a));
        a.set(100);
        assert!(a.contains(&b));
        assert!(!b.contains(&a));
        b.set(200);
        assert!(!a.contains(&b));
        assert!(!b.contains(&a));
    }

    #[test]
    fn contains_empty_and_longer_other() {
        let mut a = BitSignature::default();
        a.set(5);
        let mut b = BitSignature::default();
        b.set(200);
        // b's high bits are out of a's range, so a cannot contain b.
        assert!(!a.contains(&b));
        assert!(a.contains(&BitSignature::default()));
        assert!(a.contains(&a));
    }

    #[test]
    fn intersects() {
        let mut a = BitSignature::default();
        let mut b = BitSignature::default();
        a.set(1);
        a.set(100);
        b.set(2);
        assert!(!a.intersects(&b));
        b.set(100);
        assert!(a.intersects(&b));
    }

    #[test]
    fn union_combines_bits() {
        let mut a = BitSignature::default();
        let mut b = BitSignature::default();
        a.set(0);
        a.set(64);
        b.set(1);
        b.set(128);
        let u = a.union(&b);
        assert_eq!(u.iter().collect::<Vec<_>>(), vec![0, 1, 64, 128]);
    }

    #[test]
    fn union_with_grows_self() {
        let mut a = BitSignature::default();
        a.set(0);
        let mut b = BitSignature::default();
        b.set(1000);
        a.union_with(&b);
        assert!(a.test(0));
        assert!(a.test(1000));
    }

    #[test]
    fn intersection_keeps_common() {
        let mut a = BitSignature::default();
        let mut b = BitSignature::default();
        for bit in [0, 1, 2, 3] {
            a.set(bit);
        }
        for bit in [2, 3, 4] {
            b.set(bit);
        }
        assert_eq!(a.intersection(&b).iter().collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn intersection_with_truncates() {
        let mut a = BitSignature::default();
        a.set(0);
        a.set(100);
        let b = BitSignature::default();
        a.intersection_with(&b);
        assert!(a.is_empty());
        assert_eq!(a.capacity(), 0);
    }

    #[test]
    fn complement_removes_other_bits() {
        let mut a = BitSignature::default();
        let mut b = BitSignature::default();
        for bit in [0, 1, 2, 3, 4] {
            a.set(bit);
        }
        for bit in [2, 3, 5] {
            b.set(bit);
        }
        assert_eq!(a.complement(&b).iter().collect::<Vec<_>>(), vec![0, 1, 4]);
    }

    #[test]
    fn complement_with_ignores_beyond_other() {
        let mut a = BitSignature::default();
        a.set(100);
        let b = BitSignature::default();
        a.complement_with(&b);
        assert!(a.test(100));
    }

    #[test]
    fn partial_eq_ignores_trailing_zero_words() {
        let mut short = BitSignature::default();
        short.set(3);
        let mut long = BitSignature::default();
        long.set(3);
        // Force extra zero words on `long`.
        long.reserve(200);
        assert_eq!(short, long);
        assert_eq!(long, short);

        let mut other = BitSignature::default();
        other.set(4);
        assert_ne!(short, other);
    }

    #[test]
    fn eq_after_clear_matches_smaller() {
        let mut a = BitSignature::default();
        a.set(0);
        a.set(100);
        a.clear(100);
        let mut b = BitSignature::default();
        b.set(0);
        assert_eq!(a, b);
    }

    #[test]
    fn debug_list_form_for_sparse() {
        let mut sig = BitSignature::default();
        for bit in [0, 1, 2, 5] {
            sig.set(bit);
        }
        assert_eq!(format!("{sig:?}"), "BitSignature[0, 1, 2, 5]");
    }

    #[test]
    fn debug_list_form_for_empty() {
        let sig = BitSignature::default();
        assert_eq!(format!("{sig:?}"), "BitSignature[]");
    }

    #[test]
    fn debug_hex_form_for_dense() {
        let mut sig = BitSignature::default();
        for bit in 0..=INDEX_LIST_THRESHOLD {
            sig.set(bit);
        }
        // 33 set bits (> threshold) forces the hex/bitmap form.
        let debug = format!("{sig:?}");
        assert_eq!(
            debug,
            format!(
                "BitSignature[set={}/{} 00000001ffffffff]",
                INDEX_LIST_THRESHOLD + 1,
                BITS_PER_WORD
            )
        );
    }

    #[test]
    fn clone_is_independent() {
        let mut a = BitSignature::default();
        a.set(7);
        let mut b = a.clone();
        b.set(8);
        assert!(a.test(7));
        assert!(!a.test(8));
        assert!(b.test(8));
    }

    #[test]
    fn hash_is_consistent_with_eq() {
        use std::hash::{DefaultHasher, Hasher};

        let hash = |sig: &BitSignature| {
            let mut hasher = DefaultHasher::new();
            sig.hash(&mut hasher);
            hasher.finish()
        };

        let mut short = BitSignature::default();
        short.set(3);
        let mut long = BitSignature::default();
        long.set(3);
        long.reserve(200); // trailing zero words
        assert_eq!(short, long);
        assert_eq!(hash(&short), hash(&long));

        let mut other = BitSignature::default();
        other.set(4);
        assert_ne!(hash(&short), hash(&other));

        // Empty signatures of different lengths must hash the same too.
        let mut empty_long = BitSignature::default();
        empty_long.reserve(1000);
        assert_eq!(BitSignature::default(), empty_long);
        assert_eq!(hash(&BitSignature::default()), hash(&empty_long));
    }

    #[test]
    fn usable_as_hashmap_key() {
        let mut map = std::collections::HashMap::new();

        let mut a = BitSignature::default();
        a.set(1);
        a.set(70);
        let mut b = BitSignature::default();
        b.set(70);
        b.set(1);
        b.reserve(500); // trailing zero words, equal to `a`

        map.insert(a, "found");
        assert_eq!(map.get(&b), Some(&"found"));
    }
}
