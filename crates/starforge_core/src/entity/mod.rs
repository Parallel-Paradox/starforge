mod registry;

pub use registry::{EntityGeneration, EntityIndex, EntityKey, EntityRegistry};

use crate::prelude::*;
use crate::sparse_set::SparseIndex;

pub type EntitySignature = crate::tool::BitSignature;

/// Holds metadata for an entity, describing the route it took.
pub struct EntityMeta {
    /// A bit signature representing the entity's component trait.
    pub signature: EntitySignature,
    /// Access [`ArchetypeRegistry`] to retrieve the archetype this entity belongs to.
    pub archetype_key: ArchetypeKey,
    /// The dense row index within the archetype's storage.
    pub archetype_row: usize,
    /// Access [`SparseSetRegistry`] to retrieve the sparse component indices for this entity.
    pub sparse_component: Vec<(SparseSetKey, SparseIndex)>,
}
