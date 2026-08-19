mod id;
mod meta;
mod registry;

pub use id::TypeId;
pub use meta::TypeMeta;
pub use registry::{TypeKey, TypeRegistry, Error};

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
        assert_eq!(registry.id_to_meta(&id), Err(Error::UnknownType { id }));
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
        let bogus = TypeKey { index: 0, generation: 0, instance_id: registry.instance_id() };

        assert_eq!(
            registry.unregister(bogus),
            Err(Error::IndexOutOfBounds { index: 0, bounds: 0 })
        );
    }

    #[test]
    fn key_to_meta_out_of_bounds_index_is_rejected() {
        let mut registry = TypeRegistry::default();
        let key = registry.register(TypeMeta::of::<u8>());
        let bogus = TypeKey { index: key.index + 1, ..key };

        assert_eq!(
            registry.key_to_meta(&bogus),
            Err(Error::IndexOutOfBounds { index: bogus.index, bounds: 1 })
        );
    }

    #[test]
    fn key_from_another_registry_is_rejected() {
        let mut registry_a = TypeRegistry::default();
        let registry_b = TypeRegistry::default();
        let key = registry_a.register(TypeMeta::of::<u8>());

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
        let registry = TypeRegistry::default();
        let id = TypeId::of::<u8>();

        assert_eq!(registry.id_to_meta(&id), Err(Error::UnknownType { id }));
    }
}
