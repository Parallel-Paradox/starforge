use nonmax::NonMaxU32;
use starforge_macro::{Deref, DerefMut};
use starforge_reflect::prelude::{TypeId, TypeMeta};
use std::collections::HashMap;
use thiserror::Error;

use crate::component::{Component, ComponentStorage};

#[derive(Clone, Debug, PartialEq, Eq, Deref, DerefMut)]
pub struct ComponentMeta {
    #[deref]
    type_meta: TypeMeta,
    storage: ComponentStorage,
}

/// Maps [`TypeId`]s to stable [`ComponentKey`]s and their [`ComponentMeta`], with
/// generation-based invalidation of keys.
#[derive(Default)]
pub struct ComponentRegistry {
    id_to_key: HashMap<TypeId, ComponentKey>,
    /// Slot table: `Some((key, meta))` means live, `None` means retired/vacant waiting for reuse.
    meta_entries: Vec<Option<(ComponentKey, ComponentMeta)>>,
    retired_keys: Vec<ComponentKey>,
}

/// A stable, generational reference to a component registered in a [`ComponentRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComponentKey {
    /// Slot index into the owning registry's internal storage.
    pub index: ComponentIndex,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: ComponentGeneration,
}

/// Non-`u32::MAX` slot index for entries inside a [`ComponentRegistry`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct ComponentIndex(NonMaxU32);

/// Non-`u32::MAX` generation token attached to a [`ComponentKey`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct ComponentGeneration(NonMaxU32);

impl ComponentMeta {
    /// The storage strategy in entity registry for this component type.
    pub fn storage(&self) -> &ComponentStorage {
        &self.storage
    }

    /// Creates a new [`ComponentMeta`] for the given component type `T`.
    pub fn new<T: Component>() -> Self {
        Self { type_meta: TypeMeta::new::<T>(), storage: T::storage() }
    }

    /// Creates a new [`ComponentMeta`] instance with the provided parameters.
    /// Useful for creating metadata that may not be known at compile time such as scripting.
    pub fn new_impl(type_meta: TypeMeta, storage: ComponentStorage) -> Self {
        Self { type_meta, storage }
    }
}

impl ComponentIndex {
    /// Creates an index from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Creates an index from `usize`, rejecting values larger than `u32::MAX - 1`.
    pub fn from_usize(value: usize) -> Option<Self> {
        let value = u32::try_from(value).ok()?;
        Self::new(value)
    }
}

impl ComponentGeneration {
    /// Creates a generation from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
    }

    /// Advances to the next generation, wrapping from `u32::MAX - 1` to `0`.
    /// Risk: wrapping reuses generations, so a stale key may become valid again.
    pub fn next(self) -> Self {
        if self.get() == u32::MAX - 1 {
            tracing::warn!("ComponentGeneration wrapped to 0");
            // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
            Self::new(0).unwrap()
        } else {
            Self::new(self.get() + 1).expect("next generation must be representable")
        }
    }
}

impl ComponentRegistry {
    /// Registers `meta`, returning a stable `ComponentKey`. Re-registering the same `TypeId`
    /// returns the existing key.
    pub fn register(&mut self, meta: ComponentMeta) -> ComponentKey {
        let type_id = meta.id();
        if let Some(&existing_key) = self.id_to_key.get(&type_id) {
            return existing_key;
        }

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.meta_entries[retired_key.index.get() as usize] = Some((retired_key, meta));
            retired_key
        } else {
            let key = ComponentKey {
                index: ComponentIndex::from_usize(self.meta_entries.len())
                    .expect("ComponentRegistry cannot index more than u32::MAX - 1 entries"),
                // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
                generation: ComponentGeneration::new(0).unwrap(),
            };
            self.meta_entries.push(Some((key, meta)));
            key
        };
        self.id_to_key.insert(type_id, key);
        tracing::trace!(
            ?type_id, ?key,
            meta = ?self.meta_entries[key.index.get() as usize].as_ref().expect("just registered").1,
            "ComponentRegistry::register created new entry"
        );
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to a live entry in this registry.
    pub fn unregister(&mut self, key: ComponentKey) -> Result<(), Error> {
        let meta = self.key_to_meta(&key)?;
        let type_id = meta.id();
        tracing::trace!(
            ?type_id, ?key, meta = ?meta,
            "ComponentRegistry::unregister removing entry"
        );

        self.id_to_key.remove(&type_id);

        // `key_to_meta` above guarantees this slot is live; replacing the entry with
        // `None` drops the metadata together with the key.
        self.meta_entries[key.index.get() as usize] = None;
        self.retired_keys
            .push(ComponentKey { index: key.index, generation: key.generation.next() });

        Ok(())
    }

    /// Looks up the current `ComponentKey` registered for `type_id`.
    pub fn id_to_key(&self, type_id: &TypeId) -> Result<&ComponentKey, Error> {
        self.id_to_key.get(type_id).ok_or(Error::UnknownType { type_id: *type_id })
    }

    /// Looks up the [`ComponentMeta`] registered for `type_id`.
    pub fn id_to_meta(&self, type_id: &TypeId) -> Result<&ComponentMeta, Error> {
        let key = self.id_to_key(type_id)?;
        self.key_to_meta(key)
    }

