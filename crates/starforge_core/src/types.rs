use std::any::Any;
use std::any::TypeId as StdTypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use thiserror::Error;

/// Uniquely identifies a native Rust type or a script-defined type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeId {
    /// A type known to the Rust type system.
    Native(StdTypeId),
    /// A type defined by a script, identified by an opaque script-assigned id.
    Script(usize),
}

impl TypeId {
    /// Returns the `TypeId` for a native Rust type `T`.
    pub fn of<T: Any + 'static>() -> Self {
        let std_type_id = StdTypeId::of::<T>();
        Self::Native(std_type_id)
    }

    /// Returns the `TypeId` for a script-defined type identified by `script_id`.
    pub const fn of_script(script_id: usize) -> Self {
        Self::Script(script_id)
    }
}

/// Metadata describing a registered type: its identity, size, alignment, and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMeta {
    pub id: TypeId,
    pub size: usize,
    pub align: usize,
    pub name: &'static str,
}

impl TypeMeta {
    /// Builds `TypeMeta` for a native Rust type `T` from `size_of`/`align_of`/`type_name`.
    pub fn of<T: Any + 'static>() -> Self {
        Self::new(
            TypeId::of::<T>(),
            std::mem::size_of::<T>(),
            std::mem::align_of::<T>(),
            std::any::type_name::<T>(),
        )
    }

    /// Constructs `TypeMeta` from explicit values. Zero-sized types (size 0) are allowed;
    /// `size`/`align` invariants are otherwise checked via `debug_assert!` in debug builds.
    pub const fn new(id: TypeId, size: usize, align: usize, name: &'static str) -> Self {
        // zero-sized types (e.g. marker structs) are allowed and have size 0
        debug_assert!(size == 0 || size >= align, "Type size must be ge to alignment.");
        debug_assert!(size % align == 0, "Type size must be a multiple of alignment.");
        debug_assert!(align.is_power_of_two(), "Type alignment must be a power of two.");
        debug_assert!(!name.is_empty(), "Type name must not be empty.");

        Self { id, size, align, name }
    }
}

/// Maps `TypeId`s to stable `TypeKey`s and their `TypeMeta`, with generation-based
/// invalidation of keys whose slot has been unregistered.
pub struct TypeRegistry {
    id_to_key: HashMap<TypeId, TypeKey>,
    meta_entries: Vec<(TypeKey, TypeMeta)>,
    retired_keys: Vec<TypeKey>,
    instance_id: u32,
}

/// A stable, generational reference to a type registered in a `TypeRegistry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeKey {
    /// Slot index into the owning registry's internal storage.
    pub index: usize,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: u32,
    /// Id of the `TypeRegistry` that issued this key; used to reject foreign keys.
    pub instance_id: u32,
}

impl TypeKey {
    /// Sentinel index used by `Default`, never produced by a live registration.
    pub const INVALID_INDEX: usize = usize::MAX;
    /// Sentinel generation used by `Default`, never produced by a live registration.
    pub const TOMB_GENERATION: u32 = u32::MAX;
    /// Sentinel instance id used by `Default`, never assigned to a real `TypeRegistry`.
    pub const INVALID_INSTANCE_ID: u32 = u32::MAX;
}

impl Default for TypeKey {
    /// Produces an invalid key that never resolves against any real `TypeRegistry`.
    fn default() -> Self {
        Self {
            index: TypeKey::INVALID_INDEX,
            generation: TypeKey::TOMB_GENERATION,
            instance_id: TypeKey::INVALID_INSTANCE_ID,
        }
    }
}

impl TypeKey {
    /// Returns true if this key is valid.
    pub fn is_valid(&self) -> bool {
        self.index != TypeKey::INVALID_INDEX
            && self.generation != TypeKey::TOMB_GENERATION
            && self.instance_id != TypeKey::INVALID_INSTANCE_ID
    }
}

/// Process-wide counter handing out unique `TypeRegistry` instance ids.
static TYPE_REGISTRY_INSTANCE_ID: AtomicU32 = AtomicU32::new(0);

impl Default for TypeRegistry {
    /// Creates an empty registry with a fresh, process-unique instance id.
    fn default() -> Self {
        // wraps on overflow; fine as long as fewer than u32::MAX registries are alive at once
        let instance_id = TYPE_REGISTRY_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(instance_id, "TypeRegistry created.");
        Self {
            id_to_key: HashMap::new(),
            meta_entries: Vec::new(),
            retired_keys: Vec::new(),
            instance_id,
        }
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
            let stored_meta = &mut self.meta_entries[existing_key.index].1;
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
            self.meta_entries[retired_key.index] = (retired_key, meta);
            retired_key
        } else {
            let key = TypeKey {
                index: self.meta_entries.len(),
                generation: 0,
                instance_id: self.instance_id,
            };
            self.meta_entries.push((key, meta));
            key
        };
        self.id_to_key.insert(id, key);
        tracing::trace!(
            ?id, ?key, meta = ?self.meta_entries[key.index].1,
            "TypeRegistry::register created new entry"
        );
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to a live entry in this registry.
    pub fn unregister(&mut self, key: TypeKey) -> Result<(), TypeRegistryError> {
        let meta = self.key_to_meta(&key)?;
        let id = meta.id;
        tracing::trace!(?id, ?key, meta = ?meta, "TypeRegistry::unregister removing entry");

        self.id_to_key.remove(&id);

        let entry = &mut self.meta_entries[key.index];
        entry.0.generation = entry.0.generation.wrapping_add(1);
        self.retired_keys.push(entry.0);

        Ok(())
    }

