mod meta;
mod registry;

pub use meta::{ComponentKind, ComponentMeta};
pub use registry::{ComponentKey, ComponentRegistry, Error};

pub trait Component: 'static + Send + Sync {}

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
            ComponentKind::NonTrivial { drop_fn: meta::drop_in_place_erased::<u8> },
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
            ComponentKind::NonTrivial { drop_fn: meta::drop_in_place_erased::<u8> },
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
