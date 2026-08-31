use std::collections::HashMap;
use std::sync::Arc;

use super::{Archetype, ArchetypeMeta, Error as ArchetypeError};
use crate::tool::BitSignature;
use nonmax::NonMaxU32;
use starforge_macro::Deref;
use thiserror::Error;

/// Bitmask identifying the component set of an archetype, supposed to be built from
/// [`ComponentKey::index`].
///
/// The signature is used as the canonical lookup key in [`ArchetypeRegistry`].
/// Two archetypes with the same component combination share the same signature.
pub type ArchetypeSignature = BitSignature;

/// Registry for archetypes keyed by [`ArchetypeSignature`].
#[derive(Default)]
pub struct ArchetypeRegistry {
    sig_to_key: HashMap<ArchetypeSignature, ArchetypeKey>,
    /// Slot table: `Some((key, archetype))` means live, `None` means retired/vacant waiting for reuse.
    archetype_entries: Vec<Option<(ArchetypeKey, Archetype)>>,
    retired_keys: Vec<ArchetypeKey>,
}

/// A stable, generational reference to an archetype registered in an [`ArchetypeRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArchetypeKey {
    /// Slot index into the owning registry's internal storage.
    pub index: ArchetypeIndex,
    /// Bumped each time the slot is reused, invalidating older keys pointing at it.
    pub generation: ArchetypeGeneration,
}

/// Non-`u32::MAX` slot index for entries inside an [`ArchetypeRegistry`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct ArchetypeIndex(NonMaxU32);

/// Non-`u32::MAX` generation token attached to an [`ArchetypeKey`].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deref)]
pub struct ArchetypeGeneration(NonMaxU32);

