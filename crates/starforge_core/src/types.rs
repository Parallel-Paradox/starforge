use std::any::Any;
use std::any::TypeId as StdTypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeId {
    Native(StdTypeId),
    Script(usize),
}

impl TypeId {
    pub fn of<T: Any + 'static>() -> Self {
        let std_type_id = StdTypeId::of::<T>();
        Self::Native(std_type_id)
    }

    pub fn of_script(script_id: usize) -> Self {
        Self::Script(script_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMeta {
    id: TypeId,
    size: usize,
    align: usize,
    name: &'static str,
}

impl TypeMeta {
    pub fn of<T: Any + 'static>() -> Self {
        Self::new(
            TypeId::of::<T>(),
            std::mem::size_of::<T>(),
            std::mem::align_of::<T>(),
            std::any::type_name::<T>(),
        )
    }

    pub fn new(id: TypeId, size: usize, align: usize, name: &'static str) -> Self {
        // zero-sized types (e.g. marker structs) are allowed and have size 0
        debug_assert!(size == 0 || size >= align, "Type size must be ge to alignment.");
        debug_assert!(size % align == 0, "Type size must be a multiple of alignment.");
        debug_assert!(align.is_power_of_two(), "Type alignment must be a power of two.");
        debug_assert!(!name.is_empty(), "Type name must not be empty.");

        Self { id, size, align, name }
    }

    pub fn id(&self) -> &TypeId {
        &self.id
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn align(&self) -> usize {
        self.align
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

pub struct TypeRegistry {
    id_to_key: HashMap<TypeId, TypeKey>,
    meta_entries: Vec<(TypeKey, TypeMeta)>,
    retired_keys: Vec<TypeKey>,
    instance_id: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeKey {
    pub index: u32,
    pub generation: u16,
    pub instance_id: u16,
}

impl TypeKey {
    pub const INVALID_INDEX: u32 = u32::MAX;
    pub const TOMB_GENERATION: u16 = u16::MAX;
    pub const INVALID_INSTANCE_ID: u16 = u16::MAX;
}

impl Default for TypeKey {
    fn default() -> Self {
        Self {
            index: TypeKey::INVALID_INDEX,
            generation: TypeKey::TOMB_GENERATION,
            instance_id: TypeKey::INVALID_INSTANCE_ID,
        }
    }
}

/// Process-wide counter handing out unique `TypeRegistry` instance ids.
static TYPE_REGISTRY_INSTANCE_ID: AtomicU16 = AtomicU16::new(0);

impl Default for TypeRegistry {
    fn default() -> Self {
        // wraps on overflow; fine as long as fewer than u16::MAX registries are alive at once
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
    pub fn register(&mut self, meta: TypeMeta) -> TypeKey {
        let id = *meta.id();
        if let Some(&existing_key) = self.id_to_key.get(&id) {
            let stored_meta = &mut self.meta_entries[existing_key.index as usize].1;
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
            self.meta_entries[retired_key.index as usize] = (retired_key, meta);
            retired_key
        } else {
            let key = TypeKey {
                index: self.meta_entries.len() as u32,
                generation: 0,
                instance_id: self.instance_id,
            };
            self.meta_entries.push((key, meta));
            key
        };
        self.id_to_key.insert(id, key);
        tracing::trace!(
            ?id, ?key, meta = ?self.meta_entries[key.index as usize].1,
            "TypeRegistry::register created new entry");
        key
    }

    pub fn unregister(&mut self, key: TypeKey) -> Result<(), TypeRegistryError> {
        let meta = self.key_to_meta(&key)?;
        let id = *meta.id();
        tracing::trace!(?id, ?key, meta = ?meta, "TypeRegistry::unregister removing entry");

        self.id_to_key.remove(&id);

        let entry = &mut self.meta_entries[key.index as usize];
        entry.0.generation = entry.0.generation.wrapping_add(1);
        self.retired_keys.push(entry.0);

        Ok(())
    }

    pub fn id_to_key(&self, id: &TypeId) -> Option<&TypeKey> {
        self.id_to_key.get(id)
    }

    pub fn id_to_meta(&self, id: &TypeId) -> Result<&TypeMeta, TypeRegistryError> {
        let key = self
            .id_to_key(id)
            .ok_or(TypeRegistryError::UnknownType { id: *id })?;
        self.key_to_meta(key)
    }

    pub fn key_to_meta(&self, key: &TypeKey) -> Result<&TypeMeta, TypeRegistryError> {
        if key.instance_id != self.instance_id {
            return Err(TypeRegistryError::ForeignInstance {
                expected: self.instance_id,
                actual: key.instance_id,
            });
        }
        let (stored_key, meta) = self.meta_entries.get(key.index as usize).ok_or(
            TypeRegistryError::IndexOutOfBounds {
                index: key.index,
                bounds: self.meta_entries.len() as u32,
            },
        )?;
        if stored_key.generation != key.generation {
            return Err(TypeRegistryError::GenerationMismatch {
                expected: stored_key.generation,
                actual: key.generation,
            });
        }
        Ok(meta)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TypeRegistryError {
    #[error("Unknown type id: {id:?}")]
    UnknownType { id: TypeId },

    #[error("Index out of bounds: index {index}, bounds {bounds}")]
    IndexOutOfBounds { index: u32, bounds: u32 },

    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u16, actual: u16 },

    #[error("Foreign registry instance: expected instance id {expected}, actual {actual}")]
    ForeignInstance { expected: u16, actual: u16 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_allows_zero_sized_types() {
        let mut registry = TypeRegistry::default();
        let meta = TypeMeta::of::<()>();
        assert_eq!(meta.size(), 0);

        let key = registry.register(meta);

        assert_eq!(registry.key_to_meta(&key).unwrap().size(), 0);
    }

    #[test]
    fn register_is_idempotent_for_same_type() {
        let mut registry = TypeRegistry::default();
        let key1 = registry.register(TypeMeta::of::<u8>());
        let key2 = registry.register(TypeMeta::of::<u8>());

        assert_eq!(key1, key2);
        assert_eq!(
            registry.id_to_meta(&TypeId::of::<u8>()).unwrap().name(),
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
        assert_eq!(registry.id_to_meta(&id).unwrap().name(), "v2");
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();
        let key = registry.register(TypeMeta::of::<u8>());

        registry.unregister(key).unwrap();

        assert!(registry.id_to_key(&id).is_none());
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

        let key_b = registry.register(TypeMeta::of::<u16>());

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
