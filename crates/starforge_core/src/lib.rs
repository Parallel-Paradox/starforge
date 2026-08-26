pub mod archetype;
pub mod component;
pub mod entity;
pub mod macros;
pub mod sparse_set;
pub mod system;
pub mod tool;
pub mod types;

pub mod prelude {
    pub use crate::archetype::{Archetype, ArchetypeKey, ArchetypeRegistry};
    pub use crate::component::{
        Component, ComponentKey, ComponentKind, ComponentMeta, ComponentRegistry,
    };
    pub use crate::entity::{EntityKey, EntityMeta};
    pub use crate::macros::Component;
    pub use crate::sparse_set::{SparseSet, SparseSetKey, SparseSetRegistry};
    pub use crate::types::{TypeId, TypeKey, TypeMeta, TypeName, TypeRegistry};
}

pub struct Core {}

pub trait CoreExtract {
    fn extract(core: &mut Core) -> Self;
}
