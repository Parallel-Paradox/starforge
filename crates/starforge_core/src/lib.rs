pub mod archetype;
pub mod component;
pub mod entity;
pub mod macros;
pub mod system;
pub mod tool;
pub mod types;

pub mod prelude {
    pub use crate::archetype::{Archetype, ArchetypeKey, ArchetypeRegistry};
    pub use crate::component::{
        Component, ComponentKey, ComponentKind, ComponentMeta, ComponentRegistry,
    };
    pub use crate::macros::Component;
    pub use crate::types::{TypeId, TypeKey, TypeMeta, TypeRegistry};
}

pub struct Core {}

pub trait CoreExtract {
    fn extract(core: &mut Core) -> Self;
}
