pub use crate::prelude::*;
use crate::sparse_set::SparseIndex;

pub struct EntityMeta {
    pub archetype_key: ArchetypeKey,
    pub archetype_row: usize,
    pub sparse_component: Vec<(SparseSetKey, SparseIndex)>,
}
