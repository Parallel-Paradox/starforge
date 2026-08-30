mod registry;

pub mod archetype;
pub mod sparse_set;

use std::collections::HashMap;

pub use registry::{EntityGeneration, EntityIndex, EntityKey, EntityRegistry};

use crate::prelude::*;
use archetype::ArchetypeKey;
use sparse_set::{SparseIndex, SparseSetKey};
use starforge_reflect::basic::Parcel;

pub type EntitySignature = crate::tool::BitSignature;

/// Holds metadata for an entity, describing the route it took.
pub struct Entity {
    /// A bit signature representing the entity's component trait.
    pub signature: EntitySignature,
    /// Access [`ArchetypeRegistry`] to retrieve the archetype this entity belongs to.
    pub archetype_key: ArchetypeKey,
    /// The dense row index within the archetype's storage.
    pub archetype_row: usize,
    /// Access [`SparseSetRegistry`] to retrieve the sparse component indices for this entity.
    pub sparse_component: Vec<(SparseSetKey, SparseIndex)>,
}

#[derive(Default)]
pub struct EntityBuilder(HashMap<TypeId, Parcel>);

impl EntityBuilder {
    pub fn insert<T: Component>(&mut self, component: T) -> Option<T> {
        self.insert_impl(TypeId::of::<T>(), Parcel::new(component))
            // SAFETY: popped by the same TypeId
            .map(|parcel| unsafe { parcel.take() })
    }

    pub fn insert_impl(&mut self, id: TypeId, parcel: Parcel) -> Option<Parcel> {
        self.0.insert(id, parcel)
    }

    pub fn remove<T: Component>(&mut self, id: &TypeId) -> Option<T> {
        // SAFETY: popped by the same TypeId
        self.remove_impl(id).map(|parcel| unsafe { parcel.take() })
    }

    pub fn remove_impl(&mut self, id: &TypeId) -> Option<Parcel> {
        self.0.remove(id)
    }
}
