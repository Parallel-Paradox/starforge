mod registry;

pub mod archetype;
pub mod sparse_set;

pub use registry::{EntityGeneration, EntityIndex, EntityKey, EntityRegistry};

use archetype::ArchetypeKey;
use sparse_set::SparseSetKey;

pub type EntitySignature = crate::tool::BitSignature;

/// Holds metadata for an entity, describing the route it took.
pub struct Entity {
    /// A bit signature representing the entity's component trait.
    pub signature: EntitySignature,
    /// Access [`ArchetypeRegistry`] to retrieve the archetype this entity belongs to.
    pub archetype_key: ArchetypeKey,
    /// The dense row index within the archetype's storage.
    pub archetype_row: usize,
    /// Keys of the sparse sets containing components owned by this entity.
    pub sparse_component: Vec<SparseSetKey>,
}
