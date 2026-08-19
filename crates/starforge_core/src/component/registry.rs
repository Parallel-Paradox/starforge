use super::ComponentMeta;
use crate::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use thiserror::Error;

/// Maps [`TypeId`]s to stable [`ComponentKey`]s and their [`ComponentMeta`], with generation-based
/// invalidation of keys whose slot has been unregistered.
pub struct ComponentRegistry {
    id_to_key: HashMap<TypeId, ComponentKey>,
    meta_entries: Vec<(ComponentKey, ComponentMeta)>,
    retired_keys: Vec<ComponentKey>,
    instance_id: u32,
}

/// A stable, generational reference to a component registered in a [`ComponentRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComponentKey {
    /// Slot index into the owning registry's internal storage.
    pub index: usize,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: u32,
    /// Id of the [`ComponentRegistry`] that issued this key; used to reject foreign keys.
    pub instance_id: u32,
}

impl ComponentKey {
    /// Sentinel index used by [`Default`], never produced by a live registration.
    pub const INVALID_INDEX: usize = usize::MAX;
    /// Sentinel generation used by [`Default`], never produced by a live registration.
    pub const TOMB_GENERATION: u32 = u32::MAX;
    /// Sentinel instance id used by [`Default`], never assigned to a real [`ComponentRegistry`].
    pub const INVALID_INSTANCE_ID: u32 = u32::MAX;
}

impl Default for ComponentKey {
    /// Produces an invalid key that never resolves against any real [`ComponentRegistry`].
    fn default() -> Self {
        Self {
            index: ComponentKey::INVALID_INDEX,
            generation: ComponentKey::TOMB_GENERATION,
            instance_id: ComponentKey::INVALID_INSTANCE_ID,
        }
    }
}

impl ComponentKey {
    /// Returns true if this key is valid.
    pub fn is_valid(&self) -> bool {
        self.index != ComponentKey::INVALID_INDEX
            && self.generation != ComponentKey::TOMB_GENERATION
            && self.instance_id != ComponentKey::INVALID_INSTANCE_ID
    }
}

/// Process-wide counter handing out unique `ComponentRegistry` instance ids.
static COMPONENT_REGISTRY_INSTANCE_ID: AtomicU32 = AtomicU32::new(0);

impl Default for ComponentRegistry {
    /// Creates an empty registry with a fresh, process-unique instance id.
    fn default() -> Self {
        // wraps on overflow; fine as long as fewer than u32::MAX registries are alive at once
        let instance_id = COMPONENT_REGISTRY_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(instance_id, "ComponentRegistry created.");
        Self {
            id_to_key: HashMap::new(),
            meta_entries: Vec::new(),
            retired_keys: Vec::new(),
            instance_id,
        }
    }
}

impl ComponentRegistry {
    /// Returns the unique instance id of this registry.
    pub fn instance_id(&self) -> u32 {
        self.instance_id
    }

    /// Registers `meta`, returning a stable `ComponentKey`. Re-registering the same `TypeId`
    /// returns the existing key.
    pub fn register(&mut self, meta: ComponentMeta) -> ComponentKey {
        let type_id = meta.id;
        if let Some(&existing_key) = self.id_to_key.get(&type_id) {
            return existing_key;
        }

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.meta_entries[retired_key.index] = (retired_key, meta);
            retired_key
        } else {
            let key = ComponentKey {
                index: self.meta_entries.len(),
                generation: 0,
                instance_id: self.instance_id,
            };
            self.meta_entries.push((key, meta));
            key
        };
        self.id_to_key.insert(type_id, key);
        tracing::trace!(
            ?type_id, ?key, meta = ?self.meta_entries[key.index].1,
            "ComponentRegistry::register created new entry"
        );
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to a live entry in this registry.
    pub fn unregister(&mut self, key: ComponentKey) -> Result<(), Error> {
        let meta = self.key_to_meta(&key)?;
        let type_id = meta.id;
        tracing::trace!(
            ?type_id, ?key, meta = ?meta,
            "ComponentRegistry::unregister removing entry"
        );

        self.id_to_key.remove(&type_id);

        let entry = &mut self.meta_entries[key.index];
        entry.0.generation = entry.0.generation.wrapping_add(1);
        self.retired_keys.push(entry.0);

        Ok(())
    }

    /// Looks up the current `ComponentKey` registered for `type_id`.
    pub fn id_to_key(&self, type_id: &TypeId) -> Result<&ComponentKey, Error> {
        self.id_to_key.get(type_id).ok_or(Error::UnknownType { type_id: *type_id })
    }

    /// Looks up the `ComponentMeta` registered for `type_id`.
    pub fn id_to_meta(&self, type_id: &TypeId) -> Result<&ComponentMeta, Error> {
        let key = self.id_to_key(type_id)?;
        self.key_to_meta(key)
    }

