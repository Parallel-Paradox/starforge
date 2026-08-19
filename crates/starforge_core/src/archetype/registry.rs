use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::prelude::*;
use crate::tool::BitSignature;
use thiserror::Error;

pub type ArchetypeSignature = BitSignature;

pub struct ArchetypeRegistry {
    sign_to_key: HashMap<ArchetypeSignature, ArchetypeKey>,
    archetype_entries: Vec<(ArchetypeKey, Archetype)>,
    retired_keys: Vec<ArchetypeKey>,
    instance_id: u32,
}

/// A stable, generational reference to an archetype registered in an [`ArchetypeRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArchetypeKey {
    /// Slot index into the owning registry's internal storage.
    pub index: usize,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: u32,
    /// Id of the [`ArchetypeRegistry`] that issued this key; used to reject foreign keys.
    pub instance_id: u32,
}

impl ArchetypeKey {
    /// Sentinel index used by [`Default`], never produced by a live registration.
    pub const INVALID_INDEX: usize = usize::MAX;
    /// Sentinel generation used by [`Default`], never produced by a live registration.
    pub const TOMB_GENERATION: u32 = u32::MAX;
    /// Sentinel instance id used by [`Default`], never assigned to a real [`ArchetypeRegistry`].
    pub const INVALID_INSTANCE_ID: u32 = u32::MAX;
}

impl Default for ArchetypeKey {
    /// Produces an invalid key that never resolves against any real [`ArchetypeRegistry`].
    fn default() -> Self {
        Self {
            index: ArchetypeKey::INVALID_INDEX,
            generation: ArchetypeKey::TOMB_GENERATION,
            instance_id: ArchetypeKey::INVALID_INSTANCE_ID,
        }
    }
}

impl ArchetypeKey {
    /// Returns true if this key is valid.
    pub fn is_valid(&self) -> bool {
        self.index != ArchetypeKey::INVALID_INDEX
            && self.generation != ArchetypeKey::TOMB_GENERATION
            && self.instance_id != ArchetypeKey::INVALID_INSTANCE_ID
    }
}

/// Process-wide counter handing out unique `ArchetypeRegistry` instance ids.
static ARCHETYPE_REGISTRY_INSTANCE_ID: AtomicU32 = AtomicU32::new(0);

impl Default for ArchetypeRegistry {
    /// Creates an empty registry with a fresh, process-unique instance id.
    fn default() -> Self {
        let instance_id = ARCHETYPE_REGISTRY_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(instance_id, "ArchetypeRegistry created.");
        Self {
            sign_to_key: HashMap::new(),
            archetype_entries: Vec::new(),
            retired_keys: Vec::new(),
            instance_id,
        }
    }
}

impl ArchetypeRegistry {
    /// Returns the unique instance id of this registry.
    pub fn instance_id(&self) -> u32 {
        self.instance_id
    }

    /// Registers `archetype` for `signature`, returning a stable [`ArchetypeKey`].
    /// Registering the same signature again returns the existing key and keeps the existing
    /// archetype unchanged.
    pub fn register(
        &mut self,
        signature: ArchetypeSignature,
        archetype: Archetype,
    ) -> ArchetypeKey {
        if let Some(&existing_key) = self.sign_to_key.get(&signature) {
            return existing_key;
        }

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.archetype_entries[retired_key.index] = (retired_key, archetype);
            retired_key
        } else {
            let key = ArchetypeKey {
                index: self.archetype_entries.len(),
                generation: 0,
                instance_id: self.instance_id,
            };
            self.archetype_entries.push((key, archetype));
            key
        };
        self.sign_to_key.insert(signature, key);
        tracing::trace!(?key, "ArchetypeRegistry::register created new entry");
        key
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to an entry in this registry.
    pub fn unregister(&mut self, key: ArchetypeKey) -> Result<(), Error> {
        self.key_to_archetype(&key)?;
        tracing::trace!(?key, "ArchetypeRegistry::unregister removing entry");

        self.sign_to_key.retain(|_, stored_key| *stored_key != key);

        let entry = &mut self.archetype_entries[key.index];
        entry.0.generation = entry.0.generation.wrapping_add(1);
        self.retired_keys.push(entry.0);

        Ok(())
    }

    /// Looks up the current [`ArchetypeKey`] registered for `signature`.
    pub fn sign_to_key(&self, signature: &ArchetypeSignature) -> Result<&ArchetypeKey, Error> {
        self.sign_to_key
            .get(signature)
            .ok_or_else(|| Error::UnknownSignature { signature: signature.clone() })
    }

    /// Looks up the [`Archetype`] registered for `signature`.
    pub fn sign_to_archetype(&self, signature: &ArchetypeSignature) -> Result<&Archetype, Error> {
        let key = self.sign_to_key(signature)?;
        self.key_to_archetype(key)
    }

    /// Looks up the mutable [`Archetype`] registered for `signature`.
    pub fn sign_to_archetype_mut(
        &mut self,
        signature: &ArchetypeSignature,
    ) -> Result<&mut Archetype, Error> {
        let key = *self.sign_to_key(signature)?;
        self.key_to_archetype_mut(&key)
    }

