use super::SparseSet;
use crate::prelude::*;

use nonmax::NonMaxU32;
use std::collections::HashMap;
use thiserror::Error;

/// Maps [`TypeId`]s to stable [`SparseSetKey`]s and their [`SparseSet`]s, with
/// generation-based invalidation of keys.
pub struct SparseSetRegistry {
    id_to_key: HashMap<TypeId, SparseSetKey>,
    /// Slot table: `Some((key, set))` means live, `None` means retired/vacant waiting for reuse.
    set_entries: Vec<Option<(SparseSetKey, SparseSet)>>,
    retired_keys: Vec<SparseSetKey>,
}

/// A stable, generational reference to a sparse set registered in a [`SparseSetRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SparseSetKey {
    /// Slot index into the owning registry's internal storage.
    pub index: SparseSetIndex,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: SparseSetGeneration,
}

/// Non-`u32::MAX` slot index for entries inside a [`SparseSetRegistry`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SparseSetIndex(NonMaxU32);

/// Non-`u32::MAX` generation token attached to a [`SparseSetKey`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SparseSetGeneration(NonMaxU32);

impl SparseSetIndex {
    /// Creates an index from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Creates an index from `usize`, rejecting values larger than `u32::MAX - 1`.
    pub fn from_usize(value: usize) -> Option<Self> {
        let value = u32::try_from(value).ok()?;
        Self::new(value)
    }

    /// Returns the raw `u32` value.
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Returns this index as a `usize` for indexing vectors.
    pub fn as_usize(self) -> usize {
        self.get() as usize
    }
}

impl SparseSetGeneration {
    /// Creates a generation from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Returns the raw `u32` value.
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Advances to the next generation, wrapping from `u32::MAX - 1` to `0`.
    pub fn next(self) -> Self {
        if self.get() == u32::MAX - 1 {
            // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
            Self::new(0).unwrap()
        } else {
            Self::new(self.get() + 1).expect("next generation must be representable")
        }
    }
}

impl Default for SparseSetRegistry {
    /// Creates an empty registry.
    fn default() -> Self {
        tracing::trace!("SparseSetRegistry created.");
        Self { id_to_key: HashMap::new(), set_entries: Vec::new(), retired_keys: Vec::new() }
    }
}

impl SparseSetRegistry {
    /// Registers `set`, returning a stable [`SparseSetKey`]. Re-registering the same `TypeId`
    /// returns the existing key and keeps the existing sparse set unchanged.
    pub fn register(&mut self, set: SparseSet) -> SparseSetKey {
        let type_id = set.header().type_id;
        if let Some(&existing_key) = self.id_to_key.get(&type_id) {
            return existing_key;
        }

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.set_entries[retired_key.index.as_usize()] = Some((retired_key, set));
            retired_key
        } else {
            let key = SparseSetKey {
                index: SparseSetIndex::from_usize(self.set_entries.len())
                    .expect("SparseSetRegistry cannot index more than u32::MAX - 1 entries"),
                // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
                generation: SparseSetGeneration::new(0).unwrap(),
            };
            self.set_entries.push(Some((key, set)));
            key
        };
        self.id_to_key.insert(type_id, key);
        tracing::trace!(?type_id, ?key, "SparseSetRegistry::register created new entry");
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to a live entry in this registry.
    pub fn unregister(&mut self, key: SparseSetKey) -> Result<(), Error> {
        self.key_to_set(&key)?;
        tracing::trace!(?key, "SparseSetRegistry::unregister removing entry");

        self.id_to_key.retain(|_, stored_key| *stored_key != key);

        // `key_to_set` above guarantees this slot is live; replacing the entry with
        // `None` drops the sparse set together with the key.
        self.set_entries[key.index.as_usize()] = None;
        self.retired_keys
            .push(SparseSetKey { index: key.index, generation: key.generation.next() });

        Ok(())
    }

    /// Looks up the current [`SparseSetKey`] registered for `type_id`.
    pub fn id_to_key(&self, type_id: &TypeId) -> Result<&SparseSetKey, Error> {
        self.id_to_key.get(type_id).ok_or(Error::UnknownType { type_id: *type_id })
    }

    /// Looks up the [`SparseSet`] registered for `type_id`.
    pub fn id_to_set(&self, type_id: &TypeId) -> Result<&SparseSet, Error> {
        let key = self.id_to_key(type_id)?;
        self.key_to_set(key)
    }

    /// Looks up the mutable [`SparseSet`] registered for `type_id`.
    pub fn id_to_set_mut(&mut self, type_id: &TypeId) -> Result<&mut SparseSet, Error> {
        let key = *self.id_to_key(type_id)?;
        self.key_to_set_mut(&key)
    }

    /// Resolves `key` to its [`SparseSet`], validating that its generation is still current.
    pub fn key_to_set(&self, key: &SparseSetKey) -> Result<&SparseSet, Error> {
        let index = self.key_to_index(key)?;
        let entry = self.set_entries[index]
            .as_ref()
            .expect("key_to_index validated the slot as live");
        Ok(&entry.1)
    }

