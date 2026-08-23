use crate::prelude::*;

use nonmax::NonMaxU32;
use std::collections::HashMap;
use thiserror::Error;

/// Maps [`TypeId`]s to stable [`TypeKey`]s and their [`TypeMeta`], with generation-based
/// invalidation of keys.
pub struct TypeRegistry {
    id_to_key: HashMap<TypeId, TypeKey>,
    /// Slot table: `Some(key)` means live, `None` means retired/vacant waiting for reuse.
    meta_entries: Vec<(Option<TypeKey>, TypeMeta)>,
    retired_keys: Vec<TypeKey>,
}

/// A stable, generational reference to a type registered in a [`TypeRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeKey {
    /// Slot index into the owning registry's internal storage.
    pub index: TypeIndex,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: TypeGeneration,
}

/// Non-`u32::MAX` slot index for entries inside a [`TypeRegistry`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeIndex(NonMaxU32);

/// Non-`u32::MAX` generation token attached to a [`TypeKey`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeGeneration(NonMaxU32);

impl TypeIndex {
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

impl TypeGeneration {
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

impl Default for TypeRegistry {
    /// Creates an empty registry.
    fn default() -> Self {
        tracing::trace!("TypeRegistry created.");
        Self { id_to_key: HashMap::new(), meta_entries: Vec::new(), retired_keys: Vec::new() }
    }
}

impl TypeRegistry {
    /// Registers `meta`, returning a stable `TypeKey`. Re-registering the same `TypeId`
    /// is idempotent: it returns the existing key and overwrites its metadata. The two
    /// registrations are expected to carry identical metadata, which is checked via
    /// `debug_assert_eq!` in debug builds and logged with `tracing::warn!` otherwise.
    pub fn register(&mut self, meta: TypeMeta) -> TypeKey {
        let id = meta.id;
        if let Some(&existing_key) = self.id_to_key.get(&id) {
            let stored_meta = &mut self.meta_entries[existing_key.index.as_usize()].1;
            debug_assert_eq!(
                *stored_meta, meta,
                "TypeMeta mismatch: {id:?} was already registered with different metadata"
            );
            // release builds skip the assert above, so warn instead when metadata actually differs
            if *stored_meta != meta {
                tracing::warn!(
                    ?id, key = ?existing_key, old_meta = ?stored_meta, new_meta = ?meta,
                    "TypeMeta mismatch on re-register; overwriting with new metadata"
                );
            }
            *stored_meta = meta;
            return existing_key;
        }

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.meta_entries[retired_key.index.as_usize()] = (Some(retired_key), meta);
            retired_key
        } else {
            let key = TypeKey {
                index: TypeIndex::from_usize(self.meta_entries.len())
                    .expect("TypeRegistry cannot index more than u32::MAX - 1 entries"),
                // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
                generation: TypeGeneration::new(0).unwrap(),
            };
            self.meta_entries.push((Some(key), meta));
            key
        };
        self.id_to_key.insert(id, key);
        tracing::trace!(
            ?id, ?key, meta = ?self.meta_entries[key.index.as_usize()].1,
            "TypeRegistry::register created new entry"
        );
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to a live entry in this registry.
    pub fn unregister(&mut self, key: TypeKey) -> Result<(), Error> {
        let meta = self.key_to_meta(&key)?;
        let id = meta.id;
        tracing::trace!(?id, ?key, meta = ?meta, "TypeRegistry::unregister removing entry");

        self.id_to_key.remove(&id);

        let entry = &mut self.meta_entries[key.index.as_usize()];
        let retired_key = TypeKey { index: key.index, generation: key.generation.next() };
        entry.0 = None;
        self.retired_keys.push(retired_key);

        Ok(())
    }

    /// Looks up the current `TypeKey` registered for `id`.
    pub fn id_to_key(&self, id: &TypeId) -> Result<&TypeKey, Error> {
        self.id_to_key.get(id).ok_or(Error::UnknownType { id: *id })
    }

    /// Looks up the `TypeMeta` registered for `id`.
    pub fn id_to_meta(&self, id: &TypeId) -> Result<&TypeMeta, Error> {
        let key = self.id_to_key(id)?;
        self.key_to_meta(key)
    }

