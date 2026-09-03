mod registry;

use std::any::Any;

pub use registry::{
    ComponentGeneration, ComponentIndex, ComponentKey, ComponentMeta, ComponentRegistry, Error,
};

/// Marker trait for component types. Use `#[derive(Component)]` as convenience access.
pub trait Component: Any + Send + Sync {
    fn storage() -> ComponentStorage {
        ComponentStorage::Archetype
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentStorage {
    Archetype,
    SparseSet,
}

#[cfg(test)]
mod tests {
    use crate::component::{Component, ComponentStorage};
    use crate::macros::Component;

    #[derive(Component)]
    struct ArcheByDefault;

    #[derive(Component)]
    #[component(storage = Archetype)]
    struct ExplicitArchetype;

    #[derive(Component)]
    #[component(storage = SparseSet)]
    struct ExplicitSparseSet;

    #[test]
    fn storage_defaults_to_archetype() {
        assert_eq!(ArcheByDefault::storage(), ComponentStorage::Archetype);
    }

    #[test]
    fn storage_follows_component_attribute() {
        assert_eq!(ExplicitArchetype::storage(), ComponentStorage::Archetype);
        assert_eq!(ExplicitSparseSet::storage(), ComponentStorage::SparseSet);
    }
}