    /// Resolves `key` to its `ComponentMeta`, validating that it belongs to this registry
    /// and that its generation is still current.
    pub fn key_to_meta(&self, key: &ComponentKey) -> Result<&ComponentMeta, Error> {
        if key.instance_id != self.instance_id {
            return Err(Error::ForeignInstance {
                expected: self.instance_id,
                actual: key.instance_id,
            });
        }
        let (stored_key, meta) = self
            .meta_entries
            .get(key.index)
            .ok_or(Error::IndexOutOfBounds { index: key.index, bounds: self.meta_entries.len() })?;
        if stored_key.generation != key.generation {
            return Err(Error::GenerationMismatch {
                expected: stored_key.generation,
                actual: key.generation,
            });
        }
        Ok(meta)
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
    IndexOutOfBounds { index: usize, bounds: usize },

    /// The key's generation is stale; its slot has since been unregistered and possibly reused.
    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u32, actual: u32 },

    /// The key was issued by a different `ComponentRegistry` instance.
    #[error("Foreign registry instance: expected instance id {expected}, actual {actual}")]
    ForeignInstance { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[derive(Component)]
    struct Trivial;

    #[derive(Component)]
    struct NonTrivial(#[allow(dead_code)] Box<u8>);

    #[test]
    fn of_detects_trivial_component() {
        let meta = ComponentMeta::of::<Trivial>();
        assert!(matches!(meta.kind, ComponentKind::Trivial));
    }

    #[test]
    fn of_detects_non_trivial_component() {
        let meta = ComponentMeta::of::<NonTrivial>();
        assert!(matches!(meta.kind, ComponentKind::NonTrivial { .. }));
    }

    #[test]
    fn component_meta_equality_ignores_kind() {
        let type_id = TypeId::of_script(1);
        let a = ComponentMeta::new(type_id, ComponentKind::Trivial);
        let b = ComponentMeta::new(
            type_id,
            ComponentKind::NonTrivial {
                drop_fn: crate::component::meta::drop_in_place_erased::<u8>,
            },
        );

        // identity is defined by `id` alone; `kind` is a derived attribute of it
        assert_eq!(a, b);
    }

    #[test]
    fn register_is_idempotent_for_same_type() {
        let mut registry = ComponentRegistry::default();
        let key1 = registry.register(ComponentMeta::of::<Trivial>());
        let key2 = registry.register(ComponentMeta::of::<Trivial>());

        assert_eq!(key1, key2);
    }

    #[test]
    fn register_keeps_existing_metadata_on_reregister() {
        let mut registry = ComponentRegistry::default();
        let type_id = TypeId::of_script(2);
        registry.register(ComponentMeta::new(type_id, ComponentKind::Trivial));
        registry.register(ComponentMeta::new(
            type_id,
            ComponentKind::NonTrivial {
                drop_fn: crate::component::meta::drop_in_place_erased::<u8>,
            },
        ));

        // re-registering doesn't overwrite metadata, unlike `TypeRegistry::register`
        let meta = registry.id_to_meta(&type_id).unwrap();
        assert!(matches!(meta.kind, ComponentKind::Trivial));
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = ComponentRegistry::default();
        let type_id = TypeId::of::<Trivial>();
        let key = registry.register(ComponentMeta::of::<Trivial>());

        registry.unregister(key).unwrap();

        assert!(registry.id_to_key(&type_id).is_err());
        assert_eq!(registry.id_to_meta(&type_id), Err(Error::UnknownType { type_id }));
        assert_eq!(
            registry.key_to_meta(&key),
            Err(Error::GenerationMismatch {
                expected: key.generation.wrapping_add(1),
                actual: key.generation,
            })
        );
    }

    #[test]
    fn register_reuses_retired_slot_with_bumped_generation() {
        let mut registry = ComponentRegistry::default();
        let key_a = registry.register(ComponentMeta::of::<Trivial>());
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(ComponentMeta::of::<NonTrivial>());

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.wrapping_add(1));
        assert_eq!(key_b.instance_id, key_a.instance_id);
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = ComponentRegistry::default();
        let bogus = ComponentKey { index: 0, generation: 0, instance_id: registry.instance_id() };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_meta_out_of_bounds_index_is_rejected() {
        let mut registry = ComponentRegistry::default();
        let key = registry.register(ComponentMeta::of::<Trivial>());
        let bogus = ComponentKey { index: key.index + 1, ..key };

        assert_eq!(
            registry.key_to_meta(&bogus),
            Err(Error::IndexOutOfBounds { index: bogus.index, bounds: 1 })
        );
    }

    #[test]
    fn key_from_another_registry_is_rejected() {
        let mut registry_a = ComponentRegistry::default();
        let registry_b = ComponentRegistry::default();
        let key = registry_a.register(ComponentMeta::of::<Trivial>());

        assert_eq!(
            registry_b.key_to_meta(&key),
            Err(Error::ForeignInstance {
                expected: registry_b.instance_id(),
                actual: key.instance_id,
            })
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