    /// Resolves `key` to a mutable [`SparseSet`], validating that its generation is still current.
    pub fn key_to_set_mut(&mut self, key: &SparseSetKey) -> Result<&mut SparseSet, Error> {
        let index = self.key_to_index(key)?;
        let entry = self.set_entries[index]
            .as_mut()
            .expect("key_to_index validated the slot as live");
        Ok(&mut entry.1)
    }

    fn key_to_index(&self, key: &SparseSetKey) -> Result<usize, Error> {
        let entry = self.set_entries.get(key.index.as_usize()).ok_or(Error::IndexOutOfBounds {
            index: key.index.get(),
            bounds: self.set_entries.len(),
        })?;
        if let Some((stored_key, _)) = entry {
            if stored_key.generation != key.generation {
                return Err(Error::GenerationMismatch {
                    expected: stored_key.generation.get(),
                    actual: key.generation.get(),
                });
            }
            return Ok(key.index.as_usize());
        }

        let expected = self
            .retired_keys
            .iter()
            .find(|retired| retired.index == key.index)
            .map(|retired| retired.generation.get())
            .unwrap_or_else(|| key.generation.next().get());
        Err(Error::GenerationMismatch { expected, actual: key.generation.get() })
    }
}

/// Errors returned when looking up or unregistering entries in a `SparseSetRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// No sparse set is currently registered for the given type id.
    #[error("Unknown sparse set type id: {type_id:?}")]
    UnknownType { type_id: TypeId },

    /// The key's index does not point at a live slot in the registry.
    #[error("Index out of bounds: index {index}, bounds {bounds}")]
    IndexOutOfBounds { index: u32, bounds: usize },

    /// The key's generation is stale; its slot has since been unregistered and possibly reused.
    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentKind, ComponentMeta};
    use crate::entity::sparse_set::SparseSetHeader;
    use crate::types::TypeMeta;
    use std::mem::size_of;

    fn sparse_set_u32() -> SparseSet {
        let mut type_reg = TypeRegistry::default();
        let mut comp_reg = ComponentRegistry::default();
        let id = TypeId::of::<u32>();
        let type_key = type_reg.register(TypeMeta::of::<u32>());
        let comp_key = comp_reg.register(ComponentMeta::new(id, ComponentKind::Trivial));
        let header = SparseSetHeader::new(type_key, comp_key, &type_reg, &comp_reg).unwrap();
        SparseSet::with_capacity(header, 0)
    }

    #[test]
    fn option_sparse_set_key_has_same_size_as_sparse_set_key() {
        assert_eq!(size_of::<SparseSetKey>(), size_of::<Option<SparseSetKey>>());
    }

    #[test]
    fn register_is_idempotent_for_same_type() {
        let mut registry = SparseSetRegistry::default();
        let key_a = registry.register(sparse_set_u32());
        let key_b = registry.register(sparse_set_u32());

        assert_eq!(key_a, key_b);
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = SparseSetRegistry::default();
        let type_id = TypeId::of::<u32>();
        let key = registry.register(sparse_set_u32());

        registry.unregister(key).unwrap();

        assert_eq!(registry.id_to_key(&type_id), Err(Error::UnknownType { type_id }));
        assert!(matches!(
            registry.id_to_set(&type_id),
            Err(Error::UnknownType { type_id: actual }) if actual == type_id
        ));
        assert!(matches!(
            registry.key_to_set(&key),
            Err(Error::GenerationMismatch { expected, actual })
                if expected == key.generation.next().get() && actual == key.generation.get()
        ));
    }

    #[test]
    fn register_reuses_retired_slot_with_bumped_generation() {
        let mut registry = SparseSetRegistry::default();
        let key_a = registry.register(sparse_set_u32());
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(sparse_set_u32());

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.next());
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = SparseSetRegistry::default();
        let bogus = SparseSetKey {
            index: SparseSetIndex::new(0).unwrap(),
            generation: SparseSetGeneration::new(0).unwrap(),
        };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_set_out_of_bounds_index_is_rejected() {
        let mut registry = SparseSetRegistry::default();
        let key = registry.register(sparse_set_u32());
        let bogus =
            SparseSetKey { index: SparseSetIndex::new(key.index.get() + 1).unwrap(), ..key };

        assert!(matches!(
            registry.key_to_set(&bogus),
            Err(Error::IndexOutOfBounds { index, bounds })
                if index == bogus.index.get() && bounds == 1
        ));
    }

    #[test]
    fn id_to_set_of_unregistered_id_is_unknown_type_error() {
        let registry = SparseSetRegistry::default();
        let type_id = TypeId::of::<u32>();

        assert!(matches!(
            registry.id_to_set(&type_id),
            Err(Error::UnknownType { type_id: actual }) if actual == type_id
        ));
    }

    #[test]
    fn id_to_key_of_unregistered_id_is_unknown_type_error() {
        let registry = SparseSetRegistry::default();
        let type_id = TypeId::of::<u32>();

        assert_eq!(registry.id_to_key(&type_id), Err(Error::UnknownType { type_id }));
    }

    #[test]
    fn mutable_lookups_resolve_registered_set() {
        let mut registry = SparseSetRegistry::default();
        let type_id = TypeId::of::<u32>();
        let key = registry.register(sparse_set_u32());

        assert!(registry.id_to_set_mut(&type_id).is_ok());
        assert!(registry.key_to_set_mut(&key).is_ok());
    }
}