    /// Resolves `key` to its [`ComponentMeta`], validating that its generation is still current.
    pub fn key_to_meta(&self, key: &ComponentKey) -> Result<&ComponentMeta, Error> {
        let entry =
            self.meta_entries.get(key.index.get() as usize).ok_or(Error::IndexOutOfBounds {
                index: key.index.get(),
                bounds: self.meta_entries.len(),
            })?;
        if let Some((stored_key, meta)) = entry {
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

/// Errors returned when looking up or unregistering entries in a `ComponentRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// No component is currently registered for the given type id.
    #[error("Unknown component type id: {type_id:?}")]
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
    use crate::macros::Component;
    use starforge_reflect::basic::meta::NeedsDrop;
    use starforge_reflect::prelude::TypeName;
    use std::alloc::Layout;
    use std::mem::size_of;

    #[derive(Component)]
    struct Trivial;

    #[derive(Component)]
    #[component(storage = SparseSet)]
    struct Sparse;

    #[derive(Component)]
    struct NonTrivial(#[allow(dead_code)] Box<u8>);

    unsafe fn drop_erased_string(ptr: *mut u8) {
        unsafe { std::ptr::drop_in_place(ptr as *mut String) };
    }

    #[test]
    fn option_component_key_has_same_size_as_component_key() {
        assert_eq!(size_of::<ComponentKey>(), size_of::<Option<ComponentKey>>());
    }

    #[test]
    fn register_is_idempotent_for_same_type() {
        let mut registry = ComponentRegistry::default();
        let key1 = registry.register(ComponentMeta::new::<Trivial>());
        let key2 = registry.register(ComponentMeta::new::<Trivial>());

        assert_eq!(key1, key2);
    }

    #[test]
    fn register_preserves_storage_strategy() {
        let mut registry = ComponentRegistry::default();
        registry.register(ComponentMeta::new::<Sparse>());
        registry.register(ComponentMeta::new::<Trivial>());

        assert_eq!(
            *registry.id_to_meta(&TypeId::of::<Sparse>()).unwrap().storage(),
            ComponentStorage::SparseSet
        );
        // Components without `#[component(storage = ...)]` default to `Archetype`.
        assert_eq!(
            *registry.id_to_meta(&TypeId::of::<Trivial>()).unwrap().storage(),
            ComponentStorage::Archetype
        );
    }

    #[test]
    fn register_keeps_existing_metadata_on_reregister() {
        let mut registry = ComponentRegistry::default();
        let type_id = TypeId::of_script(2);
        registry.register(ComponentMeta::new_impl(
            TypeMeta::new_impl(
                type_id,
                TypeName::of_script("v1"),
                NeedsDrop::Trivial,
                Layout::new::<u8>(),
            ),
            ComponentStorage::Archetype,
        ));
        registry.register(ComponentMeta::new_impl(
            TypeMeta::new_impl(
                type_id,
                TypeName::of_script("v2"),
                NeedsDrop::NonTrivial { drop_fn: drop_erased_string },
                Layout::new::<String>(),
            ),
            ComponentStorage::Archetype,
        ));

        // Re-registering doesn't overwrite metadata.
        let meta = registry.id_to_meta(&type_id).unwrap();
        assert_eq!(meta.name(), &TypeName::of_script("v1"));
        assert!(matches!(meta.needs_drop(), NeedsDrop::Trivial));
        assert_eq!(*meta.storage(), ComponentStorage::Archetype);
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = ComponentRegistry::default();
        let type_id = TypeId::of::<Trivial>();
        let key = registry.register(ComponentMeta::new::<Trivial>());

        registry.unregister(key).unwrap();

        assert!(registry.id_to_key(&type_id).is_err());
        assert_eq!(registry.id_to_meta(&type_id), Err(Error::UnknownType { type_id }));
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
        let mut registry = ComponentRegistry::default();
        let key_a = registry.register(ComponentMeta::new::<Trivial>());
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(ComponentMeta::new::<NonTrivial>());

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.next());
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = ComponentRegistry::default();
        let bogus = ComponentKey {
            index: ComponentIndex::new(0).unwrap(),
            generation: ComponentGeneration::new(0).unwrap(),
        };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_meta_out_of_bounds_index_is_rejected() {
        let mut registry = ComponentRegistry::default();
        let key = registry.register(ComponentMeta::new::<Trivial>());
        let bogus =
            ComponentKey { index: ComponentIndex::new(key.index.get() + 1).unwrap(), ..key };

        assert_eq!(
            registry.key_to_meta(&bogus),
            Err(Error::IndexOutOfBounds { index: bogus.index.get(), bounds: 1 })
        );
    }

    #[test]
    fn id_to_meta_of_unregistered_id_is_unknown_type_error() {
        let registry = ComponentRegistry::default();
        let type_id = TypeId::of::<Trivial>();

        assert_eq!(registry.id_to_meta(&type_id), Err(Error::UnknownType { type_id }));
    }

    #[test]
    fn id_to_key_of_unregistered_id_is_unknown_type_error() {
        let registry = ComponentRegistry::default();
        let type_id = TypeId::of::<Trivial>();

        assert_eq!(registry.id_to_key(&type_id), Err(Error::UnknownType { type_id }));
    }
}