    /// Resolves `key` to its `TypeMeta`, validating that its generation is still current.
    pub fn key_to_meta(&self, key: &TypeKey) -> Result<&TypeMeta, Error> {
        let (stored_key, meta) =
            self.meta_entries.get(key.index.as_usize()).ok_or(Error::IndexOutOfBounds {
                index: key.index.get(),
                bounds: self.meta_entries.len(),
            })?;
        if let Some(stored_key) = stored_key {
            if stored_key.generation != key.generation {
                return Err(Error::GenerationMismatch {
                    expected: stored_key.generation.get(),
                    actual: key.generation.get(),
                });
            }
            return Ok(meta);
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

/// Errors returned when looking up or unregistering entries in a `TypeRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// No type is currently registered for the given id.
    #[error("Unknown type id: {id:?}")]
    UnknownType { id: TypeId },

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
    use std::mem::size_of;

    #[test]
    fn option_type_key_has_same_size_as_type_key() {
        assert_eq!(size_of::<TypeKey>(), size_of::<Option<TypeKey>>());
    }

    #[test]
    fn register_allows_zero_sized_types() {
        let mut registry = TypeRegistry::default();
        let meta = TypeMeta::of::<()>();
        assert_eq!(meta.size, 0);

        let key = registry.register(meta);

        assert_eq!(registry.key_to_meta(&key).unwrap().size, 0);
    }

    #[test]
    fn register_is_idempotent_for_same_type() {
        let mut registry = TypeRegistry::default();
        let key1 = registry.register(TypeMeta::of::<u8>());
        let key2 = registry.register(TypeMeta::of::<u8>());

        assert_eq!(key1, key2);
        assert_eq!(registry.id_to_meta(&TypeId::of::<u8>()).unwrap().name, TypeName::of::<u8>());
    }

    #[test]
    #[should_panic(expected = "TypeMeta mismatch")]
    fn register_panics_in_debug_on_metadata_mismatch() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of_script(1);
        registry.register(TypeMeta::new(id, 4, 4, TypeName::of_script("v1")));
        registry.register(TypeMeta::new(id, 8, 8, TypeName::of_script("v2")));
    }

    // debug_assert_eq! above only fires with debug assertions enabled; verify the documented
    // fallback behavior (last write wins) for builds where it is compiled out.
    #[cfg(not(debug_assertions))]
    #[test]
    fn register_overwrites_metadata_when_assertions_disabled() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of_script(2);
        let key1 = registry.register(TypeMeta::new(id, 4, 4, TypeName::of_script("v1")));
        let key2 = registry.register(TypeMeta::new(id, 8, 8, TypeName::of_script("v2")));

        assert_eq!(key1, key2);
        assert_eq!(registry.id_to_meta(&id).unwrap().name, TypeName::of_script("v2"));
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();
        let key = registry.register(TypeMeta::of::<u8>());

        registry.unregister(key).unwrap();

        assert!(registry.id_to_key(&id).is_err());
        assert_eq!(registry.id_to_meta(&id), Err(Error::UnknownType { id }));
        assert_eq!(
            registry.key_to_meta(&key),
            Err(Error::GenerationMismatch {
                expected: key.generation.next().get(),
                actual: key.generation.get(),
            })
        );
    }

    #[test]
    fn register_reuses_retired_slot_with_bumped_generation() {
        let mut registry = TypeRegistry::default();
        let key_a = registry.register(TypeMeta::of::<u8>());
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(TypeMeta::of::<u32>());

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.next());
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = TypeRegistry::default();
        let bogus = TypeKey {
            index: TypeIndex::new(0).unwrap(),
            generation: TypeGeneration::new(0).unwrap(),
        };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_meta_out_of_bounds_index_is_rejected() {
        let mut registry = TypeRegistry::default();
        let key = registry.register(TypeMeta::of::<u8>());
        let bogus = TypeKey { index: TypeIndex::new(key.index.get() + 1).unwrap(), ..key };

        assert_eq!(
            registry.key_to_meta(&bogus),
            Err(Error::IndexOutOfBounds { index: bogus.index.get(), bounds: 1 })
        );
    }

    #[test]
    fn id_to_meta_of_unregistered_id_is_unknown_type_error() {
        let registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();

        assert_eq!(registry.id_to_meta(&id), Err(Error::UnknownType { id }));
    }
}