impl ArchetypeIndex {
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

impl ArchetypeGeneration {
    /// Creates a generation from a raw `u32`, rejecting `u32::MAX`.
    pub fn new(value: u32) -> Option<Self> {
        NonMaxU32::new(value).map(Self)
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

impl ArchetypeRegistry {
    /// Registers an archetype built from `meta`, returning a stable [`ArchetypeKey`].
    /// The signature comes from [`ArchetypeMeta::signature`]; registering the same
    /// signature again returns the existing key and keeps the existing archetype unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`ArchetypeError::ArchetypeTooLarge`] if a single entity's bytes exceed
    /// max buffer size.
    pub fn register(&mut self, meta: Arc<ArchetypeMeta>) -> Result<ArchetypeKey, ArchetypeError> {
        let signature = meta.signature().clone();
        if let Some(&existing_key) = self.sig_to_key.get(&signature) {
            return Ok(existing_key);
        }

        let archetype = Archetype::new(meta)?;

        let key = if let Some(retired_key) = self.retired_keys.pop() {
            self.archetype_entries[retired_key.index.get() as usize] =
                Some((retired_key, archetype));
            retired_key
        } else {
            let key = ArchetypeKey {
                index: ArchetypeIndex::from_usize(self.archetype_entries.len())
                    .expect("ArchetypeRegistry cannot index more than u32::MAX - 1 entries"),
                // `0` is always representable because only `u32::MAX` is rejected by `NonMaxU32`.
                generation: ArchetypeGeneration::new(0).unwrap(),
            };
            self.archetype_entries.push(Some((key, archetype)));
            key
        };
        self.sig_to_key.insert(signature, key);
        tracing::trace!(?key, "ArchetypeRegistry::register created new entry");
        Ok(key)
    }

    /// Invalidates `key`, retiring its slot for reuse with a bumped generation.
    /// Returns an error if `key` does not resolve to an entry in this registry.
    pub fn unregister(&mut self, key: ArchetypeKey) -> Result<(), Error> {
        self.key_to_archetype(&key)?;
        tracing::trace!(?key, "ArchetypeRegistry::unregister removing entry");

        self.sig_to_key.retain(|_, stored_key| *stored_key != key);

        // `key_to_archetype` above guarantees this slot is live; replacing the entry with
        // `None` drops the archetype together with the key.
        self.archetype_entries[key.index.get() as usize] = None;
        self.retired_keys
            .push(ArchetypeKey { index: key.index, generation: key.generation.next() });

        Ok(())
    }

    /// Looks up the current [`ArchetypeKey`] registered for `signature`.
    pub fn sig_to_key(&self, signature: &ArchetypeSignature) -> Result<&ArchetypeKey, Error> {
        self.sig_to_key
            .get(signature)
            .ok_or_else(|| Error::UnknownSignature { signature: signature.clone() })
    }

    /// Looks up the [`Archetype`] registered for `signature`.
    pub fn sig_to_archetype(&self, signature: &ArchetypeSignature) -> Result<&Archetype, Error> {
        let key = self.sig_to_key(signature)?;
        self.key_to_archetype(key)
    }

    /// Looks up the mutable [`Archetype`] registered for `signature`.
    pub fn sig_to_archetype_mut(
        &mut self,
        signature: &ArchetypeSignature,
    ) -> Result<&mut Archetype, Error> {
        let key = *self.sig_to_key(signature)?;
        self.key_to_archetype_mut(&key)
    }

    /// Resolves `key` to its [`Archetype`], validating that its generation is still current.
    pub fn key_to_archetype(&self, key: &ArchetypeKey) -> Result<&Archetype, Error> {
        let index = self.key_to_index(key)?;
        let entry = self.archetype_entries[index]
            .as_ref()
            .expect("key_to_index validated the slot as live");
        Ok(&entry.1)
    }

    /// Resolves `key` to a mutable [`Archetype`], validating that its generation is still current.
    pub fn key_to_archetype_mut(&mut self, key: &ArchetypeKey) -> Result<&mut Archetype, Error> {
        let index = self.key_to_index(key)?;
        let entry = self.archetype_entries[index]
            .as_mut()
            .expect("key_to_index validated the slot as live");
        Ok(&mut entry.1)
    }

    fn key_to_index(&self, key: &ArchetypeKey) -> Result<usize, Error> {
        let entry = self.archetype_entries.get(key.index.get() as usize).ok_or(
            Error::IndexOutOfBounds {
                index: key.index.get(),
                bounds: self.archetype_entries.len(),
            },
        )?;
        if let Some((stored_key, _)) = entry {
            if stored_key.generation != key.generation {
                return Err(Error::GenerationMismatch {
                    expected: stored_key.generation.get(),
                    actual: key.generation.get(),
                });
            }
            return Ok(key.index.get() as usize);
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

/// Errors returned when looking up or unregistering entries in an `ArchetypeRegistry`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// No archetype is currently registered for the given signature.
    #[error("Unknown archetype signature: {signature:?}")]
    UnknownSignature { signature: ArchetypeSignature },

    /// The key's index does not point at an entry in the registry.
    #[error("Index out of bounds: index {index}, bounds {bounds}")]
    IndexOutOfBounds { index: u32, bounds: usize },

    /// The key's generation is stale; its slot has since been unregistered and possibly reused.
    #[error("Generation mismatch: expected generation {expected}, actual {actual}")]
    GenerationMismatch { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{ComponentKey, ComponentRegistry};
    use starforge_reflect::basic::meta::NeedsDrop;
    use starforge_reflect::prelude::{TypeId, TypeMeta, TypeName};
    use std::alloc::Layout;
    use std::mem::size_of;
    use std::sync::Arc;

    #[test]
    fn option_archetype_key_has_same_size_as_archetype_key() {
        assert_eq!(size_of::<ArchetypeKey>(), size_of::<Option<ArchetypeKey>>());
    }

    /// Owns the registry backing the columns under test, so keys derived from it stay
    /// resolvable for the lifetime of the context.
    struct TestContext {
        comp_reg: ComponentRegistry,
    }

    impl TestContext {
        /// Builds a context holding three script columns, in registration order.
        pub fn mock() -> Self {
            let mut comp_reg = ComponentRegistry::default();
            for id in [
                TypeId::of_script(1),
                TypeId::of_script(2),
                TypeId::of_script(3),
            ] {
                comp_reg.register(TypeMeta::new_impl(
                    id,
                    TypeName::of_script("test"),
                    NeedsDrop::Trivial,
                    Layout::from_size_align(8, 8).unwrap(),
                ));
            }
            Self { comp_reg }
        }

        pub fn column(&self, id: TypeId) -> ComponentKey {
            *self.comp_reg.id_to_key(&id).unwrap()
        }

        /// Builds an `Arc<ArchetypeMeta>` over the given ids, in the given order.
        pub fn meta(&self, ids: &[TypeId]) -> Arc<ArchetypeMeta> {
            let keys: Vec<ComponentKey> = ids.iter().map(|id| self.column(*id)).collect();
            Arc::new(ArchetypeMeta::new(&keys, &self.comp_reg).unwrap())
        }
    }

    #[test]
    fn register_is_idempotent_for_same_signature() {
        let ctx = TestContext::mock();
        let mut registry = ArchetypeRegistry::default();
        let meta = ctx.meta(&[TypeId::of_script(1)]);

        let key_a = registry.register(meta.clone()).unwrap();
        let key_b = registry.register(meta).unwrap();

        assert_eq!(key_a, key_b);
    }

    #[test]
    fn unregister_invalidates_the_key() {
        let ctx = TestContext::mock();
        let mut registry = ArchetypeRegistry::default();
        let meta = ctx.meta(&[TypeId::of_script(1)]);
        let signature = meta.signature().clone();
        let key = registry.register(meta).unwrap();

        registry.unregister(key).unwrap();

        assert_eq!(registry.sig_to_key(&signature), Err(Error::UnknownSignature { signature }));
        assert!(matches!(
            registry.key_to_archetype(&key),
            Err(Error::GenerationMismatch { expected, actual })
                if expected == key.generation.next().get() && actual == key.generation.get()
        ));
    }

    #[test]
    fn register_reuses_retired_slot_with_bumped_generation() {
        let ctx = TestContext::mock();
        let mut registry = ArchetypeRegistry::default();
        let key_a = registry.register(ctx.meta(&[TypeId::of_script(1)])).unwrap();
        registry.unregister(key_a).unwrap();

        let key_b = registry.register(ctx.meta(&[TypeId::of_script(2)])).unwrap();

        assert_eq!(key_b.index, key_a.index);
        assert_eq!(key_b.generation, key_a.generation.next());
    }

    #[test]
    fn unregister_unknown_key_returns_error() {
        let mut registry = ArchetypeRegistry::default();
        let bogus = ArchetypeKey {
            index: ArchetypeIndex::new(0).unwrap(),
            generation: ArchetypeGeneration::new(0).unwrap(),
        };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_archetype_out_of_bounds_index_is_rejected() {
        let ctx = TestContext::mock();
        let mut registry = ArchetypeRegistry::default();
        let key = registry.register(ctx.meta(&[TypeId::of_script(1)])).unwrap();
        let bogus =
            ArchetypeKey { index: ArchetypeIndex::new(key.index.get() + 1).unwrap(), ..key };

        assert!(matches!(
            registry.key_to_archetype(&bogus),
            Err(Error::IndexOutOfBounds { index, bounds })
                if index == bogus.index.get() && bounds == 1
        ));
    }

    #[test]
    fn unknown_signature_returns_error() {
        let ctx = TestContext::mock();
        let registry = ArchetypeRegistry::default();
        let signature = ctx.meta(&[TypeId::of_script(1)]).signature().clone();

        assert!(matches!(
            registry.sig_to_archetype(&signature),
            Err(Error::UnknownSignature { signature: actual }) if actual == signature
        ));
    }

    #[test]
    fn mutable_lookup_resolve_registered_archetype() {
        let ctx = TestContext::mock();
        let mut registry = ArchetypeRegistry::default();
        let meta = ctx.meta(&[TypeId::of_script(1)]);
        let signature = meta.signature().clone();
        let key = registry.register(meta).unwrap();

        assert!(registry.sig_to_archetype_mut(&signature).is_ok());
        assert!(registry.key_to_archetype_mut(&key).is_ok());
    }
}