    /// Resolves `key` to its [`Archetype`], validating that it belongs to this registry
    /// and that its generation is still current.
    pub fn key_to_archetype(&self, key: &ArchetypeKey) -> Result<&Archetype, Error> {
        let index = self.key_to_index(key)?;
        Ok(&self.archetype_entries[index].1)
    }

    /// Resolves `key` to a mutable [`Archetype`], validating that it belongs to this registry
    /// and that its generation is still current.
    pub fn key_to_archetype_mut(&mut self, key: &ArchetypeKey) -> Result<&mut Archetype, Error> {
        let index = self.key_to_index(key)?;
        Ok(&mut self.archetype_entries[index].1)
    }

    fn key_to_index(&self, key: &ArchetypeKey) -> Result<usize, Error> {
        if key.instance_id != self.instance_id {
            return Err(Error::ForeignInstance {
                expected: self.instance_id,
                actual: key.instance_id,
            });
        }
        let (stored_key, _) =
            self.archetype_entries.get(key.index).ok_or(Error::IndexOutOfBounds {
                index: key.index,
                bounds: self.archetype_entries.len(),
            })?;
        if stored_key.generation != key.generation {
            return Err(Error::GenerationMismatch {
                expected: stored_key.generation,
                actual: key.generation,
            });
        }
        Ok(key.index)
    }
}

/// Errors returned when looking up or unregistering entries in an `ArchetypeRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// No archetype is currently registered for the given signature.
    #[error("Unknown archetype signature: {signature:?}")]
    UnknownSignature { signature: ArchetypeSignature },

    /// The key's index does not point at an entry in the registry.
    #[error("Index out of bounds: index {index}, bounds {bounds}")]
    IndexOutOfBounds { index: usize, bounds: usize },

    /// The key's generation is stale; its slot has since been unregistered and possibly reused.
    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u32, actual: u32 },

    /// The key was issued by a different `ArchetypeRegistry` instance.
    #[error("Foreign registry instance: expected instance id {expected}, actual {actual}")]
    ForeignInstance { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archetype::ArchetypeMeta;
    use std::sync::Arc;

    fn archetype() -> Archetype {
        let type_registry = TypeRegistry::default();
        let component_registry = ComponentRegistry::default();
        let meta = ArchetypeMeta::new(vec![], &type_registry, &component_registry).unwrap();
        Archetype::new(Arc::new(meta)).unwrap()
    }

    fn signature(bit: usize) -> ArchetypeSignature {
        let mut signature = ArchetypeSignature::default();
        signature.set(bit);
        signature
    }

    #[test]
    fn register_is_idempotent_for_same_signature() {
        let mut registry = ArchetypeRegistry::default();
        let signature = signature(1);

        let key_a = registry.register(signature.clone(), archetype());
        let key_b = registry.register(signature, archetype());

        assert_eq!(key_a, key_b);
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let mut registry = ArchetypeRegistry::default();
        let signature = signature(1);
        let key = registry.register(signature.clone(), archetype());

        registry.unregister(key).unwrap();

        assert_eq!(registry.sign_to_key(&signature), Err(Error::UnknownSignature { signature }));
        assert!(matches!(
            registry.key_to_archetype(&key),
            Err(Error::GenerationMismatch { expected, actual })
                if expected == key.generation.wrapping_add(1) && actual == key.generation
        ));
    }

    #[test]
    fn register_reuses_retired_slot_with_bumped_generation() {
        let mut registry = ArchetypeRegistry::default();
        let key_a = registry.register(signature(1), archetype());
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(signature(2), archetype());

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.wrapping_add(1));
        assert_eq!(key_b.instance_id, key_a.instance_id);
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = ArchetypeRegistry::default();
        let bogus = ArchetypeKey { index: 0, generation: 0, instance_id: registry.instance_id() };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_archetype_out_of_bounds_index_is_rejected() {
        let mut registry = ArchetypeRegistry::default();
        let key = registry.register(signature(1), archetype());
        let bogus = ArchetypeKey { index: key.index + 1, ..key };

        assert!(matches!(
            registry.key_to_archetype(&bogus),
            Err(Error::IndexOutOfBounds { index, bounds })
                if index == bogus.index && bounds == 1
        ));
    }

    #[test]
    fn key_from_another_registry_is_rejected() {
        let mut registry_a = ArchetypeRegistry::default();
        let registry_b = ArchetypeRegistry::default();
        let key = registry_a.register(signature(1), archetype());

        assert!(matches!(
            registry_b.key_to_archetype(&key),
            Err(Error::ForeignInstance { expected, actual })
                if expected == registry_b.instance_id() && actual == key.instance_id
        ));
    }

    #[test]
    fn unknown_signature_returns_error() {
        let registry = ArchetypeRegistry::default();
        let signature = signature(1);

        assert!(matches!(
            registry.sign_to_archetype(&signature),
            Err(Error::UnknownSignature { signature: actual }) if actual == signature
        ));
    }

    #[test]
    fn mutable_lookups_resolve_registered_archetype() {
        let mut registry = ArchetypeRegistry::default();
        let signature = signature(1);
        let key = registry.register(signature.clone(), archetype());

        assert!(registry.sign_to_archetype_mut(&signature).is_ok());
        assert!(registry.key_to_archetype_mut(&key).is_ok());
    }
}