    /// Looks up the current `TypeKey` registered for `id`.
    pub fn id_to_key(&self, id: &TypeId) -> Result<&TypeKey, TypeRegistryError> {
        self.id_to_key
            .get(id)
            .ok_or(TypeRegistryError::UnknownType { id: *id })
    }

    /// Looks up the `TypeMeta` registered for `id`.
    pub fn id_to_meta(&self, id: &TypeId) -> Result<&TypeMeta, TypeRegistryError> {
        let key = self.id_to_key(id)?;
        self.key_to_meta(key)
    }

    /// Resolves `key` to its `TypeMeta`, validating that it belongs to this registry
    /// and that its generation is still current.
    pub fn key_to_meta(&self, key: &TypeKey) -> Result<&TypeMeta, TypeRegistryError> {
        if key.instance_id != self.instance_id {
            return Err(TypeRegistryError::ForeignInstance {
                expected: self.instance_id,
                actual: key.instance_id,
            });
        }
        let (stored_key, meta) =
            self.meta_entries
                .get(key.index)
                .ok_or(TypeRegistryError::IndexOutOfBounds {
                    index: key.index,
                    bounds: self.meta_entries.len(),
                })?;
        if stored_key.generation != key.generation {
            return Err(TypeRegistryError::GenerationMismatch {
                expected: stored_key.generation,
                actual: key.generation,
            });
        }
        Ok(meta)
    }
}

/// Errors returned when looking up or unregistering entries in a `TypeRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TypeRegistryError {
    /// No type is currently registered for the given id.
    #[error("Unknown type id: {id:?}")]
    UnknownType { id: TypeId },

    /// The key's index does not point at a live slot in the registry.
    #[error("Index out of bounds: index {index}, bounds {bounds}")]
    IndexOutOfBounds { index: usize, bounds: usize },

    /// The key's generation is stale; its slot has since been unregistered and possibly reused.
    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u32, actual: u32 },

    /// The key was issued by a different `TypeRegistry` instance.
    #[error("Foreign registry instance: expected instance id {expected}, actual {actual}")]
    ForeignInstance { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            registry.id_to_meta(&TypeId::of::<u8>()).unwrap().name,
            std::any::type_name::<u8>()
        );
    }

    #[test]
    #[should_panic(expected = "TypeMeta mismatch")]
    fn register_panics_in_debug_on_metadata_mismatch() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of_script(1);
        registry.register(TypeMeta::new(id, 4, 4, "v1"));
        registry.register(TypeMeta::new(id, 8, 8, "v2"));
    }

    // debug_assert_eq! above only fires with debug assertions enabled; verify the documented
    // fallback behavior (last write wins) for builds where it is compiled out.
    #[cfg(not(debug_assertions))]
    #[test]
    fn register_overwrites_metadata_when_assertions_disabled() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of_script(2);
        let key1 = registry.register(TypeMeta::new(id, 4, 4, "v1"));
        let key2 = registry.register(TypeMeta::new(id, 8, 8, "v2"));

        assert_eq!(key1, key2);
        assert_eq!(registry.id_to_meta(&id).unwrap().name, "v2");
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();
        let key = registry.register(TypeMeta::of::<u8>());

        registry.unregister(key).unwrap();

        assert!(registry.id_to_key(&id).is_err());
        assert_eq!(registry.id_to_meta(&id), Err(TypeRegistryError::UnknownType { id }));
        assert_eq!(
            registry.key_to_meta(&key),
            Err(TypeRegistryError::GenerationMismatch {
                expected: key.generation.wrapping_add(1),
                actual: key.generation,
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
        assert_eq!(key_b.generation, key_a.generation.wrapping_add(1));
        assert_eq!(key_b.instance_id, key_a.instance_id);
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = TypeRegistry::default();
        let bogus = TypeKey { index: 0, generation: 0, instance_id: registry.instance_id };

        assert_eq!(
            registry.unregister(bogus),
            Err(TypeRegistryError::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_meta_out_of_bounds_index_is_rejected() {
        let mut registry = TypeRegistry::default();
        let key = registry.register(TypeMeta::of::<u8>());
        let bogus = TypeKey { index: key.index + 1, ..key };

        assert_eq!(
            registry.key_to_meta(&bogus),
            Err(TypeRegistryError::IndexOutOfBounds { index: bogus.index, bounds: 1 })
        );
    }

    #[test]
    fn key_from_another_registry_is_rejected() {
        let mut registry_a = TypeRegistry::default();
        let registry_b = TypeRegistry::default();
        let key = registry_a.register(TypeMeta::of::<u8>());

        assert_eq!(
            registry_b.key_to_meta(&key),
            Err(TypeRegistryError::ForeignInstance {
                expected: registry_b.instance_id,
                actual: key.instance_id,
            })
        );
    }

    #[test]
    fn id_to_meta_of_unregistered_id_is_unknown_type_error() {
        let registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();

        assert_eq!(registry.id_to_meta(&id), Err(TypeRegistryError::UnknownType { id }));
    }
}
