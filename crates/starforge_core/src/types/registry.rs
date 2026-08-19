use super::{TypeId, TypeMeta};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use thiserror::Error;

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
    /// Returns the unique instance id of this registry.
    pub fn instance_id(&self) -> u32 {
        self.instance_id
    }

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
    pub fn unregister(&mut self, key: TypeKey) -> Result<(), Error> {
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
    pub fn id_to_key(&self, id: &TypeId) -> Result<&TypeKey, Error> {
        self.id_to_key.get(id).ok_or(Error::UnknownType { id: *id })
    }

    /// Looks up the `TypeMeta` registered for `id`.
    pub fn id_to_meta(&self, id: &TypeId) -> Result<&TypeMeta, Error> {
        let key = self.id_to_key(id)?;
        self.key_to_meta(key)
    }

    /// Resolves `key` to its `TypeMeta`, validating that it belongs to this registry
    /// and that its generation is still current.
    pub fn key_to_meta(&self, key: &TypeKey) -> Result<&TypeMeta, Error> {
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

/// Errors returned when looking up or unregistering entries in a `TypeRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
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
